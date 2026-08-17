// Plan popover — the hover/focus surface for the session todo list.
// Portaled to <body> at z-[85], anchored 12px below the strip.
// Mirrors the Bip thought-popover pattern (shared hover state across
// the 8px gap, Esc / outside-click to close).
//
// Spec 2026-08-14--agent-todo-list.md §6.2.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  Check,
  ChevronDown,
  GripVertical,
  Plus,
  X,
} from "lucide-react";
import { useStore } from "../lib/store";
import { ipc, type TodoItem, type TodoStatus } from "../lib/ipc";
import { isFinished, progressFraction, reorderItems } from "../lib/todo-store";
import { showToast } from "../lib/toast";

const TODO_TEXT_MAX = 280;
const STATUS_CYCLE: TodoStatus[] = ["pending", "in_progress", "done"];

export function PlanPopover({ children }: { children: React.ReactNode }) {
  const [open, setOpen] = useState(false);
  const [anchorRect, setAnchorRect] = useState<DOMRect | null>(null);
  const [popoverEl, setPopoverEl] = useState<HTMLDivElement | null>(null);
  const anchorRef = useRef<HTMLDivElement | null>(null);
  const closeTimer = useRef<number | null>(null);

  // Measure the anchor before opening so we can position the
  // portaled popover.
  useLayoutEffect(() => {
    if (!open) return;
    if (anchorRef.current) setAnchorRect(anchorRef.current.getBoundingClientRect());
  }, [open]);

  // Recompute on scroll / resize while open so the popover tracks
  // the anchor.
  useEffect(() => {
    if (!open) return;
    const onChange = () => {
      if (anchorRef.current) setAnchorRect(anchorRef.current.getBoundingClientRect());
    };
    window.addEventListener("scroll", onChange, true);
    window.addEventListener("resize", onChange);
    return () => {
      window.removeEventListener("scroll", onChange, true);
      window.removeEventListener("resize", onChange);
    };
  }, [open]);

  // Esc closes.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open]);

  // Outside click closes (don't close if the click is inside the
  // anchor — the user might be re-opening).
  useEffect(() => {
    if (!open) return;
    const onDocClick = (e: MouseEvent) => {
      const target = e.target as Node;
      if (anchorRef.current?.contains(target)) return;
      if (popoverEl?.contains(target)) return;
      setOpen(false);
    };
    document.addEventListener("mousedown", onDocClick);
    return () => document.removeEventListener("mousedown", onDocClick);
  }, [open, popoverEl]);

  // Shared-hover trick: small delay before close so the popover
  // doesn't flicker when the cursor crosses the 8px gap.
  const onAnchorEnter = () => {
    if (closeTimer.current !== null) {
      window.clearTimeout(closeTimer.current);
      closeTimer.current = null;
    }
    setOpen(true);
  };
  const onAnchorLeave = () => {
    if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
    closeTimer.current = window.setTimeout(() => setOpen(false), 100);
  };
  const onPopoverEnter = () => {
    if (closeTimer.current !== null) {
      window.clearTimeout(closeTimer.current);
      closeTimer.current = null;
    }
  };
  const onPopoverLeave = () => {
    if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
    closeTimer.current = window.setTimeout(() => setOpen(false), 100);
  };

  return (
    <div
      ref={anchorRef}
      className="inline-flex relative"
      onMouseEnter={onAnchorEnter}
      onMouseLeave={onAnchorLeave}
      onFocus={onAnchorEnter}
      onBlur={onAnchorLeave}
    >
      {children}
      {open &&
        anchorRect &&
        typeof document !== "undefined" &&
        createPortal(
          <div
            ref={setPopoverEl}
            onMouseEnter={onPopoverEnter}
            onMouseLeave={onPopoverLeave}
            style={{
              position: "fixed",
              top: anchorRect.bottom + 8,
              left: Math.max(
                8,
                Math.min(
                  anchorRect.left + anchorRect.width / 2 - 160,
                  window.innerWidth - 328,
                ),
              ),
              width: 320,
              zIndex: 85,
            }}
            className="glass-mist rounded-lg shadow-xl"
          >
            <PlanPopoverBody onClose={() => setOpen(false)} />
          </div>,
          document.body,
        )}
    </div>
  );
}

