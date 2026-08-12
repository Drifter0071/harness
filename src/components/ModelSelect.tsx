// Shared saved-model dropdown for the chat header + composer selector
// row. Picking a model persists it as active via the backend.

import { useEffect, useRef, useState } from "react";
import { useStore } from "../lib/store";
import { ipc } from "../lib/ipc";
import { ChevronDown, Lock, AlertTriangle, Settings } from "lucide-react";

export function ModelSelect({ compact = false }: { compact?: boolean }) {
  const {
    savedModels,
    activeModelId,
    setActiveModelId,
    setSettingsOpen,
    setSettingsFocusModelId,
  } = useStore();
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  const active = savedModels.find((m) => m.id === activeModelId) ?? null;

  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [open]);

  const pick = async (id: string) => {
    setOpen(false);
    if (id === activeModelId) return;
    setActiveModelId(id);
    try {
      await ipc.setActiveModel(id);
    } catch {
      // Best-effort persistence; the in-memory selection still applies.
    }
  };

  const openSettings = () => {
    setOpen(false);
    setSettingsFocusModelId(activeModelId);
    setSettingsOpen(true);
  };

  return (
    <div className="relative" ref={ref}>
      <button
        onClick={() => setOpen((o) => !o)}
        className="flex items-center gap-1.5 text-[12px] text-white-50 hover:text-white transition-colors cursor-pointer"
        title="Switch model"
      >
        {active ? (
          <>
            <span className="text-white">{active.label}</span>
            {active.hasKey ? (
              <Lock size={11} className="text-status-active" />
            ) : (
              <AlertTriangle size={11} className="text-status-warning" />
            )}
          </>
        ) : (
          <span className="text-white-30">No active model</span>
        )}
        <ChevronDown size={12} className={open ? "rotate-180 transition-transform" : "transition-transform"} />
      </button>

      {open && (
        <div className={`absolute z-40 mt-1.5 bg-surface-2 border border-border-strong rounded-lg shadow-xl overflow-hidden animate-fade-in ${compact ? "left-0 min-w-[240px]" : "left-0 min-w-[280px]"}`}>
          <div className="max-h-64 overflow-y-auto py-1">
            {savedModels.length === 0 && (
              <div className="px-3 py-2.5 text-[12px] text-white-30">
                No saved models yet.
              </div>
            )}
            {savedModels.map((m) => (
              <button
                key={m.id}
                onClick={() => pick(m.id)}
                className={`w-full text-left px-3 py-2 flex items-center gap-2 hover:bg-surface-3 transition-colors ${
                  m.id === activeModelId ? "bg-surface-3" : ""
                }`}
              >
                <span className={`text-[10px] ${m.id === activeModelId ? "text-status-active" : "text-white-15"}`}>
                  {m.id === activeModelId ? "●" : "○"}
                </span>
                <span className="flex-1 min-w-0">
                  <span className="block text-[13px] text-white truncate">{m.label}</span>
                  <span className="block text-[11px] text-white-30 truncate">{m.modelName}</span>
                </span>
                {m.keyUnreachable ? (
                  <AlertTriangle size={12} className="text-status-warning shrink-0" aria-label="Key unreachable" />
                ) : m.hasKey ? (
                  <Lock size={11} className="text-status-active shrink-0" aria-label="Key saved" />
                ) : (
                  <span className="text-[10px] text-status-warning shrink-0">no key</span>
                )}
              </button>
            ))}
          </div>
          <div className="border-t border-border-subtle px-3 py-2">
            <button
              onClick={openSettings}
              className="flex items-center gap-1.5 text-[12px] text-white-30 hover:text-white transition-colors"
            >
              <Settings size={12} />
              Manage models in Settings
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
