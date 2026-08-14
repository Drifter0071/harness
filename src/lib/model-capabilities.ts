// Model capability resolution.
//
// Source-of-truth order:
//   1. The user's manual override on the saved model (highest priority).
//   2. The most recent live probe result stored on the model
//      (`capabilityProbe` from Anthropic's Models API or a smoke test).
//   3. A built-in name/format heuristic for models we recognize.
//   4. `None` (the safest default — never invent reasoning controls).
//
// The intent is: the user is always the final word. The probe and
// the heuristic are seeds; the user can correct them once and never
// be asked again.

import type { SavedModel } from "./ipc";

export type ReasoningKind = "none" | "thinking" | "effort";

export interface ResolvedCapabilities {
  /** Effective context window in tokens. Always a positive number. */
  contextWindow: number;
  /** What kind of reasoning control the model supports. */
  reasoning: ReasoningKind;
  /** Subset of effort levels the user has approved. When empty, the
   *  UI shows the canonical low/medium/high set for the capability. */
  reasoningLevels: ReadonlyArray<"low" | "medium" | "high">;
  /** Which strategy produced the result. Useful for the "auto"
   *  badge in the Settings UI. */
  source:
    | "override"
    | "probe"
    | "inferred"
    | "default";
}

const CANONICAL_LEVELS = ["low", "medium", "high"] as const;

/** Built-in name/format heuristic. Kept small — anything missing
 *  falls through to `defaultForFormat`. */
const NAMED_HINTS: Record<
  string,
  { contextWindow?: number; reasoning: ReasoningKind }
> = {
  // Anthropic family — extended thinking, 200K context.
  "claude-sonnet-4-20250514": { contextWindow: 200_000, reasoning: "thinking" },
  "claude-3-5-sonnet-20241022": { contextWindow: 200_000, reasoning: "thinking" },
  "claude-3-7-sonnet-20250219": { contextWindow: 200_000, reasoning: "thinking" },
  "claude-3-5-haiku-20241022": { contextWindow: 200_000, reasoning: "thinking" },
  "claude-3-opus-20240229": { contextWindow: 200_000, reasoning: "thinking" },
  "claude-3-haiku-20240307": { contextWindow: 200_000, reasoning: "thinking" },
  // OpenAI family — gpt-* has no effort, o* does.
  "gpt-4o": { contextWindow: 128_000, reasoning: "none" },
  "gpt-4o-mini": { contextWindow: 128_000, reasoning: "none" },
  "gpt-4.1": { contextWindow: 200_000, reasoning: "none" },
  "gpt-4.1-mini": { contextWindow: 200_000, reasoning: "none" },
  "gpt-4.1-nano": { contextWindow: 200_000, reasoning: "none" },
  o1: { contextWindow: 200_000, reasoning: "effort" },
  "o1-mini": { contextWindow: 128_000, reasoning: "none" },
  "o1-preview": { contextWindow: 128_000, reasoning: "effort" },
  "o3-mini": { contextWindow: 200_000, reasoning: "effort" },
  o3: { contextWindow: 200_000, reasoning: "effort" },
  "o4-mini": { contextWindow: 200_000, reasoning: "effort" },
  // Third-party reasoning models commonly seen through OpenAI-compat
  // gateways. These are heuristic hints — the user can correct.
  "deepseek-chat": { contextWindow: 64_000, reasoning: "none" },
  "deepseek-reasoner": { contextWindow: 64_000, reasoning: "effort" },
  "kimi-k2": { contextWindow: 128_000, reasoning: "effort" },
  "glm-4-plus": { contextWindow: 128_000, reasoning: "none" },
  "glm-4.5": { contextWindow: 128_000, reasoning: "effort" },
  "glm-5.2": { contextWindow: 200_000, reasoning: "effort" },
  "qwen-plus": { contextWindow: 131_072, reasoning: "none" },
  "qwen-max": { contextWindow: 32_768, reasoning: "none" },
  "grok-4.5": { contextWindow: 131_072, reasoning: "effort" },
};

