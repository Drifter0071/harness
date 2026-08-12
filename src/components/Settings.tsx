// Settings — two-pane model setup (spec §7.1).
//
// Left: saved models list (active marker + key status). Right: editor
// with label / provider / format / base URL / model / API key (masked,
// never round-trips), test connection, thinking + reasoning prefs.
// Changes are drafts until Save; Save writes models.json + keyring.

import { useEffect, useRef, useState } from "react";
import { useStore } from "../lib/store";
import { ipc } from "../lib/ipc";
import type { ModelPrefs, ReasoningEffort, TestResult } from "../lib/ipc";
import { PROVIDER_PRESETS } from "../lib/models";
import { showToast } from "../lib/toast";
import {
  X,
  Plus,
  Lock,
  AlertTriangle,
  ChevronDown,
  RefreshCw,
  XCircle,
  Eye,
  EyeOff,
  CheckCircle2,
  Trash2,
  CircleDot,
  Play,
} from "lucide-react";
import { nanoid } from "nanoid";

const inputCls =
  "w-full px-3 py-2 bg-surface-2 border border-border-subtle rounded text-[13px] text-white outline-none focus:border-border-strong transition-colors placeholder:text-white-15";

const FORMAT_OPTIONS = [
  { value: "anthropic-messages", label: "Anthropic Messages" },
  { value: "openai-chat-completions", label: "OpenAI Chat Completions" },
];

interface EditorDraft {
  id: string;
  isNew: boolean;
  label: string;
  provider: string;
  format: string;
  baseUrl: string;
  modelName: string;
  hasKey: boolean;
  keyUnreachable: boolean;
}

function draftFromModel(m: {
  id: string;
  label: string;
  provider: string;
  format: string;
  baseUrl: string;
  modelName: string;
  hasKey: boolean;
  keyUnreachable: boolean;
}): EditorDraft {
  return {
    id: m.id,
    isNew: false,
    label: m.label,
    provider: m.provider,
    format: m.format,
    baseUrl: m.baseUrl,
    modelName: m.modelName,
    hasKey: m.hasKey,
    keyUnreachable: m.keyUnreachable,
  };
}

function newDraft(): EditorDraft {
  const preset = PROVIDER_PRESETS.anthropic;
  return {
    id: nanoid(),
    isNew: true,
    label: "",
    provider: "anthropic",
    format: preset.format,
    baseUrl: preset.baseUrl,
    modelName: preset.models[0] ?? "",
    hasKey: false,
    keyUnreachable: false,
  };
}

