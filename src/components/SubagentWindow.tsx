import { useEffect, useRef } from "react";
import { useStore } from "../lib/store";
import type { ToolCallRecord } from "../lib/store";
import type { SubagentStatus } from "../lib/ipc";
import { MessageRenderer } from "./MessageRenderer";
import {
  cancelSubagent,
  respondSubagentApproval,
} from "../lib/subagent-manager";
import {
  Brain,
  Check,
  Circle,
  Loader2,
  ShieldCheck,
  Square,
  Wrench,
  X,
  AlertTriangle,
  Ban,
} from "lucide-react";

interface SubagentWindowProps {
  requestId: string;
}

/** Read-only viewer for one dispatched subagent turn.
 *
 *  The orchestrator chat is conversational; subagent windows are
 *  NOT. The user cannot type into a subagent. Only the Health Agent
 *  (via the `subagent_message` tool) can push messages to a
 *  subagent's inbox, and the loop drains that inbox at the top of
 *  every round. Inbound Health messages surface as read-only
 *  "Health Monitor" bubbles in this viewer.
 */
export function SubagentWindow({ requestId }: SubagentWindowProps) {
  const w = useStore((s) => s.subagentWindows[requestId]);
  const healthMessages = useStore((s) => s.healthInbox[requestId] ?? []);
  const endRef = useRef<HTMLDivElement>(null);

  // Auto-scroll to the bottom as content streams in.
  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [w?.content, w?.reasoning, healthMessages.length, w?.toolCalls.length]);

  if (!w) {
    // The window can briefly exist for a request id whose registry
    // record is gone (e.g. a fast cancel). Render a minimal shell so
    // the tile still has something to display.
    return (
      <div className="flex flex-col h-full bg-surface-1 rounded border border-border-subtle overflow-hidden">
        <div className="px-3 py-1.5 text-[11px] text-white-30 border-b border-border-subtle bg-surface-2/50">
          Subagent — no data
        </div>
      </div>
    );
  }

  const status = w.record.status;
  const isWorking = status === "working";
  const isTerminal =
    status === "done" || status === "error" || status === "cancelled";

  const handleClose = () => {
    // Closing a working subagent cancels it; closing a finished one
    // just removes the window. (Lifecycle: see DESIGN §tiling.)
    if (isWorking) cancelSubagent(requestId);
    useStore.getState().removeSubagent(requestId);
  };

  return (
    <div className="flex flex-col h-full bg-surface-1 rounded border border-border-subtle overflow-hidden">
      {/* Header: name, role, status, stop, close */}
      <div className="flex items-center justify-between px-3 py-1.5 border-b border-border-subtle bg-surface-2/50 shrink-0">
        <div className="flex items-center gap-2 min-w-0">
          <StatusDot status={status} />
          <span className="text-[12px] font-medium text-white truncate">
            {w.record.name}
          </span>
          <span className="text-[10px] text-white-30 truncate">
            {w.record.role}
          </span>
        </div>
        <div className="flex items-center gap-1 shrink-0">
          {isWorking && (
            <button
              onClick={() => cancelSubagent(requestId)}
              className="glass-quartz glass-quartz--pill flex items-center justify-center w-6 h-6 text-white-50 hover:text-white"
              title="Stop subagent"
              aria-label="Stop subagent"
            >
              <Square size={11} fill="currentColor" />
            </button>
          )}
          <button
            onClick={handleClose}
            className="glass-quartz glass-quartz--pill flex items-center justify-center w-6 h-6 text-white-50 hover:text-white"
            title={isWorking ? "Stop and close" : "Close window"}
            aria-label="Close window"
          >
            <X size={11} />
          </button>
        </div>
      </div>

      {/* Task line: the delegation text the orchestrator handed over. */}
      <div className="px-3 py-1.5 text-[11px] text-white-30 border-b border-border-subtle bg-surface-1 shrink-0">
        <span className="text-white-15">Task:</span>{" "}
        <span className="text-white-50">{w.record.task}</span>
      </div>

      {/* Read-only viewer body */}
      <div className="flex-1 overflow-y-auto px-3 py-2 space-y-2 min-h-0">
        {w.toolCalls.length > 0 && (
          <div className="space-y-1">
            {w.toolCalls.map((call) => (
              <ToolCallCard key={call.id} call={call} />
            ))}
          </div>
        )}

        {/* Health Monitor inbox: read-only bubbles (user can't reply). */}
        {healthMessages.map((msg, i) => (
          <HealthBubble key={`hm-${i}-${msg.slice(0, 8)}`} message={msg} />
        ))}

        {w.content.length > 0 && (
          <div className="text-[13px] leading-relaxed text-white animate-fade-in">
            <MessageRenderer
              content={w.content}
              reasoning={w.reasoning}
            />
            {isWorking && (
              <span className="inline-flex items-center gap-1.5 mt-1">
                <Loader2 size={11} className="animate-spin text-white-30" />
                <span className="text-[10px] text-white-30">working…</span>
              </span>
            )}
          </div>
        )}

        {/* Terminal empty-state: finished without producing any visible
         *  content. The registry's `finalContent` (if any) is folded
         *  into the content above, so this only shows when the
         *  subagent ended with no body text at all. */}
        {isTerminal && w.content.length === 0 && (
          <div className="text-[11px] text-white-15 italic">
            {status === "done" && "(no output)"}
            {status === "cancelled" && "(cancelled)"}
            {status === "error" && (w.record.error ?? "(error)")}
          </div>
        )}

        <div ref={endRef} />
      </div>

      {/* Approval footer (if any): Allow / Deny. Routes through
       *  `respondSubagentApproval` so the subagent loop's approval
       *  channel stays separate from the orchestrator's
       *  `pendingApproval`. */}
      {w.approval && (
        <div className="flex items-center gap-2 px-3 py-2 border-t border-border-subtle bg-surface-2 shrink-0 animate-fade-in">
          <ShieldCheck size={13} className="text-status-warning shrink-0" />
          <div className="flex-1 min-w-0">
            <div className="text-[11px] text-white font-medium truncate">
              {w.approval.name}
            </div>
            <div className="text-[10px] text-white-30 truncate">
              {w.approval.summary}
            </div>
          </div>
          <button
            onClick={() => respondSubagentApproval(requestId, true)}
            className="flex items-center gap-1 px-2 py-0.5 rounded bg-status-active text-black text-[10px] font-medium hover:opacity-90 transition-opacity"
            title="Allow this tool call"
          >
            <Check size={10} />
            Allow
          </button>
          <button
            onClick={() => respondSubagentApproval(requestId, false)}
            className="flex items-center gap-1 px-2 py-0.5 rounded bg-surface-4 text-white-50 text-[10px] hover:text-status-error transition-colors"
            title="Deny this tool call"
          >
            <X size={10} />
            Deny
          </button>
        </div>
      )}
    </div>
  );
}

