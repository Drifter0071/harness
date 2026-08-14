# Composer v2 — Slash Commands, CommandMode, Per-Session Memory, Compacting

**Date:** 2026-08-14
**Status:** Draft (pending review)
**Author:** Mavis (brainstorming session with @gellert)

> **Revision history:**
> - v1 (this doc) — initial draft, **FAIL** on first spec-review (17 issues).
>   Fixes in progress: see the [Changelog of reviewer fixes](#changelog-of-reviewer-fixes)
>   appendix at the bottom.

## Summary

Five tightly coupled changes to the chat composer that together replace the
current "Actions / Ask levels" model with a cleaner "Commands / YOLO" model,
add a `/`-prefixed command surface, fix two long-standing file-mention bugs,
make model + mode stickiness per-session, and ship manual + automatic chat
compacting.

The subagent diagnostic report (`ISSUE.md`) is already in place and is **out
of scope** for this spec — subagent implementation will get its own spec
later.

## Goals (in scope)

- **G1** — `/`-prefixed commands with autocomplete popover, sharing
  infrastructure with the existing `@`-mention popover.
- **G2** — Replace `ActionMode::Auto | AskRisky | AskAll` with
  `CommandMode::Commands | Yolo`. `Commands` asks only for `run_command`.
  `Yolo` asks for nothing.
- **G3** — Per-session memory of last-used model and last-used mode.
- **G4** — Manual `/compact` and automatic context compacting above a
  configurable threshold.
- **G5** — Fix two file-mention bugs: popover does not dismiss when space is
  pressed mid-mention; popover keeps running searches / shows stale results.

## Non-goals

- Subagent implementation (covered by `ISSUE.md`, future spec).
- Tokenizer-accurate token counting. Rough char/4 estimate is enough for v1.
- Per-model context partitioning. Model switch is a no-op for context;
  messages[] is unified.
- Slash commands beyond the core set: `/compact`, `/mode`, `/model`,
  `/clear`, `/help`. Future additions (`/system`, `/temperature`, `/files`)
  are easy to add but not part of v1.

---

## 0. Delivery — three PRs, not one

The original draft bundled five concerns into a single PR. On review this
was flagged as too much risk: a regression in any one feature blocks the
others, and a 2000-line diff is hard to review. The spec is **split into
three PRs** that build on each other in dependency order:

| PR | Concerns | New files | Modified files | Risks |
|----|----------|-----------|----------------|-------|
| **PR1 — `ComposerPopover` refactor + G5 fix** | G5 only | `src/components/ComposerPopover.tsx` | `src/components/MentionInput.tsx`, `src/lib/mention-utils.ts`, `src/lib/mention-utils.test.ts` | Refactor risk (existing mention popover behavior must be unchanged) |
| **PR2 — Slash commands (G1)** | G1 only | `src/lib/commands.ts`, `src/lib/commands.test.ts`, `src/components/SlashCommandPopover.tsx` | `src/components/Chat.tsx`, `src/components/MentionInput.tsx` (uses primitive from PR1) | New feature, no breaking changes |
| **PR3 — Mode rename + per-session + compacting (G2, G3, G4)** | G2 + G3 + G4 | (no new top-level files; all changes inside existing modules) | `src-tauri/src/session.rs`, `src-tauri/src/agent_loop.rs`, `src-tauri/src/llm_client.rs`, `src-tauri/src/commands.rs`, `src/lib/ipc.ts`, `src/lib/store.ts`, `src/lib/session-manager.ts`, `src/components/Chat.tsx`, `src/components/Sidebar.tsx` | Migration risk; new persistence fields |

PR1 is the foundation — it extracts the shared popover primitive and fixes
the G5 bugs as part of the same refactor (the bugs are in the shared
dismissal logic). PR2 builds on PR1. PR3 is independent of PR1/PR2 except
for sharing the composer surface.

The remainder of this spec describes PR1+PR2+PR3 in order. Each section
labels which PR it belongs to. **A reader who only wants to review PR1
can stop after Section 3.** A reader reviewing PR2 stops after Section 5.
A reader reviewing PR3 reads it all.

---

## 1. Architecture overview

```
                    ┌──────────────────────────┐
                    │   Chat Composer (TS)     │
                    │  <MentionInput> core     │
                    └─────┬────────┬───────────┘
                          │        │
            ┌─────────────┘        └──────────────┐
            │                                     │
   ┌────────▼─────────┐                ┌─────────▼──────────┐
   │ Slash-command    │                │  Mention popover   │
   │ popover (NEW)    │                │  (existing, refac- │
   │ /compact /mode…  │                │   tor extracts     │
   └────────┬─────────┘                │   shared trigger)  │
            │                          └────────────────────┘
            │ parsed
            ▼
   ┌────────────────────┐
   │ Command registry   │  ← single source of truth
   │ (commands.ts)      │     names, args, handlers, help text
   └────────┬───────────┘
            │
   ┌────────┴──────────────────────────────────┐
   │                                            │
   ▼                                            ▼
┌──────────┐  ┌──────────┐  ┌─────────────┐  ┌────────────┐
│ /clear   │  │ /mode    │  │ /model      │  │ /compact   │
│ wipe UI  │  │ flip     │  │ set active  │  │ call LLM   │
│ + warn   │  │ Commands │  │ model id    │  │ summarize  │
│ before   │  │ / YOLO   │  │ persist     │  │ persist    │
└──────────┘  └──────────┘  └─────────────┘  └────────────┘

   per-session state (NEW field on Session):
     • last_model_id       (string | null)
     • last_mode           (CommandMode)
     • compact             (Option<CompactState>)
     • compact_threshold   (0.0–1.0, default 0.8)
```

Four sub-projects map to four thin layers:

1. **Slash command system** — `commands.ts` registry + composer popover
   refactor. Shares `ComposerPopover` primitive with the mention popover.
2. **Commands/YOLO rename** — rename `ActionMode` to `CommandMode`; drop the
   3rd variant. New approval gate semantics. Migrate persisted sessions.
3. **Per-session memory** — 3 new fields on `Session`; restore on open;
   persist on change.
4. **Compacting** — `/compact` handler + auto-trigger in `chatComplete` path.

The mention-bug fix (G5) is folded into the popover refactor because both
popovers share the same `ComposerPopover` primitive and the bug is in the
shared dismissal logic.

---

## 2. Slash-command popover (G1) — **belongs to PR2**

### 2.1 Trigger rule

The popover opens when the caret is on a line where the **first
non-whitespace character** is `/` AND the `/` token contains no whitespace
yet AND the caret is inside (or directly after) that token. Same trigger
semantics as VS Code and GitHub Copilot.

| Input           | Caret pos | Popover                              |
|-----------------|-----------|--------------------------------------|
| `/`             | after `/` | open, query empty, show all commands |
| `/co`           | after `co`| open, query `co`, show filtered      |
| `/compact `     | after sp  | closed (token ended)                 |
| `hello /co`     | inside    | closed (not at line start)           |
| `//`            | after `//`| open, query empty (strip leading `/`) |
| `/` + arrow up/down | n/a  | keyboard nav as today                |

`/` mid-sentence is a normal character in paths, math, code. Forcing the
popover on every `/` would be obnoxious. The line-start rule plays nicely
with the existing mention popover which already uses `parseMentionQuery` to
gate activation.

### 2.2 UI

Same chrome as the mention popover. Both popovers extract their shared bits
(textarea caret tracking, popover positioning, list, keyboard nav, scroll)
into a `ComposerPopover` primitive in `src/components/ComposerPopover.tsx`.

Slash popover items:

```
/compact        Compact chat history into a summary       (h: 12.4k → 1.8k)
/mode           Switch between Commands and YOLO         (current: Commands)
/model          Switch the active model                   (current: gpt-4o)
/clear          Clear the current chat                    (asks to confirm)
/help           Show all available commands
```

The trailing `(current: …)` annotation is only shown when the query is empty.

### 2.3 Command registry

`src/lib/commands.ts`:

```ts
export interface CommandArgSpec {
  name: string;            // "target" for /mode
  values?: string[];       // for /mode: ["Commands", "Yolo"]
  required?: boolean;
}

export interface Command {
  name: string;            // "compact", "mode", ...
  summary: string;         // one-line description
  args?: CommandArgSpec[];
  execute: (ctx: CommandContext) => Promise<CommandResult>;
}

export interface CommandContext {
  session: Session;
  input: string;           // raw "/mode Yolo" text after the "/"
  pushUserMessage: (text: string) => void;
  setMode: (m: CommandMode) => void;
  setModel: (id: string) => void;
  compact: () => Promise<void>;
  clear: () => Promise<void>;
  showToast: (msg: string) => void;
}

export interface CommandResult {
  userMessage?: string;    // optional text to show in chat as a system note
  toast?: string;
}
```

Commands are pure functions of `CommandContext`. The composer is the only
place that knows how to parse, surface, and call them. Easy to test in
isolation.

### 2.4 Send behavior

- **Enter while popover is open with a highlighted item**: insert the
  command's name + a space into the textarea; do NOT submit. User fills in
  args then Enter again. Same pattern as the mention popover commits.
- **Enter with popover closed** but a `/`-prefixed line in the textarea: the
  parser detects the leading `/`, dispatches the command, and the user
  message is **not** sent to the LLM. The command's `userMessage` (if any)
  is shown as a system note in the chat.

### 2.5 `/help`

A special command that returns a system toast listing all commands. No LLM
call needed.

### 2.6 Files (PR2)

- `src/lib/commands.ts` — new, command registry
- `src/lib/commands.test.ts` — new, command tests
- `src/components/SlashCommandPopover.tsx` — new, slash popover
  (uses the `ComposerPopover` primitive from PR1)
- `src/components/Chat.tsx` — wire slash parsing into composer; route
  detected commands to the registry
- `src/components/MentionInput.tsx` — accept the slash popover as a
  sibling and apply the arbitration rule (2.7)

### 2.7 Popover arbitration — only one popover open at a time

The mention popover activates on `@` preceded by whitespace (or start of
line) with no whitespace in the query. The slash popover activates on
`/` as the first non-whitespace character of a line, with no whitespace
in the token. **Both rules can be true at the same caret position**:
e.g., `/compact @file` at the caret just after `compact` — slash is
line-start (slash active), `@file` is preceded by a space (mention
active).

**Rule**: at most one popover is open. Arbitration priority:

1. Compute mention activation and slash activation independently.
2. If both are active, **slash wins** (it's the more "command-like" intent;
   `@` mid-token is more often a typo or non-trigger context).
3. If only one is active, that one opens.
4. If neither is active, close any open popover.

**Implementation**: `MentionInput` is refactored to accept an optional
`secondaryPopover` prop. The primary popover is the mention popover; the
secondary is the slash popover. On each recompute, the component evaluates
both activations, applies the rule above, and opens at most one. Both
popovers render into the same DOM position (one at a time) using the
shared `ComposerPopover` primitive from PR1.

**Tests** (PR2):
- `slash_wins_when_both_active` — caret in `/compact @` → slash popover
  open, mention closed.
- `mention_wins_when_slash_inactive` — caret in `hello @file` → mention
  popover open, slash closed.
- `neither_wins` — caret in `hello world` → both closed.
- `space_closes_active_popover_regardless_of_type` — type `/comp `, both
  should close (slash inactive after space, mention inactive because no
  `@` was ever typed).

---

## 3. File-mention bug fixes (G5) — **belongs to PR1**

Two related bugs, both in `MentionInput.tsx`. The fix lives in the
`ComposerPopover` refactor because the shared popover primitive is where
dismissal happens.

### 3.1 Bug 1 — popover does not dismiss when space is pressed mid-mention

Today, dismissal is in the `useEffect` keyed on `value`
(`MentionInput.tsx:65-69`), but it only fires **after** React has committed
the new value. With a fast typist this can race with `setSelectionRange`
from the commit handler and the dismissal may be missed. The popover then
re-opens on the next keystroke because `parseMentionQuery` still treats the
`@token<space>` text as an active mention — see `mention-utils.ts:26-30`,
which slices on whitespace but does not return inactive.

**Fix A** — handle dismissal in `keydown` directly. The popover's source of
truth shifts from "is the parser active right now" to "did the user just
type a non-mention character". Add to `handleKeyDown`:

```ts
if (popover.open && (e.key === " " || e.key === "Escape")) {
  e.preventDefault();
  setPopover((p) => ({ ...p, open: false }));
  return;
}
```

**Fix B** — tighten `parseMentionQuery`: when `between` contains whitespace,
return `{ active: false, query: "" }` instead of
`{ active: true, query: truncated }`. Removes the ambiguity that lets the
popover stay "active" through a space. **This is a behavior change**: the
existing parser returns `active: true, query: <truncated>`. The fix
returns `active: false, query: ""`. The current caller
(`MentionInput.tsx:46`) only checks `parsed.active`, so callers that
relied on the truncated query being non-empty will see a different result
on whitespace input. A grep for callers of `parseMentionQuery` confirms
only `MentionInput.tsx` consumes it; no other consumers exist. The
`mention-parser.test.ts` file (existing) should be reviewed for any test
that asserted `active: true` for whitespace-containing input.

### 3.2 Bug 2 — popover keeps running searches / shows stale results

Same root cause as Bug 1 — once the parser says `active: true` for a
half-typed mention, every keystroke fires `rankMentionResults` and
re-renders the list.

**Fix** — with Bug 1 fixed the popover closes promptly, so the symptom
stops. Add a belt-and-braces 1-frame `requestAnimationFrame` throttle on
`rankMentionResults` calls so a fast typist does not queue multiple searches
per keystroke.

### 3.3 Tests (PR1)

In `src/lib/mention-utils.test.ts` (new file; not currently present per
reviewer finding — `grep` confirms zero `mention-utils.test.ts` exists
today):

- `parseMentionQuery("@foo ", caret=5)` → `{ active: false, query: "" }`
- `parseMentionQuery("hello @foo", caret=10)` → unchanged (still inactive
  mid-line, since `@` is not preceded by whitespace)
- `parseMentionQuery("@foo", caret=4)` → `{ active: true, query: "foo" }`
  (unchanged)
- `parseMentionQuery("@", caret=1)` → `{ active: true, query: "" }`
  (unchanged)
- `parseMentionQuery("@@foo", caret=6)` → `{ active: false, query: "" }`
  (unchanged: `@` between trigger and caret)
- `parseMentionQuery("a@b", caret=3)` → `{ active: false, query: "" }`
  (unchanged: `@` not preceded by whitespace)

In `MentionInput.test.tsx` (new file):
- `space_dismisses_popover_immediately` — type `@auth`, popover opens;
  fire keydown for space; assert `popover.open === false` synchronously
  (not on next effect).
- `space_then_a_does_not_reopen` — type `@auth ␣a`; assert no popover
  reopens on the `a` keydown.

### 3.4 Acceptance

Type `@auth` → popover appears with matches. Type space → popover **closes
immediately**, no flash. Type `a` after the space → no popover, plain text.
Type `@auth` again on the same line → popover reopens with matches.

---

## 4. CommandMode rename (G2) — **belongs to PR3**

### 4.1 New shape

```rust
// src-tauri/src/session.rs
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum CommandMode {
    #[default]
    Commands,
    Yolo,
}
```

`RISKY_TOOLS` in `agent_loop.rs:47` (`["write_file", "edit_file",
"run_command"]`) is deleted. The `requires_approval` function in
`agent_loop.rs:238-244` becomes:

```rust
fn requires_approval(mode: &CommandMode, name: &str) -> bool {
    match mode {
        CommandMode::Commands => name == "run_command",
        CommandMode::Yolo => false,
    }
}
```

### 4.2 Approval matrix

| Tool                                  | `Commands` | `Yolo` |
|---------------------------------------|------------|--------|
| `read_file`, `list_dir`, `search_files`, `web_search`, `bip` | silent     | silent |
| `write_file`, `edit_file`             | silent     | silent |
| `run_command`                         | **ask**    | silent |

### 4.3 Persistence / migration

Existing on-disk sessions have `action_mode: "auto" | "askRisky" | "askAll"`
in the JSON. The new field is `last_mode: "commands" | "yolo"`. The disk
format version stays at `2` (the rename is in-place; new optional fields
have `#[serde(default)]` so old files load cleanly).

**Migration mechanism**: the new `last_mode` field carries
`#[serde(alias = "action_mode")]`. When serde parses an old session file,
it will look for `last_mode` first; if not found, it looks for the alias
`action_mode`. The value found in `action_mode` is then converted by a
`Deserialize` impl on `CommandMode` (or a `From<&str>` helper used during
a post-parse normalization pass) that maps:

- `"auto"` → `CommandMode::Yolo` (closest semantic — runs without asking)
- `"askRisky"` → `CommandMode::Commands` (the new default)
- `"askAll"` → `CommandMode::Commands` (collapse — `commands` is the
  stricter of the two, the safer migration target)
- anything else → `CommandMode::default()` (Commands)

After the first save under PR3, the file is rewritten with `last_mode` and
no `action_mode`. The alias stays in the codebase for at least one release
so reverts remain possible; it can be removed in a later cleanup PR.

**Default semantic change (call out)**: the old default was
`ActionMode::AskRisky`; the new default is `CommandMode::Commands`. These
are semantically equivalent for the common case (ask before run_command).
The change is visible only in the wire format for sessions that were
created with no explicit mode set; for migrated sessions the explicit
value is set in step 1. **A `#[default] Commands` enum attribute is
intentional and documented.**

### 4.4 Naming nit (carried over)

The pairing `CommandMode::Commands` (noun) vs `CommandMode::Yolo` (acronym)
is awkward. The spec keeps `Commands | Yolo` because that's the user-
facing names already decided; the spec does **not** propose a further
rename. If the team wants parallel verb-led names (`AskForCommands` /
`RunAll`) it's a follow-up that doesn't need to block PR3.

### 4.4 UI

The three-button segmented control in `Chat.tsx:500-510` becomes a
two-button toggle. Same visual style, same hint-on-hover. The
`ACTION_MODES` constant at `Chat.tsx:34-38` becomes `COMMAND_MODES` with
the new entries. Tooltip text:

- `Commands` → "Ask before running shell commands"
- `Yolo` → "Run all tools without asking"

### 4.5 Tests

In `agent_loop.rs`:

- Existing `risky_tool_requires_approval_and_deny_halts` (around line 1383)
  renames to `run_command_requires_approval_in_commands_mode_and_deny_halts`
  and asserts only `run_command` triggers approval
- New test: `write_and_edit_silent_in_commands_mode` — both fire without
  approval
- New test: `all_tools_silent_in_yolo_mode`

### 4.6 Files (PR3)

Confirmed `ActionMode` / `actionMode` / `ACTION_MODES` references via grep
across `src/` and `src-tauri/src/`:

- `src-tauri/src/agent_loop.rs` (lines 36, 264 — replace with
  `CommandMode`)
- `src-tauri/src/session.rs` (lines 73, 190, 453, 666, 864 — rename enum,
  add alias, add migration)
- `src-tauri/src/tools.rs` — no direct `ActionMode` ref; only reads
  `auto_allowed` bool. No change needed beyond ensuring the
  `ToolContext::auto_allowed` field is still set correctly when
  `requires_approval` returns true.
- `src/components/Chat.tsx` (lines 17, 34-38, 73-74, 575 — rename
  `ACTION_MODES` to `COMMAND_MODES`, update the segmented control)
- `src/components/Sidebar.tsx` (line 279 — one reference, update the
  type alias)
- `src/lib/ipc.ts` (lines 59, 103, 377, 614 — rename `ActionMode` type
  alias and the `actionMode` field in `AgentTurnRequest`)
- `src/lib/session-manager.ts` (lines 24, 47, 113, 151, 236, 455 — six
  call sites; update field name and type)
- `src/lib/store.ts` (lines 44, 159-160, 405-406 — store type)

### 4.7 Migration tests (PR3)

In `src-tauri/src/session.rs` (test module, following the existing
`migration_from_v1` pattern at lines 1089-1140):

- `migration_action_mode_auto_loads_as_yolo` — JSON with
  `action_mode: "auto"`, no `last_mode` → `last_mode == Yolo`
- `migration_action_mode_askRisky_loads_as_commands` — JSON with
  `action_mode: "askRisky"`, no `last_mode` → `last_mode == Commands`
- `migration_action_mode_askAll_loads_as_commands` — JSON with
  `action_mode: "askAll"`, no `last_mode` → `last_mode == Commands`
- `migration_missing_action_mode_defaults_to_commands` — JSON with no
  `action_mode` and no `last_mode` → `last_mode == Commands` (the new
  default)
- `migration_old_session_without_last_model_id_loads_with_none` — JSON
  with no `last_model_id` → `last_model_id == None`
- `migration_save_writes_only_last_mode` — load old format, save, parse
  the resulting JSON; assert no `action_mode` key, `last_mode` key
  present with the migrated value

---

## 5. Per-session memory (G3) — **belongs to PR3**

### 5.1 New fields on `Session`

```rust
pub struct Session {
    // ... existing ...
    #[serde(default)]
    pub last_model_id: Option<String>,
    #[serde(default = "default_command_mode")]
    pub last_mode: CommandMode,
    #[serde(default)]
    pub compact: Option<CompactState>,
    #[serde(default = "default_compact_threshold")]
    pub compact_threshold: f32,        // 0.0–1.0
}
```

`CompactState`:

```rust
pub struct CompactState {
    pub compacted_at: String,        // ISO
    pub summary_message_id: String,   // points to the summary Message
    pub original_count: usize,        // how many messages got folded in
    pub token_estimate_before: usize,
    pub token_estimate_after: usize,
}
```

### 5.2 Restore flow (when opening a session)

- Load session from disk → if `last_model_id` is set and is still in
  `savedModels`, set `activeModelId = last_model_id`. Otherwise fall back to
  the global default.
- Same for `last_mode`.
- Both stored on every change (`setActiveModel` debounced 300ms;
  `setMode` immediate). New Tauri command `set_session_model_pref` and
  `set_session_mode_pref` keep the persisted state in sync without
  re-saving the whole session on every keystroke.

### 5.3 Model-deletion mid-session

If the user deletes a model that is the `activeModelId` of an open
session, two things must happen:

1. **In-memory**: `activeModelId` is re-validated on `deleteSavedModel`.
   If the deleted model was active, the global default kicks in for the
   current session. The session's `last_model_id` on disk still points to
   the now-deleted model id — the next session-open restore flow (5.2)
   will detect this and fall back to the global default.
2. **On disk**: `last_model_id` is **not** rewritten at deletion time. The
   next time the user picks a new active model in the open session (or
   saves the session for any reason), `last_model_id` is updated to the
   new value. This avoids a write-amplification hot path on
   `deleteSavedModel`.

**Test** (PR3, in `session-manager.test.ts` or equivalent):
- `delete_saved_model_does_not_yank_active_session_mid_turn` — open a
  session with model A, fire a turn, delete model A, assert the in-flight
  turn is not interrupted (it has the model in hand already).
- `delete_saved_model_clears_last_model_id_on_next_save` — open a session
  with model A, delete model A, pick model B, save → assert
  `last_model_id == modelB.id` (not the deleted id).
- `restore_falls_back_to_global_default_when_last_model_id_missing` —
  write a session file with `last_model_id: "ghost"`, open it, assert
  `activeModelId` is the global default, not "ghost".

### 5.4 Files (PR3)

- `src-tauri/src/session.rs` — new fields, default helpers
- `src-tauri/src/commands.rs` — new IPC commands
- `src/lib/ipc.ts` — new method bindings
- `src/lib/session-manager.ts` — restore on open, persist on change,
  re-validate on model deletion
- `src/lib/store.ts` — wire to Zustand store

---

## 6. Compacting (G4) — **belongs to PR3**

### 6.1 The "compacted message" model

When compact fires, the messages array is **folded, not truncated**. The
original messages are not deleted from disk. The `Message` type gains an
optional `kind: "summary"` discriminator and `compact_meta`:

```rust
pub struct Message {
    pub id: String,
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,         // None | "summary"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_meta: Option<CompactMeta>,
}

pub struct CompactMeta {
    pub original_count: usize,
    pub token_estimate: usize,
    pub folded_message_ids: Vec<String>,  // originals the summary replaced
    pub compacted_at: String,             // ISO
}
```

### 6.2 Summary message shape — the role/kind design

**Design choice** (resolves the ambiguity flagged in spec review): the
summary message is a single `Message` with `role: "system"` and
`kind: Some("summary")`. It is **not** `role: "user"`.

```
[
  // the summary itself — this is the ONLY synthetic message inserted
  {
    id: "sum-<uuid>",
    role: "system",
    content: "<summary text>",
    kind: Some("summary"),
    compactMeta: { originalCount: 14, tokenEstimate: 1840, foldedMessageIds: [...], compactedAt: "..." }
  },
  // any messages AFTER the compaction point
  { id: "m15", role: "user", content: "now do X" },
  { id: "m16", role: "assistant", content: "..." }
]
```

**Why `role: "system"` and not `role: "user"`**:
- The LLM's view of the conversation must not look like a user pasted a
  huge summary. That's a confusing user turn.
- A `system` message establishes context for the assistant's next reply,
  which is exactly what a summary is for.
- The `kind: "summary"` discriminator lets the **UI** render it as a
  collapsed "📦 summary of 14 messages" block instead of a plain system
  message.

**Filter update in `agent_loop.rs`**: `history_from_session` (currently
filters `m.role == "user" || m.role == "assistant"`, lines 285-297) is
extended to also include `m.role == "system" && m.kind == Some("summary")`.
This means the summary is replayed to the LLM as a system message. The
orchestrator's own system prompt (currently the first message in the
history) is **not** included by `history_from_session` — it's added
separately by the loop. Adding the summary as a system message is a
parallel mechanism and does not collide with the orchestrator's system
prompt.

