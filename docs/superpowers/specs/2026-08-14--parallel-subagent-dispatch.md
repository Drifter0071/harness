# Parallel Subagent Dispatch + Roster Concurrency Knobs

**Status:** Draft for review
**Date:** 2026-08-14
**Scope:** Multi-agent loop — make the Orchestrator actually parallel, and surface two new knobs (max concurrent subagents, max goal-mode continuation turns) in the Agent Manager.

## 1. Summary

The Orchestrator currently blocks on every `delegate_subagent` call: `tool_delegate_subagent` (`src-tauri/src/tools.rs:763`) `await`s `run_subagent_turn` synchronously, and the orchestrator's tool loop (`src-tauri/src/agent_loop.rs:1104`) iterates `for call in &outcome.tool_calls` strictly serially. If the model emits N subagent dispatches in one round, the orchestrator's turn stretches to N × (one full subagent turn) before the next LLM round can even start.

This spec unblocks the orchestrator. When the model emits one or more `delegate_subagent` calls in a round, the orchestrator admits them up to a configured cap and runs them concurrently with `tokio` tasks; excess dispatches queue and start as permits free. All N results fold into **one** synthetic tool message the next LLM round sees. Per-subagent approvals stack into a multi-row card so the user isn't peppered with popups.

It also surfaces two new roster knobs that the user has been asking for, side-by-side, in the existing Agent Manager modal:

| Knob | Default | Range | Field |
|---|---|---|---|
| Max concurrent subagents | 4 | 1–16 | `Roster.maxConcurrentSubagents` |
| Max goal-mode continuation turns | 8 | 1–32 | `Roster.goalMaxTurns` (was a code constant `GOAL_MAX_TURNS`) |

Both default in `personalities::default_roster()` on first open and round-trip through the existing `load_agent_roster` / `save_agent_roster` IPC with full backward compatibility — older `agents.json` files keep loading.

## 2. Out of Scope

- **Parallel non-dispatch tool calls** (e.g. read 5 files in one round). The orchestrator's other tools stay serial. If we want that later, it's a separate spec on top of this one.
- **Subagent → subagent dispatch.** Subagents still get `ToolSpec::subagent()` (no `delegate_subagent`). The role gate in `tool_delegate_subagent` is also unchanged. Recursion stays impossible.
- **Per-tool policies for read-side parallelism, per-path write locks.** Same reasoning as above.
- **Roster per-project override / per-session override.** The knobs are roster-level (one per `.harness/agents.json`).
- **Telemetry / cost tracking per subagent.** Existing per-turn `token_stats` already aggregates everything.
- **Anthropic provider tool calling.** Still deferred.
- **Cross-process worker pool.** Everything runs in the same Tauri command task as today.

## 3. Architecture

```
Orchestrator LLM round
    │
    ▼
[execute_tool_batch]   ← new dispatcher in agent_loop.rs
    │
    ├── 0..N non-dispatch calls (read/write/edit/cmd/search/bip/…)
    │       → run sequentially inside one tokio task
    │       → push results into the assistant message list
    │       → emit agent-tool-result per call (unchanged)
    │
    └── 0..M delegate_subagent calls
            → each call: insert SubagentRecord (status=Queued),
              return ToolOutcome { is_dispatch: true,
              result: ToolResult { ok: true,
                                    summary: "queued sub-…" } }
              immediately (no subagent turn spawned here)
            → concurrently: spawn dispatch_subagent_turn tasks up
              to semaphore permits (cap = roster.maxConcurrentSubagents,
              default 4); excess wait in a small FIFO queue and
              start as permits free
            → as each finishes (Done / Error / Cancelled), update
              registry, emit agent-done
            → execute_tool_batch awaits the full batch (all M tasks,
              queued + admitted) before returning, then folds results
              into ONE synthetic tool message that the loop pushes to
              the orchestrator's messages vec
            │
            ▼
        loop continues to next LLM round
```

