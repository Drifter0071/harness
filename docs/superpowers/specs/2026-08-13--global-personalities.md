# Global Agent Personalities

**Status:** Implemented (WP1–WP9 on `feat/hci`)
**Date:** 2026-08-13
**Scope:** Phase 2.6 — promote agent personalities from per-project (`<project>/.harness/agents.json`) to a single global file, add per-personality saved-model selection + custom names + role-default prompts, and collapse the session `AgentSnapshot` to a `personalityId` only.

## 1. Summary

The previous spec ([`2026-08-12--agent-sessions-and-real-tool-loop.md`](2026-08-12--agent-sessions-and-real-tool-loop.md)) introduced a per-project agent roster stored at `<project>/.harness/agents.json`. Each project kept its own copy of the same five builtin personalities, which the user had to maintain five times. This spec retires that shape and replaces it with:

1. **One global roster** at `<data_dir>/Harness/personalities.json`. The user maintains their personalities once; every project picks them up.
2. **Per-personality saved-model selection.** A personality carries an optional `modelId`; when the agent loop runs that personality, the model's id overrides the chat tab's active model. `null` means "use the chat model" (preserves the current behavior).
3. **Per-personality role-default prompt.** Each `AgentRole` has a seed prompt in `AgentRole::default_prompt()` (Orchestrator, Coder, Architect, Researcher, HealthMonitor). The Agent Manager's "Reset to default" button + the role-change prefill both read from this same source, so the user always sees the standard prompt for the role they pick.
4. **Session = `personalityId` only.** The session file's `agent` field collapses from `{id, name, role, systemPrompt}` to `{personalityId}`. The agent loop resolves the rest from the global roster at session start, so editing a personality retroactively changes future session starts (old sessions keep their snapshot for display).
5. **Custom names + per-personality custom prompts.** Both editable in the Agent Manager.

## 2. Why a global file

The previous per-project shape had three problems:

- **Duplication.** A user who works on five projects has the same five builtin personalities (and any custom ones) sitting in each project's `agents.json`, with no way to share edits across projects.
- **Confusing UX.** The Agent Manager modal appeared per-project; the user had to remember which project's roster they were editing.
- **Coupling to project lifecycle.** Deleting a project (or moving it) lost the personality edits even if the user wanted to keep them.

The global file lives in the OS data directory alongside the saved-models file (`<data_dir>/Harness/personalities.json`). The pattern is the same as `session.rs::recents_file()` uses for `<data_dir>/Harness/recent.json`.

## 3. Data model

### 3.1 Personality

```rust
pub struct AgentPersonality {
    pub id: String,            // stable, used in session files + IPC
    pub name: String,          // free-text display name (e.g. "Bip", "Senior Coder")
    pub role: AgentRole,       // Orchestrator | Coder | Architect | Researcher | HealthMonitor
    pub system_prompt: String, // editable; default seeded from AgentRole::default_prompt()
    pub model_id: Option<String>, // SavedModel.id; None = use chat model
    pub builtin: bool,         // seeded by Harness; editable but not deletable
}
```