**Filter update in `MessageRenderer`**: messages with `kind == "summary"`
are rendered as a collapsed "Summary · 14 messages · ~1.8k tokens"
disclosure block. Click to expand and see the summary text + an
"originals" link that opens a side panel showing the folded messages
verbatim.

### 6.3 Manual trigger (`/compact`)

1. **Pre-checks**:
   - **Zero-message session**: if `messages` is empty (or contains only
     prior summaries), `/compact` is a no-op and shows a toast "Nothing to
     compact". The LLM is **not** called.
   - **In-flight turn**: if `streamingRequestId || agentRequestId` is
     set, `/compact` is **rejected** with a toast "Wait for the current
     turn to finish before compacting". The agent loop owns the session
     while a turn runs; compact would race. (This supersedes the existing
     "drop slash mid-stream" behavior in `Chat.tsx:240-241` for the
     compact case only — the other slash commands still drop.)
2. Take all messages with `kind != Some("summary")` (the "foldable" range).
3. If the foldable range has 0 or 1 messages, no LLM call is needed —
   `/compact` is a no-op with a toast "Nothing meaningful to compact yet
   (need at least 2 messages)".
4. Send the foldable messages to the LLM with a system prompt: "Summarize
   this conversation into 1–2k tokens preserving: user intent, decisions
   made, files modified, tool outcomes, open questions. **Discard file
   contents and command stdout unless the content itself is what the user
   asked about.**"
