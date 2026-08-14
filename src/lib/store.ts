import { create } from "zustand";

// ── Domain types (mirror the Rust shapes in src-tauri/src/session.rs) ──

export interface Mention {
  path: string;
  lineStart?: number;
  lineEnd?: number;
}

/** A draft queued behind the in-flight generation. Sent automatically
 *  (FIFO) when the current stream/agent turn ends. Ephemeral — never
 *  persisted with the session. */
export interface QueuedMessage {
  id: string;
  content: string;
  mentions?: Mention[];
}

export interface SessionMessage {
  id: string;
  role: string;
  content: string;
  /** ISO 8601 UTC string. */
  timestamp: string;
  mentions?: Mention[];
  agentId?: string;
  agentName?: string;
  /** Chain-of-thought captured for thinking-capable models. The
   *  frontend renders this in a collapsible "Thinking" block above
   *  the message body when present. */
  reasoning?: string;
}

export interface Session {
  version: number;
  id: string;
  title: string;
  projectPath: string;
  createdAt: string;
  updatedAt: string;
  agent: import("./ipc").AgentSnapshot;
  goalMode: boolean;
  commandMode: import("./ipc").CommandMode;
  messages: SessionMessage[];
  activities: import("./ipc").ToolActivity[];
  /** Null until the first assistant response arrives (spec §4.5b). */
  tokenStats?: TokenStats | null;
  /** Disk-based completion signal written by the agent loop. */
  lastTurn?: {
    requestId: string;
    status: "done" | "error" | "cancelled";
    at: string;
    /** Mirrored reasoning from the agent loop. */
    reasoning?: string;
  } | null;
}

/** Per-session token telemetry (spec §4.5). `cumulativeIn` is the PEAK
 *  input size (`max(cumulativeIn, lastIn)`), not a sum. */
export interface TokenStats {
  lastIn: number;
  lastOut: number;
  cumulativeIn: number;
  cumulativeOut: number;
  sessionWindowUsed: number | null;
  sessionWindowResetsAt: string | null;
}

export interface RecentProject {
  path: string;
  name: string;
  /** ISO 8601 UTC string. */
  lastOpened: string;
}

// ── Mention popover state ──

export interface MentionState {
  open: boolean;
  query: string;
  results: string[];
  selectedIndex: number;
}

const emptyMention: MentionState = {
  open: false,
  query: "",
  results: [],
  selectedIndex: 0,
};

// ── Tool calls shown in the chat ──

/** One tool call rendered as a block on its assistant message. Live
 *  events create `running` records; `agent-tool-result` finalizes them.
 *  On session load the records are rebuilt from persisted activities. */
export interface ToolCallRecord {
  /** Backend callId (or the activity id when rebuilt from disk). */
  id: string;
  name: string;
  /** Pretty JSON of the call arguments (undefined when unavailable). */
  arguments?: string;
  status: "running" | "ran" | "denied" | "error";
  summary?: string;
}

// ── Settings State ──

export interface ModelConfig {
  provider: string;
  model: string;
  apiKey: string;
  baseUrl?: string;
}// ── Store ──

interface HarnessState {
  // Workspace
  workspacePath: string | null;
  workspaceStatus: "noProject" | "loading" | "ready";
  setWorkspacePath: (path: string | null) => void;
  setWorkspaceStatus: (s: "noProject" | "loading" | "ready") => void;

  // Chat / session
  messages: SessionMessage[];
  sessionLoaded: boolean;
  appendMessage: (msg: SessionMessage) => void;
  setMessages: (msgs: SessionMessage[]) => void;
  clearMessages: () => void;
  updateMessage: (id: string, patch: Partial<SessionMessage>) => void;
  /** Streaming-friendly append: `content += delta` for the message. */
  appendToMessage: (id: string, delta: string) => void;
  /** Streaming-friendly append for the chain-of-thought buffer. */
  appendReasoning: (id: string, delta: string) => void;
  removeMessage: (id: string) => void;

