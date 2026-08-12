// Agent Manager — per-project roster editor (roster + max agents per
// session). Builtin personas are editable but not deletable; the backend
// enforces that on save. Saved via `save_agent_roster`; the store is
// refreshed with the normalized roster the backend returns.

import { useEffect, useState } from "react";
import { useStore } from "../lib/store";
import { ipc } from "../lib/ipc";
import type { AgentPersonality, AgentRoster } from "../lib/ipc";
import { X, Plus, Trash2, Bot } from "lucide-react";
import { showToast } from "../lib/toast";

const ROLES = ["Orchestrator", "Coder", "Architect", "Researcher", "HealthMonitor"] as const;

export function AgentManager() {
  const open = useStore((s) => s.agentManagerOpen);
  const setOpen = useStore((s) => s.setAgentManagerOpen);
  const workspacePath = useStore((s) => s.workspacePath);
  const roster = useStore((s) => s.agentRoster);
  const setRoster = useStore((s) => s.setAgentRoster);

  const [draft, setDraft] = useState<AgentRoster | null>(null);
  const [saving, setSaving] = useState(false);

  // Sync the draft whenever the modal opens or the roster changes.
  useEffect(() => {
    if (open && roster) {
      setDraft(JSON.parse(JSON.stringify(roster)));
    }
  }, [open, roster]);

  if (!open) return null;

  const updateAgent = (index: number, patch: Partial<AgentPersonality>) => {
    setDraft((d) => {
      if (!d) return d;
      const agents = d.agents.map((a, i) => (i === index ? { ...a, ...patch } : a));
      return { ...d, agents };
    });
  };

  const addAgent = () => {
    setDraft((d) => {
      if (!d) return d;
      const n = d.agents.length + 1;
      const agent: AgentPersonality = {
        id: `agent-${Date.now().toString(36)}`,
        name: `agent ${n}`,
        role: "Coder",
        systemPrompt: "You are a capable coding agent working inside the user's workspace.",
        builtin: false,
      };
      return { ...d, agents: [...d.agents, agent] };
    });
  };

  const removeAgent = (index: number) => {
    setDraft((d) => {
      if (!d) return d;
      return { ...d, agents: d.agents.filter((_, i) => i !== index) };
    });
  };

  const handleSave = async () => {
    if (!workspacePath || !draft) return;
    setSaving(true);
    try {
      const saved = await ipc.saveAgentRoster(workspacePath, draft);
      setRoster(saved);
      setOpen(false);
    } catch (err) {
      showToast(`Failed to save roster: ${err}`);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/70" onClick={() => setOpen(false)} />
      <div className="relative glass-mist w-[560px] max-h-[80vh] flex flex-col border border-border-strong animate-fade-in">
        <div className="flex items-center gap-2 px-4 py-3 border-b border-border-subtle">
          <Bot size={14} className="text-white-50" />
          <span className="text-[13px] font-medium text-white">Agent Manager</span>
          <span className="text-[11px] text-white-15 ml-auto">
            {draft?.agents.length ?? 0} agents · max {draft?.maxAgentsPerSession ?? 1} per session
          </span>
          <button
            onClick={() => setOpen(false)}
            className="text-white-30 hover:text-white p-1"
            aria-label="Close"
          >
            <X size={14} />
          </button>
        </div>

        <div className="flex-1 overflow-y-auto px-4 py-3 space-y-3">
          <div className="flex items-center gap-2">
            <label className="text-[11px] text-white-30 whitespace-nowrap">
              Max agents per session
            </label>
            <input
              type="number"
              min={1}
              max={8}
              value={draft?.maxAgentsPerSession ?? 1}
              onChange={(e) => {
                const v = Math.max(1, Math.min(8, Number(e.target.value) || 1));
                setDraft((d) => (d ? { ...d, maxAgentsPerSession: v } : d));
              }}
              className="bg-surface-2 border border-border-subtle rounded px-2 py-1 text-[12px] text-white w-16 outline-none focus:border-border-strong"
            />
          </div>

          {draft?.agents.map((agent, i) => (
            <div key={agent.id} className="bg-surface-2 border border-border-subtle rounded p-3 space-y-2">
              <div className="flex items-center gap-2">
                <input
                  value={agent.name}
                  onChange={(e) => updateAgent(i, { name: e.target.value })}
                  className="flex-1 bg-transparent border border-transparent hover:border-border-subtle focus:border-border-strong rounded px-2 py-1 text-[13px] text-white outline-none"
                />
                <select
                  value={agent.role}
                  onChange={(e) => updateAgent(i, { role: e.target.value as AgentPersonality["role"] })}
                  className="bg-surface-3 border border-border-subtle rounded px-2 py-1 text-[11px] text-white outline-none cursor-pointer"
                >
                  {ROLES.map((r) => (
                    <option key={r} value={r} className="bg-surface-2 text-white">
                      {r}
                    </option>
                  ))}
                </select>
                {!agent.builtin && (
                  <button
                    onClick={() => removeAgent(i)}
                    className="text-white-30 hover:text-status-error p-1"
                    title="Delete agent"
                  >
                    <Trash2 size={13} />
                  </button>
                )}
                {agent.builtin && (
                  <span className="text-[10px] text-white-15 border border-border-subtle rounded px-1.5 py-0.5">
                    builtin
                  </span>
                )}
              </div>
              <textarea
                value={agent.systemPrompt}
                onChange={(e) => updateAgent(i, { systemPrompt: e.target.value })}
                rows={3}
                className="w-full bg-surface-3 border border-border-subtle rounded px-2 py-1.5 text-[12px] text-white-50 outline-none focus:border-border-strong resize-none"
                placeholder="System prompt for this agent"
              />
            </div>
          ))}

          <button
            onClick={addAgent}
            className="w-full flex items-center justify-center gap-1.5 py-2 text-[12px] text-white-30 hover:text-white border border-dashed border-border-subtle rounded transition-colors"
          >
            <Plus size={13} />
            Add agent
          </button>
        </div>

        <div className="flex items-center justify-end gap-2 px-4 py-3 border-t border-border-subtle">
          <button
            onClick={() => setOpen(false)}
            className="px-3 py-1.5 text-[12px] text-white-30 hover:text-white"
          >
            Cancel
          </button>
          <button
            onClick={handleSave}
            disabled={saving}
            className="px-4 py-1.5 text-[12px] text-white bg-surface-4 hover:bg-border-strong rounded transition-colors disabled:opacity-50"
          >
            {saving ? "Saving..." : "Save roster"}
          </button>
        </div>
      </div>
    </div>
  );
}
