import { useState } from "react";
import { useStore } from "../lib/store";
import { ipc } from "../lib/ipc";
import { Settings, Circle, FolderPlus } from "lucide-react";

export function ActiveJobBar() {
  const {
    jobActivities,
    activeAgents,
    setSettingsOpen,
    setWorkspacePath,
  } = useStore();
  const [hovered, setHovered] = useState(false);
  const hasActivity = jobActivities.length > 0;

  const handleOpenFolder = async () => {
    try {
      const path = await ipc.pickProjectFolder();
      if (path) setWorkspacePath(path);
    } catch (err) {
      console.error("Failed to open folder:", err);
    }
  };

  return (
    <div className="fixed top-3 left-1/2 -translate-x-1/2 z-40 pointer-events-auto">
        <div className="glass-mist glass-mist--pill flex items-center gap-0.5 px-1.5 py-1">
          <button
            onClick={handleOpenFolder}
            className="glass-quartz glass-quartz--pill flex items-center justify-center w-[34px] h-[34px] text-white-50 hover:text-white"
            title="Open a folder as a workspace"
            aria-label="Open folder"
          >
            <FolderPlus size={16} />
          </button>
          <div className="w-px h-4 bg-border-subtle mx-0.5" />
          <div
            onMouseEnter={() => setHovered(true)}
            onMouseLeave={() => setHovered(false)}
          >
            {hasActivity ? (
              <div className="relative">
                <div className="flex items-center gap-1.5 px-2.5 py-1 rounded-full bg-accent-green/10 border border-accent-green/20 cursor-default">
                  <Circle size={6} className="fill-accent-green text-accent-green animate-pulse" />
                  <span className="text-[11px] text-white-50 max-w-[160px] truncate">{jobActivities[0]}</span>
                </div>
                {hovered && (
                  <div className="absolute top-full left-1/2 -translate-x-1/2 mt-2 glass-mist p-3 min-w-[280px] animate-fade-in z-50">
                    <div className="text-[11px] text-white-30 uppercase tracking-wider mb-2">Active Tasks</div>
                    <div className="space-y-2">
                      {jobActivities.map((activity, i) => (
                        <div key={i} className="flex items-center gap-2 text-[13px] text-white-50">
                          <Circle size={4} className="fill-accent-green text-accent-green flex-shrink-0" />
                          {activity}
                        </div>
                      ))}
                    </div>
                    {activeAgents.length > 0 && (
                      <div className="border-t border-border-subtle mt-2 pt-2">
                        <div className="text-[11px] text-white-30 uppercase tracking-wider mb-2">Agents</div>
                        <div className="space-y-2">
                          {activeAgents.map((agent) => (
                            <div key={agent.id} className="flex items-center gap-2 text-[13px] text-white-30">
                              <Circle
                                size={4}
                                className={`flex-shrink-0 ${
                                  agent.status === "Working"
                                    ? "fill-accent-green text-accent-green"
                                    : "fill-white-15 text-white-15"
                                }`}
                              />
                              <span className="truncate">{agent.name}</span>
                              <span className="text-[11px] text-white-15 ml-auto">{agent.role}</span>
                            </div>
                          ))}
                        </div>
                      </div>
                    )}
                  </div>
                )}
              </div>
            ) : (
              <div className="flex items-center gap-1.5 px-2.5 py-1 rounded-full cursor-default">
                <Circle size={6} className="fill-white-15 text-white-15" />
                <span className="text-[11px] text-white-15">Idle</span>
              </div>
            )}
          </div>
          <div className="w-px h-4 bg-border-subtle mx-0.5" />
          <button
            onClick={() => setSettingsOpen(true)}
            className="glass-quartz glass-quartz--pill flex items-center justify-center w-[34px] h-[34px] text-white-50 hover:text-white"
            title="Settings"
          >
            <Settings size={16} />
          </button>
        </div>
      </div>
  );
}
