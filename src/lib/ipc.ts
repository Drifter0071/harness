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
}

/** Snapshot of the agent personality that drives a session. */
export interface AgentSnapshot {
  id: string;
  name: string;
  role: AgentRole;
  systemPrompt: string;
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
  createdAt: string;
  updatedAt: string;
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

export interface AgentPersonality {
  id: string;
  name: string;
  role: AgentRole;
  systemPrompt: string;
  /** Builtin personas are editable but cannot be deleted. */
  builtin: boolean;
}

export interface AgentRoster {
  version: number;
  maxAgentsPerSession: number;
  defaultAgentId: string;
  agents: AgentPersonality[];
}

export interface AgentTurnRequest {
  requestId: string;
  sessionId: string;
  modelId: string;
  message: string;
  actionMode: ActionMode;
  goalMode: boolean;
  workspace: string;
}

// Agent loop events (Rust → TS), routed by `requestId`.

export interface AgentTurnStart {
  requestId: string;
}

export interface AgentChunk {
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

  // Agent roster (Agent Manager window)
  loadAgentRoster: (projectPath: string) =>
    invoke<AgentRoster>("load_agent_roster", { projectPath }),
  saveAgentRoster: (projectPath: string, roster: AgentRoster) =>
    invoke<AgentRoster>("save_agent_roster", { projectPath, roster }),

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
