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
import {
  ipc,
  agentEvents,
  ORCHESTRATOR_AGENT_ID,
  ORCHESTRATOR_AGENT_NAME,
} from "./ipc";
import { listen } from "@tauri-apps/api/event";
import type {
  ActionMode,
  AgentSnapshot,
  AgentTurnRequest,
  ReasoningEffort,
  Session,
  ToolActivity,
} from "./ipc";
import type { SessionMessage, TokenStats, ToolCallRecord } from "./store";
import { capabilitiesFor } from "./model-capabilities";
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

/** The chat agent on the currently loaded session. After the v3
 *  chat-agent-as-orchestrator refactor this is always the
 *  Orchestrator (a hardcoded identity), but we keep the snapshot
 *  pointer so legacy session files that carry their own agent
 *  block still round-trip without surprise. */
let currentAgent: AgentSnapshot | null = null;
/** Session id currently loaded in the store (guards the switch watcher). */
let loadedSessionId: string | null = null;

function fallbackAgent(): AgentSnapshot {
  return {};
}

/** Resolve the live `name` / `role` for the chat agent on the
 *  current session. After the v3 chat-agent-as-orchestrator
 *  refactor this is always the Orchestrator — the function is
 *  preserved for the callsites that stamp a label on assistant
 *  placeholders but no longer consults the personalities roster. */
function resolveAgentMeta(): { id: string; name: string; role: string } {
  return {
    id: ORCHESTRATOR_AGENT_ID,
    name: ORCHESTRATOR_AGENT_NAME,
    role: "Orchestrator",
  };
}

async function refreshSessions(projectPath: string): Promise<void> {
  const list = await ipc.listSessions(projectPath).catch(() => []);
  useStore.setState({ sessions: list });
}