  // Persisted tool activities of the active session (live feed source)
  activities: import("./ipc").ToolActivity[];
  setActivities: (list: import("./ipc").ToolActivity[]) => void;
  appendActivity: (activity: import("./ipc").ToolActivity) => void;

  // Tool calls rendered inline in the chat, keyed by assistant message
  // id (`agent-<requestId>`). Kept separate from `messages` so the
  // persisted session file stays on the spec's activities-only shape.
  toolCalls: Record<string, ToolCallRecord[]>;
  setToolCalls: (map: Record<string, ToolCallRecord[]>) => void;
  appendToolCall: (messageId: string, call: ToolCallRecord) => void;
  updateToolCall: (
    messageId: string,
    callId: string,
    patch: Partial<ToolCallRecord>
  ) => void;

  // Session metadata (independent of messages — useful for "created at")
  sessionMeta: { createdAt: string; updatedAt: string } | null;
  setSessionMeta: (m: { createdAt: string; updatedAt: string } | null) => void;

  // Agent session options (composer options row; persisted with the session)
  commandMode: import("./ipc").CommandMode;
  setActionMode: (mode: import("./ipc").CommandMode) => void;
  goalMode: boolean;
  setGoalMode: (on: boolean) => void;
  /** requestId of the in-flight agent turn, or null when idle. */
  agentRequestId: string | null;
  setAgentRequestId: (id: string | null) => void;
  /** Pending tool approval shown in the composer, or null. */
  pendingApproval: {
    requestId: string;
    approvalId: string;
    name: string;
    summary: string;
  } | null;
  setPendingApproval: (
    p: {
      requestId: string;
      approvalId: string;
      name: string;
      summary: string;
    } | null
  ) => void;
  /** Global agent personality roster; loaded once on app mount. */
  personalities: import("./ipc").PersonalityRoster | null;
  setPersonalities: (
    roster: import("./ipc").PersonalityRoster | null,
  ) => void;
  agentManagerOpen: boolean;
  setAgentManagerOpen: (open: boolean) => void;

  // Saved models + active model + telemetry (Phase 2.0)
  savedModels: import("./ipc").SavedModel[];
  setSavedModels: (list: import("./ipc").SavedModel[]) => void;
  activeModelId: string | null;
  setActiveModelId: (id: string | null) => void;
  /** Prefs for the ACTIVE model (loaded from the backend on switch). */
  modelPrefs: import("./ipc").ModelPrefs;
  setModelPrefs: (prefs: import("./ipc").ModelPrefs) => void;
  tokenStats: TokenStats | null;
  setTokenStats: (s: TokenStats | null) => void;
  /** requestId of the in-flight stream, or null when idle. */
  streamingRequestId: string | null;
  setStreamingRequestId: (id: string | null) => void;
  /** Drafts queued while a generation is in flight; the first item is
   *  auto-sent when the current generation ends (FIFO). */
  pendingMessages: QueuedMessage[];
  /** Push a draft onto the queue. `atFront` (force-send) puts it ahead
   *  of everything already queued so it becomes the next generation. */
  enqueueMessage: (msg: QueuedMessage, atFront?: boolean) => void;
  removeQueuedMessage: (id: string) => void;
  updateQueuedMessage: (id: string, patch: Partial<QueuedMessage>) => void;
  clearPendingMessages: () => void;
  /** When set, opening Settings pre-selects this model in the editor. */
  settingsFocusModelId: string | null;
  setSettingsFocusModelId: (id: string | null) => void;

