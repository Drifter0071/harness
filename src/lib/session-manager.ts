// Project lifecycle manager. Wires the store, IPC, and per-project
// session.json into a coherent set of side effects:
//   - On workspacePath change: flush the outgoing project's pending save,
//     load the new project's session, refresh the file tree, touch the
//     recents list.
//   - On every message append: schedule a debounced save (500 ms).
//   - On app unload: best-effort flush.

import { useEffect, useRef } from "react";
import { useStore } from "./store";
import { ipc } from "./ipc";
import type { Session, SessionMessage } from "./store";
import { showToast } from "./toast";

const DEBOUNCE_MS = 500;

interface LifecycleRefs {
  /** The project that the current debounce timer will save to. */
  pendingProject: string | null;
  timer: ReturnType<typeof setTimeout> | null;
  /** Most recent in-memory messages for the pending project (captured
   *  when the timer was scheduled, so we don't lose edits if the user
   *  switches projects before the timer fires). */
  pendingMessages: SessionMessage[];
}

let refs: LifecycleRefs = {
  pendingProject: null,
  timer: null,
  pendingMessages: [],
};

async function buildSession(projectPath: string, messages: SessionMessage[]): Promise<Session> {
  // Preserve createdAt if we already have one for this project; otherwise
  // use "now" for both. The Rust side rewrites updatedAt on save.
  const existingMeta = useStore.getState().sessionMeta;
  const now = new Date().toISOString();
  return {
    version: 1,
    projectPath,
    createdAt: existingMeta?.createdAt ?? now,
    updatedAt: now,
    messages,
    // Token telemetry persists with the session (spec §4.5).
    tokenStats: useStore.getState().tokenStats ?? null,
  };
}

async function saveNow(projectPath: string, messages: SessionMessage[]): Promise<void> {
  const session = await buildSession(projectPath, messages);
  try {
    const saved = await ipc.saveSession(projectPath, session);
    useStore.setState({
      sessionMeta: { createdAt: saved.createdAt, updatedAt: saved.updatedAt },
    });
  } catch (err) {
    showToast(`Failed to save session: ${err}`);
  }
}

/**
 * Cancel any pending debounce and synchronously flush it. Called when
 * switching projects so the outgoing project gets its last edit on disk
 * before we load the new one.
 */
export async function flushPendingSave(): Promise<void> {
  if (refs.timer) {
    clearTimeout(refs.timer);
    refs.timer = null;
  }
  if (refs.pendingProject && refs.pendingMessages) {
    await saveNow(refs.pendingProject, refs.pendingMessages);
  }
  refs.pendingProject = null;
  refs.pendingMessages = [];
}

function scheduleSave(projectPath: string, messages: SessionMessage[]): void {
  refs.pendingProject = projectPath;
  refs.pendingMessages = messages;
  if (refs.timer) clearTimeout(refs.timer);
  refs.timer = setTimeout(() => {
    const p = refs.pendingProject;
    const m = refs.pendingMessages;
    refs.timer = null;
    refs.pendingProject = null;
    refs.pendingMessages = [];
    if (p && m) saveNow(p, m).catch(() => {});
  }, DEBOUNCE_MS);
}

/**
 * Drive the project lifecycle. Mount once at the App root.
 * On every workspacePath change: flush, load, fetch file tree, touch recents.
 * Subscribes to the messages store and schedules a debounced save.
 */
export function useProjectLifecycle(): void {
  const lastPathRef = useRef<string | null>(null);
  const messagesRef = useRef<SessionMessage[]>([]);
  const pathRef = useRef<string | null>(null);

  useEffect(() => {
    const unsub = useStore.subscribe((state) => {
      const path = state.workspacePath;
      const messages = state.messages;
      messagesRef.current = messages;
      pathRef.current = path;

      if (path && messages !== messagesRef.current) {
        // This subscription is also fired on initial subscribe, but the
        // scheduler is idempotent so a redundant schedule is harmless.
      }
      if (path) {
        scheduleSave(path, messages);
      }
    });
    return unsub;
  }, []);

  useEffect(() => {
    // Watch workspacePath and react to changes.
    return useStore.subscribe(async (state) => {
      const path = state.workspacePath;
      if (path === lastPathRef.current) return;
      const prev = lastPathRef.current;
      lastPathRef.current = path;

      if (prev) {
        await flushPendingSave();
      }

      if (!path) {
        useStore.setState({
          messages: [],
          sessionLoaded: false,
          sessionMeta: null,
          tokenStats: null,
          fileTree: [],
          workspaceStatus: "noProject",
        });
        return;
      }

      useStore.setState({ workspaceStatus: "loading" });
      try {
        const [session, files, recent] = await Promise.all([
          ipc.loadSession(path).catch(() => null),
          ipc.listFilesRecursive(path).catch(() => []),
          ipc.listRecentProjects().catch(() => []),
        ]);

        if (session) {
          useStore.setState({
            messages: session.messages,
            sessionLoaded: true,
            sessionMeta: { createdAt: session.createdAt, updatedAt: session.updatedAt },
            tokenStats: session.tokenStats ?? null,
          });
        } else {
          useStore.setState({
            messages: [],
            sessionLoaded: true,
            sessionMeta: null,
            tokenStats: null,
          });
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
    });
  }, []);
}
