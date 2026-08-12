# Real Agent Sessions + Tool-Calling Agent Loop

**Status:** Draft for review
**Date:** 2026-08-12
**Scope:** Phase 2.5 — multi-session model, agent manager foundation, and a real tool-calling agent loop for Harness

## 1. Summary

This spec makes Harness's sessions real and makes its agents capable of actual work.

1. **Multi-session model** — a project now owns *many* sessions, each persisted as its own file under `<project>/.harness/sessions/<id>.json`. The existing single-`session.json` (Phase 1.5 spec) auto-migrates into the first session. The sidebar Sessions panel is wired to real backend commands (create / switch / rename / delete).
2. **Agent tool loop** — the agent becomes a real worker. A new Rust `agent_loop` drives provider-native tool calling (OpenAI `tools` / Anthropic `tool use`) on top of the existing streaming client, with a workspace-guarded tool registry (read/edit files, run commands, web search), an action-policy gate (Auto / Ask-before-risky / Ask-before-all), a persisted per-session **goal loop**, and a full live event stream to the UI.
3. **Multi-agent foundation** — an **Agent Manager** window (same modal pattern as Settings) manages per-project agent personalities in `.harness/agents.json` plus the max-agents-per-session limit. This sprint the session runs a single capable agent; the data model, event stream, and roster are shaped so the Orchestrator → sub-agent dispatch lands next sprint without a migration.
4. **Top bar gets live** — the ActiveJobBar renders real agent activity in natural language, and its hover popover shows the live tool-activity feed.

The goal: a user opens a project, creates a session, gives the agent a goal, flips the goal loop on, and the agent reads the repo, edits files, and runs commands — with the user watching and able to intervene — and everything is persisted back into `.harness/` so reopening the project restores the exact working state.

This spec supersedes the Phase 1.5 decision in `docs/superpowers/specs/2026-08-11--sessions-mentions-project-opening-and-rich-markdown.md` (§2: "Multi-session per project — we deliberately chose one chat per project root").

## 2. Out of Scope

Explicitly deferred (foundation hooks are noted where relevant):

- **Sub-agent spawning / Orchestrator dispatch.** The multi-agent loop (Orchestrator → Coder/Architect/Researcher, Health Monitor supervisor) is the *next* sprint. This spec builds the roster file, the session `agent` snapshot, and the event vocabulary those features will use.
- **Canvas auto-tiling from the agent graph.** `DynamicCanvas` keeps its current single-panel behavior. Tiling is triggered by sub-agent count next sprint.
- **Diff preview engine** (unified diff rendering of `edit_file`/`write_file`). Tool results carry a short textual summary only; full diffs are deferred.
- **Plan-mode / pre-flight approval of an entire goal.** Only per-tool-call approval is in scope.
- **File watching / live tree refresh.** Unchanged from Phase 1.5.
- **Two windows on the same project.** Last-writer-wins remains acceptable; per-project locking (`fs2`) is deferred.
- **Mobile / web-terminal parity.** The web build runs the loop minus filesystem/terminal tools (they fail gracefully with a tool-level error).
- **Telemetry / analytics.** Unchanged.

## 3. Architecture

```
┌───────────────────────────────────────────────────────────────────────┐
│ React frontend                                                        │
│  ┌────────────┐ ┌───────────────┐ ┌────────────────┐ ┌────────────┐   │
│  │ Sidebar    │ │ AgentManager  │ │ Chat composer  │ │ ActiveJob  │   │
│  │ Sessions   │ │ (modal)       │ │ row: action /  │ │ Bar (live) │   │
│  │ panel      │ │               │ │ thinking /goal │ │ + popover  │   │
│  └─────┬──────┘ └──────┬────────┘ └───────┬────────┘ └─────┬──────┘   │
│        │               │                  │                │          │
│        ▼               ▼                  ▼                ▼          │
│  ┌────────────────────────────────────────────────────────────────┐   │
│  │ Zustand store (+ agentsOpen, activeSessionId, actionMode,     │   │
│  │ goalMode, liveActivityFeed)                                   │   │
│  └──────────────────────────┬─────────────────────────────────────┘   │
│                             │ ipc + agent events (listen)             │
└─────────────────────────────┼─────────────────────────────────────────┘
                              ▼
┌───────────────────────────────────────────────────────────────────────┐
│ Tauri IPC (Rust)                                                       │
│  sessions: list/create/load/save/rename/delete_session (+ project)    │
│  roster:   load/save_agent_roster                                     │
│  agents:   run_agent_turn, respond_approval, cancel_agent (reuses     │
│            in_flight)                                                 │
│  events → UI: turn-start, chunk, tool-start, tool-result,             │
│            approval-request, approval-result, done, error             │
└───────────┬───────────────────────────────────────────────────────────┘
            ▼
┌───────────────────────────────────────────────────────────────────────┐
│ Rust core                                                             │
│  session.rs    → multi-session store in <project>/.harness/sessions/ │
│  agents.rs     → roster (agents.json) + Agent personality model      │
│  agent_loop.rs → tool-calling loop, policy gate, goal loop, health   │
│  tools.rs      → workspace-guarded tool registry                     │
│  llm_client.rs → + tool aggregation (OpenAI tools / Anthropic tools) │
│  web_search.rs → unchanged (tool-wrapped)                            │
└───────────────────────────────────────────────────────────────────────┘
```

