# Implementation Plan — Global Agent Personalities

**Source spec:** `docs/superpowers/specs/2026-08-13--global-personalities.md`
**Date:** 2026-08-13
**Branch:** `feat/hci` (work lands here, never on `main` directly)

## Execution order

```
WP1 Rust personalities module → WP2 IPC commands
  → WP3 agent-loop resolution → WP4 retire per-project roster
  → WP5 TS types + store     → WP6 Agent Manager UI
  → WP7 session-manager glue → WP8 sidebar picker
  → WP9 docs + commit
```

Each package ends with a **verification gate**. `cargo test` / `cargo check` / `npx tsc --noEmit` are run by the user in a follow-up turn (this session intentionally skipped them per the user's "no build commands" rule).

---

## WP1 — Rust: global personalities module (`src-tauri/src/personalities.rs`, new)

Create the global roster file + types + invariants + tests.

- `AgentRole::all() -> [AgentRole; 5]` and `AgentRole::display_name() -> &str` on `agent.rs` (small additions; the enum gets `Copy` because the new module uses values in `default_personalities()`).
- `personalities.rs`:
  - `AgentPersonality { id, name, role, system_prompt, model_id: Option<String>, builtin: bool }`.
  - `PersonalityRoster { version: u32 = 2, default_personality_id: String, personalities: Vec<AgentPersonality> }`.
  - `default_personalities()` seeds one builtin per `AgentRole` with `model_id = None` and `system_prompt = role.default_prompt()`.
  - `builtin_personality_id(role)` returns stable `builtin-<slug>` ids.
  - `personalities_path()` reads `HARNESS_DATA_DIR_OVERRIDE` (test-only) before falling back to `<data_dir>/Harness/personalities.json`.
  - `load_personalities()` creates the file with `default()` on first contact; corrupt files are archived version-keyed.
  - `save_personalities(&roster)` is atomic (tmp + rename) and runs `enforce_invariants` first.
  - `enforce_invariants` re-inserts any missing builtin, repairs empty `system_prompt` on builtins to the role default, and fixes a `default_personality_id` that doesn't point at a real personality.
  - `get_personality(id)` is a thin wrapper around `load_personalities` + find.
- Tests: `default_roster_seeds_all_five_roles`, `user_personalities_roundtrip_with_model_id`, `empty_builtin_prompt_is_repaired`, `corrupt_file_is_archived_and_replaced`, `default_id_repaired_when_missing`, `user_cannot_remove_a_builtin_via_save`, `get_personality_returns_none_for_unknown_id`. All wrap `HARNESS_DATA_DIR_OVERRIDE` to a tempdir.
- Register `mod personalities;` in `lib.rs`.
- Gate: `cargo check -p harness` clean (warnings only). The user verifies `cargo test --lib personalities::tests` in their own shell.

## WP2 — Rust: IPC commands (`src-tauri/src/commands.rs`)

Three Tauri commands + one helper:

- `load_personalities() -> Result<PersonalityRoster, String>`.
- `save_personalities(roster: PersonalityRoster) -> Result<PersonalityRoster, String>`.
- `get_personality(id: String) -> Result<AgentPersonality, String>` — returns the requested personality, or the roster default if `id` is unknown.
- `resolve_active_model_for_personality(personality: &AgentPersonality) -> Result<Option<String>, String>` (public, not a Tauri command) — looks up `personality.model_id` in `models.json`; if present, returns it; if missing or stale, returns `Ok(None)` so the caller falls back to the chat model.
- Register the three Tauri commands in `lib.rs::tauri::generate_handler![...]`.
- Gate: `cargo check -p harness` clean.

## WP3 — Rust: agent-loop resolution (`src-tauri/src/agent_loop.rs`, `src-tauri/src/session.rs`)

The agent loop now resolves the personality once at session start and threads the resolved values through:

- `session.rs`:
  - `AgentSnapshot` collapses to `pub personality_id: String` + legacy v1 fields (`id`, `name`, `role`, `system_prompt`) marked `#[serde(skip_serializing)]`.
  - `default_agent_snapshot()` returns the orchestrator builtin id.
  - `impl Session { pub async fn resolve_personality(&self) -> Result<AgentPersonality, String> }` looks up the session's `personality_id` in the global roster; falls back to `default_personality_id` on miss; errors only if the roster is empty after `enforce_invariants` (impossible in practice).
  - `create_session(project, agent)` heals a legacy v1 payload by lifting `id` into `personality_id` when the latter is empty.
- `agent_loop.rs`:
  - `ResolvedAgent { personality_id, name, role, system_prompt, model_id }` bundles the resolved values.
  - `run_agent_turn_inner` resolves the personality + active model at the top (with eprintln-on-failure fallbacks for stale model ids and an early `agent-error` emit if the roster is unreachable).
  - `run_turn_core` takes `&ResolvedAgent`; `build_request` + `build_system_prompt` take the resolved model id + prompt (no longer `&Session`); `assistant_message_with_id_and_reasoning` takes `&ResolvedAgent`; `ToolContext::for_agent` uses the resolved role.
  - The simple `assistant_message_with_id` helper is removed (its only caller used the legacy `session.agent.id` + `session.agent.name`).
  - `agent_loop::tests::init_test_data_dir()` is a one-shot `std::sync::Once` that sets `HARNESS_DATA_DIR_OVERRIDE` to a tempdir. Each test calls it.
  - `use crate::agent::AgentRole;` added.
- Gate: `cargo check -p harness` clean. `cargo test --lib agent_loop::tests` (user-side).

## WP4 — Rust: retire per-project roster

- Remove `mod agents;` from `lib.rs`.
- Remove the two `#[tauri::command]` wrappers `load_agent_roster` and `save_agent_roster` from `commands.rs` (the TS side stopped calling them in WP5).
- Delete `src-tauri/src/agents.rs` (now dead).
- Update the existing test in `session.rs` that asserted `parsed.agent.id == "harness"` to use the legacy optional field (`assert_eq!(parsed.agent.id.as_deref(), Some("harness"))`).
- Gate: `cargo check -p harness` clean.

## WP5 — TS: types + store + IPC bridge (`src/lib/ipc.ts`, `src/lib/store.ts`)

- `ipc.ts`:
  - `AgentPersonality` gains `modelId: string | null`. The old `maxAgentsPerSession` / `defaultAgentId` fields are dropped (this is the new shape).
  - `PersonalityRoster { version, defaultPersonalityId, personalities }` replaces `AgentRoster`.
  - `AgentSnapshot` collapses to `personalityId: string` with the legacy `id?` / `name?` / `role?` / `systemPrompt?` for back-compat.
  - `DEFAULT_PROMPTS: Record<AgentRole, string>` mirrors the Rust `AgentRole::default_prompt()` (so the Agent Manager can prefill + reset without an IPC roundtrip).
  - IPC methods: replace `loadAgentRoster` / `saveAgentRoster` with `loadPersonalities` / `savePersonalities` / `getPersonality`.
- `store.ts`:
  - `agentRoster` / `setAgentRoster` → `personalities` / `setPersonalities`.
- `App.tsx`:
  - New `usePersonalitiesBootstrap()` hook that calls `ipc.loadPersonalities()` once on mount and `setPersonalities`. Runs alongside `useProjectLifecycle` + `useModelBootstrap`.
- Gate: `npx tsc --noEmit` clean. (User verifies.)

## WP6 — TS: Agent Manager (`src/components/AgentManager.tsx`, rewritten)

The full rewrite: every row now has a model selector + role-default prompt prefill + reset link, plus a "Default" pill on the default personality.

- Read `personalities` + `savedModels` from the store.
- Local `draft: PersonalityRoster | null` (deep-copied on open / on roster change).
- `addPersonality()` picks the first role that doesn't already have a row; pre-fills the prompt with the role default; gives a stable id (`user-<base36-time>`); `modelId: null`; `builtin: false`.
- `removePersonality(i)` removes the row; if the removed row was the default, the default moves to `personalities[0]`.
- `setDefault(id)` sets `defaultPersonalityId`.
- `handleRoleChange(i, role)` **prefills** the prompt with the new role's default (the user is making an explicit role change, so a fresh prompt is the expected behaviour). The "Reset to default" link is the escape hatch for a single edit.
- `handleResetPrompt(i)` writes `DEFAULT_PROMPTS[role]` back; the button is `disabled` when the prompt already equals the default.
- `handleSave()` calls `ipc.savePersonalities(draft)` and updates the store; toasts on failure.
- Header pill shows `"<N> personalities · global"` (no max-per-session counter).
- Gate: `npx tsc --noEmit` clean.

## WP7 — TS: session-manager glue (`src/lib/session-manager.ts`)

- `currentAgent: AgentSnapshot | null` now carries the new shape (`personalityId`); `fallbackAgent()` returns `{ personalityId: "builtin-orchestrator" }`.
- `resolveAgentMeta()` reads the live roster from the store and returns `{ id, name, role }` for the current session's personality; falls back to the id + "Orchestrator" when the personality is missing.
- `applySession()` heals a legacy v1 session: if `session.agent.personalityId` is empty but `session.agent.id` is set, copy `id` → `personalityId` in-place before storing.
- The two `appendMessage({ agentId: agent.id, agentName: agent.name })` call sites now use `resolveAgentMeta()`.
- The `loadProject` flow no longer requests `ipc.loadAgentRoster`; the global roster is fetched once on app mount (WP5).
- The "no project" state clear no longer resets `agentRoster`.
- Gate: `npx tsc --noEmit` clean.

## WP8 — TS: sidebar new-session picker (`src/components/Sidebar.tsx`)

- Add `personalities` to the `useStore` destructuring in `SessionsPanel`.
- Local `newSessionPersonalityId: string | null` initialised to the roster's default on first roster load.
- `handleNewSession()` now passes `{ personalityId }` (when set) to `ipc.createSession`.
- A small `<select>` appears next to the + button **only when more than one personality exists** (single-builtin setups look exactly like before). Bound to `newSessionPersonalityId`.
- The session list entry pulls `agentId: created.agent.personalityId || created.agent.id` (back-compat for legacy v1 responses).
- Gate: `npx tsc --noEmit` clean.

## WP9 — Docs + commit

- Spec: `docs/superpowers/specs/2026-08-13--global-personalities.md` (this commit).
- Plan: `docs/superpowers/plans/2026-08-13--global-personalities.md` (this file).
- Commit message: `feat(personalities): global agent personalities + per-personality model + role-default prompts` with the body outlining the data model, the resolution flow, and the migration story.
- Push to `feat/hci`. (User runs the actual `cargo test` + `npx tsc --noEmit` in their own shell; this turn does not.)
