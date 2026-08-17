//! Session-scoped todo list (spec `2026-08-14--agent-todo-list.md`).
//!
//! The todo list is the agent's plan, visible to the user at the top of
//! the chat. The agent creates and updates items via the `todo_*` tools
//! registered in `tools.rs`; the user can also edit, reorder, add, and
//! delete items via the `edit_todo` / `append_todo` / `remove_todo` /
//! `clear_todo` IPC commands. Both paths serialize through the
//! per-session write lock held on `AppState::todo_locks` so concurrent
//! agent + user commits can never clobber each other.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex as TokioMutex;
use uuid::Uuid;

use crate::agent_loop::AgentEventSink;
use crate::session::{load_session, save_session, TodoItem, TodoList, TodoStatus};

/// Per-item input accepted from the FE (text + optional status). The
/// `id` is generated server-side on insert.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoItemInput {
    pub text: String,
    #[serde(default)]
    pub status: Option<TodoStatus>,
}

/// Per-item patch for `edit_todo`. All fields optional except `id`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoItemPatch {
    pub id: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub status: Option<TodoStatus>,
    #[serde(default)]
    pub order: Option<u32>,
}

/// Where a mutation came from. Drives the optional `source` field on
/// the `todo_changed` event payload so the FE can flash a subtle
/// highlight for agent-driven changes and stay quiet for user edits.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TodoChangeSource {
    Agent,
    User,
}

/// Event payload for the `todo_changed` Tauri event. Wire format
/// matches every other event on the bus (`DoneEvent`,
/// `TurnStartEvent`, etc.): `#[serde(rename_all = "camelCase")]`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoChangedEvent {
    pub session_id: String,
    pub list: TodoList,
    pub source: TodoChangeSource,
}

const TODO_TEXT_MAX: usize = 280;

/// Lazily fetch (or create) the per-session write lock. The lock is
/// held for the full `load → mutate → save` window so concurrent
/// commits on the same session serialize. The agent tool path uses
/// the same lock so the agent can't race the user and vice versa.
fn todo_lock(
    locks: &Arc<DashMap<(String, String), Arc<TokioMutex<()>>>>,
    project_path: &str,
    session_id: &str,
) -> Arc<TokioMutex<()>> {
    let key = (project_path.to_string(), session_id.to_string());
    if let Some(lock) = locks.get(&key) {
        return lock.value().clone();
    }
    let lock = Arc::new(TokioMutex::new(()));
    // DashMap::insert returns the prior value if any; entry().or_insert()
    // is racy under concurrent insert. Use the documented pattern:
    locks
        .entry(key)
        .or_insert_with(|| lock.clone())
        .value()
        .clone()
}

/// Validate item text. Empty or whitespace-only is allowed by the
/// validator on `todo_update` (treated as delete) but rejected on
/// insert (an empty item is meaningless).
fn validate_text_insert(text: &str) -> Result<String, String> {
    if text.is_empty() {
        return Err("Todo text cannot be empty".into());
    }
    if text.len() > TODO_TEXT_MAX {
        return Err(format!(
            "Todo text exceeds {} characters (got {})",
            TODO_TEXT_MAX,
            text.len()
        ));
    }
    Ok(text.to_string())
}

/// Truncate a todo text update to the cap. We accept whatever the FE
/// sends (the contenteditable may briefly exceed) but cap on
/// persist so the file is never malformed.
fn cap_text_update(text: &str) -> String {
    if text.len() > TODO_TEXT_MAX {
        // Truncate by chars, not bytes, to avoid panicking on
        // multi-byte UTF-8 boundaries.
        let mut out = String::with_capacity(TODO_TEXT_MAX);
        for c in text.chars().take(TODO_TEXT_MAX) {
            out.push(c);
        }
        out
    } else {
        text.to_string()
    }
}

/// Apply a mutation to a `TodoList` in-memory. Pure helper: no I/O,
/// no event emission. Returns the bumped list.
pub(crate) fn apply_set(list: &mut TodoList, items: Vec<TodoItemInput>) -> Result<(), String> {
    let mut next: Vec<TodoItem> = Vec::with_capacity(items.len());
    for (order, input) in items.into_iter().enumerate() {
        let text = validate_text_insert(&input.text)?;
        next.push(TodoItem {
            id: Uuid::new_v4().to_string(),
            text,
            status: input.status.unwrap_or(TodoStatus::Pending),
            order: order as u32,
        });
    }
    list.items = next;
    list.version = list.version.saturating_add(1);
    Ok(())
}

