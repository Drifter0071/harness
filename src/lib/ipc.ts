import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// ── Type Definitions ──

export interface DirEntry {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
}

export interface CommandOutput {
  stdout: string;
  stderr: string;
  exit_code: number;
}

export interface AgentInfo {
  id: string;
  name: string;
  role: AgentRole;
  status: AgentStatus;
  message_count: number;
}

export interface AgentMessage {
  role: string;
  content: string;
  timestamp: string;
}

export interface SearchResult {
  title: string;
  url: string;
  snippet: string;
}

export interface Mention {
  path: string;
  lineStart?: number;
  lineEnd?: number;
}

export interface SessionMessage {
  id: string;
  role: string;
  content: string;
  timestamp: string;
  mentions?: Mention[];
  agentId?: string;
  agentName?: string;
  /** Chain-of-thought captured for thinking-capable models. The
   *  frontend renders this in a collapsible "Thinking" block above
   *  the message body when present. */
  reasoning?: string;
}

export type ActionMode = "auto" | "askRisky" | "askAll";

export type ToolActivityStatus = "ran" | "denied" | "error";

/** One persisted tool-execution record (spec §4.1 `activities`). */
export interface ToolActivity {
  id: string;
  tool: string;
  summary: string;
  status: ToolActivityStatus;
  /** ISO 8601 UTC string. */
  timestamp: string;
  /** requestId of the agent turn that ran this tool (absent on
   *  non-tool records like turn errors). Used to attach tool calls
   *  to their assistant message in the chat. */
  requestId?: string;
  /** Raw JSON arguments of the call, for display in the chat. */
  arguments?: unknown;
}

/** Snapshot of the agent personality that drives a session. After the
 *  global-personalities change, only the `personalityId` is stored
 *  on the session; the rest of the fields (name, role, system prompt,
 *  model id) come from the global personality roster at session
 *  start. Old session files (v1) embedded the full snapshot; the
 *  legacy `id` field is still read on the TS side so rehydrated
 *  sessions can match back to a personality. */
export interface AgentSnapshot {
  personalityId: string;
  /** Legacy v1 field, kept for back-compat when rehydrating. */
  id?: string;
  name?: string;
  role?: AgentRole;
  systemPrompt?: string;
}

export interface Session {
  version: number;
  id: string;
  title: string;
  projectPath: string;
  createdAt: string;
  updatedAt: string;
  agent: AgentSnapshot;
  goalMode: boolean;
  actionMode: ActionMode;
  messages: SessionMessage[];
  activities: ToolActivity[];
  /** Null until the first assistant response arrives (spec §4.5b). */
  tokenStats?: {
    lastIn: number;
    lastOut: number;
    cumulativeIn: number;
    cumulativeOut: number;
    sessionWindowUsed: number | null;
    sessionWindowResetsAt: string | null;
  } | null;
  /**
   * Disk-based completion signal written by the agent loop on every
   * terminal transition. When the streaming events don't reach the
   * WebView (the cause of the "responses only show after refresh"
   * bug), the file watcher reads this and applies the terminal state.
   */
  lastTurn?: {
    requestId: string;
    status: "done" | "error" | "cancelled";
    at: string;
    /** Mirrored from the agent loop so the file-watcher fallback
     *  (which fixes the "responses only show after refresh" bug)
     *  can apply the captured chain-of-thought when the streaming
     *  events never reach the WebView. */
    reasoning?: string;
  } | null;
}

export interface RecentProject {
  path: string;
  name: string;
  lastOpened: string;
}

/**
 * Lightweight metadata for a saved session, used by the Sessions panel.
 * The full `Session` (with messages) is loaded separately via
 * `loadSessionById` when the user opens one.
 *
 * Wire shape — keep this in sync with whatever the LLM-support work
 * ships. `id` should be stable across renames of the project.
 */
