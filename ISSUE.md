# ISSUE: Subagent System Not Implemented

**Severity:** Critical  
**Date:** 2026-08-14  
**Status:** Open  

## Summary

The Harness app's Orchestrator system prompt, UI components, and design docs describe a multi-agent system where the Orchestrator delegates tasks to subagent personalities (Coder, Architect, Researcher, HealthMonitor). **None of this infrastructure actually exists.** The subagent dispatch, messaging, parallel execution, and live-viewing layers are entirely unbuilt. The app runs as a single-agent system with a multi-agent personality editor that has no runtime effect.

---

## Findings

### 1. No Subagent Dispatch Tool Exists

The Orchestrator's system prompt (`src-tauri/src/agent.rs::ORCHESTRATOR_PROMPT`) references delegating to subagent personalities, but the tool registry (`src-tauri/src/tools.rs::ToolSpec::all()`) defines only 8 tools:

| Tool | Exists |
|------|--------|
| `read_file` | ✅ |
| `list_dir` | ✅ |
| `search_files` | ✅ |
| `write_file` | ✅ |
| `edit_file` | ✅ |
| `run_command` | ✅ |
| `web_search` | ✅ |
| `bip` | ✅ (role-gated to HealthMonitor) |
| `delegate_subagent` | ❌ **Missing** |
| `subagent_message` | ❌ **Missing** |

The Orchestrator **cannot** dispatch subagents because the tool doesn't exist in the backend's tool registry.

### 2. Agent Loop Is Single-Agent Only

`src-tauri/src/agent_loop.rs` runs a single `run_turn_core` loop per request. The architecture:

```
run_agent_turn → run_turn_core → completer.complete → execute_tool → loop
```

There is no mechanism to:
- Spawn a child agent with its own system prompt and model
- Run multiple agent loops concurrently
- Wait for a subagent's result and feed it back
- Route events from child agents to the frontend

The `Agent` struct in `agent.rs` has `sub_agents: Vec<String>` and `parent_id: Option<String>` fields, but both are `#[allow(dead_code)]` — they're reserved placeholders never wired into the loop.

### 3. No Frontend Subagent Infrastructure

| Component | Status |
|-----------|--------|
| `SubagentWindow.tsx` | ❌ Does not exist |
| `subagent-manager.ts` | ❌ Does not exist |
| Subagent state in Zustand store | ❌ No `subagents` map, no subagent status tracking |
| `subagent-*` Tauri events | ❌ Not registered |
| Subagent rendering in DynamicCanvas | ❌ Not implemented |

The `AgentManager.tsx` component is a **personality editor** (rename, re-prompt, assign models) — it manages the static roster, not live subagent execution.

### 4. Health Agent Is Stubbed

`src-tauri/src/health_agent.rs` implements the tick-loop lifecycle, heuristics, and Bip state updates, but:

- The `LlmCaller` trait is only implemented by `HeuristicCaller`, which returns a canned `{"verdict": "ok"}` response — no real LLM call
- The Health Agent never actually spawns a subagent personality; it just logs alerts and updates Bip's mood
- The `bip` tool is fully implemented in `tools.rs` with role-gating (`HealthMonitor` only), but since the Health Agent never runs real LLM calls, no agent ever calls `bip`
- The Orchestrator (the only real agent) has role `Orchestrator`, which the `bip` tool rejects

**Result:** The `bip` tool exists but is unreachable from any real agent turn.

### 5. Store Has No Subagent State

The Zustand store (`src/lib/store.ts`) has no fields for:
- Active subagent list/map
- Per-subagent status, messages, or tool calls
- Subagent-to-parent-agent relationships
- Subagent window visibility

The store has `activeAgents: AgentInfo[]` and `canvasPanels: CanvasPanel[]`, but `canvasPanels` is only used for the Dynamic Canvas (file diffs, terminal output) — not for subagent threads.

### 6. No Event Routing for Subagents

The event system (`src/lib/session-manager.ts::useAgentEvents`) handles events by `requestId`:
- `agent-turn-start`, `agent-chunk`, `agent-reasoning`
- `agent-tool-start`, `agent-tool-result`
- `agent-approval-request`, `agent-approval-result`
- `agent-done`, `agent-error`

There are no `subagent-start`, `subagent-chunk`, `subagent-tool-result`, or `subagent-done` events. Even if the backend spawned subagents, the frontend wouldn't receive their output.

### 7. No Parallel Execution Support

The agent loop is a single `tokio::spawn` per turn. The `in_flight` map tracks one `watch::Sender<bool>` per requestId. There's no mechanism for:
- Multiple concurrent agent loops (each with their own requestId)
- Coordinating results across parallel agents
- Merging subagent output into the parent's context

---

## Root Cause

The multi-agent system was designed in the spec layer (Agent Manager UI, personality roster, role enum, system prompts) but the **runtime layer was never built**. The v3 refactor consolidated everything into the Orchestrator-as-chat-agent model, and subagent dispatch was deferred. The current system has:

- ✅ **Personality roster** (static data model + editor UI)
- ✅ **Role enum** (Orchestrator, Coder, Architect, Researcher, HealthMonitor)
- ✅ **Default prompts** per role
- ✅ **Health Agent lifecycle** (tick loop, alert log, Bip state)
- ✅ **Tool infrastructure** (workspace-guarded, policy-gated, event-emitting)
- ❌ **Subagent dispatch tool** (the bridge from Orchestrator → subagent)
- ❌ **Concurrent agent loop runner** (parallel execution)
- ❌ **Subagent event bus** (frontend ← backend routing)
- ❌ **Subagent UI** (window/panel for live subagent output)
- ❌ **Real Health Agent LLM integration** (currently stubbed)

---

## What Would Fix This

### Phase 1: Backend Dispatch (Critical)
1. Add `delegate_subagent` tool to `ToolSpec::all()` — accepts personality id, task description, optional model override
2. Implement the executor in `tools.rs` — spawns a child `run_turn_core` with the subagent's system prompt and model
3. Return the subagent's result as tool output (synchronous) or emit `subagent-done` event (async)
4. Register `subagent-*` Tauri events in the event sink

### Phase 2: Frontend Routing
5. Add subagent state to the Zustand store (`subagents: Map<string, SubagentState>`)
6. Route `subagent-*` events in `useAgentEvents()`
7. Render subagent threads in the Dynamic Canvas or a new SubagentWindow

### Phase 3: Health Agent Integration
8. Wire `LlmCaller` to a real provider (using the existing `llm_client.rs`)
9. Make the Health Agent's tick loop actually call the LLM and act on verdicts
10. Enable the `bip` tool for the Health Agent's real LLM turns

---

## Impact

- **Orchestrator prompt is misleading** — it tells the LLM it can delegate to subagents, but it can't
- **Agent Manager UI is cosmetic** — users can edit personalities but they never run
- **Health Agent is decorative** — Bip updates are hardcoded to "ok" via `HeuristicCaller`
- **No multi-agent concurrency** — the app is functionally single-agent despite the multi-agent design
