// Pure helpers for the todo list (spec 2026-08-14--agent-todo-list.md).
// No React, no I/O — just selectors and small reorder math.

import type { TodoItem, TodoList } from "./ipc";

/** Progress fraction: { done, total }. total is 0 when the list is empty. */
export function progressFraction(list: TodoList): { done: number; total: number } {
  const total = list.items.length;
  const done = list.items.filter((i) => i.status === "done").length;
  return { done, total };
}

/** True when the list has at least one item and every item is done.
 *  Used by the "all done" visual and matches the Rust `is_finished`. */
export function isFinished(list: TodoList): boolean {
  return list.items.length > 0 && list.items.every((i) => i.status === "done");
}

/** The first item that isn't `done`, or `null` if all are done. */
export function nextItem(list: TodoList): TodoItem | null {
  return list.items.find((i) => i.status !== "done") ?? null;
}

/** Reorder `items` by moving the entry at `from` to `to`. Both indices
 *  are positions in the input array. The returned array carries
 *  fresh sparse `order` values (step 1000) so the Rust side can
 *  apply them as a single batched `edit_todo` call.
 *
 *  Pure: does not mutate the input. */
export function reorderItems(
  items: readonly TodoItem[],
  from: number,
  to: number,
): TodoItem[] {
  if (from === to) return items.slice();
  if (from < 0 || from >= items.length) return items.slice();
  if (to < 0 || to >= items.length) return items.slice();

  const next = items.slice();
  const [moved] = next.splice(from, 1);
  next.splice(to, 0, moved);
  return next.map((item, i) => ({ ...item, order: i * 1000 }));
}
