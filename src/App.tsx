import { useEffect, useRef, useState } from "react";
import { TitleBar } from "./components/TitleBar";
import { ChatHeader } from "./components/ChatHeader";
import { Sidebar } from "./components/Sidebar";
import { Settings } from "./components/Settings";
import { Chat } from "./components/Chat";
import { AgentManager } from "./components/AgentManager";
import { WindowManager } from "./components/WindowManager";
import { useStore } from "./lib/store";
import { ipc } from "./lib/ipc";
import {
  useProjectLifecycle,
  flushPendingSave,
} from "./lib/session-manager";
import { useSubagentManager } from "./lib/subagent-manager";
import { useModelBootstrap } from "./lib/model-bootstrap";
import { ToastHost } from "./lib/toast";
import { chatSplit, DEFAULT_CHAT_RATIO, MIN_CHAT_WIDTH } from "./lib/wm";

function usePersonalitiesBootstrap() {
  const setPersonalities = useStore((s) => s.setPersonalities);
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const roster = await ipc.loadPersonalities();
        if (!cancelled) setPersonalities(roster);
      } catch (err) {
        // Best-effort: Agent Manager will show a one-time error on open.
        // The agent loop's `resolve_personality` falls back to the
        // default if the roster is missing, so sessions keep working.
        console.error("Failed to load personalities:", err);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [setPersonalities]);
}

function App() {
  const { settingsOpen, subagents, agentManagerOpen, chatRatio } = useStore();
  useProjectLifecycle();
  useModelBootstrap();
  usePersonalitiesBootstrap();
  useSubagentManager();

  useEffect(() => {
    const handler = () => {
      // Best-effort. Tauri webview does not always fire beforeunload
      // reliably on app close; the in-memory store is the source of
      // truth until the next save.
      flushPendingSave().catch(() => {});
    };
    window.addEventListener("beforeunload", handler);
    return () => window.removeEventListener("beforeunload", handler);
  }, []);

  // Track the right-hand region size so the WindowManager lays
  // tiles out against the actual available space.
  const contentRef = useRef<HTMLDivElement | null>(null);
  const [contentSize, setContentSize] = useState({ width: 0, height: 0 });
  useEffect(() => {
    if (!contentRef.current) return;
    const observer = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (!entry) return;
      setContentSize({
        width: entry.contentRect.width,
        height: entry.contentRect.height,
      });
    });
    observer.observe(contentRef.current);
    setContentSize({
      width: contentRef.current.clientWidth,
      height: contentRef.current.clientHeight,
    });
    return () => observer.disconnect();
  }, []);

  // Chat column width: null until the first subagent spawns (chat
  // owns the full width, centered). Locks the moment any subagent
  // goes live, then reflows back to full width the moment every
  // subagent reaches a terminal state (done / error / cancelled).
  //
  // The "any working" predicate (not the raw count) is what makes
  // the chat float back: a finished subagent record is preserved in
  // the registry for inspection via `list_subagents`, but the tiling
  // region collapses once no live work is happening. The chat
  // already carries the final answer — the `delegate_subagent` tool
  // folds the subagent's capped final_content into the orchestrator
  // conversation, so the subagent viewer doesn't need to stay open
  // for the user to see the result.
  const hasLiveWork = subagents.some((s) => s.status === "working");
  const ratio = chatRatio ?? DEFAULT_CHAT_RATIO;
  const tiling = hasLiveWork;
  const split = tiling
    ? chatSplit(contentSize.width, ratio, MIN_CHAT_WIDTH)
    : { chatWidth: contentSize.width, subagentWidth: 0 };

  return (
    <div className="h-screen w-screen flex flex-col bg-surface-0 overflow-hidden relative">
      <TitleBar />
      {/* Stats row beneath the title bar, centered so it sits visually
          under Bip (who lives in the title bar at center). */}
      <div className="px-3 flex justify-center">
        <ChatHeader />
      </div>
      <div className="flex flex-1 overflow-hidden">
        <Sidebar />
        <div ref={contentRef} className="flex flex-1 min-w-0 overflow-hidden">
          {/* Chat column (left). Locks to its width once a subagent
           *  spawns; never auto-resizes after. */}
          <div
            className="flex flex-col overflow-hidden shrink-0"
            style={{ width: split.chatWidth }}
          >
            <Chat />
          </div>
          {/* Subagent region (right). Empty until the first
           *  subagent spawns; the equal-grid tiles own the rest. */}
          {tiling && (
            <WindowManager
              width={split.subagentWidth}
              height={contentSize.height}
            />
          )}
        </div>
      </div>
      {settingsOpen && <Settings />}
      {agentManagerOpen && <AgentManager />}
      <ToastHost />
    </div>
  );
}

export default App;
