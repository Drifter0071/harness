// Multi-row approval card for parallel subagent dispatches (spec
// `2026-08-14--parallel-subagent-dispatch`).
//
// When the orchestrator's tool loop dispatches N subagents in one LLM
// round, each subagent can independently hit an approval gate (write
// / edit / run_command). Instead of popping a separate card inside
// each subagent window, this component aggregates every currently
// pending subagent approval into a single stacked card surfaced
// above the subagent region. Allow / Deny on a single row resolves
// just that subagent's `oneshot` sender.
//
// The single-pending-approval case still uses the existing
// per-window card in `SubagentWindow` — this card only shows when
// ≥2 subagents in the same batch are waiting, so the user isn't
// peppered with popups when the model only dispatched one subagent.

import { ShieldCheck, Check, X } from "lucide-react";
import { useStore } from "../lib/store";
import { respondSubagentApproval } from "../lib/subagent-manager";

/** All currently pending subagent approvals, regardless of how many.
 *  Returns an array of (requestId, subagentName, toolName, summary)
 *  for the renderer. */
function selectPendingBatchApprovals(
  s: ReturnType<typeof useStore.getState>,
): { requestId: string; name: string; approval: NonNullable<typeof s.subagentWindows[string]["approval"]> }[] {
  const out: { requestId: string; name: string; approval: NonNullable<typeof s.subagentWindows[string]["approval"]> }[] = [];
  for (const rec of s.subagents) {
    if (rec.status !== "working") continue;
    const w = s.subagentWindows[rec.requestId];
    if (!w?.approval) continue;
    out.push({
      requestId: rec.requestId,
      name: rec.name,
      approval: w.approval,
    });
  }
  return out;
}

export function BatchApprovalCard(): React.ReactElement | null {
  const pending = useStore(selectPendingBatchApprovals);
  // Only show the batch card when ≥2 subagents in the current batch
  // are waiting on approval. With 0 or 1 the per-window card in
  // `SubagentWindow` handles it (and the visual focus is more
  // natural there).
  if (pending.length < 2) return null;

  return (
    <div
      className="border-b border-border-subtle bg-surface-1 shrink-0 animate-fade-in"
      data-testid="batch-approval-card"
    >
      <div className="flex items-center gap-2 px-3 py-2 border-b border-border-subtle bg-surface-2/60">
        <ShieldCheck size={13} className="text-status-warning shrink-0" />
        <span className="text-[12px] font-medium text-white">
          {pending.length} tools waiting
        </span>
        <span className="text-[10px] text-white-15 ml-auto">
          Allow or deny each one
        </span>
      </div>
      <div className="divide-y divide-border-subtle max-h-[40vh] overflow-y-auto">
        {pending.map((row) => (
          <div
            key={row.requestId}
            className="flex items-center gap-2 px-3 py-2 bg-surface-1"
          >
            <div className="flex-1 min-w-0">
              <div className="text-[11px] text-white font-medium truncate">
                {row.name} · {row.approval.name}
              </div>
              <div className="text-[10px] text-white-30 truncate">
                {row.approval.summary}
              </div>
            </div>
            <button
              onClick={() => respondSubagentApproval(row.requestId, true)}
              className="flex items-center gap-1 px-2 py-0.5 rounded bg-status-active text-black text-[10px] font-medium hover:opacity-90 transition-opacity shrink-0"
              title="Allow this tool call"
              data-testid="batch-approval-allow"
              data-request-id={row.requestId}
            >
              <Check size={10} />
              Allow
            </button>
            <button
              onClick={() => respondSubagentApproval(row.requestId, false)}
              className="flex items-center gap-1 px-2 py-0.5 rounded bg-surface-4 text-white-50 text-[10px] hover:text-status-error transition-colors shrink-0"
              title="Deny this tool call"
              data-testid="batch-approval-deny"
              data-request-id={row.requestId}
            >
              <X size={10} />
              Deny
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}