export function Settings() {
  const {
    setSettingsOpen,
    savedModels,
    setSavedModels,
    activeModelId,
    setActiveModelId,
    settingsFocusModelId,
    setSettingsFocusModelId,
  } = useStore();

  const [draft, setDraft] = useState<EditorDraft | null>(null);
  const [dirty, setDirty] = useState(false);
  const [apiKeyInput, setApiKeyInput] = useState("");
  const [keyEditing, setKeyEditing] = useState(false);
  const [showKeyText, setShowKeyText] = useState(false);
  const [prefs, setPrefs] = useState<ModelPrefs>({ thinkingOn: false, reasoningEffort: "medium" });
  const [testState, setTestState] = useState<{ status: "idle" | "testing" | "done"; result?: TestResult }>({ status: "idle" });
  const [saving, setSaving] = useState(false);
  const keyInputRef = useRef<HTMLInputElement>(null);

  // Initial load: models list, then select the focused model (opened
  // from the composer's "Add an API key" path) or the first one.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const list = await ipc.listSavedModels();
        if (cancelled) return;
        setSavedModels(list);
        if (list.length > 0) {
          const focusId = settingsFocusModelId && list.some((m) => m.id === settingsFocusModelId)
            ? settingsFocusModelId
            : list[0].id;
          selectDraft(draftFromModel(list.find((m) => m.id === focusId)!));
        } else {
          setDraft(newDraft());
        }
      } catch (err) {
        if (!cancelled) showToast(`Failed to load saved models: ${err}`);
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const selectDraft = async (d: EditorDraft) => {
    setDraft(d);
    setDirty(false);
    setApiKeyInput("");
    setKeyEditing(false);
    setShowKeyText(false);
    setTestState({ status: "idle" });
    try {
      setPrefs(await ipc.getModelPrefs(d.id));
    } catch {
      setPrefs({ thinkingOn: false, reasoningEffort: "medium" });
    }
  };

  const update = (patch: Partial<EditorDraft>) => {
    setDraft((d) => (d ? { ...d, ...patch } : d));
    setDirty(true);
    setTestState({ status: "idle" });
  };

  const onProviderChange = (provider: string) => {
    const preset = PROVIDER_PRESETS[provider.toLowerCase()];
    setDraft((d) => {
      if (!d) return d;
      return {
        ...d,
        provider,
        baseUrl: preset?.baseUrl ?? d.baseUrl,
        format: preset?.format ?? d.format,
        modelName: preset?.models[0] ?? d.modelName,
      };
    });
    setDirty(true);
  };

  const cycleModel = () => {
    const preset = PROVIDER_PRESETS[draft?.provider?.toLowerCase() ?? ""];
    if (!draft || !preset || preset.models.length === 0) return;
    const i = preset.models.indexOf(draft.modelName);
    update({ modelName: preset.models[(i + 1) % preset.models.length] });
  };

  const persistPrefs = async (next: ModelPrefs) => {
    setPrefs(next);
    if (draft) {
      try {
        await ipc.setModelPrefs(draft.id, next);
      } catch {
        // non-fatal
      }
    }
  };

  const saveDraft = async (): Promise<boolean> => {
    if (!draft) return false;
    if (draft.isNew && (!draft.modelName.trim() || !draft.baseUrl.trim())) {
      showToast("Model name and Base URL are required for a new model.");
      return false;
    }
    setSaving(true);
    try {
      const saved = await ipc.saveSavedModel(
        {
          id: draft.id,
          label: draft.label.trim() || draft.modelName,
          provider: draft.provider,
          format: draft.format,
          baseUrl: draft.baseUrl,
          modelName: draft.modelName,
        },
        apiKeyInput // "" = keep existing; non-empty = overwrite
      );
      setDraft(draftFromModel(saved));
      setDirty(false);
      setApiKeyInput("");
      setKeyEditing(false);
      setShowKeyText(false);
      setSavedModels(
        savedModels.some((m) => m.id === saved.id)
          ? savedModels.map((m) => (m.id === saved.id ? saved : m))
          : [...savedModels, saved]
      );
      return true;
    } catch (err) {
      showToast(`Could not save model: ${err}`);
      return false;
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async () => {
    if (!draft) return;
    if (!window.confirm(`Delete model "${draft.label || draft.modelName}"? The API key is also removed.`)) return;
    try {
      await ipc.deleteSavedModel(draft.id);
      const next = savedModels.filter((m) => m.id !== draft.id);
      setSavedModels(next);
      if (activeModelId === draft.id) {
        setActiveModelId(null);
        await ipc.setActiveModel(null).catch(() => {});
      }
      if (next.length > 0) {
        await selectDraft(draftFromModel(next[0]));
      } else {
        setDraft(newDraft());
      }
      setTestState({ status: "idle" });
    } catch (err) {
      showToast(`Could not delete model: ${err}`);
    }
  };

  const handleSetActive = async () => {
    if (!draft) return;
    if (dirty && !(await saveDraft())) return;
    setActiveModelId(draft.id);
    await ipc.setActiveModel(draft.id).catch(() => {});
    showToast(`Active model: ${draft.label || draft.modelName}`);
  };

  const handleTest = async () => {
    if (!draft) return;
    if (dirty && !(await saveDraft())) return;
    setTestState({ status: "testing" });
    try {
      const result = await ipc.testSavedModel(draft.id);
      setTestState({ status: "done", result });
    } catch (err) {
      setTestState({ status: "done", result: { ok: false, kind: "internal", message: String(err) } });
    }
  };

  const handleResetSecretStore = async () => {
    if (!window.confirm("Reset the secret store? All saved API keys will be removed from the OS keyring. Model entries are kept; you'll need to re-add keys.")) return;
    try {
      await ipc.resetSecretStore();
      const list = await ipc.listSavedModels();
      setSavedModels(list);
      if (draft) {
        await selectDraft({ ...draft, hasKey: false, keyUnreachable: false });
      }
      showToast("Secret store reset. Re-add API keys in Settings.");
    } catch (err) {
      showToast(`Could not reset secret store: ${err}`);
    }
  };

  // Focus the key field when opened from the "Add an API key" path and
  // the model has no key. Clear the focus marker once resolved.
  useEffect(() => {
    if (!settingsFocusModelId) return;
    if (draft && draft.id === settingsFocusModelId) {
      if (!draft.hasKey) {
        setKeyEditing(true);
        setTimeout(() => keyInputRef.current?.focus(), 50);
      }
      setSettingsFocusModelId(null);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draft, settingsFocusModelId]);

  const close = () => {
    setSettingsFocusModelId(null);
    setSettingsOpen(false);
  };

  const testOk = testState.status === "done" && testState.result?.ok;
  const testErr = testState.status === "done" && !testState.result?.ok ? testState.result! : null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/70" onClick={close} />

      <div className="relative bg-surface-1 border border-border-subtle rounded-lg w-full max-w-[760px] max-h-[85vh] flex flex-col animate-fade-in">
        {/* Header */}
        <div className="flex items-center justify-between px-6 pt-5 pb-4">
          <div>
            <h1 className="text-[16px] font-medium text-white">Model Setup</h1>
            <p className="text-[13px] text-white-30 mt-0.5">Saved models and encrypted API keys (OS keyring).</p>
          </div>
          <button onClick={close} className="text-white-30 hover:text-white transition-colors p-1">
            <X size={18} />
          </button>
        </div>

        <div className="flex flex-1 min-h-0 border-t border-border-subtle">
          {/* ── Left: saved models list ── */}
          <div className="w-[260px] border-r border-border-subtle flex flex-col">
            <div className="px-3 pt-3 pb-2 flex items-center justify-between">
              <span className="text-[11px] uppercase tracking-wide text-white-30 font-medium">Saved models</span>
              <button
                onClick={() => { setDraft(newDraft()); setDirty(false); setTestState({ status: "idle" }); setApiKeyInput(""); setKeyEditing(false); }}
                className="flex items-center gap-1 text-[12px] text-white-50 hover:text-white transition-colors"
              >
                <Plus size={12} />
                Add model
              </button>
            </div>
            <div className="flex-1 overflow-y-auto pb-2">
              {savedModels.length === 0 && (
                <div className="px-3 py-2 text-[12px] text-white-30">No saved models yet.</div>
              )}
              {savedModels.map((m) => {
                const selected = draft?.id === m.id;
                return (
                  <button
                    key={m.id}
                    onClick={() => selectDraft(draftFromModel(m))}
                    className={`w-full text-left px-3 py-2.5 flex items-start gap-2 transition-colors ${
                      selected ? "bg-surface-3" : "hover:bg-surface-2"
                    }`}
                  >
                    <span className={`mt-0.5 text-[10px] ${m.id === activeModelId ? "text-status-active" : "text-white-15"}`}>
                      {m.id === activeModelId ? "●" : "○"}
                    </span>
                    <span className="flex-1 min-w-0">
                      <span className="block text-[13px] text-white truncate">{m.label}</span>
                      <span className="block text-[11px] text-white-30 truncate">{m.modelName}</span>
                      <span className="block text-[11px] mt-0.5 flex items-center gap-1">
                        {m.keyUnreachable ? (
                          <>
                            <AlertTriangle size={10} className="text-status-warning" />
                            <span className="text-status-warning">key unreachable</span>
                          </>
                        ) : m.hasKey ? (
                          <>
                            <Lock size={10} className="text-status-active" />
                            <span className="text-status-active">key saved</span>
                          </>
                        ) : (
                          <>
                            <AlertTriangle size={10} className="text-status-warning" />
                            <span className="text-status-warning">no key</span>
                          </>
                        )}
                      </span>
                    </span>
                  </button>
                );
              })}
            </div>
            <div className="border-t border-border-subtle px-3 py-2">
              <button
                onClick={handleResetSecretStore}
                className="text-[12px] text-white-30 hover:text-status-error transition-colors"
              >
                Reset secret store
              </button>
            </div>
          </div>

          {/* ── Right: editor ── */}
          <div className="flex-1 min-w-0 flex flex-col">
            <div className="flex-1 overflow-y-auto p-5 space-y-4">
              {!draft ? (
                <div className="text-[13px] text-white-30 py-8 text-center">Select a model or add one.</div>
              ) : (
                <>
                  <Field label="Label">
                    <input
                      className={inputCls}
                      value={draft.label}
                      onChange={(e) => update({ label: e.target.value })}
                      placeholder="e.g. Sonnet 4 (work)"
                    />
                  </Field>

                  <Field label="Provider">
                    <div className="relative">
                      <input
                        className={`${inputCls} pr-8`}
                        value={draft.provider}
                        onChange={(e) => onProviderChange(e.target.value)}
                        placeholder="anthropic, openai, openai-compatible, custom"
                        list="provider-list"
                      />
                      <ChevronDown size={14} className="absolute right-3 top-1/2 -translate-y-1/2 pointer-events-none text-white-30" />
                    </div>
                    <datalist id="provider-list">
                      {Object.keys(PROVIDER_PRESETS).map((p) => (
                        <option key={p} value={p} />
                      ))}
                    </datalist>
                  </Field>

                  <Field label="API Format">
                    <div className="relative">
                      <select
                        className={`${inputCls} appearance-none cursor-pointer pr-8`}
                        value={draft.format}
                        onChange={(e) => update({ format: e.target.value })}
                      >
                        {FORMAT_OPTIONS.map((f) => (
                          <option key={f.value} value={f.value} className="bg-surface-2 text-white">
                            {f.label}
                          </option>
                        ))}
                      </select>
                      <ChevronDown size={14} className="absolute right-3 top-1/2 -translate-y-1/2 pointer-events-none text-white-30" />
                    </div>
                  </Field>

                  <Field label="Base URL">
                    <input
                      className={inputCls}
                      value={draft.baseUrl}
                      onChange={(e) => update({ baseUrl: e.target.value })}
                      placeholder="https://api.example.com"
                    />
                  </Field>

                  <Field label="Model">
                    <div className="relative">
                      <input
                        className={`${inputCls} pr-16`}
                        value={draft.modelName}
                        onChange={(e) => update({ modelName: e.target.value })}
                        placeholder="e.g. claude-sonnet-4-20250514"
                      />
                      <div className="absolute right-2 top-1/2 -translate-y-1/2 flex items-center gap-0.5">
                        <button type="button" onClick={cycleModel} className="p-1 text-white-15 hover:text-white-50 transition-colors" title="Cycle models">
                          <RefreshCw size={14} />
                        </button>
                        <button type="button" onClick={() => update({ modelName: "" })} className="p-1 text-white-15 hover:text-white-50 transition-colors" title="Clear">
                          <XCircle size={14} />
                        </button>
                      </div>
                    </div>
                  </Field>

                  {/* API key — masked, never round-trips */}
                  <Field label="API Key">
                    {draft.hasKey && !keyEditing ? (
                      <div className="flex items-center gap-2">
                        <div className="flex-1 flex items-center gap-2 px-3 py-2 bg-surface-2 border border-border-subtle rounded text-[13px] text-white-50">
                          <Lock size={12} className="text-status-active" />
                          <span>•••••••• (saved encrypted)</span>
                        </div>
                        <button
                          onClick={() => setKeyEditing(true)}
                          className="text-[12px] text-white-30 hover:text-white transition-colors px-2 py-1"
                        >
                          edit
                        </button>
                      </div>
                    ) : (
                      <div className="relative">
                        <input
                          ref={keyInputRef}
                          type={showKeyText ? "text" : "password"}
                          className={`${inputCls} pr-10`}
                          value={apiKeyInput}
                          onChange={(e) => setApiKeyInput(e.target.value)}
                          placeholder={draft.hasKey ? "New key (leave empty to keep existing)" : "sk-..."}
                        />
                        <button
                          type="button"
                          onClick={() => setShowKeyText((s) => !s)}
                          className="absolute right-3 top-1/2 -translate-y-1/2 text-white-15 hover:text-white-50 transition-colors"
                        >
                          {showKeyText ? <EyeOff size={16} /> : <Eye size={16} />}
                        </button>
                      </div>
                    )}
                    {!draft.hasKey && !draft.keyUnreachable && (
                      <p className="text-[11px] text-status-warning mt-1 flex items-center gap-1">
                        <AlertTriangle size={10} /> No API key saved for this model.
                      </p>
                    )}
                    {draft.keyUnreachable && (
                      <p className="text-[11px] text-status-warning mt-1 flex items-center gap-1">
                        <AlertTriangle size={10} /> Key unreachable — the OS credential store may be locked.
                      </p>
                    )}
                  </Field>

                  {/* Test connection */}
                  <div className="flex items-center gap-3">
                    <button
                      onClick={handleTest}
                      disabled={testState.status === "testing" || saving}
                      className="flex items-center gap-1.5 text-[12px] text-white-50 hover:text-white transition-colors disabled:opacity-50"
                    >
                      <Play size={12} />
                      {testState.status === "testing" ? "Testing..." : "Test connection"}
                    </button>
                    {testOk && (
                      <span className="flex items-center gap-1 text-[12px] text-status-active">
                        <CheckCircle2 size={13} /> Connection OK
                      </span>
                    )}
                    {testErr && (
                      <span className="flex items-center gap-1 text-[12px] text-status-error min-w-0">
                        <XCircle size={13} className="shrink-0" />
                        <span className="truncate">{testErr.message}</span>
                      </span>
                    )}
                  </div>

                  {/* Thinking / reasoning prefs */}
                  {draft.format === "anthropic-messages" && (
                    <label className="flex items-center gap-2.5 cursor-pointer pt-1">
                      <button
                        type="button"
                        role="switch"
                        aria-checked={prefs.thinkingOn}
                        onClick={() => persistPrefs({ ...prefs, thinkingOn: !prefs.thinkingOn })}
                        className={`w-8 h-[18px] rounded-full transition-colors relative ${
                          prefs.thinkingOn ? "bg-status-active" : "bg-surface-4"
                        }`}
                      >
                        <span
                          className={`absolute top-[2px] w-[14px] h-[14px] rounded-full bg-white transition-all ${
                            prefs.thinkingOn ? "left-[18px]" : "left-[2px]"
                          }`}
                        />
                      </button>
                      <span className="text-[13px] text-white">Extended thinking</span>
                    </label>
                  )}
                  {draft.format === "openai-chat-completions" && (
                    <Field label="Reasoning effort">
                      <div className="relative">
                        <select
                          className={`${inputCls} appearance-none cursor-pointer pr-8`}
                          value={prefs.reasoningEffort}
                          onChange={(e) =>
                            persistPrefs({ ...prefs, reasoningEffort: e.target.value as ReasoningEffort })
                          }
                        >
                          {(["low", "medium", "high"] as const).map((r) => (
                            <option key={r} value={r} className="bg-surface-2 text-white capitalize">
                              {r}
                            </option>
                          ))}
                        </select>
                        <ChevronDown size={14} className="absolute right-3 top-1/2 -translate-y-1/2 pointer-events-none text-white-30" />
                      </div>
                    </Field>
                  )}
                </>
              )}
            </div>

            {/* Editor footer */}
            <div className="border-t border-border-subtle px-5 py-3 flex items-center justify-between">
              <button
                onClick={handleDelete}
                disabled={!draft || draft.isNew}
                className="flex items-center gap-1.5 text-[12px] text-white-30 hover:text-status-error transition-colors disabled:opacity-40 disabled:pointer-events-none"
              >
                <Trash2 size={13} />
                Delete
              </button>
              <div className="flex items-center gap-2.5">
                <button
                  onClick={handleSetActive}
                  disabled={!draft}
                  className="flex items-center gap-1.5 text-[12px] text-white-50 hover:text-white transition-colors disabled:opacity-40"
                >
                  <CircleDot size={13} />
                  {draft && draft.id === activeModelId ? "Active" : "Set active"}
                </button>
                <button
                  onClick={saveDraft}
                  disabled={saving || !draft}
                  className="glass-quartz glass-quartz--compact glass-quartz--strong px-5 py-2 text-[13px] font-medium text-white disabled:opacity-50"
                >
                  {saving ? "Saving..." : "Save"}
                </button>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="space-y-1.5">
      <label className="text-[13px] font-medium text-white">{label}</label>
      {children}
    </div>
  );
}
