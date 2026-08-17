// Unit tests for the todo list helpers. Spec
// 2026-08-14--agent-todo-list.md §12.

import { describe, expect, it } from "vitest";
import {
  isFinished,
  nextItem,
  progressFraction,
  reorderItems,
} from "./todo-store";
import type { TodoItem, TodoList } from "./ipc";

function item(
  id: string,
  text: string,
  status: "pending" | "in_progress" | "done" = "pending",
  order = 0,
): TodoItem {
  return { id, text, status, order };
}

function list(items: TodoItem[]): TodoList {
  return { items, version: 1 };
}

describe("progressFraction", () => {
  it("returns 0/0 for an empty list", () => {
    expect(progressFraction(list([]))).toEqual({ done: 0, total: 0 });
  });

  it("counts done items", () => {
    expect(
      progressFraction(
        list([
          item("a", "x", "done"),
          item("b", "y", "pending"),
          item("c", "z", "done"),
        ]),
      ),
    ).toEqual({ done: 2, total: 3 });
  });
});

describe("isFinished", () => {
  it("is false for an empty list (spec §6.1 — empty list is not 'finished')", () => {
    expect(isFinished(list([]))).toBe(false);
  });

  it("is false when at least one item is not done", () => {
    expect(
      isFinished(list([item("a", "x", "done"), item("b", "y", "pending")])),
    ).toBe(false);
  });

  it("is true when all items are done and at least one exists", () => {
    expect(
      isFinished(list([item("a", "x", "done"), item("b", "y", "done")])),
    ).toBe(true);
  });
});

describe("nextItem", () => {
  it("returns the first non-done item", () => {
    const l = list([
      item("a", "x", "done"),
      item("b", "y", "in_progress"),
      item("c", "z", "pending"),
    ]);
    expect(nextItem(l)?.id).toBe("b");
  });

  it("returns null when all items are done", () => {
    const l = list([item("a", "x", "done"), item("b", "y", "done")]);
    expect(nextItem(l)).toBeNull();
  });

  it("returns null on an empty list", () => {
    expect(nextItem(list([]))).toBeNull();
  });
});

describe("reorderItems", () => {
  const items: TodoItem[] = [
    item("a", "A", "pending", 0),
    item("b", "B", "pending", 1000),
    item("c", "C", "pending", 2000),
  ];

  it("moves an item forward and assigns sparse orders", () => {
    const next = reorderItems(items, 0, 2);
    expect(next.map((i) => i.id)).toEqual(["b", "c", "a"]);
    expect(next.map((i) => i.order)).toEqual([0, 1000, 2000]);
  });

  it("moves an item backward", () => {
    const next = reorderItems(items, 2, 0);
    expect(next.map((i) => i.id)).toEqual(["c", "a", "b"]);
  });

  it("is a no-op when from === to (returns a copy with new orders)", () => {
    const next = reorderItems(items, 1, 1);
    expect(next.map((i) => i.id)).toEqual(["a", "b", "c"]);
  });

  it("rejects out-of-bounds indices by returning a copy", () => {
    const next = reorderItems(items, 5, 0);
    expect(next.map((i) => i.id)).toEqual(["a", "b", "c"]);
  });

  it("does not mutate the input", () => {
    const snapshot = items.map((i) => i.id);
    reorderItems(items, 0, 2);
    expect(items.map((i) => i.id)).toEqual(snapshot);
  });
});