pub(crate) fn apply_append(
    list: &mut TodoList,
    items: Vec<TodoItemInput>,
    position: Option<u32>,
) -> Result<(), String> {
    if items.is_empty() {
        return Err("append_todo: `items` must not be empty".into());
    }
    let mut new_items: Vec<TodoItem> = Vec::with_capacity(items.len());
    for i in items {
        new_items.push(TodoItem {
            id: Uuid::new_v4().to_string(),
            text: validate_text_insert(&i.text)?,
            status: i.status.unwrap_or(TodoStatus::Pending),
            order: 0, // fixed up below
        });
    }
    // Pick a position. None / past-end → append.
    let pos = match position {
        Some(p) if (p as usize) <= list.items.len() => p as usize,
        _ => list.items.len(),
    };
    // Re-pack `order` sparsely: existing items keep their relative
    // positions, new items are inserted at `pos` with order = (prev +
    // next) / 2 when there's room, else bumped past the tail.
    for (i, item) in new_items.iter_mut().enumerate() {
        item.order = pos as u32 + i as u32;
    }
    list.items.splice(pos..pos, new_items);
    // Re-pack to a sparse step-1000 sequence so future inserts
    // between items have room.
    repack_orders(&mut list.items);
    list.version = list.version.saturating_add(1);
    Ok(())
}

pub(crate) fn apply_update(list: &mut TodoList, updates: Vec<TodoItemPatch>) -> Result<(), String> {
    if updates.is_empty() {
        return Err("todo_update: `updates` must not be empty".into());
    }
    for patch in updates {
        let Some(item) = list.items.iter_mut().find(|i| i.id == patch.id) else {
            return Err(format!("todo_update: unknown item id {}", patch.id));
        };
        if let Some(text) = patch.text {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                // Empty text on update = delete the item. Caller
                // signals intent; we materialize it here.
                list.items.retain(|i| i.id != patch.id);
                repack_orders(&mut list.items);
                list.version = list.version.saturating_add(1);
                return Ok(());
            }
            item.text = cap_text_update(trimmed);
        }
        if let Some(status) = patch.status {
            item.status = status;
        }
        if let Some(order) = patch.order {
            item.order = order;
        }
    }
    // After per-patch field updates, re-pack orders to keep the
    // sparse sequence clean.
    repack_orders(&mut list.items);
    list.version = list.version.saturating_add(1);
    Ok(())
}

pub(crate) fn apply_remove(list: &mut TodoList, ids: Vec<String>) -> Result<(), String> {
    if ids.is_empty() {
        return Err("todo_remove: `ids` must not be empty".into());
    }
    let before = list.items.len();
    list.items.retain(|i| !ids.contains(&i.id));
    if list.items.len() == before {
        return Err("todo_remove: no matching items found".into());
    }
    repack_orders(&mut list.items);
    list.version = list.version.saturating_add(1);
    Ok(())
}

pub(crate) fn apply_clear(list: &mut TodoList) {
    list.items.clear();
    list.version = list.version.saturating_add(1);
}

/// Re-pack `order` values to a sparse step-1000 sequence. Future
/// inserts between items just use the midpoint of the neighbors'
/// `order` values — no full re-numbering required.
fn repack_orders(items: &mut [TodoItem]) {
    items.sort_by_key(|i| i.order);
    for (i, item) in items.iter_mut().enumerate() {
        item.order = i as u32 * 1000;
    }
}

/// Validator used by the agent-side `todo_set` tool. Returns
/// `Ok(())` when the list is empty or already finished (the
/// overwrite rule); `Err(hint)` otherwise.
pub fn validate_todo_set(list: &TodoList) -> Result<(), String> {
    if list.is_finished() || list.items.is_empty() {
        Ok(())
    } else {
        Err(
            "Cannot overwrite an active todo list. Finish all items or use \
             todo_append / todo_update to modify the current list."
                .to_string(),
        )
    }
}

