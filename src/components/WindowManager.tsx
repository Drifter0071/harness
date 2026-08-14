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
 */
export function WindowManager({ width, height }: WindowManagerProps) {
  const subagents = useStore((s) => s.subagents);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [size, setSize] = useState({ width, height });

  // Track the parent-supplied size; fall back to a measured
  // container size if the parent props ever pass through 0
  // (e.g. during the first render before the chat column has its
  // measured width).
  useEffect(() => {
    if (width > 0 && height > 0) {
      setSize({ width, height });
      return;
    }
    if (!containerRef.current) return;
    setSize({
      width: containerRef.current.clientWidth,
      height: containerRef.current.clientHeight,
    });
  }, [width, height]);

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