async function saveNow(snapshot: PendingSnapshot): Promise<void> {
  // Preserve backend-authored fields the frontend snapshot doesn't own.
  // The Rust agent loop stamps `tokenStats` + `lastTurn` into the file on
  // every terminal persist; a debounced frontend save can land right
  // after a turn when the store's `tokenStats` is still null (terminal
  // event lost — the disk-fallback path) and would otherwise wipe both
  // fields from disk.
  let lastTurn: Session["lastTurn"] = null;
  let diskTokenStats: TokenStats | null = null;
  try {
    const existing = await ipc.loadSession(snapshot.projectPath, snapshot.sessionId);
    if (existing) {
      lastTurn = existing.lastTurn ?? null;
      diskTokenStats = existing.tokenStats ?? null;
    }
  } catch {
    // Best-effort: fall back to the snapshot alone.
  }
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
    tokenStats: snapshot.tokenStats ?? diskTokenStats,
    lastTurn,
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
  // During a session switch the store is briefly inconsistent:
  // `activeSessionId` already points at the target session while
  // `messages`/`activities` still belong to the loaded one. Always
  // persist under the session the in-memory state actually belongs
  // to (`loadedSessionId`) — otherwise the flush triggered by a
  // switch writes the outgoing session's history into the target
  // session's file, which is the "switching sessions shows the
  // wrong conversation / corrupts history" bug.
  if (!st.workspacePath || !loadedSessionId || !st.sessionLoaded) return null;
  return {
    projectPath: st.workspacePath,
    sessionId: loadedSessionId,
    title: st.sessions.find((s) => s.id === loadedSessionId)?.title ?? "New session",
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
  const st = useStore.getState();
  // While an agent turn runs, the Rust loop persists messages, activities
  // and token stats itself. A concurrent debounced write could clobber the
  // newer state, so the frontend defers until the turn ends.
  if (st.agentRequestId) return;
  // Mid-switch (or right after "new session" creation) the store is
  // inconsistent: activeSessionId targets a session whose messages
  // haven't been loaded yet. Saving now would write the outgoing
  // session's content under the incoming session's id.
  if (st.activeSessionId !== loadedSessionId) return;
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
  // After the v3 chat-agent-as-orchestrator refactor the per-session
  // agent identity is gone — every session runs the same hardcoded
  // chat agent. We still keep a pointer to the deserialized snapshot
  // so legacy v1 / v2 session files that carry their own agent
  // block round-trip without surprise; the snapshot is no longer
  // consulted by the agent loop or the chat header.
  currentAgent = session.agent;
  loadedSessionId = session.id;
  // Heal legacy files where the agent loop persisted one assistant
  // row per round under the same `agent-<requestId>` id: keep only
  // the last occurrence so blank intermediate tool-round rows
  // disappear from the chat instead of rendering as empty bubbles.
  const messages = dedupeById(session.messages);
  // Rebuild the per-message tool-call display from persisted activities.
  // Each activity carries the requestId of the turn that ran it, which
  // maps to the assistant placeholder id `agent-<requestId>`.
  const toolCalls: Record<string, ToolCallRecord[]> = {};
  for (const a of session.activities) {
    if (!a.requestId) continue;
    const messageId = `agent-${a.requestId}`;
    (toolCalls[messageId] ??= []).push({
      id: a.id,
      name: a.tool,
      arguments: stringifyArgs(a.arguments),
      status: a.status,
      summary: a.summary,
    });
  }
  useStore.setState({
    activeSessionId: session.id,
    messages,
    activities: session.activities,
    toolCalls,
    sessionMeta: { createdAt: session.createdAt, updatedAt: session.updatedAt },
    tokenStats: session.tokenStats ?? null,
    goalMode: session.goalMode,
    actionMode: session.actionMode,
    sessionLoaded: true,
  });
  refreshSessions(projectPath);
}

/** Compact JSON of tool-call arguments for the chat display. */
function stringifyArgs(args: unknown): string | undefined {
  if (args === undefined || args === null) return undefined;
  try {
    const s = typeof args === "string" ? args : JSON.stringify(args);
    return s.length > 0 ? s : undefined;
  } catch {
    return String(args);
  }
}

/** Collapse duplicate message ids, keeping the last occurrence. */
function dedupeById(msgs: SessionMessage[]): SessionMessage[] {
  const byId = new Map<string, SessionMessage>();
  for (const m of msgs) byId.set(m.id, m);
  return [...byId.values()];
}

/**
 * Disk-based completion reconciliation while a turn is in flight (or
 * just ended).
 *
 * If the agent loop finished and stamped `lastTurn` on the session file
 * but the terminal event never reached the WebView (the cause of the
 * "responses only show after refresh" bug), pull the terminal state
 * from disk and apply it. Streaming text and tool activities are NOT
 * touched here — the in-memory store remains the source for the
 * partially-streamed message, which the agent loop's `assistant`
 * message in `session.messages` matches by id on the next normal
 * reload.
 *
 * The `agentRequestId` guard is loosened so the polling fallback can
 * still reconcile a just-completed turn whose `agentRequestId` was
 * already cleared by an in-process event that didn't apply the
 * content (the racy case where the file watcher fired and cleared
 * state first, but the placeholder was never updated).
 */
function applyTurnCompletionFromDisk(session: Session): void {
  const st = useStore.getState();
  const marker = session.lastTurn;
  if (!marker) return;
  // Match the in-flight turn when possible, otherwise match any
  // agent- placeholder that still has empty content. The latter is
  // the post-turn recovery case.
  const inFlight = st.agentRequestId;
  const isInFlight = inFlight !== null && marker.requestId === inFlight;
  const placeholderId = `agent-${marker.requestId}`;
  const inMemory = st.messages.find((m) => m.id === placeholderId);
  if (!isInFlight && !(inMemory && (inMemory.content ?? "").length === 0)) {
    return;
  }

  // The agent loop writes the full assistant message into the session
  // file on terminal. Merge that single message into our in-memory
  // store (idempotent: it has the same id as our placeholder), then
  // clear the in-flight state exactly the way the agent-done event
  // handler does.
  const onDisk = session.messages.find((m) => m.id === placeholderId);
  if (onDisk) {
    const merged = st.messages.map((m) =>
      m.id === placeholderId
        ? { ...m, content: onDisk.content, reasoning: onDisk.reasoning }
        : m,
    );
    useStore.setState({ messages: merged });
  } else if (marker.reasoning && marker.reasoning.length > 0) {
    // The assistant message may not be on disk yet, but the
    // `lastTurn.reasoning` mirror carries the chain-of-thought. Apply
    // it to the in-memory placeholder so the user sees the
    // collapsible block once the WebView recovers.
    const merged = st.messages.map((m) =>
      m.id === placeholderId
        ? { ...m, reasoning: marker.reasoning }
        : m,
    );
    useStore.setState({ messages: merged });
  }

  // The backend stamps token telemetry into the session file on every
  // terminal persist. Apply it here too: the `agent-done` event is the
  // primary channel, but when it never reaches the WebView (the known
  // event-delivery gap this fallback exists for), disk is the only
  // source and the header stats would otherwise stay on "—".
  if (session.tokenStats) {
    useStore.setState({ tokenStats: session.tokenStats });
  }

  if (marker.status === "cancelled") {
    useStore.setState({ pendingApproval: null });
  }
  if (isInFlight) {
    useStore.setState({ agentRequestId: null, jobActivities: [] });
  }
  // Let the debounced save pick up the final state.
  scheduleSave();
}

async function loadProject(path: string): Promise<void> {
  useStore.setState({ workspaceStatus: "loading" });
  try {
    const [summaries, session, files, recent] = await Promise.all([
      ipc.listSessions(path).catch(() => []),
      ipc.loadActiveSession(path).catch(() => null),
      ipc.listFilesRecursive(path).catch(() => []),
      ipc.listRecentProjects().catch(() => []),
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

  const meta = resolveAgentMeta();
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
    agentId: meta.id,
    agentName: meta.name,
  });
  st.setAgentRequestId(requestId);
  st.setPendingApproval(null);

  // Resolve the active model's reasoning capability so the agent
  // loop can forward `reasoning_effort` to OpenAI-compat gateways.
  // Mirrors the chat path: for "thinking"-capable models the user
  // toggle is the trigger, for "effort"-capable models we use the
  // user's selected level. Without this, agent turns on reasoning
  // models would never return `delta.reasoning_content`.
  const activeModel = st.savedModels.find((m) => m.id === st.activeModelId) ?? null;
  let reasoningEffort: ReasoningEffort | undefined;
  if (activeModel) {
    const caps = capabilitiesFor(activeModel);
    if (caps.reasoning === "effort") {
      reasoningEffort = st.modelPrefs.reasoningEffort;
    } else if (caps.reasoning === "thinking" && st.modelPrefs.thinkingOn) {
      reasoningEffort = st.modelPrefs.reasoningEffort ?? "medium";
    }
  }

  const req: AgentTurnRequest = {
    requestId,
    sessionId: st.activeSessionId,
    modelId: st.activeModelId,
    message: text,
    actionMode: st.actionMode,
    goalMode: st.goalMode,
    workspace: st.workspacePath,
    reasoningEffort,
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
  const meta = resolveAgentMeta();
  st.appendMessage({
    id,
    role: "assistant",
    content: "",
    timestamp: new Date().toISOString(),
    agentId: meta.id,
    agentName: meta.name,
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
          // If no turn is in flight, a full reload is safe: disk is the
          // source of truth.
          if (!st.streamingRequestId && !st.agentRequestId) {
            try {
              const session = await ipc.loadSession(
                st.workspacePath,
                st.activeSessionId,
              );
              if (session) applySession(session, st.workspacePath);
            } catch {
              // Best-effort.
            }
            return;
          }
          // A turn is in flight. The in-memory store is ahead of disk
          // for the streaming message, so we can't just replace the
          // whole session. Instead: only consult the disk for the
          // `lastTurn` completion signal — that's the only authoritative
          // data we don't already have locally. The streaming state
          // itself still relies on the in-process events (chunks,
          // tool-result) which carry the in-flight content.
          try {
            const session = await ipc.loadSession(
              st.workspacePath,
              st.activeSessionId,
            );
            if (session) applyTurnCompletionFromDisk(session);
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
        agentEvents.onReasoning((e) => {
          const st = useStore.getState();
          if (st.agentRequestId !== e.requestId) return;
          ensurePlaceholder(e.requestId);
          st.appendReasoning(placeholderId(e.requestId), e.delta);
        })
      )
    );

    unsubs.push(
      reg(
        agentEvents.onToolStart((e) => {
          const st = useStore.getState();
          if (st.agentRequestId !== e.requestId) return;
          // Live "running" entry on the assistant message; the persisted
          // record lands on tool-result.
          st.appendToolCall(placeholderId(e.requestId), {
            id: e.callId,
            name: e.name,
            arguments: stringifyArgs(e.arguments),
            status: "running",
          });
        })
      )
    );

    unsubs.push(
      reg(
        agentEvents.onToolResult((e) => {
          const st = useStore.getState();
          if (st.agentRequestId !== e.requestId) return;
          // Finalize the inline chat block.
          st.updateToolCall(placeholderId(e.requestId), e.callId, {
            status: e.status,
            summary: e.summary,
          });
          const activity: ToolActivity = {
            id: crypto.randomUUID(),
            tool: e.name,
            summary: e.summary,
            status: e.status,
            timestamp: new Date().toISOString(),
            requestId: e.requestId,
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
          // The file-watcher fallback (applyTurnCompletionFromDisk)
          // can race with this event and clear `agentRequestId` first
          // when the agent's terminal write hits the disk before the
          // in-process event reaches the WebView. The watcher already
          // copied the final content into the placeholder in that
          // case, but we still re-apply the in-process event's
          // authoritative content + reasoning so any final-round
          // additions the disk hadn't caught yet land in the UI. The
          // only reason to bail is when a *different* turn's done
          // event arrives (stale), which would clobber the active
          // placeholder.
          if (
            st.agentRequestId !== null &&
            st.agentRequestId !== e.requestId
          ) {
            return;
          }
          ensurePlaceholder(e.requestId);
          // Replace the streamed placeholder with the final text +
          // reasoning (if the backend bundled it).
          st.updateMessage(placeholderId(e.requestId), {
            content: e.content,
            reasoning:
              e.reasoning && e.reasoning.length > 0 ? e.reasoning : undefined,
          });
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
      // A newer click may have landed while we were loading; only
      // apply when this target is still the active one, so rapid
      // switching settles on the last-clicked session.
      if (useStore.getState().activeSessionId !== target) return;
      if (session) applySession(session, state.workspacePath);
      // Keep the `.active` marker in sync so reopening the project
      // lands on the last-viewed session, not the last-created one.
      ipc.setActiveSession(state.workspacePath, target).catch(() => {});
    });
  }, []);

  // Polling fallback for the "responses only show after refresh" bug.
  // The session file watcher is the primary signal, but on some
  // platforms (and during heavy multi-tool turns) it can miss the
  // final persist. A 1s tick polls the session file and reconciles
  // any deltas. We keep running after a turn ends so the polling
  // can also catch the post-turn `lastTurn` write that the
  // `applyTurnCompletionFromDisk` path needs to apply the final
  // assistant content into the in-memory placeholder. The
  // "stale empty placeholder" filter keeps this from needlessly
  // reloading the session in the common case.
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;
    let stopped = false;
    const tick = async () => {
      if (stopped) return;
      const st = useStore.getState();
      if (!st.workspacePath || !st.activeSessionId) {
        timer = setTimeout(tick, 1000);
        return;
      }
      // Look for an unresolved placeholder: an assistant message with
      // an `agent-` prefix and empty content. If we find one, the
      // disk is the source of truth and the polling will keep
      // re-reading until it has the final state.
      const hasEmptyAgentPlaceholder = st.messages.some(
        (m) =>
          m.role === "assistant" &&
          m.id.startsWith("agent-") &&
          (m.content ?? "").length === 0,
      );
      const turnInFlight = !!st.agentRequestId;
      if (!hasEmptyAgentPlaceholder && !turnInFlight) {
        timer = setTimeout(tick, 1000);
        return;
      }
      try {
        const session = await ipc.loadSession(
          st.workspacePath,
          st.activeSessionId,
        );
        if (session) applyTurnCompletionFromDisk(session);
      } catch {
        // Best-effort.
      }
      timer = setTimeout(tick, 1000);
    };
    timer = setTimeout(tick, 1000);
    return () => {
      stopped = true;
      if (timer) clearTimeout(timer);
    };
  }, []);
}
