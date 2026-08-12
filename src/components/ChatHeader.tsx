// Chat header telemetry (spec §7.2): model, last response tokens,
// context used / window with a color-coded bar, and the provider's
// session window (hidden when no rate-limit headers came back).

import { useLayoutEffect, useRef } from "react";
import { useStore } from "../lib/store";
import { ModelSelect } from "./ModelSelect";
import { contextWindowFor } from "../lib/models";

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

  const lastIn = tokenStats?.lastIn ?? null;
  const lastOut = tokenStats?.lastOut ?? null;
  const contextWindow = active ? contextWindowFor(active.modelName) : 0;
  const contextUsed = lastIn ?? 0;
  const contextFrac = contextWindow > 0 ? Math.min(1, contextUsed / contextWindow) : 0;

  const sw = tokenStats?.sessionWindowUsed ?? null;
  const swReset = tokenStats?.sessionWindowResetsAt ?? null;

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
    <div ref={headerRef} className="px-4 py-2.5 select-none">
      <div className="max-w-3xl mx-auto space-y-1 text-[12px]">
        {/* Row 1: model */}
        <div className="flex items-center justify-between">
          <span className="text-white-30">Model</span>
          <ModelSelect compact />
        </div>

        {/* Row 2: last response */}
        <div className="flex items-center justify-between">
          <span className="text-white-30">Last response</span>
          <span className="text-white-50">
            {lastIn === null
              ? "—"
              : `${fmt(lastIn)} in · ${fmt(lastOut ?? 0)} out`}
          </span>
        </div>

        {/* Row 3: context used / window + bar */}
        <div className="flex items-center justify-between">
          <span className="text-white-30">Context used</span>
          <span className="text-white-50">
            {lastIn === null ? (
              "—"
            ) : (
              <>
                {fmt(contextUsed)} / {fmt(contextWindow)}
              </>
            )}
          </span>
        </div>
        {lastIn !== null && (
          <div className="h-px w-full bg-white-15">
            <div
              className={`h-px ${barColor(contextFrac)}`}
              style={{ width: `${Math.round(contextFrac * 100)}%` }}
            />
          </div>
        )}

        {/* Row 4: session window — hidden unless the provider sent headers */}
        {sw !== null && (
          <div className="flex items-center justify-between">
            <span className="text-white-30">Session window</span>
            <span className="text-white-50">
              {Math.round(sw * 100)}% used
              {swReset ? ` · resets in ${resetsIn(swReset)}` : ""}
            </span>
          </div>
        )}
      </div>
    </div>
  );
}
