# Implementation Plan — Agent Sessions + Real Tool Loop

**Source spec:** `docs/superpowers/specs/2026-08-12--agent-sessions-and-real-tool-loop.md`
**Date:** 2026-08-12
**Branch:** `feat/agent-sessions` (cut from `main`; all work lands here, never on `main` directly)

## Execution order

Dependency-respecting packages. Each package ends with a **verification gate**; do not start the next package until the gate passes. Packages WP1–WP6 are Rust, WP7–WP8 are TS/UI, WP9 is the final pass.

```
WP0 repo hygiene → WP1 session store → WP2 roster → WP3 llm tools
  → WP4 tool registry → WP5 agent loop → WP6 commands/IPC
  → WP7 TS ipc/store/lifecycle → WP8 UI → WP9 verification + docs
```

---

## WP0 — Repo hygiene

- Extend `.gitignore`: add `.harness/`, `dist/`, `lmmc.rs.bak`, `stderr*.txt`, `stdout.txt`, `spec_toc.txt`.
- Commit: `chore: gitignore app state and scratch files`.
- Gate: `git status --short` shows a clean tree (only intended files).

## WP1 — Rust: multi-session store (`src-tauri/src/session.rs`)

**Replace** the single-file session helpers with a sessions-directory store:

- `Session` struct → v2: add `id`, `title`, `agent` (snapshot: id/name/role/systemPrompt), `goal_mode`, `action_mode` (`auto|askRisky|askAll`), `activities: Vec<ToolActivity>`; keep `messages`, `token_stats`. All camelCase at the serde boundary.
- `ToolActivity { id, tool, summary, status: ran|denied|error, timestamp }`.
- Path helpers: `sessions_dir(project)`, `session_path(project, id)`, strict id validator (regex `^[0-9a-fA-F-]{1,64}$` — reject anything else before touching fs).
- `SessionSummary { id, title, projectPath, projectName, createdAt, updatedAt, messageCount, agentId, agentName }` derived from parsed files.
- Functions: `list_sessions` (readdir, parse, skip corrupt w/ archive, sort updatedAt desc), `create_session` (default title "New session", agent from roster default), `load_session(project, id)`, `save_session` (atomic tmp+rename, rewrites updatedAt), `rename_session` (title only), `delete_session` (remove file + `.active` if it pointed here).
- `.active` marker: read/write `sessions/.active` (atomic), `resolve_active(project) -> Option<String>` (marker if valid else newest).
- `migrate_v1_if_needed(project)`: if `session.json` exists AND `sessions/` empty → build v2 session, write, set `.active`, rename `session.json` → `session.json.migrated-<ts>`.
- Keep recents / file listing / path normalization untouched.
- **Update existing tests** (`save_and_load_roundtrip`, `session_wire_format_is_camelcase`, `corrupt_session_is_archived`) to the v2 shape; add tests: CRUD roundtrip, `.active` behavior (marker valid/invalid/missing), migration (fresh, populated, corrupt v1), id validation rejects `../`, `a/b`, empty, `!`; list sort order; unknown-version archiving.
- Gate: `cargo test` green (run inside `src-tauri`), `cargo fmt --check`.

## WP2 — Rust: agent roster (`src-tauri/src/agents.rs`, new)

- `AgentPersonality { id, name, role: AgentRole, system_prompt, builtin: bool }`; `AgentRoster { version: 1, max_agents_per_session: u32, default_agent_id: String, agents: Vec<AgentPersonality> }`.
- `load_agent_roster(project)` — creates default roster (builtin `harness`/Orchestrator persona) on first open; `save_agent_roster` atomic; corrupt → archive + default.
- Role enum reused from `agent.rs` (no changes there).
- Tests: default creation, roundtrip, corrupt archive, max-agents clamp (≥1), builtin-not-deletable rule enforced at save.
- Gate: `cargo test` green, `cargo clippy` clean.

## WP3 — Rust: `complete_with_tools` (`src-tauri/src/llm_client.rs`)

Parallel function, `run_chat` untouched:

