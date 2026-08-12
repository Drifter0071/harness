# AI Models, Encrypted Secrets, and Chat Telemetry

**Status:** Draft for review
**Date:** 2026-08-11
**Scope:** Phase 2.0 — make the chat actually call real LLMs with encrypted API keys, a model selector, per-request controls (thinking + reasoning effort), and live token / context / session-window telemetry.

## 1. Summary

This spec makes the chat surface real. It replaces the prior audit's flagged plaintext `localStorage` API keys with an **OS-keyring secret store** (via the `keyring` crate — Windows Credential Manager / Keychain / libsecret), ships an actual streaming LLM client for Anthropic Messages and OpenAI Chat Completions (the latter as a universal protocol that also covers MiniMax, GLM, Kimi, Qwen, and any other OpenAI-compatible endpoint), adds a model selector with per-model thinking toggle (Anthropic) and reasoning effort picker (any OpenAI-compat), and surfaces live token / context / session-window telemetry in a chat header.

The goal is to make the chat a usable daily tool with the same safety posture as a first-class IDE.

## 2. Out of Scope

The following are explicitly deferred to future specs:

- Multi-modal (image inputs)
- Tool use / function calling
- Multi-model router / sub-agents routing to different models per task
- Per-message model selection (every message uses the active model)
- Custom system prompts per model (no `system` field UI yet)
- Image-generation models (DALL-E, Imagen, etc.)
- File uploads to providers that support them
- Anthropic prompt caching
- Edit / regenerate a streamed message in place
- Sub-agent spawning (the existing `spawn_agent` IPC stays a stub)
- Tokenizer-based local estimation (we use real `usage` numbers from the API only)

## 3. Architecture

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  React frontend                                                              │
│  ┌──────────────┐  ┌──────────────────┐  ┌──────────────────────────────┐    │
│  │ Chat         │  │ ModelSelector    │  │ ChatStats (in chat header)    │    │
│  │  + composer  │  │  + Thinking      │  │  • Last response in/out       │    │
│  │              │  │  + Reasoning     │  │  • Context used               │    │
│  │              │  │    effort        │  │  • Context window             │    │
│  │              │  │                  │  │  • Session window (if known)  │    │
│  └──────┬───────┘  └────────┬─────────┘  └──────────────┬───────────────┘    │
│         │                   │                            │                    │
│         ▼                   ▼                            ▼                    │
│  ┌─────────────────────────────────────────────────────────────────────┐    │
│  │  Zustand store:                                                     │    │
│  │   + savedModels[] (id, label, provider, format, baseUrl,            │    │
│  │                    modelName, hasKey, timestamps)                   │    │
│  │   + activeModelId: string | null                                   │    │
│  │   + modelPrefs[id] (thinkingOn, reasoningEffort)                   │    │
│  │   + tokenStats (lastIn/Out, cumulativeIn/Out,                       │    │
│  │                 sessionWindowUsed?, sessionWindowResetsAt?)         │    │
│  └─────────────────────────────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────────────────────────────┘
                          │
                          ▼  IPC (Tauri invoke)
┌──────────────────────────────────────────────────────────────────────────────┐
│  Rust IPC commands (this spec):                                              │
│   list_saved_models / save_saved_model / delete_saved_model                  │
│   get_active_model / set_active_model                                        │
│   get_model_prefs / set_model_prefs                                          │
│   test_saved_model                                                           │
│   chat_complete (streams via Tauri events) / cancel_chat                     │
│   reset_secret_store  (deletes all keyring entries; explicit user action)    │
└──────────────────────────────────────────────────────────────────────────────┘
                          │
                          ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│  Rust crates (new modules):                                                  │
│   secrets.rs     — `SecretStore` trait + `KeyringStore` impl (OS keyring)    │
│                     + `InMemoryStore` impl (tests)                           │
│   models.rs      — provider catalog, format adapters, model metadata         │
│   llm_client.rs  — Anthropic + OpenAI streaming clients + SSE parsers         │
└──────────────────────────────────────────────────────────────────────────────┘
```

**Component boundaries** (each testable in isolation):

| Unit | What it does | Depends on |
|---|---|---|
| `secrets.rs` | `SecretStore` trait + `KeyringStore` (production) + `InMemoryStore` (tests) | `keyring` |
| `models.rs` | Static provider catalog (formats, model metadata, default base URLs), `resolve(model_id)` returning a `ResolvedModel` ready to call | `secrets` |
| `llm_client.rs` | Anthropic Messages + OpenAI Chat Completions streaming clients, SSE parsers, opportunistic session-window header reading, error mapping | `reqwest`, `models`, `secrets` |
| `Settings` (TS, rebuild) | Two-pane: Saved Models list on the left, editor form on the right; per-row "Test connection" | `useStore`, `ipc` |
| `ModelSelector` (TS) | Composer row with model + thinking + reasoning effort | `useStore`, `ipc` |
| `ChatStats` (TS) | 4-row stats panel in the chat header | `useStore` |

## 4. Data Model

### 4.1 Saved models (Rust file, no API keys)

**Location:** `<app_data>/Harness/models.json` (matching the existing `recent.json` convention from the prior spec — `dirs::data_dir().join("Harness")`).

**Wire format**: `#[serde(rename_all = "camelCase")]` on the Rust struct, consistent with the existing `Session` / `Mention` types in `session.rs`.

