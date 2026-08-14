// Chat header telemetry (spec §7.2): model, last response tokens,
// context used / window with a color-coded bar, and the provider's
// session window (hidden when no rate-limit headers came back).

import { useLayoutEffect, useRef } from "react";
import { useStore } from "../lib/store";
import { capabilitiesFor } from "../lib/model-capabilities";

const fmt = (n: number) => n.toLocaleString("en-US");

/** "4h 12m" / "45m" / "2d" style countdown from an ISO timestamp. */
export function resetsIn(resetsAt: string): string {
  const target = new Date(resetsAt).getTime();
  if (Number.isNaN(target)) return "";
  const secs = Math.max(0, Math.floor((target - Date.now()) / 1000));
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m`;
  return secs > 0 ? `${secs}s` : "now";
}

function barColor(frac: number): string {
  if (frac < 0.6) return "bg-status-active";
  if (frac <= 0.85) return "bg-status-warning";
  return "bg-status-error";
}

export function ChatHeader() {
  const { savedModels, activeModelId, tokenStats } = useStore();
  const active = savedModels.find((m) => m.id === activeModelId) ?? null;
  const caps = active ? capabilitiesFor(active) : null;

  const lastIn = tokenStats?.lastIn ?? null;
  const lastOut = tokenStats?.lastOut ?? null;
  const contextWindow = caps?.contextWindow ?? 0;
  const contextUsed = lastIn ?? 0;
  const contextFrac = contextWindow > 0 ? Math.min(1, contextUsed / contextWindow) : 0;

  const sw = tokenStats?.sessionWindowUsed ?? null;
  const swReset = tokenStats?.sessionWindowResetsAt ?? null;
  const cumulativeIn = tokenStats?.cumulativeIn ?? null;
  const cumulativeOut = tokenStats?.cumulativeOut ?? null;

  const headerRef = useRef<HTMLDivElement>(null);

  // Publish the header's height so the sidebar + toggle can anchor just
  // below it. The height varies — the session-window row only appears when
  // the provider sends rate-limit headers — so we measure, don't guess.
  useLayoutEffect(() => {
    const el = headerRef.current;
    if (!el) return;
    const publish = () => useStore.getState().setChatHeaderHeight(el.offsetHeight);
    publish();
    const ro = new ResizeObserver(publish);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  return (
    // The stat line sits directly under the topbar. White-space: nowrap
    // keeps the three label groups on a single line. Centered on its
    // own row (the parent flexbox is `justify-center`).
    <div
      ref={headerRef}
      className="py-2 select-none text-[11px] whitespace-nowrap"
    >
      {/* Row 1: model · last response · context used — one horizontal
          line with hairline dividers, centered under Bip. */}
      <div className="flex items-center justify-center gap-3">
        <span className="flex items-baseline gap-1.5 min-w-0">
          {/* lilac tag — the model identity dot. 2px, no background. */}
          <span className="inline-block w-2 h-2 rounded-full bg-accent-lilac flex-shrink-0 self-center shadow-[0_0_4px_rgba(178,141,255,0.6)]" />
          <span className="text-white-30 flex-shrink-0">Model</span>
          <span className="text-white-50 truncate" title={active?.label}>
            {active ? active.label : "—"}
          </span>
        </span>

        <span className="h-3 w-px bg-border-subtle flex-shrink-0" />

        <span className="flex items-baseline gap-1.5 min-w-0">
          {/* sun tag — last turn's token pulse */}
          <span className="inline-block w-2 h-2 rounded-full bg-accent-sun flex-shrink-0 self-center shadow-[0_0_4px_rgba(255,201,77,0.6)]" />
          <span className="text-white-30 flex-shrink-0">Last response</span>
          <span className="text-white-50">
            {lastIn === null
              ? "—"
              : `${fmt(lastIn)} in · ${fmt(lastOut ?? 0)} out`}
          </span>
        </span>

        <span className="h-3 w-px bg-border-subtle flex-shrink-0" />

        <span className="flex items-baseline gap-1.5 min-w-0">
          {/* sky tag — context-window level */}
          <span className="inline-block w-2 h-2 rounded-full bg-accent-sky flex-shrink-0 self-center shadow-[0_0_4px_rgba(91,201,255,0.6)]" />
          <span className="text-white-30 flex-shrink-0">Context used</span>
          <span className="text-white-50">
            {lastIn === null ? (
              "—"
            ) : (
              <>
                {fmt(contextUsed)} / {fmt(contextWindow)}
              </>
            )}
          </span>
        </span>
      </div>

      {/* Hairline progress bar under the row, centered */}
      {lastIn !== null && (
        <div className="h-px bg-white-15 mt-1.5 max-w-[320px] mx-auto">
          <div
            className={`h-px ${barColor(contextFrac)}`}
            style={{ width: `${Math.round(contextFrac * 100)}%` }}
          />
        </div>
      )}

      {/* Row 2: cumulative session totals + provider session window —
          only shown when the provider reports them */}
      {(cumulativeIn !== null || sw !== null) && (
        <div className="flex items-center justify-center gap-3 text-[10px] mt-1">
          {cumulativeIn !== null && (
            <span className="flex items-baseline gap-1.5 min-w-0">
              <span className="text-white-30 flex-shrink-0">This session</span>
              <span className="text-white-50">
                {fmt(cumulativeIn)} peak in · {fmt(cumulativeOut ?? 0)} out
              </span>
            </span>
          )}
          {sw !== null && (
            <span className="flex items-baseline gap-1.5 min-w-0">
              <span className="text-white-30 flex-shrink-0">Session window</span>
              <span className="text-white-50">
                {Math.round(sw * 100)}% used
                {swReset ? ` · resets in ${resetsIn(swReset)}` : ""}
              </span>
            </span>
          )}
        </div>
      )}
    </div>
  );
}