The seam is `execute_tool_batch` in `agent_loop.rs`. From the orchestrator loop's perspective the call is synchronous: it runs inline calls and admits/awaits all dispatches to completion in a single round, then returns. Internally, dispatches execute concurrently. We don't rewrite the per-call pipeline (policy gate → `execute_tool` → `agent-tool-result` event); we just decide whether a call is "dispatch" (queued into the JoinSet) or "inline" (run now, sequentially).

A `tokio::sync::Semaphore` gates how many subagent turns may run concurrently. The dispatcher owns a `tokio::task::JoinSet` of admitted tasks; queued dispatches live in a small `VecDeque<QueuedDispatch>` drained as permits free. Both `Semaphore` and `JoinSet` are already available in the existing `tokio` dep tree (the codebase already uses `tokio::spawn`, `tokio::sync::oneshot`, `tokio::sync::watch`); no new crate. Cancellation propagates via the orchestrator's existing `in_flight` watch + a new per-subagent cancel watcher.

## 4. Data Model

### 4.1 Roster (v2 → v3)

**File:** `<projectPath>/.harness/agents.json`. New fields default in memory and on first save if missing.

```json
{
  "version": 3,
  "agents": [ /* unchanged shape */ ],
  "maxConcurrentSubagents": 4,
  "goalMaxTurns": 8
}
```

`load_agent_roster` accepts `version: 1 | 2 | 3`. Versions < 3 get the defaults applied during load and a one-time rewrite on next save. `save_agent_roster` always writes `version: 3`.

```rust
pub const DEFAULT_MAX_CONCURRENT_SUBAGENTS: u8 = 4;
pub const MIN_MAX_CONCURRENT_SUBAGENTS: u8 = 1;
pub const MAX_MAX_CONCURRENT_SUBAGENTS: u8 = 16;
pub const DEFAULT_GOAL_MAX_TURNS: usize = 8;
pub const MIN_GOAL_MAX_TURNS: usize = 1;
pub const MAX_GOAL_MAX_TURNS: usize = 32;

pub struct Roster {
    pub version: u32,
    pub agents: Vec<Personality>,
    pub max_concurrent_subagents: u8,
    pub goal_max_turns: usize,
}
```

The `personalities::default_roster()` helper applies both defaults and clamps the user values to the documented ranges. Frontend Agent Manager validation mirrors the same bounds.

### 4.2 SubagentDispatchContext — one new field, one new map

```rust
pub struct SubagentDispatchContext {
    pub subagents: Arc<DashMap<String, SubagentRecord>>,
    pub inbox: Arc<DashMap<String, TokioMutex<Vec<String>>>>,
    pub completer: Arc<dyn TurnCompleter>,
    pub pending_approvals: Arc<DashMap<String, oneshot::Sender<bool>>>,
    /// In-flight cancel watchers for the orchestrator (existing).
    pub in_flight: Arc<DashMap<String, watch::Sender<bool>>>,
    /// NEW: per-subagent cancel watchers, keyed by `sub-<uuid>`. Lets
    /// the frontend's "close a working subagent window" target just
    /// that subagent without touching siblings.
    pub subagent_cancel: Arc<DashMap<String, watch::Sender<bool>>>,
    pub sink: Arc<dyn AgentEventSink>,
    pub workspace: String,
    pub chat_model_id: String,
    pub mode: CommandMode,
    pub reasoning_effort: Option<ReasoningEffort>,
    /// NEW: caps concurrent subagent turns. Built once per orchestrator
    /// turn from the roster's `max_concurrent_subagents`.
    pub semaphore: Arc<Semaphore>,
    /// NEW: the orchestrator's `requestId` for the current turn. Set
    /// on insert into `subagents` so the frontend can group approvals
    /// by orchestrator batch.
    pub parent_request_id: String,
    /// NEW: roster's `goal_max_turns` for the current turn (replaces
    /// the `GOAL_MAX_TURNS` constant).
    pub goal_max_turns: usize,
}
```

