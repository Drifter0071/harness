import { describe, expect, it } from "vitest";
import {
  DEFAULT_CHAT_RATIO,
  MIN_CHAT_WIDTH,
  chatSplit,
  computeTiles,
  gridDims,
} from "./wm";

describe("gridDims (i3-style equal grid)", () => {
  it("returns an empty grid for zero or negative counts", () => {
    expect(gridDims(0)).toEqual({ cols: 0, rows: 0 });
    expect(gridDims(-3)).toEqual({ cols: 0, rows: 0 });
    expect(gridDims(Number.NaN)).toEqual({ cols: 0, rows: 0 });
  });

  it("squares: cols = rows = sqrt(n)", () => {
    expect(gridDims(1)).toEqual({ cols: 1, rows: 1 });
    expect(gridDims(4)).toEqual({ cols: 2, rows: 2 });
    expect(gridDims(9)).toEqual({ cols: 3, rows: 3 });
    expect(gridDims(16)).toEqual({ cols: 4, rows: 4 });
  });

  it("non-squares: cols = ceil(sqrt(n)), rows = ceil(n / cols)", () => {
    expect(gridDims(2)).toEqual({ cols: 2, rows: 1 });
    expect(gridDims(3)).toEqual({ cols: 2, rows: 2 });
    expect(gridDims(5)).toEqual({ cols: 3, rows: 2 });
    expect(gridDims(6)).toEqual({ cols: 3, rows: 2 });
    expect(gridDims(7)).toEqual({ cols: 3, rows: 3 });
    expect(gridDims(8)).toEqual({ cols: 3, rows: 3 });
    expect(gridDims(10)).toEqual({ cols: 4, rows: 3 });
  });
});

describe("computeTiles", () => {
  it("returns no tiles when there is nothing to lay out", () => {
    expect(computeTiles(0, 800, 600)).toEqual([]);
    expect(computeTiles(3, 0, 600)).toEqual([]);
    expect(computeTiles(3, 800, -1)).toEqual([]);
  });

  it("single window fills the whole region", () => {
    expect(computeTiles(1, 800, 600, 4)).toEqual([
      { x: 0, y: 0, width: 800, height: 600 },
    ]);
  });

  it("two windows split the width, same size, no overlap", () => {
    expect(computeTiles(2, 800, 600, 4)).toEqual([
      { x: 0, y: 0, width: 398, height: 600 },
      { x: 402, y: 0, width: 398, height: 600 },
    ]);
  });

  it("four windows split into a 2x2 grid covering the region", () => {
    expect(computeTiles(4, 800, 600, 4)).toEqual([
      { x: 0, y: 0, width: 398, height: 298 },
      { x: 402, y: 0, width: 398, height: 298 },
      { x: 0, y: 302, width: 398, height: 298 },
      { x: 402, y: 302, width: 398, height: 298 },
    ]);
  });

  it("three windows keep equal sizes; the 2x2 grid's last cell stays empty", () => {
    const tiles = computeTiles(3, 800, 600, 4);
    expect(tiles).toHaveLength(3);
    // Every tile identical in size (i3-style).
    for (const t of tiles) {
      expect(t.width).toBe(398);
      expect(t.height).toBe(298);
    }
    expect(tiles[2]).toEqual({ x: 0, y: 302, width: 398, height: 298 });
  });

  it("zero-gap tiles tile the region exactly (no rounding drift)", () => {
    const tiles = computeTiles(6, 900, 450, 0);
    expect(tiles).toHaveLength(6);
    const { cols, rows } = gridDims(6);
    for (const t of tiles) {
      expect(t.x + t.width).toBeLessThanOrEqual(900);
      expect(t.y + t.height).toBeLessThanOrEqual(450);
    }
    const xs = new Set(tiles.map((t) => t.x));
    const ys = new Set(tiles.map((t) => t.y));
    expect(xs.size).toBe(cols);
    expect(ys.size).toBe(rows);
    // Full coverage of the 3x2 region.
    expect(Math.max(...tiles.map((t) => t.x + t.width))).toBe(900);
    expect(Math.max(...tiles.map((t) => t.y + t.height))).toBe(450);
  });
});

describe("chatSplit (chat never resizes once tiled)", () => {
  it("defaults to a 50/50 split", () => {
    expect(chatSplit(1200)).toEqual({ chatWidth: 600, subagentWidth: 600 });
    expect(DEFAULT_CHAT_RATIO).toBe(0.5);
  });

  it("honors a custom locked ratio", () => {
    expect(chatSplit(1000, 0.35)).toEqual({ chatWidth: 350, subagentWidth: 650 });
    expect(chatSplit(1000, 0.7)).toEqual({ chatWidth: 700, subagentWidth: 300 });
  });

  it("clamps a tiny ratio up to the readable chat minimum", () => {
    // 1000 * 0.3 = 300 < 320 → clamped up to the minimum.
    expect(chatSplit(1000, 0.3)).toEqual({ chatWidth: 320, subagentWidth: 680 });
  });

  it("clamps the chat column to a readable minimum", () => {
    // 800 * 0.5 = 400 > 320 → no clamp.
    expect(chatSplit(800, 0.5, 320).chatWidth).toBe(400);
    // 500 * 0.5 = 250 < 320 → clamped to 320, subagent region keeps the rest.
    expect(chatSplit(500, 0.5, 320)).toEqual({ chatWidth: 320, subagentWidth: 180 });
    expect(MIN_CHAT_WIDTH).toBe(320);
  });

  it("never lets the subagent region go negative", () => {
    expect(chatSplit(300, 0.5, 320)).toEqual({ chatWidth: 300, subagentWidth: 0 });
  });

  it("returns a zero split for a zero-width container", () => {
    expect(chatSplit(0)).toEqual({ chatWidth: 0, subagentWidth: 0 });
    expect(chatSplit(-100)).toEqual({ chatWidth: 0, subagentWidth: 0 });
  });
});
