# Agent Todo List — Spec

> **Status:** Draft, awaiting review.
> **Date:** 2026-08-14
> **Author:** Mavis (brainstorming session with gellert)
> **Related:** `PRODUCT.md` (workspace + multi-agent loop), `DESIGN.md`
> (visual contract, glass-mist / glass-quartz, monochrome palette),
> `AGENTS.md` (code conventions).

## Goal

Give the main harness agent a first-class, session-scoped **todo list** that:

- The agent can author and update via native provider tool calls
  (`todo_set`, `todo_append`, `todo_update`, `todo_remove`, `todo_clear`).
- The user can see at a glance as a compact strip in the ChatHeader row,
  expanding into a `glass-mist` popover with inline edit affordances.
- Persists in the existing session file (`.harness/sessions/<id>.json`),
  no new persistence machinery.
- Enforces a "finished list" rule so the agent can only overwrite the
  plan once all items are done, preventing the agent from clobbering a
  live plan the user is still acting on.

The todo list is **the agent's plan, visible to the user, editable by
the user**. It is not a separate chat thread, not a project-level
roadmap, not a personal scratchpad. One list per session, hidden when
empty.

## Design

### 1. Data model

#### 1.1 Rust — `src-tauri/src/session.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus { Pending, InProgress, Done }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,         // nanoid, server-side generated
    pub text: String,       // 1..=280 chars, validated
    pub status: TodoStatus,
    pub order: u32,         // 0-based, sparse allowed (gaps on delete)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TodoList {
    pub items: Vec<TodoItem>,   // kept sorted by `order` on read
    pub version: u64,           // bumped on every commit
}

// Existing `Session` struct gains:
pub struct Session {
    // ...existing fields...
    #[serde(default)]
    pub todos: TodoList,
}
```

`#[serde(default)]` on the new `todos` field means **old session files
load with an empty todo list**, no migration needed.

#### 1.2 Frontend — `src/lib/store.ts`

```ts
export type TodoStatus = "pending" | "in_progress" | "done";

export interface TodoItem {
  id: string;
  text: string;
  status: TodoStatus;
  order: number;
}

export interface TodoList {
  items: TodoItem[];
  version: number;
}

// On the store:
todos: TodoList;
hydratedTodosFor: string | null;  // session id we hydrated for
setTodos(list: TodoList): void;
```

### 2. Provider tool surface (`src-tauri/src/tools.rs`)

Five new tools, registered alongside existing workspace tools. All
return `TodoList` on success.

| Tool | Args | Effect | Risk |
|---|---|---|---|
| `todo_set` | `{ items: [{text, status?}] }` | Replace the whole list. Rejected if list is active (see §6). | `Safe` |
| `todo_append` | `{ items: [{text, status?}], position? }` | Insert items at `position` (defaults to end). | `Safe` |
| `todo_update` | `{ updates: [{id, text?, status?, order?}] }` | Patch specific items by id. | `Safe` |
| `todo_remove` | `{ ids: string[] }` | Delete items by id. | `Risky` (action-mode gated) |
| `todo_clear` | `{}` | Wipe the list. | `Risky` (action-mode gated) |

`Risk` flows through the existing action-mode gate the same way
`run_command` does — in `Commands` mode the user sees an Allow/Deny
card; in `Yolo` mode the tool runs without prompting.

Tool-call JSON schemas follow the same shape as existing tools. Each
item text enforces `minLength: 1, maxLength: 280`.

### 3. IPC commands (`src/lib/ipc.ts`, `src-tauri/src/commands.rs`)

User-initiated edits. No approval gate — these are direct human
actions, the agent will see the new state on its next turn.

| Command | Args | Returns |
|---|---|---|
| `edit_todo` | `{ session_id, id, text?, status?, order? }` | `TodoList` |
| `append_todo` | `{ session_id, items: [{text, status?}], position? }` | `TodoList` |
| `remove_todo` | `{ session_id, ids: string[] }` | `TodoList` |
| `clear_todo` | `{ session_id }` | `TodoList` |

All four commit through the same `Mutex<Session>` write path as
tool-initiated changes, bump `TodoList::version`, and emit the same
`todo_changed` event.

### 4. Streaming event

A new event on the existing Tauri event bus:

```
todo_changed {
  session_id: string,
  list: TodoList,
  source: "agent" | "user"
}
```

Emitted after every commit. The `source` field lets the frontend
choose to flash a subtle highlight on agent mutations while leaving
user edits visually quiet (user edits are already obvious from the
interaction).

### 5. Frontend components