`SubagentRecord.status` gains an explicit `Cancelled` variant (today it's effectively modeled as a `Done { content: "cancelled" }`; we make it first-class so the UI and the combined tool message can render it correctly without string sniffing).

### 4.3 Tool batch result — what the loop sees

```rust
pub struct ToolOutcome {
    pub call: ToolCall,                // echoes the input
    pub result: ToolResult,            // ok / summary / content
    /// True only for `delegate_subagent` calls; the result content is
    /// the queued summary, the real answer is folded into the batch
    /// synthetic message.
    pub is_dispatch: bool,
}
```

The loop pushes one `role: "tool"` message per *non-dispatch* outcome as today, plus **at most one** additional `role: "tool"` message for the entire dispatch group, formatted like:

```
delegate_subagent batch (3 dispatches):
  • sub-aaa (coder) — done: 412 chars
  • sub-bbb (architect) — failed: rate limit
  • sub-ccc (researcher) — cancelled
```

The synthetic message's `tool_call_id` is the orchestrator-side call id of the first dispatch in the batch, satisfying the provider's "tool result must echo a call id" pairing requirement.

## 5. Components

### 5.1 `src-tauri/src/agent_loop.rs`

- New private function `execute_tool_batch(...) -> Vec<ToolOutcome>`:
  - Splits the input `outcome.tool_calls` into `inline: Vec<ToolCall>` and `dispatch: Vec<ToolCall>` by name.
  - Runs the inline calls sequentially inside one task, emitting per-call `agent-tool-result` events.
  - For each dispatch call: calls `tool_delegate_subagent`, which now only inserts the registry row and returns `ToolOutcome { is_dispatch: true, … }`. No `run_subagent_turn` call.
  - Spins up a `JoinSet` task per dispatch. Each task acquires one `semaphore` permit (`.acquire_owned().await`), then calls a new `dispatch_subagent_turn(...)` that wraps the existing `run_subagent_turn` body, with an extra `select!` arm watching the orchestrator's `in_flight` cancel and the per-subagent cancel.
  - On any of: `Done` / `Error` / `Cancelled` / `JoinError` — updates the registry, emits `agent-done`, records the outcome.
  - When the inline calls + every dispatch task has resolved, returns the combined `Vec<ToolOutcome>` and the dispatch summary the loop will turn into the synthetic tool message.
- New private function `dispatch_subagent_turn(request_id, resolved, task, …, cancel_rx: watch::Receiver<bool>) -> TurnFinish` — body is the existing `run_subagent_turn`, with `select! { _ = cancel_rx.changed() => return TurnFinish::Error { kind: "cancelled", … }, result = run_turn_core(…) => result }` added.
- `run_turn_core` replaces its inline `for call in &outcome.tool_calls { … }` block with a single call to `execute_tool_batch` and a small follow-up step that pushes the synthetic dispatch message if any dispatches ran.
- `GOAL_MAX_TURNS` constant is removed; the turn loop reads `dispatch.goal_max_turns` from the dispatch context, which `commands::run_agent_turn` initializes from the loaded roster.
- `commands::run_agent_turn` builds `SubagentDispatchContext` with `semaphore: Arc::new(Semaphore::new(roster.max_concurrent_subagents as usize))`.

### 5.2 `src-tauri/src/tools.rs`

- `tool_delegate_subagent` shrinks: keep the role check, personality resolve, model resolve, and `dispatch.subagents.insert(...)`. Return `ToolResult { ok: true, summary: "queued sub-<uuid>", content: None }`. The actual `run_subagent_turn` call is removed from here; the new `dispatch_subagent_turn` (in `agent_loop.rs`) handles it.
- The subagent's own dispatch context (`Some(dispatch.clone())` passed to `run_subagent_turn`) gets the same semaphore. Since subagents cannot dispatch (`ToolSpec::subagent()` omits `delegate_subagent`, and the role gate in the executor is the backstop), no permits are ever acquired on a subagent turn — the semaphore is shared for code symmetry, not contention.

### 5.3 `src-tauri/src/personalities.rs`

- `Roster` gains the two fields above with serde defaults so old JSON loads cleanly.
- `default_roster()` returns a v3 roster with the two defaults.
- `save_agent_roster` round-trips both. `load_agent_roster` clamps to documented ranges and fills defaults.

### 5.4 `src/components/AgentManager.tsx`

- Two new labeled inputs, placed near the existing roster rows, with the same visual weight as the existing fields (per `DESIGN.md`):
  - "Max concurrent subagents" — number input, min 1, max 16, default 4.
  - "Max goal-mode continuation turns" — number input, min 1, max 32, default 8.
- The save handler re-loads the roster so the next orchestrator turn picks up the new semaphore size and turn cap. Roster version mismatch is non-fatal: if the file is on v1/v2, the modal shows the values the backend will apply (defaults) and writes v3 on save.

### 5.5 `src/lib/subagent-manager.ts` + `src/components/SubagentWindow.tsx`

- The store gains a derived selector `pendingApprovalsByBatch(orchestratorRequestId)` that groups pending `approval-request` events by the **owning orchestrator turn**. The grouping key isn't directly in the event today (the event carries the *subagent's* `sub-<uuid>` `requestId`); it comes from the registry, where each `SubagentRecord` is tagged at insert time with the orchestrator's `requestId` (a new field `parent_request_id: String`). When two or more subagents in the **same** orchestrator turn batch are waiting, the manager renders one card titled "N tools waiting" with one row per pending call: `[subagent name] · [tool] · [summary]` + Allow / Deny buttons.
- Deny on a row resolves just that subagent's `oneshot` sender. Allow does the same. The combined card closes when all rows are resolved.
- When only one tool is waiting across the whole session, the existing single-card path is the fallback (no behavior change for the simple case).
- Per-subagent cancel: closing a working subagent window sends `cancel_agent` for the subagent's `sub-` request id; the new `subagent_cancel` watcher flips, `dispatch_subagent_turn` exits, the registry goes to `Cancelled`.