export interface SessionSummary {
  id: string;
  projectPath: string;
  projectName: string;
  createdAt: string;
  updatedAt: string;
  messageCount: number;
  /**
   * Human-readable label, typically the first user message truncated
   * to ~40 chars. Falls back to a generated label when there's no
   * first message yet.
   */
  title: string;
  agentId?: string;
}

export type AgentRole =
  | "Orchestrator"
  | "Coder"
  | "Architect"
  | "Researcher"
  | "HealthMonitor";

export type AgentStatus =
  | "Idle"
  | "Working"
  | "WaitingForInput"
  | { Error: string }
  | "Stopped";

// ── Saved models + prefs + chat (Phase 2.0) ──
// Wire shapes mirror the Rust structs in src-tauri/src/models.rs and
// src-tauri/src/llm_client.rs. Keys NEVER round-trip to the frontend;
// `hasKey` / `keyUnreachable` are derived on the Rust side.

export interface SavedModel {
  id: string;
  label: string;
  provider: string;
  format: string;
  baseUrl: string;
  modelName: string;
  hasKey: boolean;
  /** true when the OS keyring returned an error while checking this row. */
  keyUnreachable: boolean;
  /** User-set context window override (tokens). When omitted, the
   *  frontend falls back to the catalog + inference. */
  contextWindow?: number | null;
  /** User-set reasoning capability. When omitted, inferred from the
   *  most recent probe (if any) and then a name/format heuristic. */
  reasoningCapability?: ReasoningKind | null;
  /** Subset of effort levels the model supports. When omitted or
   *  empty, the canonical low/medium/high set is shown. */
  reasoningLevels?: Array<"low" | "medium" | "high"> | null;
  /** Most recent capability probe result (Anthropic Models API hit
   *  or a smoke test). Purely informational; used to seed the
   *  inference chain and to render "Last checked" in the UI. */
  capabilityProbe?: CapabilityProbe | null;
  createdAt: string;
  updatedAt: string;
}

export type ReasoningKind = "none" | "thinking" | "effort";

export interface CapabilityProbe {
  probedAt: string;
  source: string;
  contextWindow?: number | null;
  reasoning?: ReasoningKind | null;
  reasoningLevels?: Array<"low" | "medium" | "high"> | null;
  note?: string | null;
}

/** Input shape for save_saved_model. `apiKey` travels as a separate
 *  optional arg (three states: non-empty = overwrite, "" = keep, omitted
 *  = don't touch). */
export interface SavedModelInput {
  id: string;
  label: string;
  provider: string;
  format: string;
  baseUrl: string;
  modelName: string;
  /** User-set context window override. `null`/omitted = no override. */
  contextWindow?: number | null;
  /** User-set reasoning capability override. `null`/omitted = no override. */
  reasoningCapability?: ReasoningKind | null;
  /** User-set effort-level subset. `null`/omitted = no override. */
  reasoningLevels?: ReasoningEffort[] | null;
  /** Most recent capability probe result to persist. */
  capabilityProbe?: CapabilityProbe | null;
}

export type ReasoningEffort = "low" | "medium" | "high";

export interface ModelPrefs {
  thinkingOn: boolean;
  reasoningEffort: ReasoningEffort;
}

export interface TestResult {
  ok: boolean;
  kind: string;
  message: string;
}

export interface ChatMessage {
  role: string;
  content: string;
}

export interface ChatRequest {
  requestId: string;
  modelId: string;
  messages: ChatMessage[];
  thinkingOn: boolean;
  reasoningEffort?: ReasoningEffort;
}

export interface ChatChunk {
  requestId: string;
  delta: string;
}

/** Chain-of-thought delta, emitted alongside `chat-chunk` for
 *  thinking-capable models. The frontend buffers these into the
 *  hidden "Thinking" block on the assistant message. */
export interface ChatReasoning {
  requestId: string;
  delta: string;
}

export interface ChatUsage {
  input: number;
  output: number;
}

export interface ChatSessionWindow {
  usedFraction: number;
  resetsAt: string;
}