function StatusDot({ status }: { status: SubagentStatus }) {
  if (status === "working") {
    return (
      <Circle
        size={6}
        className="fill-status-active text-status-active animate-pulse shrink-0"
      />
    );
  }
  if (status === "error") {
    return (
      <Circle size={6} className="fill-status-error text-status-error shrink-0" />
    );
  }
  if (status === "cancelled") {
    return (
      <Circle
        size={6}
        className="fill-status-warning text-status-warning shrink-0"
      />
    );
  }
  return <Circle size={6} className="fill-white-15 text-white-15 shrink-0" />;
}

function ToolCallCard({ call }: { call: ToolCallRecord }) {
  const running = call.status === "running";
  return (
    <div className="rounded border border-border-subtle bg-surface-2 px-2 py-1 animate-fade-in">
      <div className="flex items-center gap-1.5 min-w-0">
        {running ? (
          <Loader2 size={10} className="shrink-0 animate-spin text-white-50" />
        ) : call.status === "ran" ? (
          <Check size={10} className="shrink-0 text-status-active" />
        ) : call.status === "denied" ? (
          <Ban size={10} className="shrink-0 text-status-warning" />
        ) : (
          <AlertTriangle size={10} className="shrink-0 text-status-error" />
        )}
        <Wrench size={10} className="shrink-0 text-white-15" />
        <span className="text-[11px] font-medium text-white truncate">
          {call.name}
        </span>
        {call.arguments && (
          <span
            className="text-[10px] text-white-30 font-mono truncate"
            title={call.arguments}
          >
            {call.arguments}
          </span>
        )}
        {running && (
          <span className="text-[9px] text-white-30 shrink-0">running…</span>
        )}
      </div>
      {!running && call.summary && (
        <div
          className="text-[10px] text-white-30 mt-0.5 truncate"
          title={call.summary}
        >
          {call.summary}
        </div>
      )}
    </div>
  );
}

function HealthBubble({ message }: { message: string }) {
  return (
    <div className="rounded border border-border-subtle bg-surface-2 px-2.5 py-1.5 animate-fade-in">
      <div className="flex items-center gap-1.5 text-[10px] text-white-30 mb-0.5 select-none">
        <Brain size={10} className="shrink-0" />
        <span className="font-medium tracking-wide uppercase">
          Health Monitor
        </span>
      </div>
      <div className="text-[12px] leading-relaxed text-white-50 whitespace-pre-wrap">
        {message}
      </div>
    </div>
  );
}