  // Subagent tiling (tiling spec).
  //
  // The orchestrator's chat is conversational; subagent windows are
  // read-only viewers (live stream, reasoning, tool calls, approval
  // card, stop button). The user CANNOT message subagents from the
  // UI — only the Health Agent (via the `subagent_message` tool) can
  // push messages to a subagent's inbox, and the loop drains that
  // inbox at the top of every round.
  //
  // State shape:
  //   - `subagents`: live registry mirror (re-synced via
  //     `list_subagents` on WebView reload, then patched by
  //     `agent-turn-start` + terminal events). Keyed by
  //     `sub-<uuid>` request id.
  //   - `subagentWindows`: per-window stream state (placeholder id,
  //     content, reasoning, tool calls, approval card, parent
  //     orchestrator request id). Keyed by request id.
  //   - `chatRatio`: chat column fraction locked at the FIRST
  //     subagent spawn — never auto-resizes after. The chat column
  //     width is fixed from that moment on.
  //   - `healthInbox`: incoming Health Agent messages per subagent,
  //     surfaced in the read-only chat as a "Health Monitor" bubble.
  subagents: import("./ipc").SubagentRecord[];
  upsertSubagent: (rec: import("./ipc").SubagentRecord) => void;
  removeSubagent: (requestId: string) => void;
  setSubagents: (records: import("./ipc").SubagentRecord[]) => void;
  /** Wipe the entire subagent registry + every per-window state. Called
   *  when a new orchestrator turn starts — past subagent viewers close
   *  in unison so the right region resets for the new round. */
  clearAllSubagents: () => void;
  /** Per-window stream state. `recordId` keys the entry to the
   *  registry mirror so the renderer can re-key on resync. */
  subagentWindows: Record<
    string,
    {
      record: import("./ipc").SubagentRecord;
      /** id of the streaming assistant placeholder (`sub-<requestId>`). */
      placeholderId: string;
      content: string;
      reasoning: string;
      toolCalls: ToolCallRecord[];
      /** Pending tool approval surfaced inline in the read-only viewer. */
      approval:
        | {
            approvalId: string;
            name: string;
            summary: string;
          }
        | null;
    }
  >;
  ensureSubagentWindow: (rec: import("./ipc").SubagentRecord) => void;
  appendSubagentContent: (requestId: string, delta: string) => void;
  appendSubagentReasoning: (requestId: string, delta: string) => void;
  appendSubagentToolCall: (requestId: string, call: ToolCallRecord) => void;
  updateSubagentToolCall: (
    requestId: string,
    callId: string,
    patch: Partial<ToolCallRecord>,
  ) => void;
  setSubagentApproval: (
    requestId: string,
    approval:
      | {
          approvalId: string;
          name: string;
          summary: string;
        }
      | null,
  ) => void;
  finalizeSubagentWindow: (
    requestId: string,
    patch: {
      status: import("./ipc").SubagentStatus;
      finalContent?: string;
      error?: string;
      content: string;
      reasoning: string;
    },
  ) => void;
  appendSubagentInbox: (requestId: string, message: string) => void;
  /** Map of subagent request id → ordered Health Monitor messages
   *  delivered while the subagent was/is working. Surfaced as
   *  read-only "Health Monitor" bubbles in the subagent viewer. */
  healthInbox: Record<string, string[]>;
  /** Chat column fraction; locked at first subagent spawn and never
   *  auto-resizes after. `null` until the first subagent opens (no
   *  tiling yet → the chat owns the full width). */
  chatRatio: number | null;
  setChatRatio: (ratio: number) => void;
  /** Clear the locked chat ratio. Called when the last live
   *  subagent terminates so the chat reflows back to its original
   *  full-width / centered state. Safe to call when the ratio is
   *  already `null` (no-op). */
  clearChatRatio: () => void;

  // Recents
  recentProjects: RecentProject[];
  setRecentProjects: (list: RecentProject[]) => void;

  // Sessions (lightweight index, used by the Sessions panel)
  sessions: import("./ipc").SessionSummary[];
  setSessions: (list: import("./ipc").SessionSummary[]) => void;
  activeSessionId: string | null;
  setActiveSessionId: (id: string | null) => void;

  // File tree (project-relative forward-slash paths, for mention popover)
  fileTree: string[];
  setFileTree: (paths: string[]) => void;

