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
 *  Visual style mirrors the main orchestrator chat: the task the
 *  orchestrator handed over sits as a right-aligned user bubble, the
 *  subagent's response streams below as a left-aligned assistant
 *  message (with the agent name + tool calls inline, just like the
 *  main `MessageBubble`).
 *
 *  The user cannot type into a subagent. Only the Health Agent
 *  (via the `subagent_message` tool) can push messages into a
 *  subagent's inbox; those surface as read-only "Health Monitor"
 *  bubbles in the conversation.
 *
 *  No close button — subagent windows live until the next
 *  orchestrator turn starts (the registry is wiped in unison). A
 *  Stop button shows only while the subagent is working.
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

  return (
    <div className="flex flex-col h-full bg-surface-0 overflow-hidden">
      {/* Header: agent name + role + status. Stop button only while
       *  the turn is in flight. No close button — windows are
       *  closed in unison by the next orchestrator turn-start. */}
      <div className="flex items-center justify-between px-3 py-1.5 border-b border-border-subtle bg-surface-1 shrink-0">
        <div className="flex items-center gap-2 min-w-0">
          <StatusDot status={status} />
          <span className="text-[12px] font-medium text-white truncate">
            {w.record.name}
          </span>
          <span className="text-[10px] text-white-30 truncate">
            {w.record.role}
          </span>
        </div>
        {isWorking && (
          <button
            onClick={() => cancelSubagent(requestId)}
            className="glass-quartz glass-quartz--pill flex items-center justify-center w-6 h-6 text-white-50 hover:text-white shrink-0"
            title="Stop subagent"
            aria-label="Stop subagent"
          >
            <Square size={11} fill="currentColor" />
          </button>
        )}
      </div>

      {/* Read-only chat body. Mirrors the main MessageBubble layout:
       *   - Task: right-aligned user bubble (the delegation text).
       *   - Tool calls: stacked cards above the assistant text.
       *   - Health Monitor: right-aligned system-style bubble.
       *   - Subagent response: left-aligned assistant text with
       *     agent name above and the streaming "typing…" indicator
       *     while working. */}
      <div className="flex-1 overflow-y-auto px-3 py-3 space-y-3 min-h-0">
        {/* Task bubble — the orchestrator's delegation, right-aligned. */}
        <div className="flex justify-end animate-fade-in">
          <div className="max-w-[85%]">
            <div className="text-[10px] font-medium text-white-30 mb-1 text-right">
              Orchestrator
            </div>
            <div className="text-[13px] leading-relaxed bg-surface-3 text-white-50 rounded-2xl rounded-br-sm px-3.5 py-2 whitespace-pre-wrap">
              {w.record.task}
            </div>
          </div>
        </div>

        {/* Health Monitor bubbles — read-only, right-aligned. */}
        {healthMessages.map((msg, i) => (
          <div key={`hm-${i}-${msg.slice(0, 8)}`} className="flex justify-end animate-fade-in">
            <div className="max-w-[85%]">
              <div className="flex items-center justify-end gap-1.5 text-[10px] text-white-30 mb-1 select-none">
                <span className="font-medium tracking-wide uppercase">Health Monitor</span>
                <Brain size={10} className="shrink-0" />
              </div>
              <div className="text-[13px] leading-relaxed bg-surface-3 text-white-50 rounded-2xl rounded-br-sm px-3.5 py-2 whitespace-pre-wrap">
                {msg}
              </div>
            </div>
          </div>
        ))}

        {/* Subagent response — left-aligned assistant bubble. Tool
         *  calls sit above the text, exactly like the main chat. */}
        <div className="flex justify-start animate-fade-in">
          <div className="max-w-[85%]">
            <div className="text-[11px] font-medium text-white-30 mb-1 flex items-center gap-1.5">
              <span className="inline-block w-1.5 h-1.5 rounded-full bg-accent-coral" />
              {w.record.name}
            </div>
            <div className="text-[13px] leading-relaxed text-white">
              {w.toolCalls.length > 0 && (
                <div className="space-y-1.5 mb-2">
                  {w.toolCalls.map((call) => (
                    <ToolCallCard key={call.id} call={call} />
                  ))}
                </div>
              )}
              {w.content.length > 0 ? (
                <>
                  <MessageRenderer
                    content={w.content}
                    reasoning={w.reasoning}
                  />
                  {isWorking && (
                    <span className="inline-flex items-center gap-1.5 mt-1">
                      <span className="inline-block w-1.5 h-1.5 rounded-full bg-white-30 animate-bounce" style={{ animationDelay: "0ms" }} />
                      <span className="inline-block w-1.5 h-1.5 rounded-full bg-white-30 animate-bounce" style={{ animationDelay: "160ms" }} />
                      <span className="inline-block w-1.5 h-1.5 rounded-full bg-white-30 animate-bounce" style={{ animationDelay: "320ms" }} />
                      <span className="text-[10px] text-white-30">thinking…</span>
                    </span>
                  )}
                </>
              ) : isWorking ? (
                <span className="inline-flex items-center gap-1.5 mt-1">
                  <span className="inline-block w-1.5 h-1.5 rounded-full bg-white-30 animate-bounce" style={{ animationDelay: "0ms" }} />
                  <span className="inline-block w-1.5 h-1.5 rounded-full bg-white-30 animate-bounce" style={{ animationDelay: "160ms" }} />
                  <span className="inline-block w-1.5 h-1.5 rounded-full bg-white-30 animate-bounce" style={{ animationDelay: "320ms" }} />
                  <span className="text-[10px] text-white-30">thinking…</span>
                </span>
              ) : (
                <span className="text-white-30 italic text-[12px]">
                  {status === "done" && "(no output)"}
                  {status === "cancelled" && "(cancelled)"}
                  {status === "error" && (w.record.error ?? "(error)")}
                </span>
              )}
            </div>
          </div>
        </div>

        <div ref={endRef} />
      </div>

      {/* Approval footer (if any): Allow / Deny. Routes through
       *  `respondSubagentApproval` so the subagent loop's approval
       *  channel stays separate from the orchestrator's
       *  `pendingApproval`. */}
      {w.approval && (
        <div className="flex items-center gap-2 px-3 py-2 border-t border-border-subtle bg-surface-1 shrink-0 animate-fade-in">
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
    return <Circle size={6} className="fill-status-error text-status-error shrink-0" />;
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
    <div className="rounded border border-border-subtle bg-surface-2 px-2.5 py-1.5 animate-fade-in">
      <div className="flex items-center gap-1.5 min-w-0">
        {running ? (
          <Loader2 size={11} className="shrink-0 animate-spin text-white-50" />
        ) : call.status === "ran" ? (
          <Check size={11} className="shrink-0 text-status-active" />
        ) : call.status === "denied" ? (
          <Ban size={11} className="shrink-0 text-status-warning" />
        ) : (
          <AlertTriangle size={11} className="shrink-0 text-status-error" />
        )}
        <Wrench size={11} className="shrink-0 text-white-15" />
        <span className="text-[12px] font-medium text-white truncate">
          {call.name}
        </span>
        {call.arguments && (
          <span
            className="text-[11px] text-white-30 font-mono truncate"
            title={call.arguments}
          >
            {call.arguments}
          </span>
        )}
        {running && <span className="text-[10px] text-white-30 shrink-0">running…</span>}
      </div>
      {!running && call.summary && (
        <div
          className="text-[11px] text-white-30 mt-0.5 truncate"
          title={call.summary}
        >
          {call.summary}
        </div>
      )}
    </div>
  );
}