const DEFAULT_CONTEXT_WINDOW = 128_000;

/** Per-format defaults. Used when the model name is unknown. */
function defaultForFormat(format: string): {
  contextWindow: number;
  reasoning: ReasoningKind;
} {
  if (format === "anthropic-messages") {
    return { contextWindow: 200_000, reasoning: "thinking" };
  }
  // OpenAI Chat Completions is the catch-all for gateways and
  // custom providers. Reasoning control depends on the actual
  // model, so default to "none" — safer than inventing effort
  // controls for a model that may not accept them.
  return { contextWindow: DEFAULT_CONTEXT_WINDOW, reasoning: "none" };
}

/** Pure inference. `format` and `modelName` come from the saved
 *  model; the result is a hint, not a fact. The user can override
 *  the result via the saved model's `contextWindow` /
 *  `reasoningCapability` fields. */
export function inferCapabilities(
  modelName: string,
  format: string,
): { contextWindow: number; reasoning: ReasoningKind } {
  const named = NAMED_HINTS[modelName];
  if (named) {
    return {
      contextWindow: named.contextWindow ?? DEFAULT_CONTEXT_WINDOW,
      reasoning: named.reasoning,
    };
  }
  // Pattern-based fallbacks for families we don't list explicitly.
  // Keep these narrow — false positives are worse than misses.
  if (/^o\d/i.test(modelName)) {
    return { contextWindow: 200_000, reasoning: "effort" };
  }
  if (/reasoner|thinking/i.test(modelName)) {
    return { contextWindow: 64_000, reasoning: "effort" };
  }
  return defaultForFormat(format);
}

/** Final capability resolution. Walks the source-of-truth order in
 *  the module header and returns the effective values plus a
 *  provenance hint for the UI. The user override ALWAYS wins over
 *  the probe and the inference — once the user has set a value, we
 *  don't silently overwrite it. */
export function capabilitiesFor(model: SavedModel): ResolvedCapabilities {
  // Pre-compute the three layers in priority order. Layer 1 is the
  // user override (per-field), layer 2 is the live probe, layer 3
  // is name/format inference.
  const inferred = inferCapabilities(model.modelName, model.format);
  const probe = model.capabilityProbe;

  // Per-field effective value: user override → probe → inference.
  const contextWindow =
    model.contextWindow ??
    probe?.contextWindow ??
    inferred.contextWindow;
  const reasoning =
    model.reasoningCapability ??
    probe?.reasoning ??
    inferred.reasoning;

  // Levels: the user can also constrain the level set. Empty /
  // missing probe levels fall through to the user's override or the
  // canonical set.
  const reasoningLevels: ReadonlyArray<"low" | "medium" | "high"> =
    (model.reasoningLevels && model.reasoningLevels.length > 0
      ? model.reasoningLevels
      : (probe?.reasoningLevels && probe.reasoningLevels.length > 0
          ? probe.reasoningLevels
          : CANONICAL_LEVELS)) as ReadonlyArray<"low" | "medium" | "high">;

  // Provenance: "override" if the user set anything, "probe" if
  // a probe produced a result we used, "inferred" if we only had
  // the catalog, "default" if even the catalog was generic.
  const hasUserOverride =
    model.contextWindow != null || model.reasoningCapability != null;
  const hasProbe =
    probe != null &&
    (probe.contextWindow != null || probe.reasoning != null);
  const source: ResolvedCapabilities["source"] = hasUserOverride
    ? "override"
    : hasProbe
      ? "probe"
      : inferred.contextWindow === DEFAULT_CONTEXT_WINDOW
        && inferred.reasoning === defaultForFormat(model.format).reasoning
        ? "default"
        : "inferred";

  return { contextWindow, reasoning, reasoningLevels, source };
}
