import { create } from "zustand";

// ── Domain types (mirror the Rust shapes in src-tauri/src/session.rs) ──

export interface Mention {
  path: string;
  lineStart?: number;
  lineEnd?: number;
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
  actionMode: import("./ipc").ActionMode;
  messages: SessionMessage[];
  activities: import("./ipc").ToolActivity[];
  /** Null until the first assistant response arrives (spec §4.5b). */
  tokenStats?: TokenStats | null;
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
  removeMessage: (id: string) => void;

  // Persisted tool activities of the active session (live feed source)
  activities: import("./ipc").ToolActivity[];
  setActivities: (list: import("./ipc").ToolActivity[]) => void;
  appendActivity: (activity: import("./ipc").ToolActivity) => void;

  // Session metadata (independent of messages — useful for "created at")
  sessionMeta: { createdAt: string; updatedAt: string } | null;
  setSessionMeta: (m: { createdAt: string; updatedAt: string } | null) => void;

  // Agent session options (composer options row; persisted with the session)
  actionMode: import("./ipc").ActionMode;
  setActionMode: (mode: import("./ipc").ActionMode) => void;
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
  /** Roster for the Agent Manager window; loaded per project. */
  agentRoster: import("./ipc").AgentRoster | null;
  setAgentRoster: (roster: import("./ipc").AgentRoster | null) => void;

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
  /** When set, opening Settings pre-selects this model in the editor. */
  settingsFocusModelId: string | null;
  setSettingsFocusModelId: (id: string | null) => void;

  // Canvas
  canvasPanels: import("./store").CanvasPanel[];
  addCanvasPanel: (panel: import("./store").CanvasPanel) => void;
  updateCanvasPanel: (id: string, updates: Partial<import("./store").CanvasPanel>) => void;
  removeCanvasPanel: (id: string) => void;
  clearCanvas: () => void;

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
}

// ── Canvas panel (kept here for circular-import simplicity) ──

export interface CanvasPanel {
  id: string;
  agentId: string;
  agentName: string;
  role: string;
  messages: SessionMessage[];
  status: "working" | "idle" | "error";
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
  removeMessage: (id) =>
    set((state) => ({ messages: state.messages.filter((m) => m.id !== id) })),
  activities: [],
  setActivities: (list) => set({ activities: list }),
  appendActivity: (activity) =>
    set((state) => ({ activities: [...state.activities, activity] })),
  sessionMeta: null,
  setSessionMeta: (m) => set({ sessionMeta: m }),
  actionMode: "askRisky",
  setActionMode: (mode) => set({ actionMode: mode }),
  goalMode: false,
  setGoalMode: (on) => set({ goalMode: on }),
  agentRequestId: null,
  setAgentRequestId: (id) => set({ agentRequestId: id }),
  pendingApproval: null,
  setPendingApproval: (p) => set({ pendingApproval: p }),
  agentRoster: null,
  setAgentRoster: (roster) => set({ agentRoster: roster }),

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
  settingsFocusModelId: null,
  setSettingsFocusModelId: (id) => set({ settingsFocusModelId: id }),

  // Canvas
  canvasPanels: [],
  addCanvasPanel: (panel) =>
    set((state) => ({ canvasPanels: [...state.canvasPanels, panel] })),
  updateCanvasPanel: (id, updates) =>
    set((state) => ({
      canvasPanels: state.canvasPanels.map((p) =>
        p.id === id ? { ...p, ...updates } : p
      ),
    })),
  removeCanvasPanel: (id) =>
    set((state) => ({
      canvasPanels: state.canvasPanels.filter((p) => p.id !== id),
    })),
  clearCanvas: () => set({ canvasPanels: [] }),

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
}));