export interface ChatDone {
  requestId: string;
  content: string;
  /** Full chain-of-thought captured for thinking-capable models.
   *  Empty when the model didn't emit reasoning. */
  reasoning?: string;
  usage: ChatUsage;
  sessionWindow?: ChatSessionWindow;
}

export type ChatErrorKind =
  | "cancelled"
  | "auth"
  | "rate_limit"
  | "network"
  | "provider"
  | "bad_request"
  | "internal";

export interface ChatError {
  requestId: string;
  kind: ChatErrorKind;
  error: string;
  retryAfterSeconds?: number;
}

// ── Agent roster + turn loop (agent-sessions spec) ──

/** Default system prompt for a role, as seeded on first contact.
 *  Mirrors `AgentRole::default_prompt()` on the Rust side. */
export const DEFAULT_PROMPTS: Record<AgentRole, string> = {
  Orchestrator:
    "You are the orchestrator. You coordinate sub-agents, break down goals into tasks, and synthesize results.",
  Coder: "You are a senior software engineer. You write clean, efficient code and follow best practices.",
  Architect:
    "You are a system architect. You design systems, evaluate trade-offs, and create technical specifications.",
  Researcher:
    "You are a technical researcher. You gather information, analyze options, and provide well-sourced recommendations.",
  HealthMonitor:
    "You are a health monitor. You watch for stalled agents, hallucinated outputs, and loops. You flag issues and suggest corrections.",
};

/** Global agent personality. The roster is stored at
 *  `<data_dir>/Harness/personalities.json` and shared across every
 *  project the user opens. */
export interface AgentPersonality {
  id: string;
  name: string;
  role: AgentRole;
  systemPrompt: string;
  /** Saved-model id (`SavedModel.id`) to use when this personality
   *  runs. `null` means "use the active chat model". */
  modelId: string | null;
  /** Builtin personalities are seeded on first contact (one per
   *  `AgentRole`) and are editable but cannot be deleted. */
  builtin: boolean;
}

export interface PersonalityRoster {
  version: number;
  defaultPersonalityId: string;
  personalities: AgentPersonality[];
}

export interface AgentTurnRequest {
  requestId: string;
  sessionId: string;
  modelId: string;
  message: string;
  actionMode: ActionMode;
  goalMode: boolean;
  workspace: string;
  /** Optional reasoning-effort level forwarded to the provider so
   *  reasoning-capable models return chain-of-thought. `undefined`
   *  for non-reasoning models or when the user has reasoning off. */
  reasoningEffort?: ReasoningEffort;
}

// Agent loop events (Rust → TS), routed by `requestId`.

export interface AgentTurnStart {
  requestId: string;
}

export interface AgentChunk {
  requestId: string;
  delta: string;
}

/** Chain-of-thought delta for the agent loop, sibling of `AgentChunk`. */
export interface AgentReasoning {
  requestId: string;
  delta: string;
}

export interface AgentToolStart {
  requestId: string;
  callId: string;
  name: string;
  arguments: unknown;
}

export interface AgentToolResult {
  requestId: string;
  callId: string;
  name: string;
  ok: boolean;
  summary: string;
  status: ToolActivityStatus;
}

export interface AgentApprovalRequest {
  requestId: string;
  approvalId: string;
  name: string;
  summary: string;
}

export interface AgentApprovalResult {
  requestId: string;
  approvalId: string;
  allowed: boolean;
}

export interface AgentDone {
  requestId: string;
  content: string;
  /** Full chain-of-thought captured for thinking-capable models. */
  reasoning?: string;
  usage: ChatUsage;
}

export type AgentErrorKind =
  | "cancelled"
  | "auth"
  | "rate_limited"
  | "network"
  | "provider"
  | "bad_request"
  | "internal"
  | "loop_cap"
  | "stalled";

export interface AgentError {
  requestId: string;
  kind: AgentErrorKind;
  message: string;
}

// ── IPC Bridge ──

