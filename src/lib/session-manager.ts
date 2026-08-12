// Project lifecycle manager. Wires the store, IPC, and per-project sessions
// (`.harness/sessions/<id>.json`) into a coherent set of side effects:
//   - On workspacePath change: flush the outgoing project's pending save,
//     load the active session + roster, refresh the file tree, touch recents.
//   - On activeSessionId change (user switched in the Sessions panel): flush
//     the old session, load the new one.
//   - On every store change: schedule a debounced save (500 ms) for the
//     current session — unless an agent turn is in flight, in which case the
//     Rust loop owns persistence and the frontend stays out of the way.
//   - Agent loop events are routed by `requestId` into the streaming message,
//     the live activity feed, and the pending-approval card.

import { useEffect } from "react";
import { useStore } from "./store";
import { ipc, agentEvents } from "./ipc";
import { listen } from "@tauri-apps/api/event";
import type {
  ActionMode,
  AgentSnapshot,
  AgentTurnRequest,
  Session,
  ToolActivity,
} from "./ipc";
import type { SessionMessage, TokenStats } from "./store";
import { showToast } from "./toast";

const DEBOUNCE_MS = 500;

/** Snapshot captured when a save is scheduled, so nothing is lost if the
 *  user switches projects/sessions before the debounce fires. */
interface PendingSnapshot {
  projectPath: string;
  sessionId: string;
  title: string;
  messages: SessionMessage[];
  activities: ToolActivity[];
  tokenStats: TokenStats | null;
  agent: AgentSnapshot | null;
  actionMode: ActionMode;
  goalMode: boolean;
  createdAt: string | null;
}

let pending: PendingSnapshot | null = null;
let timer: ReturnType<typeof setTimeout> | null = null;

/** Agent snapshot of the currently loaded session (preserved on save). */
let currentAgent: AgentSnapshot | null = null;
/** Session id currently loaded in the store (guards the switch watcher). */
let loadedSessionId: string | null = null;

function fallbackAgent(): AgentSnapshot {
  return {
    id: "harness",
    name: "harness",
    role: "Orchestrator",
    systemPrompt: "",
  };
}

async function refreshSessions(projectPath: string): Promise<void> {
  const list = await ipc.listSessions(projectPath).catch(() => []);
  useStore.setState({ sessions: list });
}

async function saveNow(snapshot: PendingSnapshot): Promise<void> {
  const session: Session = {
    version: 2,
    id: snapshot.sessionId,
    title: snapshot.title,
    projectPath: snapshot.projectPath,
    createdAt: snapshot.createdAt ?? new Date().toISOString(),
    updatedAt: new Date().toISOString(),
    agent: snapshot.agent ?? fallbackAgent(),
    goalMode: snapshot.goalMode,
    actionMode: snapshot.actionMode,
    messages: snapshot.messages,
    activities: snapshot.activities,
    tokenStats: snapshot.tokenStats ?? null,
  };
  try {
    const saved = await ipc.saveSession(snapshot.projectPath, session);
    if (useStore.getState().activeSessionId === snapshot.sessionId) {
      useStore.setState({
        sessionMeta: { createdAt: saved.createdAt, updatedAt: saved.updatedAt },
      });
    }
    refreshSessions(snapshot.projectPath);
  } catch (err) {
    showToast(`Failed to save session: ${err}`);
  }
}

function captureSnapshot(): PendingSnapshot | null {
  const st = useStore.getState();
  if (!st.workspacePath || !st.activeSessionId || !st.sessionLoaded) return null;
  return {
    projectPath: st.workspacePath,
    sessionId: st.activeSessionId,
    title: st.sessions.find((s) => s.id === st.activeSessionId)?.title ?? "New session",
    messages: st.messages,
    activities: st.activities,
    tokenStats: st.tokenStats,
    agent: currentAgent,
    actionMode: st.actionMode,
    goalMode: st.goalMode,
    createdAt: st.sessionMeta?.createdAt ?? null,
  };
}

function scheduleSave(): void {
  // While an agent turn runs, the Rust loop persists messages, activities
  // and token stats itself. A concurrent debounced write could clobber the
  // newer state, so the frontend defers until the turn ends.
  if (useStore.getState().agentRequestId) return;
  const snapshot = captureSnapshot();
  if (!snapshot) return;
  pending = snapshot;
  if (timer) clearTimeout(timer);
  timer = setTimeout(() => {
    const snap = pending;
    timer = null;
    pending = null;
    if (snap) saveNow(snap).catch(() => {});
  }, DEBOUNCE_MS);
}