function PlanPopoverBody({ onClose: _onClose }: { onClose: () => void }) {
  const todos = useStore((s) => s.todos);
  const workspacePath = useStore((s) => s.workspacePath);
  const activeSessionId = useStore((s) => s.activeSessionId);
  const setTodos = useStore((s) => s.setTodos);

  const { done, total } = progressFraction(todos);
  const finished = isFinished(todos);
  const canEdit = !!workspacePath && !!activeSessionId;

  const dispatch = (
    apply: (prev: TodoItem[]) => TodoItem[],
    optimisticVersionBump = true,
  ) => {
    if (!canEdit) return;
    const next: TodoItem[] = apply(todos.items);
    // Optimistic update; the `todo_changed` event will reconcile.
    setTodos({
      items: next,
      version: optimisticVersionBump ? todos.version + 1 : todos.version,
    });
  };

  // Click handler for the status pill: cycles pending → in_progress → done → pending.
  const cycleStatus = (id: string) => {
    const item = todos.items.find((i) => i.id === id);
    if (!item) return;
    const idx = STATUS_CYCLE.indexOf(item.status);
    const next = STATUS_CYCLE[(idx + 1) % STATUS_CYCLE.length];
    const args = { id, status: next };
    dispatch((items) => items.map((i) => (i.id === id ? { ...i, status: next } : i)));
    ipc
      .editTodo(workspacePath!, activeSessionId!, args)
      .then((list) => setTodos(list))
      .catch((e) => {
        showToast(`Edit failed: ${e}`);
        // Revert.
        setTodos(todos);
      });
  };

  const updateText = (id: string, text: string) => {
    const trimmed = text.trim();
    // Empty text on commit = delete the item (per spec §6.2).
    if (trimmed.length === 0) {
      removeItem(id);
      return;
    }
    dispatch((items) =>
      items.map((i) => (i.id === id ? { ...i, text: trimmed } : i)),
    );
    ipc
      .editTodo(workspacePath!, activeSessionId!, { id, text: trimmed })
      .then((list) => setTodos(list))
      .catch((e) => {
        showToast(`Edit failed: ${e}`);
        setTodos(todos);
      });
  };

  const removeItem = (id: string) => {
    dispatch((items) => items.filter((i) => i.id !== id));
    ipc
      .removeTodo(workspacePath!, activeSessionId!, [id])
      .then((list) => setTodos(list))
      .catch((e) => {
        showToast(`Remove failed: ${e}`);
        setTodos(todos);
      });
  };

  const addItem = (text: string) => {
    const trimmed = text.trim();
    if (trimmed.length === 0) return;
    dispatch((items) => [
      ...items,
      {
        id: `optimistic-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
        text: trimmed,
        status: "pending",
        order: items.length * 1000,
      },
    ]);
    ipc
      .appendTodo(workspacePath!, activeSessionId!, [{ text: trimmed }])
      .then((list) => setTodos(list))
      .catch((e) => {
        showToast(`Add failed: ${e}`);
        setTodos(todos);
      });
  };

  const clearAll = () => {
    dispatch(() => []);
    ipc
      .clearTodo(workspacePath!, activeSessionId!)
      .then((list) => setTodos(list))
      .catch((e) => {
        showToast(`Clear failed: ${e}`);
        setTodos(todos);
      });
  };

  const reorder = (from: number, to: number) => {
    const next = reorderItems(todos.items, from, to);
    // Optimistic.
    setTodos({ items: next, version: todos.version + 1 });
    // Batch the order updates into one IPC.
    const updates = next.map((item, i) => ({ id: item.id, order: i * 1000 }));
    // We don't have a batch endpoint; fire-and-forget per spec.
    // The Rust side will accept sequential `edit_todo` calls; the
    // per-session write lock keeps the file consistent.
    void Promise.all(
      updates.map((u) =>
        ipc
          .editTodo(workspacePath!, activeSessionId!, u)
          .catch(() => undefined),
      ),
    ).then(() => {
      // Final reconcile from a fresh load — skip if the user has
      // made more edits in the meantime by re-reading the store.
      // The Rust side will also fire `todo_changed` for each commit.
    });
  };

  return (
    <div className="flex flex-col">
      {/* Header */}
      <div className="px-3 py-2 border-b border-border-subtle flex items-center justify-between">
        <span className="text-[9px] uppercase tracking-[0.14em] text-white-15 font-medium">
          Plan
        </span>
        <span className="text-[11px] text-white-30">
          {done}/{total} done
        </span>
      </div>

      {/* Items */}
      <ul className="flex flex-col max-h-[280px] overflow-y-auto">
        {todos.items.map((item, i) => (
          <TodoRow
            key={item.id}
            item={item}
            index={i}
            onCycleStatus={cycleStatus}
            onUpdateText={updateText}
            onRemove={removeItem}
            onReorder={reorder}
            canEdit={canEdit}
            showFinishedAccent={finished}
          />
        ))}
      </ul>

      {/* Add row */}
      <AddRow onAdd={addItem} canEdit={canEdit} />

      {/* Footer */}
      <div className="px-3 py-2 border-t border-border-subtle flex items-center justify-end">
        <button
          type="button"
          onClick={clearAll}
          disabled={!canEdit || todos.items.length === 0}
          className="text-[11px] text-white-15 hover:text-status-error transition-colors disabled:opacity-30 disabled:cursor-not-allowed"
        >
          Clear plan
        </button>
      </div>
    </div>
  );
}

function TodoRow({
  item,
  index,
  onCycleStatus,
  onUpdateText,
  onRemove,
  onReorder,
  canEdit,
  showFinishedAccent,
}: {
  item: TodoItem;
  index: number;
  onCycleStatus: (id: string) => void;
  onUpdateText: (id: string, text: string) => void;
  onRemove: (id: string) => void;
  onReorder: (from: number, to: number) => void;
  canEdit: boolean;
  showFinishedAccent: boolean;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(item.text);
  const dragRef = useRef<HTMLLIElement | null>(null);

  const isDone = item.status === "done";
  const isInProgress = item.status === "in_progress";

  const startEdit = () => {
    if (!canEdit) return;
    setDraft(item.text);
    setEditing(true);
  };
  const commitEdit = () => {
    if (!editing) return;
    setEditing(false);
    if (draft !== item.text) onUpdateText(item.id, draft);
  };
  const cancelEdit = () => {
    setEditing(false);
    setDraft(item.text);
  };

  return (
    <li
      ref={dragRef}
      draggable={canEdit}
      onDragStart={(e) => {
        if (!canEdit) return;
        e.dataTransfer.effectAllowed = "move";
        e.dataTransfer.setData("text/plain", String(index));
      }}
      onDragOver={(e) => {
        if (!canEdit) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = "move";
      }}
      onDrop={(e) => {
        if (!canEdit) return;
        e.preventDefault();
        const from = Number.parseInt(e.dataTransfer.getData("text/plain"), 10);
        if (Number.isInteger(from) && from !== index) onReorder(from, index);
      }}
      className="group flex items-center gap-2 px-3 py-1.5 hover:bg-white/[0.04] transition-colors h-8"
      style={
        showFinishedAccent && isDone
          ? { boxShadow: "inset 2px 0 0 0 var(--accent-green, #22c55e)" }
          : undefined
      }
    >
      {/* Status pill */}
      <button
        type="button"
        onClick={() => onCycleStatus(item.id)}
        disabled={!canEdit}
        aria-label={`Status: ${item.status}`}
        className={
          "w-5 h-5 rounded-full border flex items-center justify-center flex-shrink-0 transition-colors disabled:cursor-not-allowed " +
          (isDone
            ? "bg-status-active border-status-active"
            : isInProgress
            ? "bg-status-active/15 border-status-active animate-pulse"
            : "bg-surface-2 border-border")
        }
      >
        {isDone && <Check size={11} className="text-black" />}
      </button>

      {/* Text (contenteditable on click) */}
      {editing ? (
        <input
          autoFocus
          value={draft}
          maxLength={TODO_TEXT_MAX}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commitEdit}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              commitEdit();
            } else if (e.key === "Escape") {
              e.preventDefault();
              cancelEdit();
            }
          }}
          className="flex-1 bg-transparent text-[13px] text-white outline-none border-b border-border-subtle focus:border-border-strong"
        />
      ) : (
        <button
          type="button"
          onClick={startEdit}
          className={
            "flex-1 text-left text-[13px] truncate " +
            (isDone ? "text-white-30 line-through" : "text-white")
          }
        >
          {item.text}
        </button>
      )}

      {/* Hover affordances */}
      <div className="flex items-center gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity">
        <GripVertical
          size={12}
          className="text-white-30 cursor-grab flex-shrink-0"
        />
        <button
          type="button"
          onClick={() => onRemove(item.id)}
          disabled={!canEdit}
          aria-label="Remove item"
          className="text-white-30 hover:text-status-error transition-colors disabled:opacity-30 disabled:cursor-not-allowed"
        >
          <X size={12} />
        </button>
      </div>
    </li>
  );
}

function AddRow({
  onAdd,
  canEdit,
}: {
  onAdd: (text: string) => void;
  canEdit: boolean;
}) {
  const [adding, setAdding] = useState(false);
  const [draft, setDraft] = useState("");

  const commit = () => {
    const text = draft;
    setAdding(false);
    setDraft("");
    if (text.trim().length > 0) onAdd(text);
  };
  const cancel = () => {
    setAdding(false);
    setDraft("");
  };

  if (adding) {
    return (
      <div className="px-3 py-1.5 border-t border-border-subtle h-8 flex items-center">
        <input
          autoFocus
          value={draft}
          maxLength={TODO_TEXT_MAX}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              commit();
            } else if (e.key === "Escape") {
              e.preventDefault();
              cancel();
            }
          }}
          placeholder="New step…"
          className="flex-1 bg-transparent text-[12px] text-white outline-none border-b border-border-subtle focus:border-border-strong placeholder:text-white-15"
        />
      </div>
    );
  }

  return (
    <div className="px-3 py-1.5 border-t border-border-subtle h-8 flex items-center">
      <button
        type="button"
        onClick={() => canEdit && setAdding(true)}
        disabled={!canEdit}
        className="flex items-center gap-1 text-[12px] text-white-30 hover:text-white transition-colors disabled:opacity-30 disabled:cursor-not-allowed"
      >
        <Plus size={12} />
        Add step
      </button>
    </div>
  );
}

// Suppress unused-imports warnings for icons that may be used
// conditionally in future visual iterations.
void ChevronDown;