5. **Error handling**: if the LLM call throws, the session is **left
   unchanged** (no half-written summary message, no `CompactState`
   update). The error is surfaced as a toast "Compaction failed: <reason>".
   The LLM is retried by the user invoking `/compact` again.
6. On success: replace the folded range with a single summary message
   (`role: "system"`, `kind: "summary"`, `compactMeta: {…}`).
7. Persist, emit a `chat-compacted` event so the UI can show a "Compacted ·
   14 → 1 message" toast.
8. The `Session.compact` is updated with `compacted_at`,
   `summary_message_id`, `original_count`, token estimates.
9. The `Session.last_turn` field is **cleared** (set to `None`): the file
   watcher's fallback (which uses `lastTurn.reasoning`) points at a
   `requestId` that may no longer be in the visible message list after
   compaction. The next turn writes a fresh `last_turn`.

### 6.4 Auto trigger

Before each `chatComplete` call, the frontend estimates token usage
(rough: `chars / 4` per message, summed). If `> threshold` (default 0.8)
of the active model's `contextWindow`, fire a compact **before** the call.
The trigger uses the same code path as `/compact` — manual and auto are
the same operation.

**Edge cases**:
- **Unknown `contextWindow`**: `capabilitiesFor` returns
  `contextWindow: 0` for models without a known window. Auto-compact is
  **skipped** for these models (the threshold check is meaningless with a
  0 denominator; treating it as "always trigger" would compact every
  turn). The composer footer shows "· unknown window" instead of a
  percentage.
