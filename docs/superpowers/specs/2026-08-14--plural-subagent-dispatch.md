# Plural subagent dispatch — `delegate_subagents`

**Status:** Implemented (branch `feature/plural-subagents`)
**Author:** mavis
**Date:** 2026-08-14
**Builds on:** [2026-08-14--parallel-subagent-dispatch.md](2026-08-14--parallel-subagent-dispatch.md)

## Problem

The merged parallel-subagent dispatcher (PR #3,
[commit `12e075f`](https://github.com/Drifter0071/harness/pull/3))
parallelizes N `delegate_subagent` calls **only when the model emits
all N in one LLM round**. In practice the model is conservative: it
emits one dispatch per round, awaits the synthetic batch message,
and only then emits the next. Net effect — subagents still serialize
in the in-app build, despite the parallel dispatcher being in place.

The screenshot below captures it: the orchestrator emits one
`delegate_subagent` to Researcher, the round completes
(`subagent sub-… (Researcher) finished with 1975 chars`), and only
**then** does the next round fire the Coder dispatch. That's the
pre-merge serial waterfall — and on the merged `main` it would still
happen because the model is too conservative to bundle.

## Goal

Give the model a single tool call that fans out into N concurrent
subagent turns, without relying on it to emit N separate
`tool_calls` in one round.

## Solution

Add a **plural tool** `delegate_subagents` that takes a single
`dispatches: [{personality, task}, ...]` list. One model-side
`tool_call` registers N subagent rows under the same call id, and
the existing `execute_tool_batch` parallel dispatcher spawns N
JoinSet tasks under the same subagent semaphore cap as the singular
tool. The model still gets exactly one combined `role: "tool"`
message back, summarizing every dispatched subagent's outcome.

Both tools stay side by side. The singular `delegate_subagent` is
the right shape for a single one-off dispatch; the plural tool is
the right shape for fan-out.

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
- **Per-entry shape:** `{personality, task}` — same fields as the
  singular tool
- **Per-entry validation failures** (missing `personality`/`task`,
  unknown target) are collected and reported in the queued summary
  as `"skipped N (…)"`. Valid entries in the same batch still
  register.
- **Role gate:** hard-coded to the Orchestrator (subagent recursion
  stays impossible — `ToolSpec::subagent()` filters out the plural
  tool too).

## Returned summary

```
queued 3 subagents (Coder, Architect, Researcher) — running in background
```

or, with some per-entry failures:

```
queued 2 subagents (Coder, Architect); skipped 1 (#2: "ghost" is not a roster personality id)
```

## Behavior

- One `agent-tool-start` / `agent-tool-result` event pair per
  `tool_call` (i.e. per call id), not per subagent. The UI's tool log
  shows a single "queued 3 subagents" entry, mirroring the singular
  tool's per-call entry shape.
- One `role: "tool"` message per call id (i.e. one for the entire
  fan-out), satisfying the provider's "tool result must echo a call
  id" pairing rule. The message body is the same combined batch
  summary the singular tool would emit, but with N sub-ids instead
  of one.
- All N subagent turns run under the same
  `dispatch.subagent_semaphore` (cap = `roster.max_concurrent_subagents`,
  default 4). Excess dispatches queue and start as permits free —
  same behavior as N singular calls in one round.
- The `BatchApprovalCard` (≥2 pending subagent approvals) still
  triggers normally; it watches per-subagent state, not tool name.

## Design

### Dispatch index shape

`SubagentDispatchContext::dispatch_call_index` is now
`DashMap<String, Vec<String>>` (was `String`). One call id can map
to N subagent ids. The singular tool inserts a one-element vec; the
plural tool appends.

### Registration helpers

A new private helper `register_one_dispatch` is extracted from the
singular tool's body. It does the personality resolve, the registry
insert, the inbox + cancel-watcher inserts, and the dispatch-index
update. The singular `tool_delegate_subagent` calls it once; the
new `tool_delegate_subagents` loops it N times.

A second helper `register_plural_dispatches` wraps the loop for the
batch dispatcher, returning `(Vec<RegisteredDispatch>, Vec<String>)`
for `(registered, failures)`.

### Batch dispatcher

The dispatch branch in `execute_tool_batch` now calls the
registration helpers directly instead of relying on `execute_one`
to populate the index. This closes a registration gap in the
original PR #3 code, where `execute_tool_batch` read from
`dispatch_call_index` without ever populating it for
`delegate_subagent` calls (so dispatches silently no-op'd in the
in-app build).

The synthetic tool message at the end of the batch is deduped by
call id: the plural tool's N sub-ids collapse to one `role: "tool"`
message keyed by the model call id.

### System prompt

The orchestrator's `ORCHESTRATOR_PROMPT` (in `agent.rs`) is updated
to prefer the plural tool for 2+ independent dispatches and to
explain when the singular tool is the right call.

## Tests

Unit tests in `tools.rs`:

- `delegate_subagents_refuses_non_orchestrator` — role gate
- `delegate_subagents_rejects_empty_or_missing_array` — three shapes
  (missing, empty, wrong type) all rejected
- `delegate_subagents_registers_all_dispatches_under_one_call_id` —
  3 entries → 3 registry rows, all indexed under the same call id
- `delegate_subagents_partially_succeeds_with_per_entry_failures` —
  mixed batch, valid entries register, failures reported
- `delegate_subagents_fails_when_every_entry_is_invalid` — all-bad
  batch returns `ok: false`, nothing registered

The existing `delegate_subagent_returns_queued_summary` test is
updated to assert the new `Vec<String>` index shape (singular →
1-element vec).

## Risk

- The dispatch branch rewrite is the most invasive change. The
  previous code path was never exercised end-to-end (the
  registration gap meant dispatches silently no-op'd). The new
  path is covered by the unit tests for `register_*` helpers and
  the existing `delegate_subagent_returns_queued_summary` test
  still passes.
- The `Vec<String>` change to `dispatch_call_index` is a
  wire-shape change inside `SubagentDispatchContext`. It does not
  cross the Tauri IPC boundary, so the frontend is unaffected.
- The dedup of synthetic tool messages by call id is a behavior
  change for the singular tool as well, but it's a no-op when
  each call id has one entry (the singular case), so existing
  behavior is preserved.

## Out of scope

- **Subagent → subagent dispatch.** Still impossible. `ToolSpec::subagent()`
  filters out both `delegate_subagent` and `delegate_subagents`.
- **Per-tool policies for read-side parallelism, per-path write
  locks.** Same as PR #3 — separate spec.
- **A separate `cancel_subagents` batch-cancel button.** The existing
  per-subagent `cancel_agent` IPC still works; clicking the close
  X on one subagent window cancels only that one. A future
  "stop all subagents in this batch" button can iterate
  `subagent_cancel` (kept for symmetry in this PR).