#### 5.1 `src/components/PlanStrip.tsx` (new)

The hairline anchor strip. Lives in the ChatHeader row (the centered
stats row directly under the title bar), added to its left side.
Existing model/tokens/usage telemetry stays on the right.

- **Hidden when** `items.length === 0`. Mounted always (zero-cost
  render). The first `todo_changed` event with non-empty items makes
  it appear.
- **Rest visual:** `glass-mist` pill, `--m-radius: 999px`,
  `text-[11px] text-white-50 font-medium gap-1.5`.
- **Content (rest):** `Plan` wordmark (section-label recipe:
  `text-[9px] uppercase tracking-[0.14em] text-white-15`) + progress
  fraction `n/m` in `text-white` + `next: <next pending text>` in
  `text-white-30` + chevron (`ChevronDown`, 12px, rotates 180° when
  `data-open="true"`).
- **"All done" transition:** strip text changes to `Plan · done` in
  `text-status-active` for 1.2s, then settles to plain `Plan`. The
  `status.active` left rule (2px) appears on the popover header during
  this window.
- **`data-tauri-drag-region="false"`** on the strip so the title bar
  drag region isn't triggered.

#### 5.2 `src/components/PlanPopover.tsx` (new)

The hover/focus surface. Portaled to `<body>` at `z-[85]` (just under
Bip's thought popover at `z-[90]`). Anchored 12px below the strip.
Width 320px, max-height 360px (internal scroll for >10 items).

Uses the same shared-hover state trick as Bip's thought popover so
the 8px gap doesn't close it mid-read. Click outside closes. `Esc`
closes. Click inside doesn't close.

**Structure (top → bottom):**

1. **Header row.** `Plan` label (section-label recipe) on the left,
   `n/m done` count on the right. `text-[11px] text-white-30`,
   `px-3 py-2 border-b border-border-subtle`.
2. **Items list.** `flex flex-col gap-px`, each row 32px tall,
   `px-3 hover:bg-white/[0.04]`:
   - **Status pill (24×24 left):** clickable. `rounded-full
     border`. Pending = `bg-surface-2 border-border`. In progress =
     `bg-status-active/15 border-status-active` with `animate-pulse`.
     Done = `bg-status-active border-status-active` with a `Check`
     Lucide (12px) inside. Click dispatches `edit_todo` with the new
     status.
   - **Text.** `text-[13px]`. `text-white` when pending or
     in_progress; `text-white-30 line-through` when done. Click →
     `contenteditable`. `Enter` commits, `Esc` reverts, blur
     commits. Empty text on commit is treated as delete.
   - **Hover affordances (right side, fade in on row hover, 100ms):**
     `GripVertical` (12px, `text-white-30 cursor-grab`) and `X` (12px,
     `text-white-30 hover:text-status-error`).
3. **`+ Add step` row** at the bottom. `Plus` icon (12px) + `text-white-30
   text-[12px]`. Click → inline input (same recipe as item text),
   `Enter` commits via `append_todo`. The `+` row also serves as the
   drop target for the end of the list when dragging.
4. **Footer.** `Clear plan` button on the right (`text-white-15
   text-[11px]`, hover `text-status-error`). Dispatches `clear_todo`.

**Drag-reorder.** Native HTML5 drag (`draggable`, `onDragStart`,
`onDragOver`, `onDrop`). On drop we compute new sparse `order` values
as `step * 1000` so future inserts between items don't require
re-numbering the whole list. One `edit_todo` per affected item,
batched in a single IPC call (the FE coalesces contiguous moves into
one round-trip).

**Animation budget.** Strip fade-in: 150ms. Popover open: 100ms
`translate-y-1 → 0`. All animations respect `prefers-reduced-motion`
(zero-out transforms).

#### 5.3 `src/lib/todo-store.ts` (new, small)

Pure helpers, no React. Selectors and derived state:

```ts
export function progressFraction(list: TodoList): { done: number; total: number };
export function isFinished(list: TodoList): boolean;  // total > 0 && all done
export function nextItem(list: TodoList): TodoItem | null;  // first non-done
export function reorderItems(items: TodoItem[], from: number, to: number): TodoItem[];
```

#### 5.4 Store wiring (`src/lib/store.ts`)

- `setTodos(list)` — replace the slice. Called on session hydrate
  (`useProjectLifecycle`) and on every `todo_changed` event.
- Optimistic update path: when a user edit dispatches, the FE
  immediately applies the change to the store, then reconciles when
  the `todo_changed` event arrives. If the incoming `version` is
  higher than the local optimistic version, the local state is
  replaced (last-write-wins, no conflict UI in v1).

### 6. The "finished" rule (list-level overwrite permission)

The list is `finished` when `items.length > 0 && items.every(i =>
i.status === Done)`. This is **derived state**, not stored — the
frontend computes it from the items, the Rust validator computes it
the same way.

**Validator (in `todo_set` handler):**

```rust
fn validate_todo_set(session: &Session, new_items: &[TodoItem]) -> Result<(), ToolError> {
    let current = &session.todos.items;
    let is_empty = current.is_empty();
    let is_finished = !current.is_empty()
        && current.iter().all(|i| i.status == TodoStatus::Done);

    if !is_empty && !is_finished {
        return Err(ToolError::Rejected(
            "Cannot overwrite an active todo list. Finish all items or use \
             todo_append / todo_update to modify the current list.".into()
        ));
    }
    Ok(())
}
```

This is a **soft enforcement** — the agent gets a clear error and a
hint, the LLM self-corrects. The user can always wipe via
`clear_todo` (or by manually checking off remaining items) to allow
overwrite.

### 7. System prompt addition (`src-tauri/src/agent.rs`)

A short paragraph added to the harness agent's system prompt,
alongside the existing workspace tools description:

```
## Plan (todos)

You have a session-scoped todo list visible to the user at the top of
the chat as a compact menu. Use it to plan non-trivial multi-step work
so the user can see what's coming and edit it before you start.

Rules:
- At the start of any non-trivial task, call `todo_append` (or
  `todo_set` if the list is empty) to publish your plan.
- Update items as you work: `todo_update` to mark `in_progress` when
  you start, `done` when you finish. Don't batch — update live.
- You may only call `todo_set` (replace the whole list) when the
  current list is empty or all items are `done`. The system will
  reject otherwise.
- Keep item text short and action-oriented ("Read the auth handler",
  not "We will now begin the process of reading..."). Max 280 chars.
- For trivial single-step requests, no plan is needed.
```

### 8. Context for the LLM (every turn)

The current todo list is included in the message context the LLM
sees, formatted as a compact block before the conversation. Format:

```
[Plan: 2/5 done]
  ✓ Read the auth handler
  ✓ Inspect the token refresh path
  → Add retry logic to the refresh call
  ⏵ Add telemetry around failed refreshes
  ⏵ Write a test for the new retry branch
```

Slots into the existing context-assembly code in `agent.rs` next to
the active-model and session-metadata blocks.

### 9. Chat breadcrumb

A small inline marker in the chat area on every `todo_changed` event
("Plan updated: 3 items"), in the same style as the existing tool
call chips in `Chat.tsx`. The popover is the source of truth; the
breadcrumb is a brief notification so the user notices the change
even if the popover is closed.

### 10. Persistence

No new files. `Session` serializes/deserializes the new `todos` field
through the existing `session_watcher.rs` debounced flush. Hydration
on session load is part of the existing session-load event payload.

### 11. Error handling

| Failure | Behavior |
|---|---|
| Agent calls `todo_set` on an active list | Tool returns `ToolError::Rejected` with the hint. Agent loop emits the rejection as a normal tool-error event; LLM self-corrects. |
| `edit_todo` IPC fails (session file lock, deserialization) | Frontend rolls back the optimistic update and surfaces a toast. Strip/popover revert to the last-known-good state. |
| Item id doesn't exist in `todo_update` / `todo_remove` | `ToolError::InvalidArgs`. Agent retries with correct ids. |
| `text` empty after editing | Treated as delete. FE enforces via `contenteditable` commit logic. |
| `text` over 280 chars | Validator rejects, error names the limit. FE `maxLength={280}`. |
| Concurrent agent + user edits to the same item | Rust session holds a `Mutex<Session>` for writes. `version` is bumped on every commit. FE compares version on incoming `todo_changed`; older local optimistic state is replaced. |
| Session file fails to parse (corrupt JSON, pre-todo schema) | `#[serde(default)]` on `todos` means old sessions load with an empty list. No migration. |
| Tauri event bus unavailable (`npm run dev` browser) | All `todo_*` tool calls return a clear "not available in browser" error. Strip stays hidden. |

### 12. Edge cases

- **Empty list + `todo_set({items: []})`.** Allowed (clears the list).
- **`todo_append` with `position` past end.** Treated as "append at
  end". No error.
- **User drags an item to its current position.** No-op, no IPC.
- **Popover open + user types in chat composer.** Popover stays open,
  no focus stealing.
- **User mid-rename + `todo_changed` arrives.** Cancels the
  contenteditable, replaces local state, unsaved text lost.
  Acceptable for v1.
- **Long lists (>20 items).** Popover max-height 360px, internal
  scroll, fixed 32px row height. Drag works within the scroll
  container.
- **`isFinished` highlight only fires when transitioning from
  non-finished to finished** (not on session load of a pre-existing
  finished list).

## Testing

### Unit (Vitest)

- `src/lib/todo-store.test.ts` — selectors (`progressFraction`,
  `isFinished`, `nextItem`), ordering invariants, sparse `order`
  math for drag-reorder.
- `src/lib/store.test.ts` (extend) — `setTodos` hydration, version
  bumping, optimistic-update reconciliation.
- `src/components/PlanPopover.test.tsx` — render states (empty
  hidden, active list, finished), hover affordances, drag-reorder
  callback, clear-plan dispatch, contenteditable commit logic.
- `src/components/PlanStrip.test.tsx` — render states (empty, in
  progress, all-done highlight then settle), popover open/close.

### Rust (`cargo test`)

- `src-tauri/src/session.rs` — round-trip serde for `TodoList`,
  default-empty for old sessions, version bump on mutation.
- `src-tauri/src/tools.rs` — `validate_todo_set` happy path (empty
  list, all-done list) and rejection path (active list);
  `todo_append` / `todo_update` / `todo_remove` / `todo_clear` arg
  validation; 280-char cap; empty-text-delete; sparse `order`
  assignment.

### Integration (manual)

1. Tauri dev session, prompt the harness agent with a 3-step task.
   Verify:
   - Strip appears with all 3 items.
   - Items flip to `in_progress` / `done` as the agent works.
   - Popover edits round-trip through `edit_todo`.
   - `Clear plan` wipes the list.
   - Restart the app, confirm list persists in
     `.harness/sessions/<id>.json`.
2. Verify the "finished → overwrite" rule: complete all items, then
   prompt the agent with a new multi-step task — confirm it can
   call `todo_set`. Interrupt mid-list — confirm `todo_set` is
   rejected with the expected error and the LLM self-corrects.

### Quality gates (per `AGENTS.md`)

- `npx tsc --noEmit` clean.
- `cargo fmt && cargo clippy` clean.
- No new Tailwind colors — everything reuses the existing palette
  (`surface-*`, `white-*`, `border-*`, `status.active`,
  `status.error`).

## Out of scope (v1)

- Conflict-resolution UI (last-write-wins is enough for v1).
- Per-turn todo history / "what was the plan at step 3?" replay.
- Project-level (workspace-scoped) todo lists.
- Personal human-only scratchpad mode.
- Sub-agent todos — the orchestrator can dispatch sub-agents and
  each sub-agent has its own plan in its own session tile, but the
  parent todo list does not auto-decompose.
- Todo templates or recurring plans.
- Drag-to-reorder across sessions (would need a different storage
  model).

## Files touched

**New:**
- `src/components/PlanStrip.tsx`
- `src/components/PlanPopover.tsx`
- `src/lib/todo-store.ts`
- `src/components/PlanStrip.test.tsx`
- `src/components/PlanPopover.test.tsx`
- `src/lib/todo-store.test.ts`

**Modified:**
- `src-tauri/src/session.rs` — add `TodoList`, `TodoItem`,
  `TodoStatus`; add `todos` field to `Session`.
- `src-tauri/src/tools.rs` — add the five `todo_*` tool defs and
  handlers; add `validate_todo_set`.
- `src-tauri/src/agent.rs` — system-prompt addition; include todo
  list in turn context; emit `todo_changed` after tool commits.
- `src-tauri/src/commands.rs` — register `edit_todo`, `append_todo`,
  `remove_todo`, `clear_todo` IPC handlers.
- `src-tauri/src/hci/methods_sessions_agents.rs` — wire the new IPC
  commands if they live in the HCI layer.
- `src/lib/ipc.ts` — type signatures for the four user IPC commands.
- `src/lib/store.ts` — add `todos`, `hydratedTodosFor`, `setTodos`;
  `todo_changed` event subscription.
- `src/components/ChatHeader.tsx` — render `PlanStrip` in the left
  side of the centered stats row.
- `src/components/Chat.tsx` — render the `todo_changed` breadcrumb
  next to the existing tool-call chips.
- `src/lib/session-manager.ts` — hydrate `todos` on session load.
- `DESIGN.md` — add a `Plan (todo list)` subsection to the
  Component patterns section, documenting the strip + popover
  recipe and the "all done" transition.