  // Mention popover
  mentionState: MentionState;
  setMentionState: (m: Partial<MentionState>) => void;
  closeMention: () => void;

  // Agents
  activeAgents: import("./ipc").AgentInfo[];
  setActiveAgents: (agents: import("./ipc").AgentInfo[]) => void;

  // Job bar
  jobActivities: string[];
  setJobActivities: (activities: string[]) => void;

  // Settings
  settingsOpen: boolean;
  settingsTab: "general" | "models" | "keys";
  setSettingsOpen: (open: boolean) => void;
  setSettingsTab: (tab: "general" | "models" | "keys") => void;

  // Sidebar
  sidebarOpen: boolean;
  setSidebarOpen: (open: boolean) => void;
  /** Measured height of the chat header strip (px); the sidebar anchors below it. */
  chatHeaderHeight: number;
  setChatHeaderHeight: (height: number) => void;

  // Todo list (spec 2026-08-14--agent-todo-list.md). Mirrored from
  // the session; updated by `setTodos` (hydrate) and the
  // `todo_changed` event (live). `setTodos` is also used by the
  // optimistic-update path on user IPC edits.
  todos: import("./ipc").TodoList;
  /** Session id the todos slice was last hydrated for. Used to skip
   *  re-hydration when the same session is re-loaded. */
  hydratedTodosFor: string | null;
  setTodos: (list: import("./ipc").TodoList) => void;
  setHydratedTodosFor: (sessionId: string | null) => void;
}

// ── Store implementation ──