/**
 * Cancel any pending debounce and synchronously flush it. Called when
 * switching projects/sessions so the outgoing session gets its last edit
 * on disk before the new one loads.
 */
export async function flushPendingSave(): Promise<void> {
  if (timer) {
    clearTimeout(timer);
    timer = null;
  }
  const snap = pending;
  pending = null;
  if (snap) await saveNow(snap).catch(() => {});
}

/** Apply a loaded session to the store. */
function applySession(session: Session, projectPath: string): void {
  currentAgent = session.agent;
  loadedSessionId = session.id;
  useStore.setState({
    activeSessionId: session.id,
    messages: session.messages,
    activities: session.activities,
    sessionMeta: { createdAt: session.createdAt, updatedAt: session.updatedAt },
    tokenStats: session.tokenStats ?? null,
    goalMode: session.goalMode,
    actionMode: session.actionMode,
    sessionLoaded: true,
  });
  refreshSessions(projectPath);
}

async function loadProject(path: string): Promise<void> {
  useStore.setState({ workspaceStatus: "loading" });
  try {
    const [summaries, session, files, recent, roster] = await Promise.all([
      ipc.listSessions(path).catch(() => []),
      ipc.loadActiveSession(path).catch(() => null),
      ipc.listFilesRecursive(path).catch(() => []),
      ipc.listRecentProjects().catch(() => []),
      ipc.loadAgentRoster(path).catch(() => null),
    ]);

    let active = session;
    if (!active && summaries.length > 0) {
      // Fall back to the newest summary (Rust resolves the same way, but be
      // defensive if the marker raced a deletion).
      const newest = [...summaries].sort((a, b) => (a.updatedAt < b.updatedAt ? 1 : -1))[0];
      active = await ipc.loadSession(path, newest.id).catch(() => null);
    }
    if (!active) {
      // First open of this project: create a fresh session.
      active = await ipc.createSession(path).catch(() => null);
    }

    useStore.setState({ sessions: summaries });
    if (active) {
      applySession(active, path);
    } else {
      useStore.setState({ sessionLoaded: true });
    }

    useStore.setState({
      fileTree: files,
      recentProjects: recent,
      agentRoster: roster,
      workspaceStatus: "ready",
    });

    // Touch recents (LRU bump). Errors here are non-fatal.
    try {
      const updated = await ipc.touchRecentProject(path);
      const next = [updated, ...recent.filter((p) => p.path !== updated.path)].slice(0, 20);
      useStore.setState({ recentProjects: next });
    } catch {
      // best-effort
    }
  } catch (err) {
    showToast(`Failed to load project: ${err}`);
    useStore.setState({ workspaceStatus: "ready" });
  }

  // Start the Rust-side session file watcher so the UI re-reads
  // session JSON from disk when it changes. Without this, the chat
  // path's flushPendingSave writes go to disk but the in-memory
  // store never sees them.
  try {
    await ipc.watchSession(path);
  } catch {
    // Best-effort. The watcher is a debugging aid; not fatal.
  }
}

// ── Agent turn entry points (used by the composer) ──

/** Start one agent turn: append the user message + assistant placeholder,
 *  register the requestId, and kick off the Rust loop. */
export async function startAgentTurn(
  text: string,
  mentions?: { path: string; lineStart?: number; lineEnd?: number }[]
): Promise<void> {
  const st = useStore.getState();
  if (!st.workspacePath || !st.activeSessionId || !st.activeModelId) return;
  if (st.agentRequestId) return; // a turn is already running

  const requestId = crypto.randomUUID();
  const now = new Date().toISOString();
  const agent = currentAgent ?? fallbackAgent();

  st.appendMessage({
    id: requestId,
    role: "user",
    content: text,
    timestamp: now,
    mentions: mentions && mentions.length > 0 ? mentions : undefined,
  });
  st.appendMessage({
    id: `agent-${requestId}`,
    role: "assistant",
    content: "",
    timestamp: now,
    agentId: agent.id,
    agentName: agent.name,
  });
  st.setAgentRequestId(requestId);
  st.setPendingApproval(null);

  const req: AgentTurnRequest = {
    requestId,
    sessionId: st.activeSessionId,
    modelId: st.activeModelId,
    message: text,
    actionMode: st.actionMode,
    goalMode: st.goalMode,
    workspace: st.workspacePath,
  };
  try {
    await ipc.runAgentTurn(req);
  } catch (err) {
    st.removeMessage(`agent-${requestId}`);
    st.setAgentRequestId(null);
    showToast(`Failed to start agent turn: ${err}`);
  }
}

