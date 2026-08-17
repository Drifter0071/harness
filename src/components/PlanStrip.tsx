// Plan strip — the compact anchor for the session-scoped todo list.
// Sits in the ChatHeader row (between the model/tokens/usage cluster
// and the conditional session-window row). Hover/focus expands into
// the PlanPopover. Hidden when the list is empty.
//
// Spec 2026-08-14--agent-todo-list.md §6.1.

import { useStore } from "../lib/store";
import { isFinished, nextItem, progressFraction } from "../lib/todo-store";
import { PlanPopover } from "./PlanPopover";

export function PlanStrip() {
  const todos = useStore((s) => s.todos);
  // Hidden when no items.
  if (todos.items.length === 0) return null;

  const { done, total } = progressFraction(todos);
  const finished = isFinished(todos);
  const next = nextItem(todos);

  return (
    <div className="flex items-center justify-center mt-1.5 select-none">
      <PlanPopover>
        <button
          type="button"
          data-testid="plan-strip"
          className="glass-mist glass-mist--pill flex items-center gap-1.5 px-2.5 py-1 rounded-full text-[11px] text-white-50 font-medium hover:text-white transition-colors"
        >
          <span className="text-[9px] uppercase tracking-[0.14em] text-white-15 font-medium">
            Plan
          </span>
          <span className="text-white">{done}/{total}</span>
          {!finished && next && (
            <span className="text-white-30 max-w-[200px] truncate">
              · next: {next.text}
            </span>
          )}
          {finished && (
            <span className="text-status-active">· done</span>
          )}
          {/* Chevron rotates 180° when popover is open. The
              popover manages its own open state via the shared
              hover/focus trick. */}
          <svg
            width="10"
            height="10"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
            className="text-white-30 ml-0.5"
            aria-hidden="true"
          >
            <polyline points="6 9 12 15 18 9" />
          </svg>
        </button>
      </PlanPopover>
    </div>
  );
}