Builtins are seeded on first contact: one per `AgentRole`, with stable ids (`builtin-orchestrator`, `builtin-coder`, `builtin-architect`, `builtin-researcher`, `builtin-health-monitor`). `enforce_invariants` re-inserts any missing builtin and repairs an empty `system_prompt` on a builtin to the role default (so a stray UI save can't blank out the seed).

### 3.2 Roster

```rust
pub struct PersonalityRoster {
    pub version: u32,                  // 2
    pub default_personality_id: String, // stable; new sessions start on this
    pub personalities: Vec<AgentPersonality>,
}
```

### 3.3 Session `AgentSnapshot` — collapsed to a personalityId

The session file no longer embeds a snapshot of the agent's name/role/prompt. It carries the `personalityId` only; everything else is resolved live from the roster.

```rust
pub struct AgentSnapshot {
    pub personality_id: String,  // canonical
    // Legacy v1 fields; accepted on input so old session files keep
    // loading, but never serialized.
    pub id: Option<String>,
    pub name: Option<String>,
    pub role: Option<AgentRole>,
    pub system_prompt: Option<String>,
}
```

A v1 session file (which has `id` / `name` / `role` / `systemPrompt` inline) is healed at the `create_session` boundary: the legacy `id` is lifted into `personality_id`. Old session files loaded via `load_session` get healed in `applySession` on the TS side. Either way, the next save writes the v2 shape.

## 4. Resolution flow

1. `Session::resolve_personality` looks up the session's `personality_id` in the global roster. If empty or unknown, it falls back to the roster's `default_personality_id`.
2. `commands::resolve_active_model_for_personality` reads the resolved personality's `model_id`; if set AND still present in `models.json`, that's the model the agent loop uses. Otherwise, fall back to the chat model the frontend passed in `run_agent_turn`.
3. `agent_loop` calls both, once per turn, and threads the resolved values (`ResolvedAgent`) through `run_turn_core`. Every tool call, every message persistence, and the LLM call itself use the same resolved name / role / prompt / model id.
4. The `Bip` mood driver (via the `bip` tool) still works as before — the health agent can call `bip(mood, thought)` and the resolved role doesn't matter for that.

## 5. UI surface

### 5.1 Agent Manager modal (reworked)

- Per-row controls: name input, role select, model select (saved-models dropdown with "Use chat model" as the empty value), delete button (non-builtin only).
- "Default" pill on the row that's `defaultPersonalityId`; click any other row to set it as default.
- Per-row system prompt textarea, with a "Reset to default" link in the top-right of the section. The link is disabled when the prompt already matches the role default.
- Changing the role dropdown **prefills the prompt with the new role's default** (the user is making an explicit role change, so a fresh prompt is the expected behaviour; the "Reset to default" link remains the escape hatch for individual edits).
- The header pill no longer shows the per-project counter ("global" replaces "max N per session").

### 5.2 Sidebar — new-session personality picker

A small `<select>` appears next to the + button when more than one personality exists. The user picks the personality for the next new session; the default selection is the roster's `default_personality_id`. The picker persists across + clicks (until the user changes it) so consecutive new sessions with the same personality don't require a re-pick.

The picker is hidden when only one personality exists, so single-builtin setups look exactly like before.

### 5.3 Active job bar — unchanged

Bip's hover popover, the work / success / error mood derivations, and the agent activity feed do not change. The `ActiveJobBar` continues to drive Bip from local store state (streaming, agent request, job activities). The Health Agent's `bip` tool refactor is in a follow-up.

## 6. IPC

| Command                  | Args                                     | Returns                  |
|--------------------------|------------------------------------------|--------------------------|
| `load_personalities`     | —                                        | `PersonalityRoster`      |
| `save_personalities`     | `roster: PersonalityRoster`               | `PersonalityRoster` (normalized) |
| `get_personality`        | `id: string`                              | `AgentPersonality` (defaults to roster default if id unknown) |

Removed: `load_agent_roster` / `save_agent_roster` (the per-project surface is gone).

## 7. Tests

Rust unit tests live in `src-tauri/src/personalities.rs::tests`:

- `default_roster_seeds_all_five_roles` — first contact seeds one builtin per role.
- `user_personalities_roundtrip_with_model_id` — non-builtin personalities persist + reload with `model_id` intact.
- `empty_builtin_prompt_is_repaired` — whitespace-only prompt on a builtin is replaced with the role default on save.
- `corrupt_file_is_archived_and_replaced` — corrupt JSON is archived, not silently destroyed.
- `default_id_repaired_when_missing` — invalid `default_personality_id` is repaired to the first row.
- `user_cannot_remove_a_builtin_via_save` — dropping a builtin from a payload is repaired; user edits to remaining builtins survive.
- `get_personality_returns_none_for_unknown_id` — fallback path.

All tests redirect the global file via `HARNESS_DATA_DIR_OVERRIDE` (a test-only env var) so they don't touch the user's real data dir.

`agent_loop::tests` calls `init_test_data_dir()` (a one-shot `std::sync::Once`) to set the same env var, so the agent-loop tests that now resolve a personality don't write to the real data dir.

## 8. Migration

No automatic migration of the per-project `<project>/.harness/agents.json`. The file is no longer read; existing files are left on disk (and ignored). If a user had custom personalities, the only recovery is to re-create them in the Agent Manager. This is acceptable because v0 hadn't shipped to real users yet and the per-project surface was only used by the dev environment.

Old session files (v1, with the full `AgentSnapshot`) still load: the v1 fields are deserialized into the optional legacy fields on the new `AgentSnapshot`, then healed into `personality_id` at the `applySession` boundary on the TS side (and at `create_session` for new sessions). The next save writes the v2 shape.

## 9. Out of Scope

- **Personality-level reasoning-effort override.** A personality could pin a `ReasoningEffort` to always pass to the provider; deferred until a real meta-LLM call exercises the health agent.
- **Workspace-scoped overrides** (per-project "this project uses Coder-2 instead of the global Coder"). The user can rename the builtin in the Agent Manager if they need a project-specific persona; full override layering is a future spec.
- **Sharing + import / export of the personalities file.** The file is a regular JSON in the OS data dir; advanced sharing is a future spec.
- **Per-personality temperature / sampling overrides.** Same reasoning-effort reasoning.

## 10. Follow-ups

- The Health Agent's `bip` tool should start reading the resolved personality's `name` for richer default thoughts (e.g. a Coder personality that just got stuck gets a thought that mentions "your Coder agent", not "the agent").
- The Settings → Models picker (when adding a new saved model) could show which personalities currently use that model, so the user knows what they're affecting.
