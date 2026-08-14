import { useEffect } from "react";
import { ActiveJobBar } from "./components/ActiveJobBar";
import { TitleBar } from "./components/TitleBar";
import { ChatHeader } from "./components/ChatHeader";
import { DynamicCanvas } from "./components/DynamicCanvas";
import { Sidebar } from "./components/Sidebar";
import { Settings } from "./components/Settings";
import { Chat } from "./components/Chat";
import { AgentManager } from "./components/AgentManager";
import { useStore } from "./lib/store";
import { ipc } from "./lib/ipc";
import { useProjectLifecycle, flushPendingSave } from "./lib/session-manager";
import { useModelBootstrap } from "./lib/model-bootstrap";
import { ToastHost } from "./lib/toast";

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
  const { settingsOpen, canvasPanels, agentManagerOpen } = useStore();
  useProjectLifecycle();
  useModelBootstrap();
  usePersonalitiesBootstrap();

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

  return (
    <div className="h-screen w-screen flex flex-col bg-surface-0 overflow-hidden relative">
      <TitleBar />
      {/* Stats row (model / last response / context used) sits directly
          under the topbar. The panel buttons + Bip are inside the
          TitleBar itself — no more pill row above the stats. */}
      <div className="flex justify-center">
        <ChatHeader />
      </div>
      <div className="flex flex-1 overflow-hidden">
        <Sidebar />
        <div className="flex-1 flex flex-col overflow-hidden">
          {canvasPanels.length > 0 ? <DynamicCanvas /> : <Chat />}
        </div>
      </div>
      {settingsOpen && <Settings />}
      {agentManagerOpen && <AgentManager />}
      <ToastHost />
    </div>
  );
}

export default App;