/** Cancel the in-flight agent turn (if any). */
export async function cancelAgentTurn(): Promise<void> {
  const requestId = useStore.getState().agentRequestId;
  if (requestId) await ipc.cancelAgent(requestId).catch(() => {});
}

/** Resolve a pending tool approval. */
export function respondApproval(allow: boolean): void {
  const p = useStore.getState().pendingApproval;
  if (!p) return;
  useStore.setState({ pendingApproval: null });
  ipc.respondApproval(p.requestId, allow).catch(() => {});
}

// ── Agent loop event routing ──

function placeholderId(requestId: string): string {
  return `agent-${requestId}`;
}

function ensurePlaceholder(requestId: string): void {
  const st = useStore.getState();
  const id = placeholderId(requestId);
  if (st.messages.some((m) => m.id === id)) return;
  const agent = currentAgent ?? fallbackAgent();
  st.appendMessage({
    id,
    role: "assistant",
    content: "",
    timestamp: new Date().toISOString(),
    agentId: agent.id,
    agentName: agent.name,
  });
}

function handleAgentTerminal(requestId: string): void {
  const st = useStore.getState();
  if (st.agentRequestId !== requestId) return;
  st.setAgentRequestId(null);
  st.setPendingApproval(null);
  // Debounced save picks up the final state (messages/activities/tokens).
  scheduleSave();
}