**Component boundaries**

| Unit | What it does | Depends on |
|---|---|---|
| `session.rs` (Rust) | Multi-session CRUD, migration from v1, `.active` marker, atomic writes, corrupt-file archiving | `tokio::fs`, `serde` |
| `agents.rs` (Rust) | Roster load/save (`.harness/agents.json`), defaults on first open | `tokio::fs` |
| `agent_loop.rs` (Rust) | Runs one agent turn: provider call → tool calls → policy gate → execute → repeat; emits events; goal cap + stall detection | `llm_client`, `tools`, `commands::AppState` |
| `tools.rs` (Rust) | Tool specs (JSON schema) + executors, workspace path guard, command timeout | `tokio::process`, `web_search` |
| `llm_client.rs` (Rust) | New: `complete_with_tools()` returning text + tool calls + usage for both providers | existing streaming plumbing |
| `Sidebar` (TS) | Sessions panel: list/create/switch/rename/delete against project | `ipc`, store |
| `AgentManager` (TS) | Modal: roster CRUD + max-agents-per-session | `ipc`, store |
| `Chat` (TS) | Composer options row (action mode / thinking / goal), agent event subscription | `ipc`, `chatEvents`-style listener |
| `ActiveJobBar` (TS) | Live natural-language activity + popover feed from events | store `liveActivityFeed` |

Every unit is understandable without reading the others; the contract is the `ipc` namespace, the event payloads, and the store shape.

## 4. Data Model

### 4.1 Session store (v2)

**Location:** `<projectPath>/.harness/sessions/<id>.json`, one self-contained file per session. No index file — `list_sessions` reads the directory and parses each file. Session `id` is a UUID; the loader validates ids with a strict `[0-9a-fA-F-]` check before touching the filesystem (path-traversal guard for the sessions dir).

**Shape (v2):**

```json
{
  "version": 2,
  "id": "9f2c...",
  "title": "Fix login redirect — harness",
  "projectPath": "C:/code/myapp",
  "createdAt": "2026-08-12T10:00:00Z",
  "updatedAt": "2026-08-12T10:30:00Z",
  "agent": {
    "id": "harness",
    "name": "harness",
    "role": "Orchestrator",
    "systemPrompt": "You are the orchestrator. …"
  },
  "goalMode": false,
  "actionMode": "askRisky",
  "messages": [
    {
      "id": "abc",
      "role": "user",
      "content": "Explain @src/auth/login.ts",
      "timestamp": "2026-08-12T10:00:00Z",
      "mentions": [{ "path": "src/auth/login.ts" }]
    }
  ],
  "activities": [
    {
      "id": "act1",
      "tool": "write_file",
      "summary": "wrote src/auth/login.ts (+42 lines)",
      "status": "ran",
      "timestamp": "2026-08-12T10:29:00Z"
    }
  ],
  "tokenStats": { "lastIn": 0, "lastOut": 0, "cumulativeIn": 0, "cumulativeOut": 0 }
}
```

