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
      <TitleBar />
      {/* Top zone: pill row (row 1) + stats row (row 2) share the same
          3-column tracks (1fr / auto / 1fr). The center column auto-sizes
          to fit Bip and the stats row, so the stats line is visually
          anchored directly under Bip and never drifts to one side. */}
      <div className="px-3 grid grid-cols-[1fr_auto_1fr] gap-2 items-start">
        <ActiveJobBar />
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
