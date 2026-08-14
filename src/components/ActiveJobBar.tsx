// Bip's thoughts popover.
//
// The mascot itself now lives in the TitleBar (window chrome), so the
// popover no longer renders any pill row — that responsibility moved
// out entirely. This file just hosts the `BipThoughtsPopover` export
// that the TitleBar uses when the user hovers the Bip.
//
// Kept as a separate component (rather than inlined into TitleBar) so
// the popover's data hooks (job activities, agent roster, token stats)
// stay isolated and don't re-render the whole topbar on every store
// change.

import { createPortal } from "react-dom";
import { useStore } from "../lib/store";
import { Circle } from "lucide-react";
import { AgentMascot, type MascotMood } from "./AgentMascot";

const fmt = (n: number) => n.toLocaleString("en-US");

/** Mount the popover nothing — kept for API compatibility with the
 *  old `App.tsx` layout. The actual popover is `BipThoughtsPopover`,
 *  which the TitleBar renders directly. */
export function ActiveJobBar() {
  return null;
}

export function BipThoughtsPopover({
  open,
  pos,
  mood,
  onMouseEnter,
  onMouseLeave,
}: {
  open: boolean;
  pos: { top: number; left: number } | null;
  mood: MascotMood;
  onMouseEnter?: () => void;
  onMouseLeave?: () => void;
}) {
  const jobActivities = useStore((s) => s.jobActivities);
  const activeAgents = useStore((s) => s.activeAgents);
  const tokenStats = useStore((s) => s.tokenStats);
  const savedModels = useStore((s) => s.savedModels);
  const activeModelId = useStore((s) => s.activeModelId);
  const streamingRequestId = useStore((s) => s.streamingRequestId);
  const agentRequestId = useStore((s) => s.agentRequestId);

  const hasActivity = jobActivities.length > 0;
  const working = hasActivity || !!streamingRequestId || !!agentRequestId;

  const thought = working
    ? agentRequestId && streamingRequestId
      ? "Two things at once — an agent turn and a stream. Juggling."
      : agentRequestId
        ? "Agent turn in flight — tools are moving."
        : streamingRequestId
          ? "Streaming a reply — words incoming."
          : hasActivity
            ? `On it — ${jobActivities.length} task${jobActivities.length === 1 ? "" : "s"} in flight.`
            : "Busy, actually."
    : "Nothing on my plate. Dozing by the capsule.";

  const activeModel = savedModels.find((m) => m.id === activeModelId) ?? null;
  const lastIn = tokenStats?.lastIn ?? null;
  const lastOut = tokenStats?.lastOut ?? null;
  const cumulativeIn = tokenStats?.cumulativeIn ?? null;
  const cumulativeOut = tokenStats?.cumulativeOut ?? null;
  const sw = tokenStats?.sessionWindowUsed ?? null;

  if (!open || !pos) return null;
  return createPortal(
    <div
      className="fixed z-[90] glass-mist p-3 min-w-[300px] max-h-[60vh] overflow-y-auto animate-fade-in"
      style={{
        top: pos.top,
        left: pos.left,
        transform: "translateX(-50%)",
      }}
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
    >
      <div className="flex items-start gap-2">
        <div className="relative mt-0.5 flex-shrink-0">
          <AgentMascot mood={mood} size={20} />
        </div>
        <div className="min-w-0">
          <div className="text-[10px] text-white-30 uppercase tracking-wider">
            Bip's thoughts
          </div>
          <div className="text-[12px] text-white-50 leading-snug">{thought}</div>
        </div>
      </div>

      {hasActivity && (
        <div className="border-t border-border-subtle mt-2.5 pt-2.5">
          <div className="text-[11px] text-white-30 uppercase tracking-wider mb-1.5">
            Active Tasks
          </div>
          <div className="space-y-1.5">
            {jobActivities.map((activity, i) => (
              <div
                key={i}
                className="flex items-center gap-2 text-[13px] text-white-50"
              >
                <Circle size={4} className="fill-accent-green text-accent-green flex-shrink-0" />
                {activity}
              </div>
            ))}
          </div>
        </div>
      )}

      {activeAgents.length > 0 && (
        <div className="border-t border-border-subtle mt-2.5 pt-2.5">
          <div className="text-[11px] text-white-30 uppercase tracking-wider mb-1.5">
            Agents
          </div>
          <div className="space-y-1.5">
            {activeAgents.map((agent) => (
              <div
                key={agent.id}
                className="flex items-center gap-2 text-[13px] text-white-30"
              >
                <Circle
                  size={4}
                  className={`flex-shrink-0 ${
                    agent.status === "Working"
                      ? "fill-accent-green text-accent-green"
                      : "fill-white-15 text-white-15"
                  }`}
                />
                <span className="truncate">{agent.name}</span>
                <span className="text-[11px] text-white-15 ml-auto">
                  {agent.role}
                </span>
              </div>
            ))}
          </div>
        </div>
      )}

      <div className="border-t border-border-subtle mt-2.5 pt-2.5">
        <div className="text-[11px] text-white-30 uppercase tracking-wider mb-1.5">
          Turn
        </div>
        <div className="flex items-center gap-2 text-[12px] text-white-50">
          <Circle
            size={4}
            className={
              working
                ? "fill-accent-mint text-accent-mint animate-pulse"
                : "fill-white-15 text-white-15"
            }
          />
          {working
            ? agentRequestId
              ? "Agent turn running"
              : "Streaming response"
            : "Idle"}
        </div>
      </div>

      {(activeModel || tokenStats) && (
        <div className="border-t border-border-subtle mt-2.5 pt-2.5">
          <div className="text-[11px] text-white-30 uppercase tracking-wider mb-1.5">
            Session
          </div>
          <div className="space-y-1 text-[12px]">
            <div className="flex items-baseline justify-between gap-4">
              <span className="text-white-30 flex-shrink-0">Model</span>
              <span className="text-white-50 truncate">
                {activeModel ? activeModel.label : "—"}
              </span>
            </div>
            {lastIn !== null && (
              <div className="flex items-baseline justify-between gap-4">
                <span className="text-white-30 flex-shrink-0">Last response</span>
                <span className="text-white-50">
                  {fmt(lastIn)} in · {fmt(lastOut ?? 0)} out
                </span>
              </div>
            )}
            {cumulativeIn !== null && (
              <div className="flex items-baseline justify-between gap-4">
                <span className="text-white-30 flex-shrink-0">This session</span>
                <span className="text-white-50">
                  {fmt(cumulativeIn)} peak in · {fmt(cumulativeOut ?? 0)} out
                </span>
              </div>
            )}
            {sw !== null && (
              <div className="flex items-baseline justify-between gap-4">
                <span className="text-white-30 flex-shrink-0">Session window</span>
                <span className="text-white-50">{Math.round(sw * 100)}% used</span>
              </div>
            )}
          </div>
        </div>
      )}
    </div>,
    document.body
  );
}