- **`agent`** — snapshot of the personality that drove the session (from the roster). Persisting the snapshot means reopening a session restores the exact agent even if the roster later changes; the orchestrator dispatch next sprint will reuse this field.
- **`goalMode`** / **`actionMode`** — persisted per session so reopening restores the working posture. `actionMode`: `"auto" | "askRisky" | "askAll"`.
- **`messages`** — same shape as today (roles `user | assistant | system`, `mentions`, optional `agentId`/`agentName`). Tool traffic does **not** become fake chat bubbles.
- **`activities`** — the persisted tool-work record (new). One entry per tool execution: id, tool name, one-line summary, status (`ran | denied | error`), timestamp. Rendered as compact activity entries in the chat stream and in the job-bar popover. `summary` is derived on the Rust side from the tool result (e.g. `wrote src/x.ts (+42 lines)`, `ran npm test (exit 0, 1m12s)`).
- `version: 2` — loader starts fresh on unknown versions, archiving the file version-keyed (existing pattern).

**`SessionSummary`** (already in `src/lib/ipc.ts`) gains `agentName` and is now derived from the parsed file. `list_sessions` sorts `updatedAt` descending. Per-project file list skips the `.harness` dir itself (already in `IGNORE_DIRS`).

### 4.2 Active-session marker

**Location:** `<projectPath>/.harness/sessions/.active` — a single line containing the last-active session id. On project open: if `.active` points at an existing session, load it; otherwise load the most recently updated session; if none, show the empty state. Written on every session switch (cheap, atomic tmp+rename). This is project-local state, not global, so recents stay untouched.

### 4.3 Agent roster

**Location:** `<projectPath>/.harness/agents.json`. Created on first project open with one builtin personality.

```json
{
  "version": 1,
  "maxAgentsPerSession": 3,
  "agents": [
    {
      "id": "harness",
      "name": "harness",
      "role": "Orchestrator",
      "systemPrompt": "You are the orchestrator. …",
      "builtin": true
    }
  ]
}
```

- `role` uses the existing `AgentRole` enum (`Orchestrator | Coder | Architect | Researcher | HealthMonitor`).
- `builtin` personalities can be edited but not deleted; user-created ones can be deleted.
- This sprint: the session runs the primary personality (`harness` by default; the Agent Manager lets the user pick which personality drives new sessions). `maxAgentsPerSession` is stored now; the orchestrator consumes it next sprint.

### 4.4 Migration from v1

On first `list_sessions`/`load` for a project: if `<project>/.harness/session.json` exists **and** `.harness/sessions/` is empty/missing → read v1, build a v2 session (new UUID, title from first user message or "Session"), write `sessions/<id>.json`, set `.active`, then rename `session.json` → `session.json.migrated-<unix_ts>` (kept as a backup, never deleted). Corrupt v1 files are archived with the existing version-keyed pattern.

## 5. IPC Surface

All session commands become **project-scoped** (the Phase 1.5 stub signatures without `projectPath` are replaced — `load_session_by_id`/`forget_session`/`list_sessions` in `src/lib/ipc.ts` are updated to match):

| Command | Args | Returns |
|---|---|---|
| `list_sessions` | `projectPath` | `Vec<SessionSummary>` |
| `create_session` | `projectPath`, `agentId?` | `Session` (empty, v2) |
| `load_session` | `projectPath`, `sessionId` | `Session` |
| `save_session` | `projectPath`, `session` | `Session` (rewrites `updatedAt`) |
| `rename_session` | `projectPath`, `sessionId`, `title` | `SessionSummary` |
| `delete_session` | `projectPath`, `sessionId` | `()` (removes file + `.active` if it pointed here) |
| `load_agent_roster` | `projectPath` | `AgentRoster` (defaults on first open) |
| `save_agent_roster` | `projectPath`, `roster` | `AgentRoster` |
| `run_agent_turn` | `sessionId`, `modelId`, `actionMode`, `goalMode`, `messageText` | `()` — spawns, streams events |
| `respond_approval` | `requestId`, `allow: bool` | `()` |
| `cancel_agent` | `requestId` | `()` (reuses in-flight cancel map) |

Session-id validation: strict `[0-9a-fA-F-]` whitelist before any filesystem access. All wire types stay camelCase (serde `rename_all` boundary).

Frontend debounced save (existing `session-manager.ts`) reworks to track **active session per project**: debounce keyed by `projectPath + sessionId`, flush on project switch *and* session switch.

## 6. Agent Runtime

### 6.1 `agent_loop.rs` — the turn loop

`run_agent_turn` spawns a tokio task (same pattern as `chat_complete`):

