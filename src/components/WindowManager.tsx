import { useLayoutEffect, useRef, useState } from "react";
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
 *  Size measurement: a `ResizeObserver` on the container div, paired
 *  with a synchronous `getBoundingClientRect` read in a layout effect
 *  so the first paint already uses the correct tile size (no flash
 *  of zero-sized tiles while the observer fires asynchronously).
 */
export function WindowManager({ width, height }: WindowManagerProps) {
  const subagents = useStore((s) => s.subagents);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [size, setSize] = useState({ width, height });

  // useLayoutEffect so the very first paint uses the real container
  // size; the observer picks up subsequent resizes (sidebar open/close,
  // chat column lock-in, window resize). The ref is stable across the
  // empty/populated branches, so the observer stays attached.
  useLayoutEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const measure = () => {
      const rect = el.getBoundingClientRect();
      if (rect.width > 0 && rect.height > 0) {
        setSize({ width: Math.round(rect.width), height: Math.round(rect.height) });
      }
    };
    measure();

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
  }, [subagents.length]);

  if (subagents.length === 0) {
    // Defensive: the parent only mounts this component while tiling
    // is active, but render an empty container if it ever does fire
    // so the layout doesn't collapse.
    return <div ref={containerRef} className="h-full flex-1 min-w-0" />;
  }

  const tiles = computeTiles(subagents.length, size.width, size.height);

  return (
    <div
      ref={containerRef}
      className="relative h-full overflow-hidden flex-1 min-w-0"
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