export const ipc = {
  // Workspace
  pickProjectFolder: () => invoke<string | null>("pick_project_folder"),
  readFile: (path: string) => invoke<string>("read_file", { path }),
  writeFile: (path: string, content: string) =>
    invoke<void>("write_file", { path, content }),
  listDirectory: (path: string) =>
    invoke<DirEntry[]>("list_directory", { path }),
  searchFiles: (path: string, pattern: string) =>
    invoke<string[]>("search_files", { path, pattern }),
  createDirectory: (path: string) =>
    invoke<void>("create_directory", { path }),

  // CLI
  executeCommand: (command: string, args: string[], cwd?: string) =>
    invoke<CommandOutput>("execute_command", { command, args, cwd }),
  killProcess: (pid: number) =>
    invoke<void>("kill_process", { pid }),

  // Agents
  spawnAgent: (name: string, role: AgentRole, systemPrompt: string) =>
    invoke<string>("spawn_agent", { name, role, systemPrompt }),
  sendAgentMessage: (agentId: string, content: string) =>
    invoke<AgentMessage>("send_agent_message", { agentId, content }),
  getAgentStatus: (agentId: string) =>
    invoke<AgentInfo>("get_agent_status", { agentId }),
  listActiveAgents: () =>
    invoke<AgentInfo[]>("list_active_agents"),
  stopAgent: (agentId: string) =>
    invoke<void>("stop_agent", { agentId }),
  healthCheck: () =>
    invoke<Record<string, AgentStatus>>("health_check"),

  // Search
  webSearch: (query: string) =>
    invoke<SearchResult[]>("web_search", { query }),

  // Saved models + per-model prefs (Phase 2.0)
  listSavedModels: () => invoke<SavedModel[]>("list_saved_models"),
  saveSavedModel: (model: SavedModelInput, apiKey?: string) =>
    invoke<SavedModel>("save_saved_model", { model, apiKey }),
  deleteSavedModel: (id: string) =>
    invoke<void>("delete_saved_model", { id }),
  getActiveModel: () => invoke<string | null>("get_active_model"),
  setActiveModel: (id: string | null) =>
    invoke<void>("set_active_model", { id }),
  getModelPrefs: (id: string) =>
    invoke<ModelPrefs>("get_model_prefs", { id }),
  setModelPrefs: (id: string, prefs: ModelPrefs) =>
    invoke<void>("set_model_prefs", { id, prefs }),
  testSavedModel: (id: string) =>
    invoke<TestResult>("test_saved_model", { id }),
  probeSavedModel: (id: string) =>
    invoke<CapabilityProbe>("probe_saved_model", { id }),
  resetSecretStore: () => invoke<void>("reset_secret_store"),

  // Chat streaming (Phase 2.0)
  chatComplete: (request: ChatRequest) =>
    invoke<void>("chat_complete", { request }),
  cancelChat: (requestId: string) =>
    invoke<void>("cancel_chat", { requestId }),

  // Sessions + recents + file listing (agent-sessions spec)
  listSessions: (projectPath: string) =>
    invoke<SessionSummary[]>("list_sessions", { projectPath }),
  loadActiveSession: (projectPath: string) =>
    invoke<Session | null>("load_active_session", { projectPath }),
  createSession: (projectPath: string, agent?: AgentSnapshot) =>
    invoke<Session>("create_session", { projectPath, agent }),
  loadSession: (projectPath: string, sessionId: string) =>
    invoke<Session | null>("load_session", { projectPath, sessionId }),
  saveSession: (projectPath: string, session: Session) =>
    invoke<Session>("save_session", { projectPath, session }),
  watchSession: (workspacePath: string) =>
    invoke<void>("watch_session", { workspacePath }),
  renameSession: (projectPath: string, sessionId: string, title: string) =>
    invoke<SessionSummary>("rename_session", { projectPath, sessionId, title }),
  deleteSession: (projectPath: string, sessionId: string) =>
    invoke<void>("delete_session", { projectPath, sessionId }),
  setActiveSession: (projectPath: string, sessionId: string) =>
    invoke<void>("set_active_session", { projectPath, sessionId }),
  listRecentProjects: () => invoke<RecentProject[]>("list_recent_projects"),
  forgetRecentProject: (path: string) =>
    invoke<void>("forget_recent_project", { path }),
  touchRecentProject: (projectPath: string) =>
    invoke<RecentProject>("touch_recent_project", { projectPath }),
  listFilesRecursive: (projectPath: string) =>
    invoke<string[]>("list_files_recursive", { projectPath }),

  // Global agent personalities (Agent Manager window). The roster
  // lives at <data_dir>/Harness/personalities.json and is shared
  // across every project. `getPersonality` falls back to the roster
  // default on the Rust side when the id is unknown.
  loadPersonalities: () => invoke<PersonalityRoster>("load_personalities"),
  savePersonalities: (roster: PersonalityRoster) =>
    invoke<PersonalityRoster>("save_personalities", { roster }),
  getPersonality: (id: string) =>
    invoke<AgentPersonality>("get_personality", { id }),

  // Agent turn loop
  runAgentTurn: (req: AgentTurnRequest) =>
    invoke<void>("run_agent_turn", {
      requestId: req.requestId,
      sessionId: req.sessionId,
      modelId: req.modelId,
      message: req.message,
      actionMode: req.actionMode,
      goalMode: req.goalMode,
      workspace: req.workspace,
      reasoningEffort: req.reasoningEffort,
    }),
  respondApproval: (requestId: string, allow: boolean) =>
    invoke<void>("respond_approval", { requestId, allow }),
  cancelAgent: (requestId: string) =>
    invoke<void>("cancel_agent", { requestId }),
};

