// Agent Manager — global subagent personality editor.
//
// The roster lives at <data_dir>/Harness/personalities.json (shared
// across every project the user opens) and is loaded once on app
// mount. The editor lets the user:
//
//   - rename a personality (free text)
//   - pick a role from the four subagent roles
//   - pick a saved model from the Settings → Models list
//     (or "Use chat model" for the default)
//   - edit the system prompt, with a one-click "Reset to default"
//     that re-applies the role's seed prompt (the role's standard
//     prompt)
//
// The Orchestrator is NOT in the roster — it's the chat agent, with
// a hardcoded prompt in `src-tauri/src/agent.rs::ORCHESTRATOR_PROMPT`.
// Builtin subagent personalities (one per subagent role) are seeded
// on first contact and are editable but cannot be deleted.

import { useEffect, useMemo, useState, type CSSProperties } from "react";
import { useStore } from "../lib/store";
import { ipc } from "../lib/ipc";
import {
  DEFAULT_PROMPTS,
  type AgentPersonality,
  type AgentRole,
  type PersonalityRoster,
} from "../lib/ipc";
import { X, Plus, Trash2, Bot, RotateCcw } from "lucide-react";
import { showToast } from "../lib/toast";

/** Roles exposed as user-editable subagent personalities. The
 *  Orchestrator is the chat agent and is never a personality. */
type SubagentRole = Exclude<AgentRole, "Orchestrator">;

const ROLES: SubagentRole[] = [
  "Coder",
  "Architect",
  "Researcher",
  "HealthMonitor",
];