```json
{
  "version": 1,
  "models": [
    {
      "id": "abc123",
      "label": "Sonnet 4 (work)",
      "provider": "anthropic",
      "format": "anthropic-messages",
      "baseUrl": "https://api.anthropic.com",
      "modelName": "claude-sonnet-4-20250514",
      "hasKey": true,
      "createdAt": "2026-08-11T15:00:00Z",
      "updatedAt": "2026-08-11T15:00:00Z"
    }
  ]
}
```

- `hasKey: true/false` is the user-facing signal; the key itself never appears here. `hasKey` is computed at read time by calling `SecretStore::has(model_id)` — it's not stored in the file.
- `provider` is the loose human-friendly name (`anthropic`, `openai`, `openai-compatible`, `custom`).
- `format` is the wire format the client must use (`anthropic-messages` or `openai-chat-completions`). The OpenAI format is the universal protocol — it works against `api.openai.com` and against any OpenAI-compatible endpoint (MiniMax, GLM, Kimi, Qwen, etc.).
- `baseUrl` is always read from the saved model. **The client never hard-codes a base URL** — that's the whole point of the field. Frontend normalizes: strip trailing slashes from `baseUrl` before joining `/chat/completions` (or `/messages` for Anthropic).

### 4.2 API keys (OS keyring + Rust trait abstraction)

We use `keyring` (the OS keyring crate) directly — no `tauri-plugin-stronghold` dependency. The keyring gives us:
- Windows Credential Manager
- macOS Keychain
- libsecret on Linux

…all of which are encrypted at rest by the OS with the user's session credentials, and require no passphrase prompt. Stronghold would add a second encryption layer on top of a key that itself is in the keyring — security theater, not a real improvement.

**Storage layout:**
- **Service name**: `Harness`
- **Account per entry**: `<model_id>` (e.g. `Harness / abc123`)
- **Password**: the raw API key string
- **No `models.json` ↔ keyring cross-check** beyond "the entry exists"; we don't store anything in models.json that would be redundant.

**Rust surface** (one trait, two impls):

```rust
pub trait SecretStore: Send + Sync {
    fn set(&self, account: &str, value: &str) -> Result<(), String>;
    fn get(&self, account: &str) -> Result<Option<String>, String>;
    fn has(&self, account: &str) -> Result<bool, String>;
    fn delete(&self, account: &str) -> Result<(), String>;
}

pub struct KeyringStore;
impl SecretStore for KeyringStore { /* delegates to keyring crate */ }

// For tests:
pub struct InMemoryStore { map: Mutex<HashMap<String, String>> }
impl SecretStore for InMemoryStore { /* in-process HashMap */ }
```

The frontend's `hasKey` boolean comes from `SecretStore::has(model_id)` returning `Ok(true/false)`. A `keyring` error (e.g. the credential store is locked, perms denied) bubbles up as `Err(...)` and is surfaced in the UI as a separate state (see §7.4 — "key unreachable").

**Why not Stronghold**: a 32-byte key in the keyring is functionally identical to a 32-byte password in Stronghold. Stronghold's added value is password-based key derivation (argon2); we get that for free from the OS credential manager.

**No migration complexity**: the old `localStorage["harness_model_entries"]` is read once on first launch and each entry's `apiKey` is written via `SecretStore::set(model_id, key)`. The frontend's `models` field is then written to `models.json` (which has only the metadata, no keys).

### 4.3 Per-model preferences (localStorage, no Rust)

```ts
interface ModelPrefs {
  thinkingOn: boolean;
  // OpenAI-compat only:
  reasoningEffort: "low" | "medium" | "high";
}
```

Defaults: `{ thinkingOn: false, reasoningEffort: "medium" }`.