### 5.6 No changes to

- Event vocabulary (`agent-turn-start`, `agent-chunk`, `agent-reasoning`, `agent-tool-start`, `agent-tool-result`, `agent-approval-request`, `agent-approval-result`, `agent-done`, `agent-error`).
- `sub-` request id prefix routing.
- `subagent_message` health inbox, the Health Agent's role-gated tool.
- The orchestrator's own `in_flight` cancel watcher and the existing `cancel_agent(request_id)` IPC.
- On-disk session format (`.harness/sessions/<id>.json`).
- The chat composer, Bip, the ActiveJobBar, the canvas tiling logic.
- The LLM client (`llm_client.rs`) and provider integration.

## 6. Error handling

- A subagent future that panics or is dropped: the dispatcher catches `JoinError`, sets `SubagentStatus::Error`, writes `final_content = "subagent turn aborted"` (never the panic message), emits `agent-done` with that status. The orchestrator's combined tool message shows `failed: aborted` for that id.
- A subagent exceeding the goal-mode cap: already handled by the existing `goal_turns` counter inside `run_subagent_turn`. With the new `dispatch.goal_max_turns` plumbed in, the cap is per-turn from the roster. The combined tool message shows `done: <n>/<cap> turns (cap)`.
- A subagent whose personality is unresolvable: `tool_delegate_subagent` returns `ok: false` synchronously, the dispatcher records `failed: <reason>` for that slot, no turn is spawned. Other dispatches in the same batch proceed.
- Whole-batch failure (e.g. semaphore acquire error, roster corrupt at load): the orchestrator turn is terminated with `TurnFinish::Error`, surfaced via the existing `agent-done` error event. Same UX as a single-call panic today.
- Approval timeout: a subagent's `oneshot` is dropped when the user navigates away or closes the subagent window without verdict. The pending tool is reported as `failed: approval cancelled`. The other dispatches in the same batch continue running.

## 7. Cancellation

- `cancel_agent(orchestrator_request_id)` flips the existing orchestrator `in_flight` watch. The dispatcher JoinSet drops; every in-flight `dispatch_subagent_turn` future is dropped; every queued dispatch is drained and marked `Cancelled` with message `"orchestrator cancelled before admission"`.
- Per-subagent cancel (the "X" on a working subagent window) flips the per-subagent `subagent_cancel` watcher for that one id only. The dispatcher's `select!` arm in `dispatch_subagent_turn` resolves, the future returns `TurnFinish::Error { kind: "cancelled", … }`, the registry goes to `Cancelled`, siblings keep running. The subagent window UI is unchanged — closing a window is still the trigger.
- Cancellation is observed at two safe points: between LLM rounds inside the subagent turn, and at the top of every `gate_tool` call. We don't try to interrupt an in-flight provider HTTP request — same posture as today.