export function AgentManager() {
  const open = useStore((s) => s.agentManagerOpen);
  const setOpen = useStore((s) => s.setAgentManagerOpen);
  const roster = useStore((s) => s.personalities);
  const setRoster = useStore((s) => s.setPersonalities);
  const savedModels = useStore((s) => s.savedModels);

  const [draft, setDraft] = useState<PersonalityRoster | null>(null);
  const [saving, setSaving] = useState(false);

  // Sync the draft whenever the modal opens or the roster changes.
  useEffect(() => {
    if (open && roster) {
      setDraft(JSON.parse(JSON.stringify(roster)));
    }
  }, [open, roster]);

  if (!open) return null;

  const updatePersonality = (
    index: number,
    patch: Partial<AgentPersonality>,
  ) => {
    setDraft((d) => {
      if (!d) return d;
      const personalities = d.personalities.map((p, i) =>
        i === index ? { ...p, ...patch } : p,
      );
      return { ...d, personalities };
    });
  };

  const addPersonality = () => {
    setDraft((d) => {
      if (!d) return d;
      // Pick the first role that doesn't already have a personality.
      const used = new Set(d.personalities.map((p) => p.role));
      const freeRole =
        ROLES.find((r) => !used.has(r)) ?? "Coder";
      const personality: AgentPersonality = {
        id: `user-${Date.now().toString(36)}`,
        name: `${freeRole} (custom)`,
        role: freeRole,
        systemPrompt: DEFAULT_PROMPTS[freeRole],
        modelId: null,
        builtin: false,
      };
      return {
        ...d,
        personalities: [...d.personalities, personality],
      };
    });
  };

  const removePersonality = (index: number) => {
    setDraft((d) => {
      if (!d) return d;
      const next = d.personalities.filter((_, i) => i !== index);
      // If we removed the default, point it at the first remaining row.
      const removed = d.personalities[index];
      let defaultPersonalityId = d.defaultPersonalityId;
      if (removed.id === defaultPersonalityId && next.length > 0) {
        defaultPersonalityId = next[0].id;
      }
      return { ...d, personalities: next, defaultPersonalityId };
    });
  };

  const setDefault = (id: string) => {
    setDraft((d) => (d ? { ...d, defaultPersonalityId: id } : d));
  };

  const handleRoleChange = (index: number, role: SubagentRole) => {
    setDraft((d) => {
      if (!d) return d;
      const personalities = d.personalities.map((p, i) => {
        if (i !== index) return p;
        // When the user changes a personality's role, prefill the
        // system prompt with the role's default — the most useful
        // default and the one the user can revert to via "Reset to
        // default". The user's previous text is dropped; this is
        // an explicit change of role, so a fresh prompt is expected.
        return {
          ...p,
          role,
          systemPrompt: DEFAULT_PROMPTS[role],
        };
      });
      return { ...d, personalities };
    });
  };

  const handleResetPrompt = (index: number) => {
    const p = draft?.personalities[index];
    if (!p) return;
    // p.role is one of the four subagent roles by construction — the
    // role <select> only offers those. `DEFAULT_PROMPTS` is keyed by
    // subagent roles, so the cast is sound.
    updatePersonality(index, {
      systemPrompt: DEFAULT_PROMPTS[p.role as SubagentRole],
    });
  };

  const handleSave = async () => {
    if (!draft) return;
    setSaving(true);
    try {
      const saved = await ipc.savePersonalities(draft);
      setRoster(saved);
      setOpen(false);
    } catch (err) {
      showToast(`Failed to save personalities: ${err}`);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/70" onClick={() => setOpen(false)} />
      <div className="relative glass-mist w-[640px] max-h-[80vh] flex flex-col border border-border-strong animate-fade-in">
        <div className="flex items-center gap-2 px-4 py-3 border-b border-border-subtle">
          <Bot size={14} className="text-white-50" />
          <span className="text-[13px] font-medium text-white">
            Agent Manager
          </span>
          <span className="text-[11px] text-white-15 ml-auto">
            {draft?.personalities.length ?? 0} subagent personalities · global
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
          {draft?.personalities.map((p, i) => {
            const isDefault =
              draft.defaultPersonalityId === p.id;
            return (
              <PersonalityRow
                key={p.id}
                index={i}
                personality={p}
                isDefault={isDefault}
                savedModels={savedModels}
                onChange={(patch) => updatePersonality(i, patch)}
                onRoleChange={(role) => handleRoleChange(i, role)}
                onResetPrompt={() => handleResetPrompt(i)}
                onRemove={() => removePersonality(i)}
                onSetDefault={() => setDefault(p.id)}
              />
            );
          })}

          <button
            onClick={addPersonality}
            className="w-full flex items-center justify-center gap-1.5 py-2 text-[12px] text-white-30 hover:text-white border border-dashed border-border-subtle rounded transition-colors"
          >
            <Plus size={13} />
            Add personality
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
            className="glass-quartz glass-quartz--pill px-4 py-1.5 text-[12px] text-white"
          >
            {saving ? "Saving..." : "Save personalities"}
          </button>
        </div>
      </div>
    </div>
  );
}

function PersonalityRow({
  personality,
  isDefault,
  savedModels,
  onChange,
  onRoleChange,
  onResetPrompt,
  onRemove,
  onSetDefault,
}: {
  index: number;
  personality: AgentPersonality;
  isDefault: boolean;
  savedModels: { id: string; label: string; modelName: string }[];
  onChange: (patch: Partial<AgentPersonality>) => void;
  onRoleChange: (role: SubagentRole) => void;
  onResetPrompt: () => void;
  onRemove: () => void;
  onSetDefault: () => void;
}) {
  // Is the prompt identical to the role's default? If so, hide the
  // reset link (no-op) — the "Reset to default" button is only useful
  // once the user has actually edited the prompt. The cast is sound
  // because the row's role <select> only offers subagent roles.
  const defaultPrompt = DEFAULT_PROMPTS[personality.role as SubagentRole];
  const isAtDefault = useMemo(
    () => personality.systemPrompt === defaultPrompt,
    [personality.systemPrompt, defaultPrompt],
  );

  return (
    <div className="bg-surface-2 border border-border-subtle rounded p-3 space-y-2">
      <div className="flex items-center gap-2">
        <input
          value={personality.name}
          onChange={(e) => onChange({ name: e.target.value })}
          className="flex-1 bg-transparent border border-transparent hover:border-border-subtle focus:border-border-strong rounded px-2 py-1 text-[13px] text-white outline-none"
          placeholder="Personality name"
          aria-label="Personality name"
        />
        <select
          value={personality.role}
          onChange={(e) => onRoleChange(e.target.value as SubagentRole)}
          className="bg-surface-3 border border-border-subtle rounded px-2 py-1 text-[11px] text-white outline-none cursor-pointer"
          aria-label="Role"
        >
          {ROLES.map((r) => (
            <option key={r} value={r} className="bg-surface-2 text-white">
              {r}
            </option>
          ))}
        </select>
        <select
          value={personality.modelId ?? ""}
          onChange={(e) =>
            onChange({ modelId: e.target.value || null })
          }
          className="bg-surface-3 border border-border-subtle rounded px-2 py-1 text-[11px] text-white outline-none cursor-pointer max-w-[180px]"
          aria-label="Model"
          title="The saved model used when this personality runs"
        >
          <option value="">Use chat model</option>
          {savedModels.map((m) => (
            <option key={m.id} value={m.id}>
              {m.label}
            </option>
          ))}
        </select>
        {!personality.builtin && (
          <button
            onClick={onRemove}
            className="text-white-30 hover:text-status-error p-1"
            title="Delete personality"
            aria-label="Delete personality"
          >
            <Trash2 size={13} />
          </button>
        )}
        {personality.builtin && (
          <span className="text-[10px] text-white-15 border border-border-subtle rounded px-1.5 py-0.5">
            builtin
          </span>
        )}
      </div>

      <div className="flex items-center gap-2">
        <button
          onClick={onSetDefault}
          disabled={isDefault}
          className="text-[10px] uppercase tracking-[0.14em] text-white-15 hover:text-white disabled:text-accent-mint disabled:opacity-100 disabled:cursor-default"
          title={
            isDefault
              ? "This is the default personality for new sessions"
              : "Make this the default personality for new sessions"
          }
          style={{ "--slider-fill": "0%" } as CSSProperties}
        >
          {isDefault ? "Default" : "Set as default"}
        </button>
        <span className="text-[10px] text-white-15 ml-auto">
          {personality.builtin ? "Seeded by Harness" : "Custom"}
        </span>
      </div>

      <div className="space-y-1">
        <div className="flex items-center justify-between">
          <label className="text-[10px] uppercase tracking-[0.14em] text-white-15">
            System prompt
          </label>
          <button
            onClick={onResetPrompt}
            disabled={isAtDefault}
            className="flex items-center gap-1 text-[10px] text-white-15 hover:text-white disabled:opacity-40 disabled:cursor-default"
            title={
              isAtDefault
                ? "Already using the default prompt for this role"
                : "Reset to the default prompt for this role"
            }
          >
            <RotateCcw size={10} />
            Reset to default
          </button>
        </div>
        <textarea
          value={personality.systemPrompt}
          onChange={(e) => onChange({ systemPrompt: e.target.value })}
          rows={3}
          className="w-full bg-surface-3 border border-border-subtle rounded px-2 py-1.5 text-[12px] text-white-50 outline-none focus:border-border-strong resize-none"
          placeholder={`Standard ${personality.role} prompt…`}
        />
      </div>
    </div>
  );
}