- **In-flight turn**: same rule as manual — auto-compact is **skipped** if
  a turn is mid-flight. The check happens on the *next* send. (If a
  long-running turn is what's pushing us over the threshold, the user
  can `/compact` manually once the turn lands.)
- **Session switch mid-auto-compact**: auto-compact is bound to the
  active session id at trigger time. If the user switches sessions while
  the LLM summarization is in flight, the compact completes against the
  **original** session (using its session id throughout). The held user
  message (if any) is **dropped** with a toast "Compaction finished
  against a different session — message discarded. Resend?".
- **Threshold configurable**: `compact_threshold` is a per-session field
  (default 0.8). A user can set it lower (e.g., 0.5) for cautious
  compacting, higher (0.95) for near-full-window use. A future settings
  panel can expose a global default.

### 6.5 Auto-compact UX (default: silent + undo)

When auto-compact fires, the behavior is:

1. The pending user message is held (not sent to the LLM yet).
2. Compact runs.
3. A toast appears: "Compacted · 14 messages → 1 summary · ~12.4k → ~1.8k
   tokens · [Undo]".
4. The pending user message is sent.
5. **Undo** (within 30s) restores the originals from the
   `CompactState.folded_message_ids`, removes the summary, and re-sends
   the held message.

Alternative options (A: ask before compacting, C: skip the turn and surface
a "context full" message) are documented but not implemented in v1.
Changing the default is a one-line config flip.