1. Build provider messages: system prompt (session agent's `systemPrompt` + workspace root path + tool usage guidance) + session history (excluding `activities`; assistant tool-call history is reconstructed from the loop's own turn state, not from persisted messages — persisted messages stay human-readable).
2. Call `llm_client::complete_with_tools(model_id, messages, tools, thinking/reasoning prefs, cancel_rx)`.
   - Text deltas stream to the UI immediately as `agent-chunk` (the assistant message is appended client-side and grown, like today).
   - Return value: `TurnOutcome { text, tool_calls: Vec<ToolCall>, usage }`.
3. If `tool_calls` non-empty:
   - For each call: **policy gate** → `auto`: execute; `askRisky`: execute `read_file`/`list_dir`/`search_files`/`web_search` automatically, emit `approval-request` and wait for others; `askAll`: emit `approval-request` and wait for every call.
   - Wait = one `oneshot` per request stored in `AppState.pending_approvals`; `respond_approval` resolves it. Loop cancellation or task drop resolves all pending approvals as `denied` (clean abort, no hanging UI).
   - Execute via `tools::execute_tool`, emit `tool-start` (before) and `tool-result` (after; summary + status), append the provider-appropriate tool-result message, **go to 2**.
4. If no tool calls: this is a final text response. In non-goal mode, end the turn. In goal mode, the loop continues only if the model explicitly asked to continue (goal mode instructs the model to emit a `continue` marker in its text when work remains — see §6.4); otherwise end.
5. Emit `agent-done { content, usage }`; update session `tokenStats` (peak-in / sum-out rules from Phase 2.0) and append the final assistant message.

Iteration cap: **24 tool-call rounds** per turn in both modes (a goal loop is a single `run_agent_turn` with a long horizon). Exceeding the cap emits `agent-error { kind: "loop_cap" }`.

**Stall detection (Health Monitor seed):** if 3 consecutive tool calls are identical (`tool` + `args` equal) and the last produced no workspace change, the loop stops with `agent-error { kind: "stalled" }` and a plain-language message. Identical-call detection is per-turn state.

### 6.2 Tool registry — `tools.rs`

Every tool: JSON-schema spec (for provider tool definitions) + async executor `(workspace_root, args) -> ToolResult`.

| Tool | Auto-run in `askRisky`? | Notes |
|---|---|---|
| `read_file(path)` | yes | path guard |
| `list_dir(path)` | yes | shallow, non-recursive |
| `search_files(pattern, path?)` | yes | substring/regex search, depth-capped (reuses existing walker) |
| `write_file(path, content)` | no | creates parent dirs; path guard |
| `edit_file(path, old, new)` | no | exact-replacement edit; fails if `old` not found once |
| `run_command(args, cwd?)` | no | args array (no shell string), 60s timeout, output truncated 16 KB per stream |
| `web_search(query)` | yes | existing DuckDuckGo client |

**Workspace path guard (all file tools):** resolve the target and assert it is inside `workspace_root` after normalization (lexical for non-existent paths, `canonicalize` for existing ones, case-insensitive compare on Windows). A violation returns a tool error, never an exception. `run_command` defaults `cwd` to the workspace root and refuses `cwd` values outside it.

**ToolResult:** `{ ok: bool, summary: String, content: Option<String> }` — `summary` feeds `activities[]` and the job bar; `content` is the full output fed back to the model (truncated to ~32 KB).

### 6.3 Provider tool calling — `llm_client.rs`

New function `complete_with_tools` (sibling of `run_chat`, shares `resolve`, error mapping, cancel wiring):

- **OpenAI (`openai-chat-completions`):** add `"tools": [{ "type": "function", "function": { name, description, parameters } }]` to the request body. Aggregate streamed `choices[].delta.tool_calls` (index-keyed: id, name, `arguments` JSON fragment concatenation) while still streaming `content` deltas. `finish_reason == "tool_calls"` → return the aggregated calls. Tool results are appended as a `"role": "tool"` message with `tool_call_id`.
- **Anthropic (`anthropic-messages`):** add `"tools": [{ name, description, input_schema }]`. Aggregate SSE `content_block_start` (`type: tool_use`, captures id + name), `content_block_delta` (`input_json_delta` partial_json concatenation), `content_block_stop`. If any tool_use blocks were captured, return them instead of emitting `chat-done`. Tool results are sent back as a `"user"` message with `content: [{ type: "tool_result", tool_use_id, content }]` per the Anthropic API.
- Text deltas keep streaming through a callback so the UI stays live during reasoning between tool calls.
- Existing `run_chat` stays for the plain-chat path; `complete_with_tools` is a parallel function (no regressions to the current chat UX).