- New types: `ToolSpec { name, description, parameters: serde_json::Value }`, `ToolCall { id, name, arguments: Value }`, `TurnOutcome { text, tool_calls: Vec<ToolCall>, usage: ChatUsage }`.
- `complete_with_tools(request, tools: Vec<ToolSpec>, text_cb: impl Fn(&str), cancel_rx) -> Result<TurnOutcome, AgentTurnError>`.
- **OpenAI:** body adds `"tools": [...]`; aggregate `delta.tool_calls` (index-keyed id/name/arguments-fragment); keep streaming content via `text_cb`; `finish_reason == "tool_calls"` → return aggregated calls. Result messages built as `{role:"tool", tool_call_id, content}`.
- **Anthropic:** body adds `"tools": [...]`; aggregate `content_block_start` (tool_use id/name), `content_block_delta` (`input_json_delta.partial_json` concat), `content_block_stop`; text deltas via `text_cb`; if any tool_use blocks → return them. Results sent back as `{role:"user", content:[{type:"tool_result", tool_use_id, content}]}`.
- **System prompt mapping:** OpenAI → `system` role message; Anthropic → top-level `system` field.
- Refactor shared bits (resolve, error mapping, retry-after, cancellation select) into helpers reused by both `run_chat` and `complete_with_tools`.
- Tests: pure helper fns only (message/body builders, tool-call fragment aggregation state machine) — no network.
- Gate: `cargo test` green, `clippy` clean.

## WP4 — Rust: tool registry (`src-tauri/src/tools.rs`, new)

- `ToolSpec::all() -> Vec<ToolSpec>` with JSON schemas: `read_file`, `list_dir`, `search_files`, `write_file`, `edit_file`, `run_command`, `web_search`.
- `execute_tool(workspace, name, args, auto_allowed) -> Result<ToolResult, String>`; `ToolResult { ok, summary, content: Option<String> }` (content capped 32 KB).
- **Workspace guard** (`fn resolve_within(workspace, target) -> Result<PathBuf>`): lexical normalize for non-existent; `canonicalize` for existing; Windows case-insensitive compare; reject symlink escapes (canonicalize parent chain).
- `edit_file`: exact single-occurrence replace (fail on 0 or >1 matches); `write_file` creates parents; `run_command`: args array, cwd forced within workspace, 60 s timeout (kill child), stdout/stderr truncated 16 KB each; `search_files` reuses existing walker (depth-capped); `web_search` wraps existing client (auto-allowed).
- `auto_allowed` derived from action mode: `auto` → all; `askRisky` → read/list/search/web_search; `askAll` → none.
- Tests: guard matrix (inside/outside/nonexistent/`..`/symlink-out), `edit_file` semantics (0/1/many matches), command timeout + truncation (tiny `cmd`/`echo` helpers, `cfg(windows)` guarded), web_search arg passthrough.
- Gate: `cargo test` green, `clippy` clean.

## WP5 — Rust: agent loop (`src-tauri/src/agent_loop.rs`, new)

- `run_agent_turn(session, model_id, message_text, app, state)` spawns tokio task; returns immediately.
- Loop state: working conversation (provider-form messages incl. tool blocks), iteration count, last-3-calls ring for stall detection, `[continue]` marker.
- Flow: system prompt (personality + workspace root + tool guidance) → `complete_with_tools` → if tool calls: per-call policy gate → execute (`tools.rs`) → emit events → append provider result messages → next iteration; else final text → goal-mode `[continue]` check → done.
- Policy gate: `auto`/`askRisky`/`askAll` mapping to `tools::auto_allowed`; non-auto → insert oneshot in `state.pending_approvals`, emit `agent-approval-request`, await; `respond_approval` resolves; loop drop → resolve all pending as denied.
- Cap: 24 tool rounds → `agent-error { kind: "loop_cap" }`. Stall: 3 identical calls with no change → `agent-error { kind: "stalled" }` with plain-language message.
- Events emitted (camelCase): `agent-turn-start`, `agent-chunk`, `agent-tool-start`, `agent-tool-result`, `agent-approval-request`, `agent-approval-result`, `agent-done` (content + usage), `agent-error`.
- Token stats: peak-in / sum-out rules; persisted to session on `agent-done` (via save callback).
- **Mock seam:** `complete_with_tools` behind a small trait/fn param so tests run offline.
- Tests: turn with mocked tool calls → results fed back; cap exceeded; stall detected; `[continue]` loop continues then ends; approval deny ends turn; cancel mid-approval resolves denied.
- Gate: `cargo test` green, `clippy` clean.

## WP6 — Rust: commands + registration (`commands.rs`, `lib.rs`)

- `AppState` gains `pending_approvals: DashMap<String, oneshot::Sender<bool>>`.
- New commands: `list_sessions(projectPath)`, `create_session(projectPath, agentId?)`, `load_session(projectPath, sessionId)`, `save_session(projectPath, session)`, `rename_session(projectPath, sessionId, title)`, `delete_session(projectPath, sessionId)`, `load_agent_roster(projectPath)`, `save_agent_roster(projectPath, roster)`, `run_agent_turn(...)`, `respond_approval(requestId, allow)`, `cancel_agent(requestId)`.
- Old `load_session(projectPath)` command is **replaced** by the 2-arg version (ipc.ts updated in WP7). `list_sessions`/`forget_session` were never registered — nothing to remove.
- `run_agent_turn` args: `requestId, sessionId, modelId, messageText, actionMode, goalMode` (session is re-loaded from disk inside the task so the loop works with the persisted posture).
- Register all in `lib.rs`.
- Gate: `cargo build` clean; `cargo test` green; `clippy` + `fmt` clean.

