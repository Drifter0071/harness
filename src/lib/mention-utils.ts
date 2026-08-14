// ── Pure-logic helpers used by MentionInput and the markdown renderer ──
// No DOM, no Tauri imports — safe to unit-test in Vitest on Node.

export type ParsedMention = { active: boolean; query: string };

const SAFE_URL_SCHEMES = new Set(["https:", "data:", "mailto:", "tel:"]);

/**
 * Parse the @-mention query at the caret.
 *
 * Rules (matches spec §8.1, §8.2):
 *   - The mention is "active" when the caret is preceded by `@` and the
 *     character before that is whitespace or start-of-string.
 *   - The query runs from `@` to the caret, stopping at another `@`.
 *   - If the text between `@` and the caret contains whitespace, the
 *     mention is **inactive** (the token ended — user typed past it).
 */
export function parseMentionQuery(text: string, caret: number): ParsedMention {
  if (caret < 1) return { active: false, query: "" };
  const at = text.lastIndexOf("@", caret - 1);
  if (at < 0) return { active: false, query: "" };
  if (at > 0) {
    const before = text[at - 1];
    if (!/[\s\n]/.test(before)) return { active: false, query: "" };
  }
  // Walk back to find any other @ between the trigger and caret.
  const between = text.slice(at + 1, caret);
  if (between.includes("@")) return { active: false, query: "" };
  // If there's any whitespace in the between text, the mention token has
  // ended — the popover must close immediately so it doesn't flash.
  if (/[\s\n]/.test(between)) return { active: false, query: "" };
  return { active: true, query: between };
}

export type RankedResult = string;

/** Browse cap when the query is empty (bare `@`) — the full list is
 *  still reachable by scrolling, and the footer shows the remainder. */
export const MENTION_BROWSE_LIMIT = 120;
/** Ranked match cap while typing. */
export const MENTION_MATCH_LIMIT = 10;

/**
 * Pure ranking — the single source of truth for result order.
 * Used by tests and as the underlying engine for the throttled variant.
 */
function rankMentionResultsRaw(query: string, candidates: string[]): RankedResult[] {
  if (!query) return candidates.slice(0, MENTION_BROWSE_LIMIT);
  const q = query.toLowerCase();
  const pathPrefixQuery = q.includes("/");
  const scored: Array<{ path: string; score: number; idx: number }> = [];

  candidates.forEach((path, idx) => {
    const filename = path.split("/").pop() ?? path;
    const lowerFilename = filename.toLowerCase();
    const lowerPath = path.toLowerCase();
    let score = 0;
    if (pathPrefixQuery && lowerPath.startsWith(q)) {
      score = 6;
    } else if (lowerFilename.startsWith(q)) {
      score = 4;
    } else if (lowerFilename.includes(q)) {
      score = 3;
    } else if (lowerPath.includes(q)) {
      score = 2;
    } else if (fuzzyMatch(lowerFilename, q)) {
      score = 1;
    }
    if (score > 0) {
      scored.push({ path, score, idx });
    }
  });

  scored.sort((a, b) => b.score - a.score || a.idx - b.idx);
  return scored.slice(0, MENTION_MATCH_LIMIT).map((s) => s.path);
}

/** Alias so existing callers (tests) keep working. */
export const rankMentionResults = rankMentionResultsRaw;

// ── RAF throttle ───────────────────────────────────────────────────────────────

/**
 * 1-frame RAF gate so a fast typist cannot queue multiple rank calls per
 * keystroke. The pending job is discarded on every new call so only the
 * latest query fires once the frame ticks.
 */
let _rafHandle = 0;
let _pendingQuery = "";
let _pendingCandidates: string[] = [];
let _pendingResolve: ((r: RankedResult[]) => void) | null = null;

export function rankMentionResultsThrottled(
  query: string,
  candidates: string[],
  onResult: (r: RankedResult[]) => void,
) {
  // Cancel any in-flight RAF so a stale job never runs.
  if (_rafHandle !== 0) cancelAnimationFrame(_rafHandle);
  _pendingQuery = query;
  _pendingCandidates = candidates;
  _pendingResolve = onResult;
  _rafHandle = requestAnimationFrame(() => {
    _rafHandle = 0;
    const resolve = _pendingResolve!;
    _pendingResolve = null;
    resolve(rankMentionResultsRaw(_pendingQuery, _pendingCandidates));
  });
}

function fuzzyMatch(text: string, q: string): boolean {
  let i = 0;
  for (const ch of text) {
    if (ch === q[i]) i++;
    if (i === q.length) return true;
  }
  return false;
}

/**
 * Allowlist-check a URL. Returns the URL if safe, empty string if dropped.
 */
export function filterAndSanitizeUrl(url: string): string {
  if (!url) return "";
  const trimmed = url.trim();
  if (!trimmed) return "";
  const colon = trimmed.indexOf(":");
  if (colon < 0) return "";
  const scheme = trimmed.slice(0, colon + 1).toLowerCase();
  if (!SAFE_URL_SCHEMES.has(scheme)) return "";
  return trimmed;
}

/**
 * Strip `on*="..."` event handler attributes from a chunk of HTML.
 * Defensive — marked does not emit these by default.
 */
export function stripEventHandlers(html: string): string {
  return html.replace(/\s+on[a-z]+\s*=\s*"[^"]*"/gi, "");
}
