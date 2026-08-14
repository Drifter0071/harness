import { describe, expect, it } from "vitest";
import {
  capabilitiesFor,
  inferCapabilities,
  type ResolvedCapabilities,
} from "./model-capabilities";
import type { SavedModel } from "./ipc";

function model(over: Partial<SavedModel> = {}): SavedModel {
  return {
    id: "m",
    label: "test",
    provider: "openai",
    format: "openai-chat-completions",
    baseUrl: "https://api.example.com",
    modelName: "custom-model",
    hasKey: true,
    keyUnreachable: false,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    ...over,
  };
}

describe("inferCapabilities", () => {
  it("recognizes claude sonnet 4 as thinking", () => {
    expect(inferCapabilities("claude-sonnet-4-20250514", "anthropic-messages"))
      .toEqual({ contextWindow: 200_000, reasoning: "thinking" });
  });

  it("recognizes gpt-4o as no reasoning", () => {
    expect(inferCapabilities("gpt-4o", "openai-chat-completions"))
      .toEqual({ contextWindow: 128_000, reasoning: "none" });
  });

  it("recognizes o3 as effort", () => {
    expect(inferCapabilities("o3", "openai-chat-completions"))
      .toEqual({ contextWindow: 200_000, reasoning: "effort" });
  });

  it("recognizes deepseek-reasoner as effort", () => {
    expect(inferCapabilities("deepseek-reasoner", "openai-chat-completions"))
      .toEqual({ contextWindow: 64_000, reasoning: "effort" });
  });

  it("recognizes kimi-k2 as effort", () => {
    expect(inferCapabilities("kimi-k2", "openai-chat-completions"))
      .toEqual({ contextWindow: 128_000, reasoning: "effort" });
  });

  it("falls back to anthropic defaults for unknown anthropic-format model", () => {
    const cap = inferCapabilities("claude-experimental", "anthropic-messages");
    expect(cap.reasoning).toBe("thinking");
    expect(cap.contextWindow).toBe(200_000);
  });

  it("falls back to no reasoning for unknown openai-compat model", () => {
    const cap = inferCapabilities("some-1m-model", "openai-chat-completions");
    expect(cap.reasoning).toBe("none");
    // Unknown openai-compat defaults to 128k (the safe guess; the user
    // can override via Settings).
    expect(cap.contextWindow).toBe(128_000);
  });

  it("catches o-family names by prefix", () => {
    expect(inferCapabilities("o7-research-preview", "openai-chat-completions").reasoning)
      .toBe("effort");
  });

  it("catches reasoner suffix", () => {
    expect(inferCapabilities("my-reasoner-v2", "openai-chat-completions").reasoning)
      .toBe("effort");
  });
});

describe("capabilitiesFor", () => {
  it("returns the user override when both fields are set", () => {
    const m = model({
      contextWindow: 1_000_000,
      reasoningCapability: "effort",
      reasoningLevels: ["low", "high"],
    });
    const cap: ResolvedCapabilities = capabilitiesFor(m);
    expect(cap).toEqual({
      contextWindow: 1_000_000,
      reasoning: "effort",
      reasoningLevels: ["low", "high"],
      source: "override",
    });
  });

  it("uses the probe result when no user override is set", () => {
    const m = model({
      modelName: "unknown-1m",
      capabilityProbe: {
        probedAt: "2026-01-01T00:00:00Z",
        source: "smoke-test",
        contextWindow: 1_000_000,
        reasoning: "effort",
        reasoningLevels: ["low", "medium", "high"],
        note: null,
      },
    });
    const cap = capabilitiesFor(m);
    expect(cap.contextWindow).toBe(1_000_000);
    expect(cap.reasoning).toBe("effort");
    expect(cap.source).toBe("probe");
  });

  it("lets the user override the probe for one field", () => {
    const m = model({
      modelName: "unknown",
      contextWindow: 500_000, // user override
      capabilityProbe: {
        probedAt: "2026-01-01T00:00:00Z",
        source: "smoke-test",
        contextWindow: 200_000, // probe disagrees
        reasoning: "effort",
        reasoningLevels: null,
        note: null,
      },
    });
    const cap = capabilitiesFor(m);
    expect(cap.contextWindow).toBe(500_000); // user wins
    expect(cap.reasoning).toBe("effort"); // probe fills the gap
    expect(cap.source).toBe("override");
  });

  it("uses the named catalog when no override and no probe", () => {
    const cap = capabilitiesFor(model({ modelName: "claude-sonnet-4-20250514", format: "anthropic-messages" }));
    expect(cap.contextWindow).toBe(200_000);
    expect(cap.reasoning).toBe("thinking");
  });

  it("uses format defaults when the name is unknown and no override", () => {
    const cap = capabilitiesFor(model({ modelName: "new-thing-1m", format: "anthropic-messages" }));
    expect(cap.reasoning).toBe("thinking");
    expect(cap.contextWindow).toBe(200_000);
  });

  it("falls back to canonical levels when probe levels are empty", () => {
    const m = model({
      reasoningCapability: "effort",
      reasoningLevels: ["low", "medium", "high"],
      capabilityProbe: {
        probedAt: "2026-01-01T00:00:00Z",
        source: "smoke-test",
        contextWindow: null,
        reasoning: null,
        reasoningLevels: [], // empty — should fall back
        note: null,
      },
    });
    const cap = capabilitiesFor(m);
    // User has set the override + levels; probe is empty.
    expect(cap.reasoningLevels).toEqual(["low", "medium", "high"]);
  });
});