// ── Chat streaming events (Rust → TS) ──
// The Rust side emits exactly one terminal event per requestId
// (`chat-done` or `chat-error`), with all `chat-chunk` events before it.

export const chatEvents = {
  onChunk: (fn: (e: ChatChunk) => void) =>
    listen<ChatChunk>("chat-chunk", (e) => fn(e.payload)),
  onReasoning: (fn: (e: ChatReasoning) => void) =>
    listen<ChatReasoning>("chat-reasoning", (e) => fn(e.payload)),
  onDone: (fn: (e: ChatDone) => void) =>
    listen<ChatDone>("chat-done", (e) => fn(e.payload)),
  onError: (fn: (e: ChatError) => void) =>
    listen<ChatError>("chat-error", (e) => fn(e.payload)),
};

// ── Agent loop events (Rust → TS) ──
// The Rust side emits exactly one terminal event per requestId
// (`agent-done` or `agent-error`), with all other events before it.

export const agentEvents = {
  onTurnStart: (fn: (e: AgentTurnStart) => void) =>
    listen<AgentTurnStart>("agent-turn-start", (e) => fn(e.payload)),
  onChunk: (fn: (e: AgentChunk) => void) =>
    listen<AgentChunk>("agent-chunk", (e) => fn(e.payload)),
  onReasoning: (fn: (e: AgentReasoning) => void) =>
    listen<AgentReasoning>("agent-reasoning", (e) => fn(e.payload)),
  onToolStart: (fn: (e: AgentToolStart) => void) =>
    listen<AgentToolStart>("agent-tool-start", (e) => fn(e.payload)),
  onToolResult: (fn: (e: AgentToolResult) => void) =>
    listen<AgentToolResult>("agent-tool-result", (e) => fn(e.payload)),
  onApprovalRequest: (fn: (e: AgentApprovalRequest) => void) =>
    listen<AgentApprovalRequest>("agent-approval-request", (e) => fn(e.payload)),
  onApprovalResult: (fn: (e: AgentApprovalResult) => void) =>
    listen<AgentApprovalResult>("agent-approval-result", (e) => fn(e.payload)),
  onDone: (fn: (e: AgentDone) => void) =>
    listen<AgentDone>("agent-done", (e) => fn(e.payload)),
  onError: (fn: (e: AgentError) => void) =>
    listen<AgentError>("agent-error", (e) => fn(e.payload)),
};
