# Composer v2 — Slash Commands, CommandMode, Per-Session Memory, Compacting

**Date:** 2026-08-14
**Status:** Draft (pending review)
**Author:** Mavis (brainstorming session with @gellert)

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

## 2. Slash-command popover (G1)

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

### 2.6 Files

- `src/lib/commands.ts` — new, command registry
- `src/lib/commands.test.ts` — new, command tests
- `src/components/ComposerPopover.tsx` — new, shared popover primitive
- `src/components/SlashCommandPopover.tsx` — new, slash popover
- `src/components/Chat.tsx` — wire slash parsing into composer; route
  detected commands to the registry

---

## 3. File-mention bug fixes (G5)

Two related bugs, both in `MentionInput.tsx`. The fix lives in the
`ComposerPopover` refactor (G1) because the shared popover primitive is
where dismissal happens.

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
popover stay "active" through a space.

### 3.2 Bug 2 — popover keeps running searches / shows stale results

Same root cause as Bug 1 — once the parser says `active: true` for a
half-typed mention, every keystroke fires `rankMentionResults` and
re-renders the list.

**Fix** — with Bug 1 fixed the popover closes promptly, so the symptom
stops. Add a belt-and-braces 1-frame `requestAnimationFrame` throttle on
`rankMentionResults` calls so a fast typist does not queue multiple searches
per keystroke.

### 3.3 Tests

In `src/lib/mention-utils.test.ts`:

- `parseMentionQuery("@foo ", caret=5)` → `{ active: false, query: "" }`
- `parseMentionQuery("hello @foo", caret=10)` → unchanged (still inactive
  mid-line)
- existing `parseMentionQuery` tests reviewed to make sure none relied on
  the old "still active through space" behavior

### 3.4 Acceptance

Type `@auth` → popover appears with matches. Type space → popover **closes
immediately**, no flash. Type `a` after the space → no popover, plain text.
Type `@auth` again on the same line → popover reopens with matches.

---

## 4. CommandMode rename (G2)

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

Existing on-disk sessions have `action_mode: "auto" | "askRisky" | "askAll"`.
A v1→v2 migration step in `session.rs` (alongside the existing v1→v2 message
migration around line 660) maps:

- `auto` → `yolo` (closest semantic — runs without asking)
- `askRisky` → `commands` (the new default)
- `askAll` → `commands` (collapse — `commands` is the stricter of the two,
  the safer migration target)

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

### 4.6 Files

- `src-tauri/src/session.rs` — rename enum, add migration
- `src-tauri/src/agent_loop.rs` — replace `ActionMode` references, update
  `requires_approval`, update tests
- `src-tauri/src/tools.rs` — update tool gate if needed (currently only
  reads `auto_allowed` boolean from ctx, not the enum)
- `src/components/Chat.tsx` — rename `ACTION_MODES` to `COMMAND_MODES`,
  update segmented control
- `src/lib/ipc.ts` — update `ActionMode` type alias
- `src/lib/store.ts` — update store types

---

## 5. Per-session memory (G3)

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

### 5.3 Files

- `src-tauri/src/session.rs` — new fields, default helpers
- `src-tauri/src/commands.rs` — new IPC commands
- `src/lib/ipc.ts` — new method bindings
- `src/lib/session-manager.ts` — restore on open, persist on change
- `src/lib/store.ts` — wire to Zustand store

---

## 6. Compacting (G4)

### 6.1 The "compacted message" model

When compact fires, the messages array is **folded, not truncated**. The
original messages are not deleted from disk. The `Message` type gains an
optional `kind: "summary"` discriminator:

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
}
```

The summary message replaces the folded range in the **rendered** view
(collapsed, expandable) but the originals stay in storage for "show
original" / "undo compact" actions.

### 6.2 Summary message shape

```
[
  { role: "system", content: "You are the Orchestrator. The following is a summary of the conversation so far..." },
  { role: "user", content: "<summary text>", kind: "summary", compactMeta: {…} },
  { role: "assistant", content: "Acknowledged. Continuing from summary." },
  // any messages AFTER the compaction point
  { role: "user", content: "now do X" },
  { role: "assistant", content: "..." }
]
```

### 6.3 Manual trigger (`/compact`)

1. Take all messages with `kind != "summary"` (or everything if no prior
   summary).
2. Send them to the LLM with a system prompt: "Summarize this conversation
   into 1–2k tokens preserving: user intent, decisions made, files
   modified, tool outcomes, open questions. **Discard file contents and
   command stdout unless the content itself is what the user asked about.**"
3. Replace the folded range with a single summary message.
4. Persist, emit a `chat-compacted` event so the UI can show a "Compacted ·
   14 → 1 message" toast.
5. The `CompactState` is updated with `compacted_at`,
   `summary_message_id`, `original_count`, token estimates.

### 6.4 Auto trigger

Before each `chatComplete` call, the frontend estimates token usage
(rough: `chars / 4` per message, summed). If `> threshold` (default 0.8)
of the active model's `contextWindow`, fire a compact **before** the call.
The trigger uses the same code path as `/compact` — manual and auto are
the same operation.

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

### 6.7 Tests

- `compact_folds_messages_into_summary` — manual `/compact` on a
  10-message session
- `auto_compact_fires_above_threshold` — mock model with small context
  window, force over-threshold, assert compact runs
- `compact_persists_summary_message` — save + reload, summary message +
  originals all present
- `undo_compact_restores_originals` — `CompactState.folded_message_ids`
  re-expand
- `parse_mention_…` already covered in 3.3

### 6.8 Files

- `src-tauri/src/session.rs` — `Message` gains `kind` + `compactMeta`;
  `CompactState`; compact functions
- `src-tauri/src/agent_loop.rs` — new `compact_chat` path; auto-trigger
  before each call
- `src-tauri/src/llm_client.rs` — add `complete_with_system_prompt` (or
  extend existing) for the summarization call
- `src/components/Chat.tsx` — wire `/compact`; show token estimate in
  composer footer; show undo toast
- `src/lib/commands.ts` — register `/compact`
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