### 6.6 Token estimation

Rough — `chars / 4` per message, summed. Real tokenizer-accurate counts
are a future improvement. The estimate is displayed in the composer
footer ("~12.4k / 16k tokens · 78%") and triggers the auto-compact.

### 6.7 Tests (PR3)

In `src-tauri/src/agent_loop.rs` and a new `src-tauri/src/compact.rs`:

- `compact_folds_messages_into_summary` — manual `/compact` on a
  10-message session, assert 1 summary message + the original 10 are
  in `folded_message_ids`.
- `compact_summary_is_role_system_kind_summary` — assert the inserted
  message has `role: "system"`, `kind: Some("summary")`,
  `compactMeta.originalCount == 10`.
- `compact_with_zero_messages_is_noop` — empty session, no LLM call, no
  toast about "compacted 0 → 1", instead a "Nothing to compact" toast.
- `compact_with_only_one_message_is_noop` — single message, same
  as above.
- `compact_summarization_failure_leaves_session_unchanged` — mock LLM
  returns `Err`; assert no summary message in `messages`,
  `Session.compact == None`, error toast fired.
- `compact_mid_flight_turn_is_rejected` — set `streamingRequestId`,
  invoke `/compact`; assert no LLM call, "wait for current turn" toast.
- `compact_clears_last_turn` — session with a `last_turn` set, run
  compact, assert `last_turn == None`.
