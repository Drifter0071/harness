feat(orchestrator): add `delegate_subagents` plural tool for fan-out

The merged parallel-subagent dispatcher (PR #3) only parallelizes
N `delegate_subagent` calls when the model emits all N in one LLM
round. In practice the model is conservative and emits one dispatch
per round, then waits for the result before queuing the next. Net
effect: subagents still serialize in the in-app build.

This commit adds a new tool `delegate_subagents` (plural) that takes
a single `dispatches: [{personality, task}, ...]` list. One tool call
fans out into N subagent rows under the same call id, and the
existing `execute_tool_batch` runs them concurrently under the
subagent semaphore cap. The model emits one `tool_call`, the
orchestrator emits N concurrent subagent turns.

## What

- **New `delegate_subagents` ToolSpec** (`src-tauri/src/tools.rs`).
  Hard-gated to the Orchestrator (same role check as singular). Max
  16 entries (matches the subagent semaphore's roster cap). Per-entry
  validation failures (missing `personality` / `task`, unknown
  target) are collected and reported in the queued summary so partial
  successes still register the good entries.
- **`SubagentDispatchContext::dispatch_call_index` is now
  `DashMap<String, Vec<String>>`** so one model-side call id can map
  to N subagent ids. Singular tool inserts a one-element vec; plural
  appends. The batch dispatcher iterates the vec.
- **Dispatch branch now calls the registration helpers directly**
  (`register_one_dispatch` / `register_plural_dispatches`) instead of
  relying on `execute_one` to populate `dispatch_call_index`. This
  closes a registration gap in the original parallel-subagent code
  where `execute_tool_batch` read from the index without ever
  populating it for `delegate_subagent` calls (so dispatches
  silently no-op'd in the in-app build).
- **Synthetic tool message deduped by call id.** The plural tool's N
  sub-ids collapse to one `role: "tool"` message keyed by the model
  call id, satisfying provider pairing (one tool result per
  `tool_call`, regardless of fan-out).
- **Orchestrator system prompt** updated to prefer the plural tool
  for 2+ independent dispatches.
- **`ToolSpec::subagent()` filters out `delegate_subagents`** along
  with the singular tool — subagent recursion stays impossible
  through either path.

## Why

In-app testing on PR #3's merge showed the orchestrator firing one
`delegate_subagent` per LLM round (e.g. "Researcher finished, then
Coder running…") — the per-round output budget pressure makes the
model serialize. A plural tool removes the per-round pressure: one
`tool_call` is enough to fan out, no per-round output growth.

## How to test

In the in-app chat, ask the orchestrator something with 3+
independent sub-tasks ("research X, code Y, review Z"). You should
see all three subagent windows open and stream concurrently
(timestamps within ms of each other), and one combined tool result
back to the orchestrator.

In Agent Manager, the existing per-personality knobs still work.
The plural tool respects the same `max_concurrent_subagents` cap.

Unit tests in `tools.rs` cover:
- role gate (non-orchestrator refused)
- missing / empty / wrong-type `dispatches` rejected
- all-good batch: 3 registry rows under one call id
- partial-success: per-entry failures reported, valid entries still register
- all-bad batch: `ok: false`, nothing registered

## Risk

- The dispatch branch rewrite is the most invasive change. The
  previous code path was never exercised end-to-end (the registration
  gap meant dispatches silently no-op'd). The new path is covered by
  the unit tests for `register_*` helpers and the existing
  `delegate_subagent_returns_queued_summary` test still passes.
- The `Vec<String>` change to `dispatch_call_index` is a wire-shape
  change inside `SubagentDispatchContext`. It does not cross the
  Tauri IPC boundary, so the frontend is unaffected.
- The dedup of synthetic tool messages by call id is a behavior
  change for the singular tool as well, but it's a no-op when each
  call id has one entry (the singular case), so existing behavior
  is preserved.

## Spec

`docs/superpowers/specs/2026-08-14--plural-subagent-dispatch.md` —
to be written before merge.