## 8. Testing

Rust, matching the codebase's `cargo test` discipline. New tests live in `src-tauri/src/agent_loop.rs` and `src-tauri/src/tools.rs` (extending the existing `// -- delegate_subagent / subagent_message --` block). The frontend test extends `src/lib/wm.test.ts` (no new test infra).

1. **`dispatch_is_parallel_within_cap`** — orchestrator turn emits 3 `delegate_subagent` calls. Mock completer delays 500ms per call. Assert wall time ≤ 600ms (parallel, not 1500ms serial). All 3 registry rows reach `Done`.
2. **`dispatch_queues_over_cap`** — 5 dispatches with `max_concurrent_subagents = 2`. Assert: 2 run concurrently, 3 sit in queue, all 5 finish, no more than 2 run at any instant (use a `AtomicUsize` "in-flight" counter from the mock).
3. **`dispatch_combines_results_into_one_tool_message`** — 2 dispatches. Assert: assistant message history contains exactly one new `role: "tool"` entry covering both ids, with the combined batch summary as content.
4. **`dispatch_cancels_siblings_on_orchestrator_cancel`** — 4 dispatches, trigger cancel mid-flight. Assert: 0 rows remain `Working`, all 4 are `Cancelled`, no LLM round fires after the cancel.
5. **`subagent_solo_cancel_doesnt_touch_siblings`** — 2 parallel dispatches, cancel just one. Assert: the other finishes normally, the cancelled one is `Cancelled`, both rows visible in registry.
6. **`max_concurrent_subagents_round_trips_through_roster`** — save roster with `maxConcurrentSubagents = 8, goalMaxTurns = 16`, reload, assert equal. Older v1/v2 file gets the defaults applied on load.
7. **`max_concurrent_subagents_clamps_out_of_range`** — load a roster with `maxConcurrentSubagents = 99` and `goalMaxTurns = 0`. Assert: clamped to 16 and 1 respectively.
8. **`delegation_with_zero_dispatches_is_a_noop`** — orchestrator turn emits only inline calls. Assert: no synthetic batch message is pushed, no semaphore permits acquired.
9. **`goal_max_turns_is_honored_from_roster`** — set `goalMaxTurns = 3` in roster, run a goal-mode subagent that emits 5 rounds. Assert: turn terminates after 3 rounds with `done: 3/3 turns (cap)`.
10. (Frontend) **`subagent_manager_aggregates_pending_approvals`** — emit 3 `approval-request` events from 3 different subagents in the same orchestrator turn batch. Assert: a single multi-row card renders with 3 rows; clicking Allow on row 2 resolves only row 2's `oneshot`.
11. (Frontend) **`single_approval_falls_back_to_existing_card`** — emit 1 `approval-request`. Assert: existing single-card UI is rendered (no regression).

## 9. Migration & compatibility

- Older `agents.json` files (v1 / v2) load via the existing `load_agent_roster` path with the two new fields defaulted. No migration script; first save upgrades to v3.
- Older `sessions.json` (v2) on disk: untouched. The orchestrator's per-turn tool loop rewrite is fully internal.
- Wire / IPC: `run_agent_turn`, `cancel_agent`, `respond_approval`, `load_agent_roster`, `save_agent_roster` are all backward-compatible. The new `subagent_cancel` map is internal; no new IPC command.
- Frontend: Agent Manager UI is the only visible change. The SubagentWindow and subagent-manager are additive — the new multi-row card path activates only when ≥2 subagents in the same batch are waiting.

## 10. Open questions deferred

- Should `maxConcurrentSubagents` be per-project (current plan) or per-session (override at session start)? Defer until we see real multi-project usage.
- Should the semaphore cap also throttle **non-dispatch** tool calls in the same batch? Out of scope; we run them inline.
- Should the user be able to "promote" a running subagent to orchestrator-level visibility (pin it to the chat)? Cool idea, not in this spec.
