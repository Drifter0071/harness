// Model catalog + provider-agnostic helpers (Phase 2.0).
//
// - PROVIDER_PRESETS: preset baseUrl / format / model suggestions.
// - contextWindowFor: published context-window lookup for the chat
//   header telemetry (spec §7.6); unknown models get a sane default.
// - formatMessagesForProvider: internal SessionMessage[] → wire
//   ChatMessage[] (role/content), provider-aware.
// - migrateLocalStorageEntries: one-shot localStorage → keyring
//   migration (spec §8). Full abort on any failure; localStorage is
//   preserved so the next launch retries.

import { ipc } from "./ipc";
import type {
  ChatMessage,
  SavedModelInput,
} from "./ipc";
import type { SessionMessage } from "./store";

// ── Formats ──

export const API_FORMATS = [
  { value: "anthropic-messages", label: "Anthropic Messages" },
  { value: "openai-chat-completions", label: "OpenAI Chat Completions" },
] as const;

export const FORMAT_LABELS: Record<string, string> = Object.fromEntries(
  API_FORMATS.map((f) => [f.value, f.label])
);

/** Everything else is a variant of OpenAI Chat Completions on the wire. */
export function normalizeFormat(format: string): string {
  const f = format.toLowerCase();
  if (f.includes("anthropic")) return "anthropic-messages";
  return "openai-chat-completions";
}

// ── Provider presets ──

export interface ProviderPreset {
  label: string;
  baseUrl: string;
  format: string;
  models: string[];
}

export const PROVIDER_PRESETS: Record<string, ProviderPreset> = {
  anthropic: {
    label: "anthropic",
    baseUrl: "https://api.anthropic.com",
    format: "anthropic-messages",
    models: [
      "claude-sonnet-4-20250514",
      "claude-3-5-haiku-20241022",
      "claude-3-opus-20240229",
    ],
  },
  openai: {
    label: "openai",
    baseUrl: "https://api.openai.com",
    format: "openai-chat-completions",
    models: ["gpt-4o", "gpt-4o-mini", "gpt-4.1", "o3-mini"],
  },
  "openai-compatible": {
    label: "openai-compatible",
    baseUrl: "",
    format: "openai-chat-completions",
    models: [],
  },
  custom: {
    label: "custom",
    baseUrl: "",
    format: "openai-chat-completions",
    models: [],
  },
};

// ── Context windows ──

const CONTEXT_WINDOWS: Record<string, number> = {
  // Anthropic
  "claude-sonnet-4-20250514": 200_000,
  "claude-3-5-sonnet-20241022": 200_000,
  "claude-3-7-sonnet-20250219": 200_000,
  "claude-3-5-haiku-20241022": 200_000,
  "claude-3-opus-20240229": 200_000,
  "claude-3-haiku-20240307": 200_000,
  // OpenAI
  "gpt-4o": 128_000,
  "gpt-4o-mini": 128_000,
  "gpt-4.1": 200_000,
  "gpt-4.1-mini": 200_000,
  "gpt-4.1-nano": 200_000,
  "o1": 200_000,
  "o3-mini": 200_000,
  // OpenAI-compat third parties (best-known values; user can override)
  "deepseek-chat": 64_000,
  "deepseek-reasoner": 64_000,
  "kimi-k2": 128_000,
  "glm-4-plus": 128_000,
  "glm-4.5": 128_000,
  "qwen-plus": 131_072,
  "qwen-max": 32_768,
};

const DEFAULT_CONTEXT_WINDOW = 128_000;

export function contextWindowFor(modelName: string): number {
  return CONTEXT_WINDOWS[modelName] ?? DEFAULT_CONTEXT_WINDOW;
}

// ── Message formatting ──

/** Convert internal session messages to the provider wire shape.
 *  Anthropic rejects a `system` role inside `messages` (it has a
 *  separate `system` param we don't use), so system entries are
 *  stripped. OpenAI-compat accepts system, so they pass through. */
export function formatMessagesForProvider(
  messages: SessionMessage[],
  format: string
): ChatMessage[] {
  if (normalizeFormat(format) === "anthropic-messages") {
    return messages
      .filter((m) => m.role === "user" || m.role === "assistant")
      .map((m) => ({ role: m.role, content: m.content }));
  }
  return messages.map((m) => ({ role: m.role, content: m.content }));
}

// ── localStorage → keyring migration (spec §8) ──

interface LegacyModelEntry {
  id?: string;
  provider?: string;
  format?: string;
  baseUrl?: string;
  apiKey?: string;
  modelName?: string;
  label?: string;
}

/** Run the one-time migration. Returns true when a migration ran (and
 *  succeeded); false when there was nothing to do. Throws on failure —
 *  the caller shows a toast and the next launch retries.
 *
 *  No module-level guard: once the localStorage entry is gone the
 *  function naturally no-ops, and a failed run MUST be retryable
 *  (spec §8.3).
 *
 *  `save` is injectable for tests; it defaults to the real IPC. */
export async function migrateLocalStorageEntries(
  save: (model: SavedModelInput, apiKey?: string) => Promise<unknown> = (
    model,
    apiKey
  ) => ipc.saveSavedModel(model, apiKey)
): Promise<boolean> {
  const raw = localStorage.getItem("harness_model_entries");
  if (!raw) return false;

  let entries: LegacyModelEntry[];
  try {
    entries = JSON.parse(raw);
  } catch {
    // Corrupt payload — drop it rather than loop forever.
    localStorage.removeItem("harness_model_entries");
    return false;
  }

  if (!Array.isArray(entries) || entries.length === 0) {
    localStorage.removeItem("harness_model_entries");
    return false;
  }

  for (const e of entries) {
    if (!e || (!e.id && !e.modelName)) continue; // junk row, skip
    const model: SavedModelInput = {
      id: e.id && e.id.length > 0 ? e.id : `migrated-${e.modelName}-${Math.random().toString(36).slice(2, 8)}`,
      label: e.label && e.label.length > 0 ? e.label : e.modelName ?? "Imported model",
      provider: e.provider ?? "custom",
      format: normalizeFormat(e.format ?? "openai-chat-completions"),
      baseUrl: e.baseUrl ?? "",
      modelName: e.modelName ?? "",
    };
    // Non-empty keys are written to the keyring; empty/absent keys are
    // left alone. Any thrown error aborts the whole batch (spec §8.3).
    await save(model, e.apiKey && e.apiKey.length > 0 ? e.apiKey : "");
  }

  // Full batch succeeded — only now remove the localStorage entry.
  localStorage.removeItem("harness_model_entries");
  return true;
}