- `compact_persists_summary_message` — save + reload, summary message +
  `compact_meta` roundtrip cleanly.
- `undo_compact_restores_originals` — `CompactState.folded_message_ids`
  re-expand into the original message list.
- `auto_compact_fires_above_threshold` — mock model with 4k context
  window, force 5k chars of history, assert compact runs.
- `auto_compact_skipped_for_zero_context_window` — model with
  `contextWindow: 0`, force over-threshold, assert compact does **not**
  run.
- `auto_compact_skipped_mid_turn` — same as the manual mid-flight test
  but via the auto path.
- `auto_compact_session_switch_completes_against_original_session` —
  trigger auto-compact, switch `activeSessionId` mid-flight, assert the
  compact finishes against the original session id and the held message
  is dropped.
- `history_from_session_includes_summary_message` — verify the
  filter update: a `role: "system" + kind: "summary"` message is in
  the LLM history, an `role: "system" + kind: None` message is **not**
  (defensive — we don't expect the latter, but it prevents future
  bugs).
- `complete_with_system_prompt_smoke` — non-empty summary is produced
  for a non-trivial input set; the summary `id` is non-empty and
  `summary_message_id == id`.

### 6.8 Files (PR3)

- `src-tauri/src/session.rs` — `Message` gains `kind` + `compactMeta`;
  `CompactState`; compact functions
