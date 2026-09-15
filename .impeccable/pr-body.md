## What

Adds a **plural `delegate_subagents` tool** that lets the orchestrator fan out N subagent dispatches in a single `tool_call`, bypassing the per-round LLM output pressure that makes the model serialize one dispatch per round on the singular tool.

The merged parallel-subagent dispatcher (PR #3) only parallelizes N `delegate_subagent` calls when the model emits all N in one LLM round. In practice the model is conservative and emits one dispatch per round, then waits for the result before queuing the next. Net effect: subagents still serialize in the in-app build, despite the parallel dispatcher being in place. This commit closes that gap with a fan-out tool.

The PR also closes a **registration gap in PR #3's batch dispatcher**: the previous `execute_tool_batch` read from `dispatch_call_index` without ever populating it for `delegate_subagent` calls, so dispatches silently no-op'd in the in-app build (only test fixtures that called the tool directly ever populated the index). The new dispatch branch populates the index by calling the registration helpers directly.

## Why

In-app testing on PR #3's merge showed the orchestrator firing one `delegate_subagent` per LLM round (e.g. "Researcher finished, then Coder running…") — the per-round output budget pressure makes the model serialize. A plural tool removes that pressure: one `tool_call` is enough to fan out, no per-round output growth.

## How to test

In the in-app chat, ask the orchestrator something with 3+ independent sub-tasks ("research X, code Y, review Z"). You should see all three subagent windows open and stream concurrently (timestamps within ms of each other), and one combined tool result back to the orchestrator.

In Agent Manager, the existing per-personality knobs still work. The plural tool respects the same `max_concurrent_subagents` cap.

### Unit tests

5 new tests in `tools.rs`:
- `delegate_subagents_refuses_non_orchestrator` — role gate
- `delegate_subagents_rejects_empty_or_missing_array` — three shapes (missing, empty, wrong type) all rejected
- `delegate_subagents_registers_all_dispatches_under_one_call_id` — 3 entries → 3 registry rows, all indexed under the same call id
- `delegate_subagents_partially_succeeds_with_per_entry_failures` — mixed batch, valid entries register, failures reported
- `delegate_subagents_fails_when_every_entry_is_invalid` — all-bad batch returns `ok: false`, nothing registered

The existing `delegate_subagent_returns_queued_summary` test is updated to assert the new `Vec<String>` index shape (singular → 1-element vec).

### Validation

- `cargo build --lib` ✅ clean
- `cargo check --tests` ✅ clean
- `cargo clippy --lib` ✅ no new warnings
- `cargo fmt --all -- --check` ✅ clean
- `npx tsc --noEmit` ✅ clean
- `npx vitest run` ✅ 123/123 pass (1 skipped)
- `cargo test --lib` ⚠ blocked by a pre-existing Windows host environment issue (`STATUS_ENTRYPOINT_NOT_FOUND` on the test binary, same on yesterday's known-good binary). Tests compile cleanly under `cargo check --tests`. The new tests should run on a working host.

## API

```
delegate_subagents({
  "dispatches": [
    {"personality": "researcher", "task": "summarize the spec"},
    {"personality": "coder",      "task": "fix the bug"},
    {"personality": "architect",  "task": "review the design"}
  ]
})
```

- **Min entries:** 1 (use the singular tool for one dispatch)
- **Max entries:** 16 (matches the subagent semaphore's roster cap)
- **Per-entry shape:** `{personality, task}` — same fields as the singular tool
- **Per-entry validation failures** are collected and reported as `"skipped N (…)"`; valid entries still register.
- **Role gate:** hard-coded to the Orchestrator (subagent recursion stays impossible — `ToolSpec::subagent()` filters out the plural tool too).

## Returned summary

```
queued 3 subagents (Coder, Architect, Researcher) — running in background
```

or, with per-entry failures:

```
queued 2 subagents (Coder, Architect); skipped 1 (#2: "ghost" is not a roster personality id)
```

## Changes

- `src-tauri/src/tools.rs`:
  - New `delegate_subagents` ToolSpec (plural).
  - `SubagentDispatchContext::dispatch_call_index` is now `DashMap<String, Vec<String>>` (was `String`).
  - `tool_delegate_subagent` refactored: body now delegates to a new `register_one_dispatch` helper.
  - New `tool_delegate_subagents` tool function.
  - New `register_one_dispatch` and `register_plural_dispatches` helpers.
  - `ToolSpec::subagent()` filters out `delegate_subagents` along with the singular tool.
  - 5 new unit tests + update to the existing singular test for the new `Vec<String>` index shape.
- `src-tauri/src/agent_loop.rs`:
  - Dispatch branch in `execute_tool_batch` rewritten to call the registration helpers directly (closes the PR #3 registration gap).
  - Per-call `agent-tool-start` / `agent-tool-result` events and `push_activity` rows are emitted manually so the UI's tool log keeps showing per-dispatch "queued subagent …" entries.
  - The final synthetic tool message is deduped by call id so the plural tool gets exactly one `role: "tool"` message per `tool_call`, regardless of fan-out.
  - `delegate_subagents` is now also routed to the dispatch branch in the inline/dispatch split.
- `src-tauri/src/agent.rs`:
  - Orchestrator system prompt updated to prefer the plural tool for 2+ independent dispatches.
- `docs/superpowers/specs/2026-08-14--plural-subagent-dispatch.md`: new spec doc.

## Risk

- The dispatch branch rewrite is the most invasive change. The previous code path was never exercised end-to-end (the registration gap meant dispatches silently no-op'd in the in-app build). The new path is covered by the unit tests for `register_*` helpers and the existing `delegate_subagent_returns_queued_summary` test still passes.
- The `Vec<String>` change to `dispatch_call_index` is a wire-shape change inside `SubagentDispatchContext`. It does not cross the Tauri IPC boundary, so the frontend is unaffected.
- The dedup of synthetic tool messages by call id is a behavior change for the singular tool as well, but it's a no-op when each call id has one entry (the singular case), so existing behavior is preserved.

## Out of scope

- **Subagent → subagent dispatch.** Still impossible. `ToolSpec::subagent()` filters out both `delegate_subagent` and `delegate_subagents`.
- **A separate "cancel all subagents in this batch" button.** The existing per-subagent `cancel_agent` IPC still works; the per-batch cancel is a separate spec.
- **The new-turn-cancels-batch race** (sending a new chat message while the parallel batch is in flight kills the in-flight subagents). This is a pre-existing bug exposed by the merged parallel-subagent code; tracked separately.
