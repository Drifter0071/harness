import { useEffect, useRef, useState } from "react";
import { useStore } from "../lib/store";
import { SubagentWindow } from "./SubagentWindow";
import { computeTiles } from "../lib/wm";

interface WindowManagerProps {
  /** Width of the subagent region in px (the chat column is the
   *  remainder, owned by the parent). */
  width: number;
  /** Height of the subagent region in px. */
  height: number;
}

/** Equal-grid tiling of the subagent region.
 *
 *  All subagent windows are always the same size regardless of
 *  count (i3-style): `cols = ceil(sqrt(n))`, `rows = ceil(n/cols)`.
 *  The chat column is rendered by the parent App — this component
 *  only owns the subagent region.
 *
 *  Size measurement: uses a ResizeObserver on the container div so
 *  that re-renders caused by Zustand state changes (subagents added)
 *  are always paired with an up-to-date container measurement — the
 *  effect re-runs on EVERY observed size change, not just prop
 *  changes.
 */
export function WindowManager({ width, height }: WindowManagerProps) {
  const subagents = useStore((s) => s.subagents);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [size, setSize] = useState({ width, height });

  // Observe the container's own size. Re-runs on every observed
  // resize, so it always has up-to-date dimensions regardless of
  // whether the re-render was triggered by a prop change or a
  // Zustand state update (e.g. subagents added).
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    // Measure synchronously first — the element may already have
    // its layout size from the previous render.
    const syncWidth = el.clientWidth;
    const syncHeight = el.clientHeight;
    if (syncWidth > 0 && syncHeight > 0) {
      setSize({ width: syncWidth, height: syncHeight });
    }

    const ro = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (!entry) return;
      const { inlineSize: w, blockSize: h } = entry.borderBoxSize?.[0] ??
        { inlineSize: entry.contentRect.width, blockSize: entry.contentRect.height };
      if (w > 0 && h > 0) {
        setSize({ width: Math.round(w), height: Math.round(h) });
      }
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []); // intentionally empty — self-contained, no prop deps

  if (subagents.length === 0) {
    return <div ref={containerRef} className="h-full" />;
  }

  const tiles = computeTiles(subagents.length, size.width, size.height);

  return (
    <div
      ref={containerRef}
      className="relative h-full overflow-hidden"
      data-testid="window-manager"
    >
      {tiles.map((tile, i) => {
        const rec = subagents[i];
        if (!rec) return null;
        return (
          <div
            key={rec.requestId}
            className="absolute p-0.5"
            style={{
              left: tile.x,
              top: tile.y,
              width: tile.width,
              height: tile.height,
            }}
          >
            <SubagentWindow requestId={rec.requestId} />
          </div>
        );
      })}
    </div>
  );
}