- `src-tauri/src/agent_loop.rs` — new `compact_chat` path; auto-trigger
  before each call; update `history_from_session` filter
- `src-tauri/src/compact.rs` — new, compact-specific logic
  (LLM call, fold/replace, error handling)
- `src-tauri/src/llm_client.rs` — add `complete_with_system_prompt` (or
  extend existing) for the summarization call
- `src/components/Chat.tsx` — wire `/compact`; show token estimate in
  composer footer; show undo toast; handle in-flight rejection
- `src/components/MessageRenderer.tsx` — render `kind: "summary"`
  messages as collapsed disclosure
- `src/lib/commands.ts` — register `/compact` (from PR2)
- `src/lib/chat-errors.ts` — handle compact errors

---

## 7. Out-of-scope follow-ups

- **Subagent implementation** (covered by `ISSUE.md`, separate spec).
- **Tokenizer-accurate token counting** (tiktoken or provider-specific).
- **Per-model context partitioning** (forks per model).
- **More slash commands** (`/system`, `/temperature`, `/tokens`, `/tree`,
  `/files`).
- **Voice / TTS** for the orchestrator's streamed responses (existing
  Bip mascot could be extended).

---

## 8. Migration / compatibility

- v1 session files (`action_mode: "auto" | "askRisky" | "askAll"`)
  migrate to v2 in `session.rs` on load.
