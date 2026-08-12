import { useEffect } from "react";
import { ActiveJobBar } from "./components/ActiveJobBar";
import { DynamicCanvas } from "./components/DynamicCanvas";
import { Sidebar } from "./components/Sidebar";
import { Settings } from "./components/Settings";
import { Chat } from "./components/Chat";
import { AgentManager } from "./components/AgentManager";
import { useStore } from "./lib/store";
import { useProjectLifecycle, flushPendingSave } from "./lib/session-manager";
import { useModelBootstrap } from "./lib/model-bootstrap";
import { ToastHost } from "./lib/toast";

function App() {
  const { settingsOpen, canvasPanels, agentManagerOpen } = useStore();
  useProjectLifecycle();
  useModelBootstrap();

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
      <ActiveJobBar />
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