The thinking budget is **not** user-settable — Anthropic picks a reasonable default (`budget_tokens: 4096`) when thinking is enabled. Reasoning effort is offered for any OpenAI-compat model because many providers (not just OpenAI's o-series) honor it; if a particular model rejects the param, the client silently retries without it.

### 4.4 Active model (localStorage, single value)

```ts
activeModelId: string | null;
```

Sticky last-used across launches and across projects. Per-project pinning is a v2 feature.

### 4.5 Token stats (persisted to `session.json`)

```ts
interface TokenStats {
  lastIn: number; lastOut: number;
  cumulativeIn: number; cumulativeOut: number;
  // Anthropic session-window usage, if the headers are present
  // (or any other provider that exposes similar headers):
  sessionWindowUsed: number | null;       // 0..1
  sessionWindowResetsAt: string | null;   // ISO 8601
}
```

Cumulative numbers **persist with the session** (not in-memory). They live as a sibling field of `messages` in `Session` (added in §4.5b below). Resets when a new session starts (project switch).

**Important — what each number means:**
- `lastIn` / `lastOut` — the input/output tokens reported for the **most recent** assistant response.
- `cumulativeIn` / `cumulativeOut` — running totals across the session. `cumulativeIn` is **NOT** summed naively (turn N's `input_tokens` already includes the prior turns; summing would double-count). Instead, we update it with the **delta**: `cumulativeIn = max(cumulativeIn, lastIn)` — i.e. the most recent input is always at least as large as any prior input, and the cumulative tracks the **peak input size** of the session so far. The actual "context used" is `lastIn` (the most recent input IS the server-side context for that turn).
- The bar in the chat header uses `lastIn` as "Context used" — that's the honest number.

### 4.5b Session shape extension

The existing `Session` type from the prior spec gains a `tokenStats: TokenStats | null` field. Default is `null` (no API calls yet). On first `chat-done`, it becomes the new totals.

## 5. IPC Surface

All new commands live in `src-tauri/src/commands.rs` (registration in `lib.rs`). Module-specific logic lives in `secrets.rs`, `models.rs`, and `llm_client.rs`.

### 5.1 Saved model commands

| Command | Args | Returns | Notes |
|---|---|---|---|
| `list_saved_models` | — | `SavedModel[]` | `hasKey` per row |
| `save_saved_model` | `SavedModel`, `apiKey: Option<string>` | `SavedModel` | `apiKey` is a three-state field: `Some(non-empty)` overwrites the key in the keyring; `Some("")` is treated as "no change" (keeps the existing key, if any); `None` means "don't touch the key" (the field is omitted entirely in the TS call). |
| `delete_saved_model` | `id: string` | `void` | Removes from `models.json` and from the keyring |
| `get_active_model` | — | `string \| null` |  |
| `set_active_model` | `id: string \| null` | `void` |  |

### 5.2 Per-model prefs

| Command | Args | Returns |
|---|---|---|
| `get_model_prefs` | `id: string` | `ModelPrefs` (default if unset) |
| `set_model_prefs` | `id: string`, `prefs: ModelPrefs` | `void` |

### 5.3 Secret store (Rust-side, used by `save_saved_model` and `chat_complete` internally; not exposed to frontend)

The Rust `SecretStore` trait is defined in §4.2. The default `KeyringStore` impl is the production path. Commands are:

- `secrets::set(id, key)` — write to keyring; returns `Result<(), String>`.
- `secrets::get(id) -> Option<String>` — read; returns `Ok(None)` if the entry doesn't exist, `Err(...)` if the keyring is unreachable.
- `secrets::has(id) -> bool` — distinguishes `Ok(true/false)` (legitimate presence/absence) from `Err(...)` (keyring unreachable, surfaced in the UI as a separate disabled state per §7.4).
- `secrets::delete(id)` — idempotent.
- `secrets::delete_all()` — used by the "Reset secret store" button; requires explicit user confirmation in the UI.
- `secrets::test(provider, format, base_url, key)` — the one-shot validation call described in §6.3. Used by Settings' "Test connection" button.

### 5.4 Chat

| Command | Args | Returns / Emits | Notes |
|---|---|---|---|
| `chat_complete` | `request: ChatRequest` | Streams via `app.emit("chat-chunk", ...)` then `chat-done { requestId, content, usage, sessionWindow? }` or `chat-error { requestId, kind, error, retryAfterSeconds? }` (full payload shapes in §5.5) | `requestId` is caller-supplied (nanoid) so the frontend can correlate. The actual response is never returned via `invoke` — it's all events. |
| `cancel_chat` | `requestId: string` | `void` | Aborts the in-flight stream (best-effort; SSE may not respect abort instantly) |

```rust
struct ChatRequest {
  request_id: String,
  model_id: String,            // refers to a SavedModel
  messages: Vec<ChatMessage>,  // [{ role, content }]
  thinking_on: bool,
  reasoning_effort: Option<ReasoningEffort>,
}

enum ReasoningEffort { Low, Medium, High }
```

### 5.5 Streaming event contract (Rust → TS)

**Contract guarantees** (must hold for every `chat_complete` invocation):

1. **Exactly one terminal event per `requestId`**: either `chat-done` or `chat-error`. Never both, never neither (even on panic — we wrap the whole stream in a `tokio::spawn` whose `JoinHandle` returns the `Result`; on panic we emit `chat-error { kind: "internal" }`).
2. **All `chat-chunk` events for a request are emitted before `chat-done`** (no chunk can race with the terminal).
3. **Empty stream is allowed** — the model can return zero chunks (e.g. immediately refused). `chat-done` still fires with `content: ""` and `usage: { input: 0, output: 0 }`.

```ts
interface ChatChunk {
  requestId: string;
  delta: string;       // text fragment to append
}

interface ChatDone {
  requestId: string;
  content: string;     // full assembled response (also useful if the frontend missed chunks)
  usage: {
    input: number;
    output: number;
  };
  sessionWindow?: {
    usedFraction: number;     // 0..1
    resetsAt: string;         // ISO 8601
  };
}

type ChatErrorKind =
  | "cancelled"        // user-initiated cancel
  | "auth"             // 401/403 — invalid or missing API key
  | "rate_limit"       // 429 — has retryAfterSeconds if the provider sent Retry-After
  | "network"          // connection error / DNS / TLS
  | "provider"         // 5xx from the provider
  | "bad_request"      // 400 — model rejected the request (incl. unsupported reasoning_effort)
  | "internal";        // panic / bug on our side

interface ChatError {
  requestId: string;
  kind: ChatErrorKind;
  error: string;        // human-readable, for toasts
  retryAfterSeconds?: number;  // only set for "rate_limit"
}
```

## 6. LLM Client Internals

### 6.1 Anthropic Messages path

**Endpoint:** `POST {base_url}/v1/messages`
**Headers:** `x-api-key: <key>`, `anthropic-version: 2023-06-01`, `content-type: application/json`
**Body:**
```json
{
  "model": "<model_name>",
  "max_tokens": 4096,
  "messages": [{"role": "user", "content": "..."}],
  "stream": true
}
```
**With thinking enabled:** add `"thinking": {"type": "enabled", "budget_tokens": 4096}`.

**SSE parsing** — pin these shapes with golden-file tests (the test contract is the API contract):
- `event: message_start` + `data: {"message": {"usage": {"input_tokens": N}}}` → capture into `usage.input` (the input token count is reported in the first event, before any content arrives)
- `event: content_block_delta` + `data: {"delta": {"type": "text_delta", "text": "..."}}` → emit `chat-chunk`
- `event: message_delta` + `data: {"usage": {"output_tokens": N}}` → capture into `usage.output`
- `event: message_stop` → end of stream; emit `chat-done`
- `event: error` + `data: {...}` → emit `chat-error`

Other events (`content_block_start`, `content_block_stop`, `ping`) are ignored.

**Response headers read opportunistically** (any subset may be present):
- `anthropic-ratelimit-status` (`allowed` or `throttled`)
- `anthropic-ratelimit-requests-remaining` (numeric)
- `anthropic-ratelimit-tokens-remaining` (numeric)
- `anthropic-ratelimit-requests-reset` (ISO 8601 timestamp or seconds-from-now)
- `anthropic-ratelimit-tokens-reset` (same)
- `retry-after` (seconds)

From these we synthesize a `sessionWindow { usedFraction, resetsAt }` if we have enough info; otherwise we leave the UI's session-window row hidden.

### 6.2 OpenAI Chat Completions path (universal protocol)

**Endpoint:** `POST {base_url}/chat/completions`
**Headers:** `authorization: Bearer <key>`, `content-type: application/json`
**Body:**
```json
{
  "model": "<model_name>",
  "messages": [{"role": "user", "content": "..."}],
  "stream": true,
  "stream_options": {"include_usage": true}
}
```
**With reasoning effort:** add `"reasoning_effort": "low" | "medium" | "high"`.

**Unknown-param retry**: if the pre-stream response is a 400 with a JSON body whose `error.message` field contains the literal substring `"reasoning_effort"` (case-insensitive), we mark this model as `reasoningEffortUnsupported: true` in `modelPrefs` and retry **once** without the param. The UI then disables the reasoning-effort picker for that model and shows a "This model doesn't support reasoning effort" tooltip.

We do NOT silently retry on any other 400 — bad model name, malformed body, etc. surface as `bad_request` with the provider's message. We do NOT retry mid-stream errors (the response is 200 with `data: {"error": ...}` SSE events); those surface as `provider` errors with the stream's content preserved.

If the user later changes the model, the `reasoningEffortUnsupported` flag is per-model and resets when the model is re-saved.

**SSE parsing:**
- `data: {"choices": [{"delta": {"content": "..."}}]}` → emit `chat-chunk`
- `data: {"choices": [], "usage": {"prompt_tokens": N, "completion_tokens": M}}` → capture into final `usage`
- `data: [DONE]` → end of stream; emit `chat-done`

**Response headers read opportunistically** (varies wildly across OpenAI-compat providers):
- `x-ratelimit-remaining-requests`, `x-ratelimit-remaining-tokens`
- `x-ratelimit-reset-requests`, `x-ratelimit-reset-tokens`
- `x-quota-remaining`, `x-quota-reset`
- `retry-after`

If we have at least one of these, surface it; otherwise hide the row.

### 6.3 Test connection

- **Anthropic**: `POST {base_url}/v1/messages` with `max_tokens: 1`, `messages: [{role: "user", content: "hi"}]`, no stream. 200 → ok; 401 → `auth`; 5xx → `provider`; anything else → `bad_request` with the body as the message.
- **OpenAI-compat**: a tiered probe (the spec tried `GET /models` first, but many OpenAI-compat providers — vLLM, ollama's OpenAI shim, some enterprise gateways — don't implement `/models`, so the probe is more defensive):
  1. `GET {base_url}/models` (no body). 200 → ok. 401 → `auth`. **404 is also "ok"** — endpoint reached, no auth issue, just no `/models` index. We treat 200 and 404 as success.
  2. If `/models` returns 405/404, fall back to a one-shot `POST {base_url}/chat/completions` with `max_tokens: 1`, `messages: [{role: "user", content: "hi"}]`, no `stream`. This is one token of cost but works on every OpenAI-compat provider. 200 → ok; 401 → `auth`; 400 → the error body usually names the missing param, so we surface it.
  3. If the fallback also fails with a network error, return `network` and the error message.

The UI shows the test result with the kind icon: ✓ ok / ⚠ auth / ⚠ rate_limit / ⚠ network / ⚠ provider. The error message includes the provider's response body (truncated to 200 chars) so the user can debug.

### 6.4 Cancellation

- A `tokio::sync::oneshot::Sender<()>` is stored in a `DashMap<String, Sender>` keyed by `request_id` at the start of `chat_complete`.
- The entire `reqwest` request + SSE stream loop is wrapped in a `tokio::select!` that arms the cancel signal. On cancel, we drop the response body stream, remove the entry from the `DashMap`, and emit `chat-error { kind: "cancelled" }`.
- **Honesty**: cancellation is guaranteed at SSE event boundaries. Bytes already in the TCP buffer may still arrive at the OS, but they will not be parsed or emitted as `chat-chunk`s. The provider's server-side processing continues until the connection is closed (we close it by dropping the response body).
- `cancel_chat(request_id)` looks up the sender and fires it. Idempotent — calling `cancel_chat` after the request has already completed is a no-op (the sender is gone from the map).

### 6.5 Crate deps to add

- `eventsource-stream = "2"` — SSE framing
- `keyring = "3"` — OS keyring (Windows Credential Manager / Keychain / libsecret)
- `dashmap = "6"` — concurrent in-flight-request tracking
- `rand = "0.8"` — for any future randomness needs (e.g. request IDs are nanoid'd on the TS side; the Rust side doesn't need its own randomness today, but it's commonly pulled in transitively)

We do **not** add `tauri-plugin-stronghold` — see §4.2 for the rationale. The OS keyring is the secret store; Stronghold would be security theater on top.

`reqwest` and `tokio` are already in `Cargo.toml`. `tauri-plugin-stronghold` is **not** added, so we don't need to register it in `lib.rs`, and we don't need a `capabilities/default.json` for it. The existing Tauri command-handler pattern (Rust-side `#[tauri::command]` registered in `invoke_handler!`) is unchanged; new commands follow the same pattern and TS wrappers go in `src/lib/ipc.ts`.

## 7. UI Surface

### 7.1 Settings rebuild

Two-pane modal (760px wide). The existing `Settings.tsx` is replaced.

```
┌───── Settings (modal, 760px) ─────────────────────────────┐
│  Model Setup                                [X close]      │
│  ──────────────────────────────────────────────────────── │
│  ┌─ Saved models (260px) ─┐ ┌─ Editor (480px) ───────────┐ │
│  │ ● Sonnet 4 (work)     │ │ Label:    [Sonnet 4 (work)]│ │
│  │   claude-sonnet-4     │ │ Provider: [anthropic    ▾] │ │
│  │   🔒 key saved        │ │ Format:   [Anthropic Msgs▾]│ │
│  │                       │ │ Base URL: [https://api...  ]│ │
│  │ ○ GPT-4o (default)    │ │ Model:    [claude-sonnet-4 ]│ │
│  │   gpt-4o              │ │                             │ │
│  │   🔒 key saved        │ │ API Key: ●●●●●●●● [edit]   │ │
│  │                       │ │   (saved encrypted)         │ │
│  │ ○ DeepSeek V3         │ │                             │ │
│  │   deepseek-chat       │ │ ☐ Test connection           │ │
│  │   ⚠ no key            │ │                             │ │
│  │                       │ │ ┌─ Thinking ─────────────┐   │ │
│  │ [+ Add model]         │ │ │ ☐ Extended thinking    │   │ │
│  │                       │ │ │ Reasoning: [Medium  ▾] │   │ │
│  └───────────────────────┘ │ └────────────────────────┘   │ │
│                            │  [Delete]            [Save]  │ │
│                            └─────────────────────────────┘ │
└────────────────────────────────────────────────────────────┘
```

**Behaviors:**
- Left list: click row → loads that model into editor. `●` = active. `🔒 key saved` / `⚠ no key` status. `+ Add model` appends a new entry.
- Editor: changes are drafts until Save. Save writes atomically to `models.json` and the keyring.
- API key field is masked by default; "edit" reveals an input. Empty key + existing `hasKey` keeps the existing key. Non-empty key overwrites.
- **Plaintext key never round-trips to the frontend.** The `SavedModel` shape has `hasKey`, not `apiKey`.
- "Test connection" calls `test_saved_model(id)` → green check or red error with provider's error message.
- Thinking toggle: shown when `format === 'anthropic-messages'`. Persisted in `modelPrefs`.
- Reasoning effort picker: shown when `format === 'openai-chat-completions'`. Default: `medium`. Persisted.
- Provider preset autofill: `anthropic` and `openai` selections fill `baseUrl`. `openai-compatible` and `custom` require a manual base URL.
- Provider model suggestions: existing `PROVIDER_PRESETS.models` cycle next to the model input.

**"No key" UX**: when the user tries to send with a model that has `hasKey=false`, the chat composer disables the input with "Add an API key in Settings" + a button that opens Settings focused on that model.

### 7.2 Chat header

A 4-row slim panel above the messages (always visible while reading):

```
┌─ Chat header ──────────────────────────────────────────────┐
│  Model: Sonnet 4 (work)                       ▾            │
│  Last response  2,341 in  287 out                           │
│  Context used   12,400  /  Context window  200,000  [bar]   │
│  Session window 23% used · resets in 4h 12m                │
└──────────────────────────────────────────────────────────┘
```

- Click the model name → opens the same dropdown as the composer.
- "Session window" row: hidden if no provider returned rate-limit headers.
- Context bar: thin 1px line under the row, color-coded (green < 60%, yellow 60–85%, red > 85%).

### 7.3 Composer selector row

Inside the existing composer, above the textarea:

```
┌─ Selector row ─────────────────────────────────────────────┐
│  Model: [Sonnet 4 (work) ▾]   (anthropic)                │
│                                          [●○] Thinking   │
│                                          Reasoning [Med▾]│
└──────────────────────────────────────────────────────────┘
```

- `Thinking [●○]` toggle: shown only when `activeModel.format === 'anthropic-messages'`. Wired to `set_model_prefs` immediately.
- `Reasoning [Low/Medium/High ▾]`: shown only when `activeModel.format === 'openai-chat-completions'`. Wired to `set_model_prefs` immediately.
- Subtle `(provider)` label next to the model name, 10px white-30.

### 7.4 Disabled states

| State | Composer |
|---|---|
| No active model | "Add a model in Settings to start" + open button, input disabled |
| `activeModelId` set but no model with that id exists in the list (e.g. file was deleted, or the user changed the `models.json` externally) | "No active model" — same UI as the first row. Do not auto-pick a fallback. |
| Active model, no key (`SecretStore::has` returns `Ok(false)`) | "Add an API key in Settings" + open button, input disabled |
| Active model, key unreachable (`SecretStore::has` returns `Err(_)`, e.g. the OS credential store is locked or perms denied) | "API key unreachable — click to retry" + retry button. The retry calls `SecretStore::has` again; if it now succeeds, the composer un-disables. If it keeps failing, the user can "Reset secret store" from Settings, which calls `SecretStore::delete_all` and re-prompts to re-add keys. |
| Stream in progress | Input disabled, "Streaming..." placeholder |

### 7.5 Streaming flow

1. User hits send → optimistic `assistant` message bubble appended with `content: ""`.
2. `chat_complete` invoked; `requestId` is the message's `id`.
3. Each `chat-chunk` event appends the delta to the message's `content`. `MessageRenderer` (the existing rich-markdown pipeline) re-renders as content grows.
4. `chat-done` → token stats updated:
   - `lastIn = usage.input`
   - `lastOut = usage.output`
   - `cumulativeIn = max(cumulativeIn, lastIn)` (peak input across the session — not a sum; see §7.6)
   - `cumulativeOut += usage.output` (output is independent per turn; summing is correct)
   - `sessionWindow` updated if present
   - `Session.tokenStats` written into the `session.json` on next debounced save (per the prior spec's lifecycle manager).
5. `chat-error`:
   - `kind: "cancelled"` → optimistic message removed (silent — the user pressed cancel).
   - `kind: "auth"` → message replaced with "(auth failed: re-add API key)" + toast. Composer falls back to the "no key" disabled state.
   - `kind: "rate_limit"` → message replaced with "(rate limited — try again in N seconds)" + toast. Composer re-enables.
   - `kind: "network"` / `"provider"` / `"bad_request"` / `"internal"` → message replaced with the error text + toast. Composer re-enables.

### 7.6 Context counting (honest)

- `Context used` is **`lastIn`** — the input tokens reported for the most recent response. This is the actual server-side context for that turn (the model processed exactly that many tokens of input).
- `Context window` is the model's published max, looked up in the model catalog (`200_000` for Anthropic Sonnet/Opus, `128_000` for OpenAI gpt-4o, `200_000` for gpt-4.1, etc.). Models not in the catalog get a sane default of `128_000`.
- `Total in (cumulative)` is the running total in the chat header, but it is **not** the sum of `usage.input_tokens` per turn (that would double-count). It is `max(cumulativeIn, lastIn)` — the peak input size seen this session. The UI label is "Total input (peak)" to be honest.
- `Total out (cumulative)` is the **literal sum** of `output_tokens` across all turns (output tokens don't double-count; each is independent).

## 8. localStorage → keyring migration

On first launch with the new code:
1. The frontend reads `localStorage["harness_model_entries"]` once. If empty/missing → no-op, skip the rest.
2. For each entry, the TS side calls `save_saved_model(model, apiKey)` with the key. The Rust side:
   - Upserts the model in `models.json` by `id` (not append — see idempotency below).
   - Calls `SecretStore::set(model_id, api_key)` for each entry that has a non-empty key.
3. The frontend collects per-entry results. **If any single `save_saved_model` call fails**, the migration aborts: the localStorage entry is preserved (we don't delete it), and a toast fires: "Could not migrate saved models. Please try again from Settings." The next launch retries.
4. **Only after all entries succeed** does the frontend delete `localStorage["harness_model_entries"]`. After that, subsequent launches go directly to the keyring-backed store.
5. **Idempotency**: `save_saved_model` does an **upsert by `id`**, not an append. Re-running the migration on a partially-migrated state is safe — entries that already have a key in the keyring are overwritten with the same key (idempotent), entries that don't are added.

**Why not Stronghold**: the spec dropped Stronghold in §4.2. The migration target is the OS keyring, not a Stronghold vault.

## 9. Error Handling

The `ChatError.kind` discriminator (§5.5) drives the UI response. All paths show a toast AND update the message bubble (or remove it for `cancelled`).

| `kind` | UI response |
|---|---|
| `cancelled` | Optimistic message removed silently. |
| `auth` | Optimistic message replaced with "(auth failed: re-add API key)". Toast: "Invalid API key — re-add in Settings." Composer falls back to the "no key" disabled state. |
| `rate_limit` | Optimistic message replaced with "(rate limited — try again in Ns)". Toast: "Rate limited — try again in N seconds" (N from `Retry-After` if present, else generic). Composer re-enables. |
| `network` | Optimistic message replaced with the error text. Toast: "Network error — please retry." Composer re-enables. |
| `provider` (5xx) | Optimistic message replaced with "(provider error)". Toast: "Provider error — please retry." Composer re-enables. |
| `bad_request` (400) | Optimistic message replaced with the provider's error body (truncated to 200 chars). Toast: same text. Composer re-enables. |
| `internal` | Optimistic message replaced with "(internal error)". Toast: "Internal error — please retry." Composer re-enables. Logs go to stderr with `eprintln!`. |

**Settings / secrets errors** (not chat):

| Failure | UI response |
|---|---|
| `save_saved_model` fails (disk full) | Toast: "Could not save model"; no partial write (atomic rename in Rust) |
| `test_saved_model` returns `auth` | Inline red "Invalid API key" in the editor |
| `test_saved_model` returns `network` | Inline red "Could not reach <provider>" |
| `test_saved_model` returns other | Inline red with the truncated error body |
| `SecretStore::has` returns `Err(_)` on any model | That model is shown as "key unreachable" in the Settings list and the "API key unreachable" disabled state in the composer (§7.4). The "Reset secret store" button is offered. |
| Browser-only dev (`npm run dev`) tries to call a real provider | Toast: "Real LLM calls require the desktop app (Tauri). Run `npm run tauri:dev`." — providers reject `localhost:1420` CORS |

## 10. Dependencies

| Package | Type | Why |
|---|---|---|
| `eventsource-stream` | Rust | SSE framing |
| `keyring` | Rust | OS keyring — Windows Credential Manager / Keychain / libsecret |
| `dashmap` | Rust | In-flight request tracking |
| `rand` | Rust | (transitive; not directly used) |

No new runtime TS deps. The existing `nanoid` is used for `requestId`s.

We deliberately do **not** add `tauri-plugin-stronghold` — see §4.2.

## 11. Testing

### 11.1 Rust unit tests

The `SecretStore` trait is the boundary. The production `KeyringStore` is not unit-tested directly (it would need a real OS keyring) — instead:

- `InMemoryStore` tests: `set/get/has/delete` round-trip, `has` returns `Ok(false)` after `delete`, `delete` is idempotent.
- The trait contract: a function that takes `&dyn SecretStore` and exercises all four operations is unit-tested against `InMemoryStore`. Any future impl must pass the same test.
- `models::resolve` returns a `ResolvedModel` with the API key populated from the trait (uses `InMemoryStore` in tests).
- `parse_anthropic_sse_chunk` against 6 golden-file fixtures: `message_start` with `input_tokens`, `content_block_delta` with text, `message_delta` with `output_tokens`, `message_stop`, `error` event, multi-line event. The `message_start` case is critical — it's where `input_tokens` is first reported.
- `parse_openai_sse_chunk` against 6 golden-file fixtures: content delta, usage chunk, `[DONE]`, 400 pre-stream response with `"reasoning_effort"` in the error message, mid-stream error SSE event, multi-line event.
- `extract_session_window_headers` against headers from Anthropic's documented format and from a synthetic OpenAI-compat provider.
- `format_anthropic_messages` and `format_openai_messages` against a known `SessionMessage` fixture (verifies role mapping, content array vs string).
- `reasoning_effort` retry logic: given a 400 response body containing `"reasoning_effort"`, retry fires; given a 400 with no mention of `reasoning_effort`, no retry.

### 11.2 TS unit tests (Vitest)

- `extractMentions` (already covered, unchanged)
- `formatMessagesForProvider` — the conversion from internal `SessionMessage` to provider-specific shape
- The one-time `migrateLocalStorageEntries` function — given a `localStorage` payload, produces the right `save_saved_model` calls in order, aborts on the first failure, and only deletes `localStorage` after the full batch succeeds
- The `ChatError` dispatch — given each `kind`, the UI handler produces the right toast and message-bubble replacement

### 11.3 Smoke

Manual end-to-end (this is the only way to test real LLM streaming):

1. Open the desktop app (`npm run tauri:dev`).
2. Settings → Add Anthropic model with a real key. Save.
3. Verify the key doesn't appear in `localStorage` (open devtools and check).
4. Back to chat → send a message → verify streaming chunks arrive, message bubble updates, "Last response" row fills in.
5. Open Settings → reveal the key field → verify it shows `●●●●●●●` (the actual key is never returned from the backend).
6. Switch to a second saved model. Send a message. Switch back. Send a message. Verify each model has its own token stats.
7. Close + relaunch. Verify model + key + prefs persist (because the key is in the OS keyring).
8. Open `session.json` and verify the assistant message + token stats are persisted.

## 12. Risks

- **OS keyring entry loss = API key loss.** If the OS credential store is corrupted (or the user reinstalls their OS without backing up), the keyring entries are unreadable. The "Reset secret store" button in Settings loses all keys; the user re-enters them. Same threat model as any "sealed by your OS" secret.
- **SSE parsing is provider-specific.** Anthropic and OpenAI have slightly different shapes; the golden-file tests pin the contract. If a provider changes shape, the test fails first.
- **Browser-only dev mode can't call real providers.** Providers reject `http://localhost:1420` CORS. Documented in §9. The `npm run dev` path remains usable for non-LLM UI work.
- **Long-running streams + app close.** Stream is killed; partial content persists in `session.json` along with `tokenStats`. Restored on next launch.
- **Provider rate-limit headers are flaky.** Different providers use different headers. The opportunistic parser covers the most common cases; the rest show as missing in the UI. Fine.
- **OpenAI-compat providers may reject `reasoning_effort`.** The client silently retries without it (per §6.2) and marks the model as `reasoningEffortUnsupported`. If a provider also rejects `stream_options.include_usage`, the final usage block is missing from the stream; the UI shows `—` in the Last-response row.
- **No tokenizer for local estimation.** We rely on real `usage` numbers from the API. The cumulative number is the **peak** input, not a sum (see §7.6).
- **Migration is one-shot with full abort on any failure.** If any entry's `save_saved_model` fails during the localStorage → keyring migration, the whole batch aborts, localStorage is preserved, and the next launch retries. No partial state.
- **Model catalog is hardcoded.** A new model released by a provider won't appear in the cycle until we update the catalog. Users can always type a custom `modelName` in the editor.
- **CSP is `null` in `tauri.conf.json`.** This spec ships real LLM credentials. While CSP doesn't affect local IPC, a permissive CSP is a real risk surface for the React app if a future XSS is introduced. **Out of scope to fix in this spec**, but flagged for a follow-up. Recommended: `"csp": "default-src 'self'; img-src 'self' https: data:; connect-src 'self' ipc: https://ipc.localhost"` (or a similar restrictive policy) before any production release.
- **Existing dead code to remove in this spec's implementation:**
  - `src/components/Settings.tsx:81` does `localStorage.setItem("harness_model_entries", JSON.stringify(entries))` — removed.
  - `src-tauri/src/commands.rs` has `save_api_key`, `get_api_keys`, `delete_api_key` IPC commands + `AppState::api_keys: Mutex<HashMap<String, String>>` (in-memory only) — removed; superseded by the new keyring-backed IPC.
  - `src/lib/store.ts:113` has `settingsTab: "general" | "models" | "keys"` which the current UI doesn't use — removed; the new two-pane Settings has no tabs.