## WP7 — TS: ipc, store, lifecycle (`src/lib/ipc.ts`, `store.ts`, `session-manager.ts`)

- `ipc.ts`: replace stub `listSessions`/`loadSessionById`/`forgetSession` with project-scoped `listSessions(projectPath)`, `createSession`, `loadSession(projectPath, id)`, `saveSession`, `renameSession`, `deleteSession`; add `loadAgentRoster`/`saveAgentRoster`; add `runAgentTurn`, `respondApproval`, `cancelAgent`; add `agentEvents` listener family (turnStart, chunk, toolStart, toolResult, approvalRequest, approvalResult, done, error) mirroring `chatEvents`; update `SessionSummary` (+`agentName`, `actionMode`, `goalMode`); add `ToolActivity`/`AgentRoster` types.
- `store.ts`: add `agentsOpen` + setters; `actionMode`/`goalMode` (initialized from active session); `liveActivityFeed: ToolActivity[]`; `agentRoster` + `defaultAgentId`; keep `activeSessionId` (now real); `streamingRequestId` reused by the agent path.
- `session-manager.ts`: rework to key saves by `projectPath + activeSessionId`; on project open → `list_sessions` + `resolve_active` → load active session (or newest) → set messages/agent/actionMode/goalMode; flush (debounced save) on project switch *and* session switch; migration happens implicitly on first `list_sessions`.
- Gate: `npx tsc --noEmit` green.

## WP8 — TS UI

- **Sidebar** (`Sidebar.tsx`): Sessions panel — list from `list_sessions(projectPath)`; row click → switch (flush → `loadSession` → populate store → `saveSession`-keyed state); **+ New session** in panel header (also in the empty state); hover **rename** (inline input, Enter saves); **delete** (existing X → confirm step; deletes + clears active if needed).
- **Chat** (`Chat.tsx`): submit routes to `runAgentTurn` (plain `chatComplete` no longer used); subscribe `agentEvents` (route by `streamingRequestId`); render `activities` as compact mono rows (tool name + summary + status dot) between bubbles; **approval card** component (tool name, args summary, Approve/Deny → `respondApproval`); **composer options row** above input: action-mode segmented control (`Auto`/`Ask risky`/`Ask all`), Thinking toggle (moved from the model row), Goal toggle; persist to session via `saveSession`.
- **ActiveJobBar** (`ActiveJobBar.tsx`): status from `liveActivityFeed` — idle / working (`<agent> is <verb> <target>` built from current activity) / waiting-on-approval; hover popover renders the live feed (same recipes as today).
- **AgentManager** (`AgentManager.tsx`, new modal, chrome from `Settings.tsx`): roster list + edit form + New personality + Max agents per session + Default personality radio; save via `saveAgentRoster`; opened from a job-bar button; empty state when no project open.
- Gate: `npx tsc --noEmit` green; `npm run dev` manual smoke (web) — sidebar/session flows work with mock-less backend unavailable? (web build can't invoke Rust; smoke in Tauri).

## WP9 — Verification + docs

- `npx tsc --noEmit`; `cargo fmt --check`; `cargo clippy -- -D warnings`; `cargo test`.
- Manual acceptance (Tauri): spec §10 checklist — migration, 3-session CRUD, tool-call session with Auto, goal-loop multi-edit + Stop, approval pause/resume, kill-app mid-loop reopen, live job bar.
- Reconcile `DESIGN.md` (sessions panel + AgentManager + composer row + job bar) and `PRODUCT.md` (agent loop now real; update "stubbed/partial" section).
- Final: commit on `feat/agent-sessions`; present diff summary to user; do **not** merge/push without explicit approval (no remote configured — user decides remote/merge).

## Risks & mitigations

- **Anthropic tool-result history:** must include the assistant tool_use blocks when replaying — the working conversation is the single source during a turn; persisted messages stay human-readable (per spec).
- **Windows command exec:** args array + `CreateProcess` semantics differ from shell — tests use `cmd /c echo` guarded by `cfg(windows)`; never pass a raw shell string.
- **Approval UX mid-stream:** frontend routes approval events by `requestId`; a stopped loop resolves pending approvals as denied, so no orphaned cards.
- **Legacy chat regression:** `chatComplete`/`chatEvents` remain registered; only the session composer switches to the agent path.
