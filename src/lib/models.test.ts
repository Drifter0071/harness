import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import {
  contextWindowFor,
  formatMessagesForProvider,
  migrateLocalStorageEntries,
  normalizeFormat,
} from "./models";
import type { SessionMessage } from "./store";
import type { SavedModelInput } from "./ipc";

const msgs = (): SessionMessage[] => [
  { id: "m1", role: "user", content: "hello", timestamp: "2026-08-12T00:00:00Z" },
  { id: "m2", role: "assistant", content: "hi there", timestamp: "2026-08-12T00:00:01Z" },
  { id: "m3", role: "system", content: "system note", timestamp: "2026-08-12T00:00:02Z" },
];

describe("normalizeFormat", () => {
  it("maps display names and kebab ids to the two real wire formats", () => {
    expect(normalizeFormat("anthropic-messages")).toBe("anthropic-messages");
    expect(normalizeFormat("Anthropic Messages")).toBe("anthropic-messages");
    expect(normalizeFormat("openai-chat-completions")).toBe("openai-chat-completions");
    expect(normalizeFormat("OpenAI Compatible")).toBe("openai-chat-completions");
    expect(normalizeFormat("Groq")).toBe("openai-chat-completions");
    expect(normalizeFormat("DeepSeek")).toBe("openai-chat-completions");
  });
});

describe("formatMessagesForProvider", () => {
  it("strips system messages for anthropic (system has its own param)", () => {
    const out = formatMessagesForProvider(msgs(), "anthropic-messages");
    expect(out).toEqual([
      { role: "user", content: "hello" },
      { role: "assistant", content: "hi there" },
    ]);
  });

  it("keeps system messages for openai-compat", () => {
    const out = formatMessagesForProvider(msgs(), "openai-chat-completions");
    expect(out).toHaveLength(3);
    expect(out[2]).toEqual({ role: "system", content: "system note" });
  });

  it("preserves message order", () => {
    const roles = formatMessagesForProvider(msgs(), "anthropic-messages").map((m) => m.role);
    expect(roles).toEqual(["user", "assistant"]);
  });
});

describe("contextWindowFor", () => {
  it("looks up known models", () => {
    expect(contextWindowFor("claude-sonnet-4-20250514")).toBe(200_000);
    expect(contextWindowFor("gpt-4o")).toBe(128_000);
    expect(contextWindowFor("gpt-4.1")).toBe(200_000);
  });

  it("defaults unknown models to 128k", () => {
    expect(contextWindowFor("some-brand-new-model")).toBe(128_000);
  });
});

describe("migrateLocalStorageEntries", () => {
  let storage: Record<string, string>;
  const save = vi.fn(
    async (_model: SavedModelInput, _apiKey?: string): Promise<unknown> => ({})
  );

  beforeEach(() => {
    storage = {};
    // happy-dom's Node runtime doesn't expose a global localStorage;
    // stub a minimal compliant one so the migration is testable.
    vi.stubGlobal("localStorage", {
      getItem: (k: string) => storage[k] ?? null,
      setItem: (k: string, v: string) => { storage[k] = String(v); },
      removeItem: (k: string) => { delete storage[k]; },
      clear: () => { storage = {}; },
      key: (i: number) => Object.keys(storage)[i] ?? null,
      get length() { return Object.keys(storage).length; },
    });
    save.mockClear();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("does nothing when localStorage is empty", async () => {
    const ran = await migrateLocalStorageEntries(save);
    expect(ran).toBe(false);
    expect(save).not.toHaveBeenCalled();
  });

  it("calls save_saved_model per entry, in order", async () => {
    storage["harness_model_entries"] = JSON.stringify([
      { id: "a", provider: "anthropic", format: "Anthropic Messages", baseUrl: "https://api.anthropic.com", modelName: "claude-sonnet-4-20250514", apiKey: "sk-1" },
      { id: "b", provider: "openai", format: "OpenAI Chat Completions", baseUrl: "https://api.openai.com", modelName: "gpt-4o", apiKey: "sk-2" },
    ]);
    const ran = await migrateLocalStorageEntries(save);
    expect(ran).toBe(true);
    expect(save).toHaveBeenCalledTimes(2);
    expect(save.mock.calls[0][0]).toMatchObject({
      id: "a",
      format: "anthropic-messages",
      modelName: "claude-sonnet-4-20250514",
    });
    expect(save.mock.calls[0][1]).toBe("sk-1");
    expect(save.mock.calls[1][0]).toMatchObject({ id: "b", format: "openai-chat-completions" });
    // localStorage is deleted only after the full batch succeeds
    expect(storage["harness_model_entries"]).toBeUndefined();
  });

  it("aborts on the first failure and preserves localStorage", async () => {
    storage["harness_model_entries"] = JSON.stringify([
      { id: "a", modelName: "m-a", apiKey: "k" },
      { id: "b", modelName: "m-b", apiKey: "k" },
    ]);
    save.mockRejectedValueOnce(new Error("keyring locked"));
    await expect(migrateLocalStorageEntries(save)).rejects.toThrow("keyring locked");
    expect(save).toHaveBeenCalledTimes(1);
    expect(storage["harness_model_entries"]).toBeDefined();
  });

  it("generates an id when the legacy entry has none", async () => {
    storage["harness_model_entries"] = JSON.stringify([
      { provider: "openai", modelName: "gpt-4o", apiKey: "sk" },
    ]);
    await migrateLocalStorageEntries(save);
    expect(save.mock.calls[0][0].id).toMatch(/^migrated-/);
  });
});
