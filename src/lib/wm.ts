// ── Tiling window-manager engine (pure, UI-agnostic) ──
//
// Tiling model (user-specified):
//   - The main chat window NEVER changes size once subagents appear — it
//     just shifts left. The width is fixed at `chatRatio` (default 0.5)
//     the moment the first subagent spawns and never auto-resizes after.
//   - The right region opens for subagent windows.
//   - ALL subagent windows are always the same size regardless of count
//     (i3-style equal grid): `cols = ceil(sqrt(n))`, `rows = ceil(n/cols)`.
//
// Everything here is a pure function over numbers so the layout is unit-
// testable and the components stay dumb renderers.

/** Width fraction of the main chat column once subagents are tiled.
 *  Locked in the store when the first subagent window opens; never
 *  auto-resized after that. */
export const DEFAULT_CHAT_RATIO = 0.5;

/** Smallest chat column we tolerate before the right region eats into
 *  it — the orchestrator conversation must stay readable. */
export const MIN_CHAT_WIDTH = 320;

export interface GridDims {
  cols: number;
  rows: number;
}

/** Equal-grid dimensions for `count` windows (i3-style):
 *  `cols = ceil(sqrt(n))`, `rows = ceil(n / cols)`. */
export function gridDims(count: number): GridDims {
  if (!Number.isFinite(count) || count <= 0) return { cols: 0, rows: 0 };
  const cols = Math.ceil(Math.sqrt(count));
  const rows = Math.ceil(count / cols);
  return { cols, rows };
}

export interface Tile {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Absolute tile rects for the subagent region, top-left origin.
 *  Every tile is the SAME size (the grid is uniform); a partially
 *  filled last row simply leaves empty cells. `gap` in px. */
export function computeTiles(
  count: number,
  width: number,
  height: number,
  gap = 4,
): Tile[] {
  const { cols, rows } = gridDims(count);
  if (cols === 0 || rows === 0 || width <= 0 || height <= 0) return [];
  const cellW = (width - gap * (cols - 1)) / cols;
  const cellH = (height - gap * (rows - 1)) / rows;
  const tiles: Tile[] = [];
  for (let i = 0; i < count; i++) {
    const col = i % cols;
    const row = Math.floor(i / cols);
    tiles.push({
      x: Math.round(col * (cellW + gap)),
      y: Math.round(row * (cellH + gap)),
      width: Math.round(cellW),
      height: Math.round(cellH),
    });
  }
  return tiles;
}

export interface ChatSplit {
  /** Width of the main chat column (never shrinks after first spawn). */
  chatWidth: number;
  /** Width of the subagent region to the right. */
  subagentWidth: number;
}

/** Split the container between the fixed chat column and the subagent
 *  region. `ratio` is the chat fraction the moment tiling began; the
 *  chat column is clamped to `MIN_CHAT_WIDTH` so the conversation
 *  stays usable even in a narrow window. */
export function chatSplit(
  containerWidth: number,
  ratio: number = DEFAULT_CHAT_RATIO,
  minChatWidth: number = MIN_CHAT_WIDTH,
): ChatSplit {
  if (!Number.isFinite(containerWidth) || containerWidth <= 0) {
    return { chatWidth: 0, subagentWidth: 0 };
  }
  // Order matters: clamp the ratio-based width up to the readable minimum,
  // then down to the container so the chat column can never exceed it.
  const chatWidth = Math.min(
    containerWidth,
    Math.max(minChatWidth, containerWidth * ratio),
  );
  return {
    chatWidth,
    subagentWidth: Math.max(0, containerWidth - chatWidth),
  };
}