// ============================= IPC commands =============================
//
// Four user-initiated commands. All take (project_path, session_id, ...)
// and return the updated `TodoList` so the FE can reconcile without a
// follow-up `todo_changed` event parse. They emit `todo_changed` with
// `source: "user"` for any FE-driven change.

#[tauri::command]
pub async fn edit_todo(
    app: AppHandle,
    state: State<'_, crate::commands::AppState>,
    project_path: String,
    session_id: String,
    id: String,
    text: Option<String>,
    status: Option<TodoStatus>,
    order: Option<u32>,
) -> Result<TodoList, String> {
    let updates = vec![TodoItemPatch {
        id,
        text,
        status,
        order,
    }];
    apply_user_mutation(&app, &state, &project_path, &session_id, |list| {
        apply_update(list, updates)
    })
    .await
}

#[tauri::command]
pub async fn append_todo(
    app: AppHandle,
    state: State<'_, crate::commands::AppState>,
    project_path: String,
    session_id: String,
    items: Vec<TodoItemInput>,
    position: Option<u32>,
) -> Result<TodoList, String> {
    apply_user_mutation(&app, &state, &project_path, &session_id, |list| {
        apply_append(list, items, position)
    })
    .await
}

#[tauri::command]
pub async fn remove_todo(
    app: AppHandle,
    state: State<'_, crate::commands::AppState>,
    project_path: String,
    session_id: String,
    ids: Vec<String>,
) -> Result<TodoList, String> {
    apply_user_mutation(&app, &state, &project_path, &session_id, |list| {
        apply_remove(list, ids)
    })
    .await
}

#[tauri::command]
pub async fn clear_todo(
    app: AppHandle,
    state: State<'_, crate::commands::AppState>,
    project_path: String,
    session_id: String,
) -> Result<TodoList, String> {
    apply_user_mutation(&app, &state, &project_path, &session_id, |list| {
        apply_clear(list);
        Ok(())
    })
    .await
}

/// Shared helper for the four user IPC commands: acquire the
/// per-session write lock, load + mutate + save, emit the event,
/// return the new list. The lock is released when `_guard` drops.
async fn apply_user_mutation(
    app: &AppHandle,
    state: &State<'_, crate::commands::AppState>,
    project_path: &str,
    session_id: &str,
    f: impl FnOnce(&mut TodoList) -> Result<(), String>,
) -> Result<TodoList, String> {
    let lock = todo_lock(&state.todo_locks, project_path, session_id);
    let _guard = lock.lock().await;

    let mut session = load_session(project_path, session_id)
        .await?
        .ok_or_else(|| format!("Session {} not found", session_id))?;

    f(&mut session.todos)?;
    let new_list = session.todos.clone();
    let saved = save_session(project_path, session).await?;

    // Emit the event so any other open window (or a freshly-mounted
    // popover) sees the new state without polling. The IPC return
    // value already gives the originator the answer.
    let event = TodoChangedEvent {
        session_id: session_id.to_string(),
        list: saved.todos.clone(),
        source: TodoChangeSource::User,
    };
    if let Ok(payload) = serde_json::to_value(&event) {
        let _ = app.emit("todo_changed", payload);
    }

    Ok(new_list)
}

// ============================= Agent-side emitter =============================
//
// Used by the agent tool path: same event, source = "agent".