- Wire format on disk changes: `action_mode` → `last_mode` (with the new
  enum values); new fields `last_model_id`, `compact`, `compact_threshold`
  with `#[serde(default)]` so old files load cleanly even before the
  migration runs.
- The `ActionMode` type alias in `src/lib/ipc.ts` is renamed; any code
  importing the old name fails at compile time and is fixed in the same
  PR.

---

## 9. Open questions for the spec reviewer

1. **Variant naming nit**: `CommandMode::Commands` reads oddly next to
   `CommandMode::Yolo` (one is a noun, one is an acronym). Alternatives
   on the table if reviewer flags it: `AskForCommands` / `RunAll`, or
   `Command` (singular) / `Yolo`. The Rust convention is PascalCase
   variants so a rename is cheap.
2. **Auto-compact UX default** is "silent + 30s undo toast". Worth
   confirming with the reviewer that this is the right default vs.
   "ask before compacting" for the v1 cut.
3. **Per-session mode persistence** writes on every toggle. With disk I/O
   this is fine for a single toggle but if it ever becomes a hot path, a
   debounce is the obvious fix.

---

## Appendix A: Changelog of reviewer fixes (v1 → v1.1)

The first draft (v1) of this spec was reviewed by the `verifier` subagent
on 2026-08-14 and returned `VERDICT: FAIL` with 17 issues. This v1.1
revision addresses them. Cross-references use the reviewer's check numbers.

| Check | Severity | Status | Fix |
|-------|----------|--------|-----|
| 1 — Migration mechanism undefined | P0 | ✅ Fixed | §4.3: `#[serde(alias = "action_mode")]` + post-parse normalization; full mapping table |
| 2 — Incomplete file list | P0 | ✅ Fixed | §4.6: full grep-confirmed file list including `Sidebar.tsx` and `session-manager.ts` |
| 3 — Model deleted mid-session | P1 | ✅ Fixed | §5.3: re-validate on `deleteSavedModel`; tests added |
| 4 — `/compact` on zero messages | P1 | ✅ Fixed | §6.3: explicit no-op path with toast |
| 5 — `/compact` mid-flight turn | P1 | ✅ Fixed | §6.3: explicit rejection with toast; overrides the existing "drop slash mid-stream" rule for `/compact` only |
| 6 — LLM summarization failure | P1 | ✅ Fixed | §6.3 step 5: error handling, session left unchanged, error toast |
| 7 — Session switch mid-auto-compact | P1 | ✅ Fixed | §6.4: bound to original session id; held message dropped with toast |
| 8 — Summary message role/kind | P0 | ✅ Fixed | §6.1–6.2: `role: "system"` + `kind: "summary"`; `history_from_session` filter updated; `MessageRenderer` updated |
| 9 — `parseMentionQuery` Fix B is a behavior change | P2 | ✅ Acknowledged | §3.1: explicit call-out that the test is a fix-and-test pair, not just a test of existing behavior |
| 10 — `Commands` / `Yolo` naming | P3 | ⏸ Deferred | §4.4: kept for v1; follow-up PR if the team wants verb-led variants |
| 11 — Missing migration tests | P2 | ✅ Fixed | §4.7: 6 migration tests added following `migration_from_v1` pattern |
| 12 — 0-context-window model | P2 | ✅ Fixed | §6.4: auto-compact skipped for `contextWindow: 0`; composer shows "unknown window" |
| 13 — Both popovers can fire | P1 | ✅ Fixed | §2.7: arbitration rule added; slash wins when both are active |
| 14 — `lastTurn` after compact | P2 | ✅ Fixed | §6.3 step 9: `last_turn` is cleared on compact |
| 15 — Default semantic change | P2 | ✅ Fixed | §4.3: explicit call-out that `#[default] Commands` is intentional |
| 16 — `complete_with_system_prompt` smoke | P2 | ✅ Fixed | §6.7: `complete_with_system_prompt_smoke` test added |
| 17 — Bundled PR too much | P3 | ✅ Fixed | §0: split into three PRs (PR1, PR2, PR3) with clear file lists per PR |

**Summary**: 15 issues fixed, 1 deferred to follow-up (naming), 1 explicitly
acknowledged as a fix-and-test pair (parseMentionQuery).