function useAgentEvents(): void {
  useEffect(() => {
    // In browser-only dev there's no Tauri event bus; swallow rejections.
    const reg = (p: Promise<() => void>) => {
      p.catch(() => {});
      return p;
    };
    const unsubs: Promise<() => void>[] = [];

    // Live reload: when the Rust-side session file watcher sees a
    // write to `<workspace>/.harness/sessions/<id>.json`, re-read
    // the active session from disk and apply it. This is what makes
    // the chat path's `flushPendingSave` visible in the UI without
    // an app restart.
    unsubs.push(
      reg(
        listen<{ path: string }>("session-file-changed", async () => {
          const st = useStore.getState();
          if (!st.workspacePath || !st.activeSessionId) return;
          // Skip if a streaming turn is in flight — the in-memory
          // store is ahead of disk and re-reading would clobber it.
          if (st.streamingRequestId || st.agentRequestId) return;
          try {
            const session = await ipc.loadSession(
              st.workspacePath,
              st.activeSessionId,
            );
            if (session) applySession(session, st.workspacePath);
          } catch {
            // Best-effort.
          }
        })
      )
    );


    unsubs.push(
      reg(
        agentEvents.onTurnStart((e) => {
          ensurePlaceholder(e.requestId);
        })
      )
    );

    unsubs.push(
      reg(
        agentEvents.onChunk((e) => {
          const st = useStore.getState();
          if (st.agentRequestId !== e.requestId) return;
          ensurePlaceholder(e.requestId);
          st.appendToMessage(placeholderId(e.requestId), e.delta);
        })
      )
    );

    unsubs.push(
      reg(
        agentEvents.onToolStart((e) => {
          const st = useStore.getState();
          if (st.agentRequestId !== e.requestId) return;
          // Transient live entry; the persisted record lands on tool-result.
          st.setJobActivities([
            ...st.jobActivities,
            `run ${e.name} ${JSON.stringify(e.arguments) ?? ""}`.trim(),
          ]);
        })
      )
    );

    unsubs.push(
      reg(
        agentEvents.onToolResult((e) => {
          const st = useStore.getState();
          if (st.agentRequestId !== e.requestId) return;
          const activity: ToolActivity = {
            id: crypto.randomUUID(),
            tool: e.name,
            summary: e.summary,
            status: e.status,
            timestamp: new Date().toISOString(),
          };
          st.appendActivity(activity);
        })
      )
    );

    unsubs.push(
      reg(
        agentEvents.onApprovalRequest((e) => {
          const st = useStore.getState();
          if (st.agentRequestId !== e.requestId) return;
          st.setPendingApproval({
            requestId: e.requestId,
            approvalId: e.approvalId,
            name: e.name,
            summary: e.summary,
          });
        })
      )
    );

    unsubs.push(
      reg(
        agentEvents.onApprovalResult((e) => {
          const st = useStore.getState();
          if (st.pendingApproval?.approvalId === e.approvalId) {
            st.setPendingApproval(null);
          }
        })
      )
    );

    unsubs.push(
      reg(
        agentEvents.onDone((e) => {
          const st = useStore.getState();
          if (st.agentRequestId !== e.requestId) return;
          ensurePlaceholder(e.requestId);
          // Replace the streamed placeholder with the final text.
          st.updateMessage(placeholderId(e.requestId), { content: e.content });
          st.setJobActivities([]);
          // Token telemetry: cumulativeIn is the PEAK input (max), cumulativeOut is a sum.
          const lastIn = e.usage?.input ?? 0;
          const lastOut = e.usage?.output ?? 0;
          const prev = st.tokenStats;
          st.setTokenStats({
            lastIn,
            lastOut,
            cumulativeIn: prev ? Math.max(prev.cumulativeIn, lastIn) : lastIn,
            cumulativeOut: (prev?.cumulativeOut ?? 0) + lastOut,
            sessionWindowUsed: prev?.sessionWindowUsed ?? null,
            sessionWindowResetsAt: prev?.sessionWindowResetsAt ?? null,
          });
          handleAgentTerminal(e.requestId);
        })
      )
    );

    unsubs.push(
      reg(
        agentEvents.onError((e) => {
          const st = useStore.getState();
          if (st.agentRequestId !== e.requestId) return;
          ensurePlaceholder(e.requestId);
          const id = placeholderId(e.requestId);
          if (e.kind === "cancelled") {
            // User cancelled: drop the half-written placeholder.
            st.removeMessage(id);
            st.setJobActivities([]);
          } else {
            st.updateMessage(id, {
              content: `${st.messages.find((m) => m.id === id)?.content ?? ""}\n\n⚠️ ${e.message}`,
            });
            showToast(`Agent error (${e.kind}): ${e.message}`);
          }
          handleAgentTerminal(e.requestId);
        })
      )
    );

    return () => {
      unsubs.forEach((p) => p.then((u) => u()).catch(() => {}));
    };
  }, []);
}

// ── Lifecycle hook ──

/**
 * Drive the project lifecycle. Mount once at the App root.
 */
export function useProjectLifecycle(): void {
  useAgentEvents();

  // Save on store changes (debounced, skipped while an agent turn runs).
  useEffect(() => {
    return useStore.subscribe((state) => {
      if (state.workspacePath && state.sessionLoaded) {
        scheduleSave();
      }
    });
  }, []);

  // React to workspacePath changes.
  useEffect(() => {
    let lastPath: string | null = null;
    const unsub = useStore.subscribe(async (state) => {
      const path = state.workspacePath;
      if (path === lastPath) return;
      const prev = lastPath;
      lastPath = path;

      if (prev) {
        await flushPendingSave();
      }

      if (!path) {
        currentAgent = null;
        loadedSessionId = null;
        useStore.setState({
          messages: [],
          activities: [],
          sessionLoaded: false,
          sessionMeta: null,
          tokenStats: null,
          agentRequestId: null,
          pendingApproval: null,
          activeSessionId: null,
          agentRoster: null,
          fileTree: [],
          workspaceStatus: "noProject",
        });
        return;
      }

      await loadProject(path);
    });
    return unsub;
  }, []);

  // React to activeSessionId changes (Sessions panel switch).
  useEffect(() => {
    return useStore.subscribe(async (state) => {
      if (!state.workspacePath || !state.sessionLoaded) return;
      if (!state.activeSessionId || state.activeSessionId === loadedSessionId) return;
      if (state.agentRequestId) return; // don't yank the session mid-turn
      const target = state.activeSessionId;
      await flushPendingSave();
      const session = await ipc.loadSession(state.workspacePath, target).catch(() => null);
      if (session) applySession(session, state.workspacePath);
    });
  }, []);
}