/// Emit a `todo_changed` event from the agent loop. The sink is the
/// loop's `Arc<dyn AgentEventSink>` (already has the Tauri `AppHandle`).
pub fn emit_todo_changed(
    sink: &Arc<dyn AgentEventSink>,
    session_id: &str,
    list: &TodoList,
    source: TodoChangeSource,
) {
    let event = TodoChangedEvent {
        session_id: session_id.to_string(),
        list: list.clone(),
        source,
    };
    if let Ok(payload) = serde_json::to_value(&event) {
        sink.emit("todo_changed", payload);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(text: &str) -> TodoItemInput {
        TodoItemInput {
            text: text.to_string(),
            status: None,
        }
    }

    fn patch(id: &str, text: Option<&str>) -> TodoItemPatch {
        TodoItemPatch {
            id: id.to_string(),
            text: text.map(String::from),
            status: None,
            order: None,
        }
    }

    #[test]
    fn is_finished_true_when_all_done() {
        let mut list = TodoList::default();
        let a = TodoItem {
            id: "a".into(),
            text: "x".into(),
            status: TodoStatus::Pending,
            order: 0,
        };
        let mut b = a.clone();
        b.id = "b".into();
        list.items.push(a);
        list.items.push(b);
        assert!(!list.is_finished());
        list.items[0].status = TodoStatus::Done;
        list.items[1].status = TodoStatus::Done;
        assert!(list.is_finished());
    }

    #[test]
    fn is_finished_false_on_empty() {
        assert!(!TodoList::default().is_finished());
    }

    #[test]
    fn validate_todo_set_allows_empty() {
        assert!(validate_todo_set(&TodoList::default()).is_ok());
    }

    #[test]
    fn validate_todo_set_allows_finished() {
        let mut list = TodoList::default();
        list.items.push(TodoItem {
            id: "a".into(),
            text: "x".into(),
            status: TodoStatus::Done,
            order: 0,
        });
        assert!(validate_todo_set(&list).is_ok());
    }

    #[test]
    fn validate_todo_set_blocks_active() {
        let mut list = TodoList::default();
        list.items.push(TodoItem {
            id: "a".into(),
            text: "x".into(),
            status: TodoStatus::InProgress,
            order: 0,
        });
        assert!(validate_todo_set(&list).is_err());
    }

    #[test]
    fn apply_set_assigns_ids_and_bumps_version() {
        let mut list = TodoList {
            version: 7,
            ..Default::default()
        };
        apply_set(&mut list, vec![item("one"), item("two")]).unwrap();
        assert_eq!(list.items.len(), 2);
        assert_ne!(list.items[0].id, list.items[1].id);
        assert!(list.items.iter().all(|i| !i.id.is_empty()));
        assert_eq!(list.version, 8);
        assert_eq!(list.items[0].order, 0);
        assert_eq!(list.items[1].order, 1);
    }

    #[test]
    fn apply_set_rejects_empty_text() {
        let mut list = TodoList::default();
        assert!(apply_set(&mut list, vec![item("")]).is_err());
    }

    #[test]
    fn apply_set_rejects_oversize_text() {
        let mut list = TodoList::default();
        let big = "x".repeat(TODO_TEXT_MAX + 1);
        assert!(apply_set(&mut list, vec![item(&big)]).is_err());
    }

    #[test]
    fn apply_update_empty_text_deletes_item() {
        let mut list = TodoList::default();
        apply_set(&mut list, vec![item("keep me")]).unwrap();
        let id = list.items[0].id.clone();
        apply_update(&mut list, vec![patch(&id, Some(""))]).unwrap();
        assert!(list.items.is_empty());
    }

    #[test]
    fn apply_update_unknown_id_errors() {
        let mut list = TodoList::default();
        assert!(apply_update(&mut list, vec![patch("nope", Some("x"))]).is_err());
    }

    #[test]
    fn apply_append_preserves_existing_order() {
        let mut list = TodoList::default();
        apply_set(&mut list, vec![item("a"), item("b")]).unwrap();
        let original_ids: Vec<String> = list.items.iter().map(|i| i.id.clone()).collect();
        apply_append(&mut list, vec![item("c")], None).unwrap();
        assert_eq!(list.items.len(), 3);
        assert_eq!(list.items[0].id, original_ids[0]);
        assert_eq!(list.items[1].id, original_ids[1]);
        // Repacked to 0, 1000, 2000.
        assert_eq!(list.items[0].order, 0);
        assert_eq!(list.items[1].order, 1000);
        assert_eq!(list.items[2].order, 2000);
    }

    #[test]
    fn apply_remove_unknown_ids_errors() {
        let mut list = TodoList::default();
        apply_set(&mut list, vec![item("a")]).unwrap();
        assert!(apply_remove(&mut list, vec!["nope".into()]).is_err());
    }

    #[test]
    fn apply_clear_resets_items_and_bumps_version() {
        let mut list = TodoList {
            version: 5,
            ..Default::default()
        };
        apply_set(&mut list, vec![item("a")]).unwrap();
        apply_clear(&mut list);
        assert!(list.items.is_empty());
        assert_eq!(list.version, 7);
    }
}