export const useStore = create<HarnessState>((set) => ({
  // Workspace
  workspacePath: null,
  workspaceStatus: "noProject",
  setWorkspacePath: (path) => set({ workspacePath: path }),
  setWorkspaceStatus: (s) => set({ workspaceStatus: s }),

  // Chat / session
  messages: [],
  sessionLoaded: false,
  appendMessage: (msg) => set((state) => ({ messages: [...state.messages, msg] })),
  setMessages: (msgs) => set({ messages: msgs, sessionLoaded: true }),
  clearMessages: () => set({ messages: [] }),
  updateMessage: (id, patch) =>
    set((state) => ({
      messages: state.messages.map((m) => (m.id === id ? { ...m, ...patch } : m)),
    })),
  appendToMessage: (id, delta) =>
    set((state) => ({
      messages: state.messages.map((m) =>
        m.id === id ? { ...m, content: m.content + delta } : m
      ),
    })),
  appendReasoning: (id, delta) =>
    set((state) => ({
      messages: state.messages.map((m) =>
        m.id === id ? { ...m, reasoning: (m.reasoning ?? "") + delta } : m
      ),
    })),
  removeMessage: (id) =>
    set((state) => ({ messages: state.messages.filter((m) => m.id !== id) })),
  activities: [],
  setActivities: (list) => set({ activities: list }),
  appendActivity: (activity) =>
    set((state) => ({ activities: [...state.activities, activity] })),
  toolCalls: {},
  setToolCalls: (map) => set({ toolCalls: map }),
  appendToolCall: (messageId, call) =>
    set((state) => ({
      toolCalls: {
        ...state.toolCalls,
        [messageId]: [...(state.toolCalls[messageId] ?? []), call],
      },
    })),
  updateToolCall: (messageId, callId, patch) =>
    set((state) => ({
      toolCalls: {
        ...state.toolCalls,
        [messageId]: (state.toolCalls[messageId] ?? []).map((c) =>
          c.id === callId ? { ...c, ...patch } : c
        ),
      },
    })),
  sessionMeta: null,
  setSessionMeta: (m) => set({ sessionMeta: m }),
  commandMode: "commands",
  setActionMode: (mode) => set({ commandMode: mode }),
  goalMode: false,
  setGoalMode: (on) => set({ goalMode: on }),
  agentRequestId: null,
  setAgentRequestId: (id) => set({ agentRequestId: id }),
  pendingApproval: null,
  setPendingApproval: (p) => set({ pendingApproval: p }),
  personalities: null,
  setPersonalities: (personalities) => set({ personalities }),
  agentManagerOpen: false,
  setAgentManagerOpen: (open) => set({ agentManagerOpen: open }),

  // Saved models + active model + telemetry (Phase 2.0)
  savedModels: [],
  setSavedModels: (list) => set({ savedModels: list }),
  activeModelId: null,
  setActiveModelId: (id) => set({ activeModelId: id }),
  modelPrefs: { thinkingOn: false, reasoningEffort: "medium" },
  setModelPrefs: (prefs) => set({ modelPrefs: prefs }),
  tokenStats: null,
  setTokenStats: (s) => set({ tokenStats: s }),
  streamingRequestId: null,
  setStreamingRequestId: (id) => set({ streamingRequestId: id }),
  pendingMessages: [],
  enqueueMessage: (msg, atFront = false) =>
    set((state) => ({
      pendingMessages: atFront
        ? [msg, ...state.pendingMessages]
        : [...state.pendingMessages, msg],
    })),
  removeQueuedMessage: (id) =>
    set((state) => ({
      pendingMessages: state.pendingMessages.filter((m) => m.id !== id),
    })),
  updateQueuedMessage: (id, patch) =>
    set((state) => ({
      pendingMessages: state.pendingMessages.map((m) =>
        m.id === id ? { ...m, ...patch } : m
      ),
    })),
  clearPendingMessages: () => set({ pendingMessages: [] }),
  settingsFocusModelId: null,
  setSettingsFocusModelId: (id) => set({ settingsFocusModelId: id }),

  // Subagent tiling (tiling spec)
  subagents: [],
  upsertSubagent: (rec) =>
    set((state) => {
      const idx = state.subagents.findIndex((r) => r.requestId === rec.requestId);
      if (idx === -1) return { subagents: [rec, ...state.subagents] };
      const next = state.subagents.slice();
      next[idx] = rec;
      return { subagents: next };
    }),
  removeSubagent: (requestId) =>
    set((state) => {
      const { [requestId]: _drop, ...restWindows } = state.subagentWindows;
      const { [requestId]: _dropInbox, ...restInbox } = state.healthInbox;
      const nextSubagents = state.subagents.filter(
        (r) => r.requestId !== requestId,
      );
      return {
        subagents: nextSubagents,
        subagentWindows: restWindows,
        healthInbox: restInbox,
      };
    }),
  setSubagents: (records) => set({ subagents: records }),
  /** Wipe the entire subagent registry + every per-window state. Called
   *  when a new orchestrator turn starts — past subagent viewers close
   *  in unison so the right region resets for the new round. */
  clearAllSubagents: () =>
    set({
      subagents: [],
      subagentWindows: {},
      healthInbox: {},
    }),
  subagentWindows: {},
  ensureSubagentWindow: (rec) =>
    set((state) => {
      if (state.subagentWindows[rec.requestId]) {
        return {
          subagentWindows: {
            ...state.subagentWindows,
            [rec.requestId]: { ...state.subagentWindows[rec.requestId], record: rec },
          },
        };
      }
      return {
        subagentWindows: {
          ...state.subagentWindows,
          [rec.requestId]: {
            record: rec,
            placeholderId: `sub-${rec.requestId}`,
            content: "",
            reasoning: "",
            toolCalls: [],
            approval: null,
          },
        },
      };
    }),
  appendSubagentContent: (requestId, delta) =>
    set((state) => {
      const w = state.subagentWindows[requestId];
      if (!w) return {};
      return {
        subagentWindows: {
          ...state.subagentWindows,
          [requestId]: { ...w, content: w.content + delta },
        },
      };
    }),
  appendSubagentReasoning: (requestId, delta) =>
    set((state) => {
      const w = state.subagentWindows[requestId];
      if (!w) return {};
      return {
        subagentWindows: {
          ...state.subagentWindows,
          [requestId]: { ...w, reasoning: w.reasoning + delta },
        },
      };
    }),
  appendSubagentToolCall: (requestId, call) =>
    set((state) => {
      const w = state.subagentWindows[requestId];
      if (!w) return {};
      return {
        subagentWindows: {
          ...state.subagentWindows,
          [requestId]: { ...w, toolCalls: [...w.toolCalls, call] },
        },
      };
    }),
  updateSubagentToolCall: (requestId, callId, patch) =>
    set((state) => {
      const w = state.subagentWindows[requestId];
      if (!w) return {};
      return {
        subagentWindows: {
          ...state.subagentWindows,
          [requestId]: {
            ...w,
            toolCalls: w.toolCalls.map((c) =>
              c.id === callId ? { ...c, ...patch } : c,
            ),
          },
        },
      };
    }),
  setSubagentApproval: (requestId, approval) =>
    set((state) => {
      const w = state.subagentWindows[requestId];
      if (!w) return {};
      return {
        subagentWindows: {
          ...state.subagentWindows,
          [requestId]: { ...w, approval },
        },
      };
    }),
  finalizeSubagentWindow: (requestId, patch) =>
    set((state) => {
      const w = state.subagentWindows[requestId];
      if (!w) return {};
      const finishedAt = new Date().toISOString();
      const record: import("./ipc").SubagentRecord = {
        ...w.record,
        status: patch.status,
        finalContent: patch.finalContent,
        error: patch.error,
        finishedAt,
      };
      return {
        subagents: state.subagents.map((r) =>
          r.requestId === requestId ? record : r,
        ),
        subagentWindows: {
          ...state.subagentWindows,
          [requestId]: {
            ...w,
            record,
            content: patch.content,
            reasoning: patch.reasoning,
            approval: null,
          },
        },
      };
    }),
  appendSubagentInbox: (requestId, message) =>
    set((state) => ({
      healthInbox: {
        ...state.healthInbox,
        [requestId]: [...(state.healthInbox[requestId] ?? []), message],
      },
    })),
  healthInbox: {},
  chatRatio: null,
  setChatRatio: (ratio) => set({ chatRatio: ratio }),
  clearChatRatio: () =>
    set((state) => (state.chatRatio === null ? state : { chatRatio: null })),

  // Recents
  recentProjects: [],
  setRecentProjects: (list) => set({ recentProjects: list }),

  // Sessions
  sessions: [],
  setSessions: (list) => set({ sessions: list }),
  activeSessionId: null,
  setActiveSessionId: (id) => set({ activeSessionId: id }),

  // File tree
  fileTree: [],
  setFileTree: (paths) => set({ fileTree: paths }),

  // Mention
  mentionState: { ...emptyMention },
  setMentionState: (m) =>
    set((state) => ({ mentionState: { ...state.mentionState, ...m } })),
  closeMention: () => set({ mentionState: { ...emptyMention } }),

  // Agents
  activeAgents: [],
  setActiveAgents: (agents) => set({ activeAgents: agents }),

  // Job bar
  jobActivities: [],
  setJobActivities: (activities) => set({ jobActivities: activities }),

  // Settings
  settingsOpen: false,
  settingsTab: "general",
  setSettingsOpen: (open) => set({ settingsOpen: open }),
  setSettingsTab: (tab) => set({ settingsTab: tab }),

  // Sidebar
  sidebarOpen: false,
  setSidebarOpen: (open) => set({ sidebarOpen: open }),
  chatHeaderHeight: 0,
  setChatHeaderHeight: (height) => set({ chatHeaderHeight: height }),

  // Todo list (spec 2026-08-14--agent-todo-list.md)
  todos: { items: [], version: 0 },
  hydratedTodosFor: null,
  setTodos: (list) => set({ todos: list }),
  setHydratedTodosFor: (sessionId) => set({ hydratedTodosFor: sessionId }),
}));