### 6.4 Goal loop

The composer **Goal** toggle persists `goalMode` on the session. Semantics:

- **Off (default):** one user message → one agent turn. The agent may call tools within that turn (that's how "do this one task" works), then replies and stops.
- **On:** the session's working objective is the user's message. The system prompt instructs: work autonomously toward the goal, and when work remains, end your reply with the literal marker `[continue]`. The loop keeps going (capped at 24 rounds) until a final reply without `[continue]`, a stall/loop error, a cancellation, or an approval denial.

The marker is a deliberate, greppable protocol (no hidden state, works with any model), and `[continue]` is stripped from the rendered message.

## 7. Event Stream (Rust → UI)

All events camelCase, payload keyed by `requestId` (== the in-flight request id, displayed like today):

| Event | Payload | UI effect |
|---|---|---|
| `agent-turn-start` | `{ requestId }` | job bar "working" |
| `agent-chunk` | `{ requestId, delta }` | grow assistant bubble |
| `agent-tool-start` | `{ requestId, tool, argsSummary, activityId }` | activity feed entry (pending), job bar sentence |
| `agent-tool-result` | `{ requestId, activityId, ok, summary, status }` | finalize activity entry |
| `agent-approval-request` | `{ requestId, activityId, tool, argsSummary, description }` | approval prompt (inline in chat / modal), job bar "waiting" |
| `agent-approval-result` | `{ requestId, activityId, allow }` | close prompt |
| `agent-done` | `{ requestId, content, usage }` | finalize message + token stats, job bar idle |
| `agent-error` | `{ requestId, kind, error, retryAfterSeconds? }` | existing error handling + bubble policy |

`kind` reuses the Phase 2.0 vocabulary (`auth | rate_limit | network | provider | bad_request | internal`) plus `loop_cap | stalled`.

The frontend subscribes once (pattern from `Chat.tsx`), routes by `requestId === streamingRequestId`, and maintains `liveActivityFeed` in the store (mirrors `activities[]` while streaming; persisted at turn end).

## 8. UI Changes

### 8.1 Sessions panel (Sidebar)

Wires the existing shell to real data: `list_sessions(projectPath)` on project open; active row = `activeSessionId`; click switches (flush current → `load_session` → set messages/agent/activity feed → set `.active`); hover row gets **rename** (inline input) and **delete** (confirm; already-visible X becomes confirm-first). Header gets a **+ New session** button (`create_session` → clears chat, activates). Empty state copy stays.

### 8.2 Agent Manager window (new modal)

Same chrome as `Settings` (backdrop `bg-black/70`, panel `bg-surface-1 border border-border-subtle rounded-lg`, `animate-fade-in`). Opened from the job bar (new gear-menu entry or a dedicated icon) and from a "Manage agents" link in the composer area.

Contents: roster list (name, role badge, builtin tag, delete for non-builtin) · edit form (name, role select, system prompt textarea) · **New personality** button · **Max agents per session** number input. Every change saves to `agents.json` via `save_agent_roster`. The manager is read-only-aware: if no project is open, it shows the empty state with the standard copy and opens disabled.

### 8.3 Composer options row (Chat)

Directly above the input, below the model row — a single row, three controls:

1. **Action mode** segmented control: `Auto` / `Ask risky` / `Ask all` (labels: Auto / Ask before risky / Ask before everything). Persisted to session `actionMode`.
2. **Thinking** toggle — the existing Anthropic toggle moves here (unchanged behavior).
3. **Goal** toggle — new; persisted to session `goalMode`. When on and the agent is mid-loop, the Stop button is prominent.

### 8.4 ActiveJobBar

Status pill reflects live state: idle / working (`<agent name> is <verb phrase from current activity>` — e.g. "harness is editing src/auth/login.ts") / waiting (approval). The hover popover shows the live activity feed (tool name, summary, status dot), same visual recipes as today.

### 8.5 Chat stream rendering of activities

`MessageBubble` gains an activity-row renderer (compact mono line, status dot, tool name + summary) for entries that aren't tied to a chat bubble. Approval prompts render as an inline action card (tool name, args summary, Approve / Deny buttons) — cards are transient, not persisted into `messages`.

## 9. Error Handling & Safety

- **Corrupt session files:** archived version-keyed (existing pattern), skipped from lists, never crash the app.
- **Path traversal:** every file tool guards; sessions-dir id validated against a strict whitelist; `run_command` cwd confined to the workspace.
- **Command execution:** args array (no shell interpolation), 60s timeout (kills the child), output truncated, non-zero exit is a tool error, not a crash.
- **Approval aborts:** cancellation resolves all pending approvals as denied; the loop terminates cleanly; no orphaned UI prompts.
- **Provider errors:** reused mapping; `retryAfterSeconds` honored (loop surfaces as `agent-error` with a retry hint); auth errors reuse the composer's "add key" routing.
- **Rate limits / transient failures:** tool-call round retries once on `rate_limit`/`network` before failing the turn.

## 10. Testing

Rust (`cargo test`, plus `clippy` + `fmt` before commit):

- Session store: CRUD roundtrip, `.active` marker behavior, v1→v2 migration (fresh, populated, corrupt), unknown-version archiving, list sort, id-validation rejection (`../` and friends).
- Tools: workspace guard matrix (inside/outside/nonexistent/symlink-out), `edit_file` exact-match semantics, command timeout + truncation, arg-array invocation.
- Policy gate: state machine across `auto`/`askRisky`/`askAll` (unit-testable via injected resolver), approval-deny path, cancel-resolves-denied.
- Loop: iteration cap, identical-call stall detection, `[continue]` goal-loop flow (with a mocked `complete_with_tools` — provider I/O stays out of unit tests).
- Wire format: camelCase assertions on the new structs (existing pattern in `session.rs` tests).

TS (`npx tsc --noEmit`; no test suite configured — session-manager logic is exercised manually):

- session-manager: active-session keying, flush on project and session switch.
- Event subscription routing by `requestId`.

Manual acceptance (web + `npm run tauri:dev`):

1. Open a project with a legacy `session.json` → it migrates into a session, chat shows the old messages.
2. Create 3 sessions, switch between them, rename, delete — sidebar reflects, `.harness/sessions/` has 3 files + `.active`.
3. Send "explain this repo" (Auto mode) → agent reads files, streams a reply, `activities` persists.
4. Flip Goal on, ask for a multi-file change → agent edits files, runs a command, completes; Stop interrupts mid-loop; approval prompt (Ask risky + a write) pauses and resumes on Approve/Deny.
5. Kill the app mid-loop, reopen → session restores (pending turn is lost, history is not).
6. ActiveJobBar shows live activity and the popover feed while the loop runs.

## 11. Multi-Agent Foundation (next sprint hooks)

Deliberate seams so the orchestrator lands without migration:

- Session `agent` snapshot → sub-agent sessions dispatch against the roster by `agentId`.
- `agents.json` roster + `maxAgentsPerSession` → orchestrator spawns up to the cap.
- Event vocabulary (`tool-start`/`tool-result`, per-`requestId`) → per-sub-agent panels + canvas tiling key off the same events.
- `AgentRole` enum unchanged → sub-agent personalities come from the manager UI.
- Tool registry is process-local and shared — sub-agents run the same guarded tools.

## 12. Files Touched

| File | Change |
|---|---|
| `src-tauri/src/session.rs` | multi-session store, migration, `.active` |
| `src-tauri/src/agents.rs` (new) | roster model + I/O |
| `src-tauri/src/tools.rs` (new) | tool specs + executors + path guard |
| `src-tauri/src/agent_loop.rs` (new) | turn loop, policy gate, goal loop, stall detection |
| `src-tauri/src/llm_client.rs` | `complete_with_tools`, tool aggregation (both providers) |
| `src-tauri/src/commands.rs` | new commands, `pending_approvals` state |
| `src-tauri/src/lib.rs` | register commands |
| `src/lib/ipc.ts` | project-scoped session commands, event listeners, roster types |
| `src/lib/store.ts` | `activeSessionId` wiring, `agentsOpen`, `actionMode`, `goalMode`, `liveActivityFeed` |
| `src/lib/session-manager.ts` | active-session tracking, per-session debounced save |
| `src/components/Sidebar.tsx` | real sessions panel + new/rename/delete |
| `src/components/AgentManager.tsx` (new) | roster modal |
| `src/components/Chat.tsx` | composer options row, approval card, activity rendering |
| `src/components/ActiveJobBar.tsx` | live state + popover feed |
| `DESIGN.md` / `PRODUCT.md` | reconcile sessions panel + agent story |
