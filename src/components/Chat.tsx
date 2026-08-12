import { useEffect, useRef, useState } from "react";
import { useStore } from "../lib/store";
import { ipc, chatEvents } from "../lib/ipc";
import type { ChatError } from "../lib/ipc";
import { nanoid } from "nanoid";
import { MessageRenderer } from "./MessageRenderer";
import { MentionInput, extractMentions } from "./MentionInput";
import { ChatHeader } from "./ChatHeader";
import { ModelSelect } from "./ModelSelect";
import { formatMessagesForProvider } from "../lib/models";
import { chatErrorActions } from "../lib/chat-errors";
import { showToast } from "../lib/toast";
import { startAgentTurn, cancelAgentTurn, respondApproval } from "../lib/session-manager";
import { Send, Loader2, Square, Settings, RefreshCw, ShieldCheck, Target, Check, X, Wrench } from "lucide-react";
import type { SessionMessage, Mention } from "../lib/store";
import type { ActionMode } from "../lib/ipc";

const nowIso = () => new Date().toISOString();

type ComposerState =
  | "noActive" // no active model, or id not in the list
  | "noKey"
  | "unreachable"
  | "auth" // last chat failed with 401/403 → re-add the key
  | "streaming"
  | "ready";

const ACTION_MODES: { value: ActionMode; label: string; hint: string }[] = [
  { value: "auto", label: "Auto", hint: "Run all tools without asking" },
  { value: "askRisky", label: "Ask risky", hint: "Ask before writes and commands" },
  { value: "askAll", label: "Ask all", hint: "Ask before every tool call" },
];

export function Chat() {
  const { messages, appendMessage, setJobActivities, workspacePath } = useStore();
  const [input, setInput] = useState("");
  const [isProcessing, setIsProcessing] = useState(false); // non-LLM commands
  const [authBlockedFor, setAuthBlockedFor] = useState<string | null>(null);
  const messagesEndRef = useRef<HTMLDivElement>(null);

  // Keep the latest streaming requestId without re-subscribing.
  const streamingRequestId = useStore((s) => s.streamingRequestId);
  const agentRequestId = useStore((s) => s.agentRequestId);
  const pendingApproval = useStore((s) => s.pendingApproval);
  const activities = useStore((s) => s.activities);
  const actionMode = useStore((s) => s.actionMode);
  const setActionMode = useStore((s) => s.setActionMode);
  const goalMode = useStore((s) => s.goalMode);
  const setGoalMode = useStore((s) => s.setGoalMode);
  const sessionLoaded = useStore((s) => s.sessionLoaded);
  const activeSessionId = useStore((s) => s.activeSessionId);
  const savedModels = useStore((s) => s.savedModels);
  const activeModelId = useStore((s) => s.activeModelId);
  const modelPrefs = useStore((s) => s.modelPrefs);

  const activeModel = savedModels.find((m) => m.id === activeModelId) ?? null;

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);

  // ── Streaming event wiring ──
  useEffect(() => {
    const unsubs: Promise<() => void>[] = [];
    // In browser-only dev there's no Tauri event bus; swallow the
    // rejection instead of leaving an unhandled promise.
    const reg = (p: Promise<() => void>) => {
      p.catch(() => {});
      unsubs.push(p);
    };

    reg(
      chatEvents.onChunk((e) => {
        const st = useStore.getState();
        if (st.streamingRequestId && e.requestId === st.streamingRequestId) {
          st.appendToMessage(e.requestId, e.delta);
        }
      })
    );

    reg(
      chatEvents.onDone((e) => {
        const st = useStore.getState();
        if (!st.streamingRequestId || e.requestId !== st.streamingRequestId) return;
        st.updateMessage(e.requestId, { content: e.content });

        // Token telemetry (spec §7.5 step 4): cumulativeIn is the PEAK
        // input (`max`), cumulativeOut is a literal sum.
        const lastIn = e.usage?.input ?? 0;
        const lastOut = e.usage?.output ?? 0;
        const prev = st.tokenStats;
        st.setTokenStats({
          lastIn,
          lastOut,
          cumulativeIn: prev ? Math.max(prev.cumulativeIn, lastIn) : lastIn,
          cumulativeOut: (prev?.cumulativeOut ?? 0) + lastOut,
          sessionWindowUsed: e.sessionWindow?.usedFraction ?? prev?.sessionWindowUsed ?? null,
          sessionWindowResetsAt: e.sessionWindow?.resetsAt ?? prev?.sessionWindowResetsAt ?? null,
        });

        st.setStreamingRequestId(null);
        st.setJobActivities([]);
      })
    );

    reg(
      chatEvents.onError((e) => {
        const st = useStore.getState();
        if (!st.streamingRequestId || e.requestId !== st.streamingRequestId) return;
        handleChatError(e);
      })
    );

    return () => {
      unsubs.forEach((p) => p.then((u) => u()).catch(() => {}));
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const handleChatError = (err: ChatError) => {
    const st = useStore.getState();
    const actions = chatErrorActions(err);
    st.setStreamingRequestId(null);
    st.setJobActivities([]);
    if (actions.bubble === null) {
      st.removeMessage(err.requestId);
    } else {
      st.updateMessage(err.requestId, { content: actions.bubble, agentName: undefined });
    }
    if (actions.toast) showToast(actions.toast);
    if (err.kind === "auth") {
      // Composer falls back to the "no key" disabled state (spec §7.5).
      const model = st.savedModels.find((m) => m.id === st.activeModelId) ?? null;
      if (model) setAuthBlockedFor(model.id);
    }
  };

  // Clear the auth block when the user switches models or closes
  // Settings (where they may have re-added the key).
  useEffect(() => {
    setAuthBlockedFor(null);
  }, [activeModelId]);

  const settingsOpen = useStore((s) => s.settingsOpen);
  useEffect(() => {
    if (!settingsOpen) setAuthBlockedFor(null);
  }, [settingsOpen]);

  const handleSubmit = async (textOverride?: string, mentionsOverride?: Mention[]) => {
    const text = (textOverride ?? input).trim();
    if (!text || isProcessing || streamingRequestId || agentRequestId) return;
    const mentions = mentionsOverride ?? extractMentions(text);
    if (!textOverride) setInput("");

    // System commands stay on the old path (no LLM involved).
    if (text.startsWith("/")) {
      await handleCommand(text);
      return;
    }

    const st = useStore.getState();
    const model = st.savedModels.find((m) => m.id === st.activeModelId) ?? null;
    if (!model) return;

    // Agent mode: the Rust-owned loop (workspace sessions). Requires an
    // open project with a session.
    if (st.workspacePath && st.activeSessionId) {
      setJobActivities(["Agent working..."]);
      await startAgentTurn(text, mentions);
      return;
    }

    const userMsg: SessionMessage = {
      id: nanoid(),
      role: "user",
      content: text,
      timestamp: nowIso(),
      mentions: mentions.length > 0 ? mentions : undefined,
    };
    st.appendMessage(userMsg);

    const requestId = nanoid();
    st.appendMessage({
      id: requestId,
      role: "assistant",
      content: "",
      timestamp: nowIso(),
      agentName: model.label,
    });
    st.setStreamingRequestId(requestId);
    st.setJobActivities(["Streaming..."]);

    try {
      const history = useStore.getState().messages; // includes the user msg
      await ipc.chatComplete({
        requestId,
        modelId: model.id,
        messages: formatMessagesForProvider(history, model.format),
        thinkingOn: model.format === "anthropic-messages" ? st.modelPrefs.thinkingOn : false,
        reasoningEffort:
          model.format === "openai-chat-completions" ? st.modelPrefs.reasoningEffort : undefined,
      });
    } catch (err) {
      // IPC-level failure (shouldn't happen — the backend spawns the
      // stream), but handle it like an internal error regardless.
      handleChatError({ requestId, kind: "internal", error: String(err) });
    }
  };

  const handleCancel = async () => {
    const st = useStore.getState();
    if (st.agentRequestId) {
      await cancelAgentTurn();
    } else if (st.streamingRequestId) {
      await ipc.cancelChat(st.streamingRequestId).catch(() => {});
    }
  };

  const handleCommand = async (cmd: string) => {
    const parts = cmd.split(/\s+/);
    const command = parts[0].toLowerCase();
    if (command === "/search") {
      const query = parts.slice(1).join(" ");
      if (!query) {
        appendMessage({ id: nanoid(), role: "system", content: "Usage: /search <query>", timestamp: nowIso() });
        return;
      }
      setIsProcessing(true);
      setJobActivities([`Searching: ${query}`]);
      try {
        const results = await ipc.webSearch(query);
        const formatted = results.map((r, i) => `**${i + 1}. ${r.title}**\n${r.url}\n${r.snippet}`).join("\n\n");
        appendMessage({ id: nanoid(), role: "assistant", content: formatted || "No results found.", timestamp: nowIso(), agentName: "Search" });
      } catch (err) {
        appendMessage({ id: nanoid(), role: "system", content: `Search failed: ${err}`, timestamp: nowIso() });
      } finally {
        setIsProcessing(false);
        setJobActivities([]);
      }
    } else {
      appendMessage({ id: nanoid(), role: "system", content: `Unknown command: ${command}. Available: /search`, timestamp: nowIso() });
    }
  };

  // ── Composer disabled state (spec §7.4) ──
  const composerState: ComposerState = streamingRequestId || agentRequestId
    ? "streaming"
    : !activeModel
      ? "noActive"
      : authBlockedFor === activeModel.id
        ? "auth"
        : activeModel.keyUnreachable
          ? "unreachable"
          : !activeModel.hasKey
            ? "noKey"
            : "ready";

  const composerBlocked = composerState !== "ready" && composerState !== "streaming";

  const retryKeyUnreachable = async () => {
    try {
      const list = await ipc.listSavedModels();
      useStore.getState().setSavedModels(list);
    } catch (err) {
      showToast(`Could not reach the secret store: ${err}`);
    }
  };

  const openSettingsForActive = () => {
    const st = useStore.getState();
    if (activeModelId) st.setSettingsFocusModelId(activeModelId);
    st.setSettingsOpen(true);
  };

  // The blocked composer is not a dead zone: clicking the box routes to
  // the action that unblocks it (spec §7.4 — disabled, but actionable).
  const handleComposerClick = () => {
    if (composerState === "noActive") {
      useStore.getState().setSettingsOpen(true);
    } else if (composerState === "noKey" || composerState === "auth") {
      openSettingsForActive();
    } else if (composerState === "unreachable") {
      retryKeyUnreachable();
    }
  };

  const composerTitle =
    composerState === "noActive"
      ? "Add a model to start chatting"
      : composerState === "noKey" || composerState === "auth"
        ? "Add an API key in Settings"
        : composerState === "unreachable"
          ? "Retry the secret store"
          : undefined;

  const placeholder =
    composerState === "streaming"
      ? agentRequestId
        ? "Agent working..."
        : "Streaming..."
      : composerState === "noActive"
        ? "Add a model in Settings to start"
        : composerState === "noKey" || composerState === "auth"
          ? "Add an API key in Settings"
          : composerState === "unreachable"
            ? "API key unreachable — click to retry"
            : "input your instructions here";

  return (
    <div className="flex-1 flex flex-col overflow-hidden">
      <ChatHeader />

      {messages.length === 0 ? (
        <div className="flex-1 flex items-start justify-center px-4 pt-16">
          {workspacePath
            ? <EmptyState variant="no-messages" />
            : <EmptyState variant="no-project" />}
        </div>
      ) : (
        <div className="flex-1 overflow-y-auto px-4 py-6">
          <div className="max-w-3xl mx-auto space-y-4">
            {messages.map((msg) => <MessageBubble key={msg.id} message={msg} />)}
          </div>
          <div ref={messagesEndRef} />
        </div>
      )}

      <div className="px-4 py-3">
        <div className="max-w-3xl mx-auto">
          {/* Selector row (spec §7.3) */}
          {composerState !== "noActive" && (
            <div className="flex items-center gap-3 mb-2 select-none">
              <span className="text-[11px] text-white-30">Model:</span>
              <ModelSelect compact />
              {activeModel && (
                <span className="text-[10px] text-white-30">({activeModel.provider})</span>
              )}
              {activeModel?.format === "anthropic-messages" && (
                <label className="flex items-center gap-1.5 ml-auto cursor-pointer">
                  <button
                    type="button"
                    role="switch"
                    aria-checked={modelPrefs.thinkingOn}
                    onClick={() => {
                      const next = { ...useStore.getState().modelPrefs, thinkingOn: !useStore.getState().modelPrefs.thinkingOn };
                      useStore.getState().setModelPrefs(next);
                      if (activeModel) ipc.setModelPrefs(activeModel.id, next).catch(() => {});
                    }}
                    className={`w-7 h-4 rounded-full transition-colors relative ${
                      modelPrefs.thinkingOn ? "bg-status-active" : "bg-surface-4"
                    }`}
                  >
                    <span
                      className={`absolute top-[2px] w-3 h-3 rounded-full bg-white transition-all ${
                        modelPrefs.thinkingOn ? "left-[14px]" : "left-[2px]"
                      }`}
                    />
                  </button>
                  <span className="text-[11px] text-white-50">Thinking</span>
                </label>
              )}
              {activeModel?.format === "openai-chat-completions" && (
                <label className="flex items-center gap-1.5 ml-auto cursor-pointer">
                  <span className="text-[11px] text-white-30">Reasoning</span>
                  <select
                    value={modelPrefs.reasoningEffort}
                    onChange={(e) => {
                      const next = { ...useStore.getState().modelPrefs, reasoningEffort: e.target.value as "low" | "medium" | "high" };
                      useStore.getState().setModelPrefs(next);
                      if (activeModel) ipc.setModelPrefs(activeModel.id, next).catch(() => {});
                    }}
                    className="bg-surface-2 border border-border-subtle rounded px-2 py-0.5 text-[11px] text-white outline-none cursor-pointer focus:border-border-strong"
                  >
                    <option value="low" className="bg-surface-2 text-white">Low</option>
                    <option value="medium" className="bg-surface-2 text-white">Medium</option>
                    <option value="high" className="bg-surface-2 text-white">High</option>
                  </select>
                </label>
              )}
            </div>
          )}

          {/* Agent options row (spec §4.2): action-mode gate + goal mode.
              Only meaningful when an agent session is open. */}
          {composerState !== "noActive" && workspacePath && activeSessionId && (
            <div className="flex items-center gap-3 mb-2 select-none">
              <span className="text-[11px] text-white-30">Actions:</span>
              <div className="flex items-center bg-surface-2 border border-border-subtle rounded overflow-hidden">
                {ACTION_MODES.map((m) => (
                  <button
                    key={m.value}
                    type="button"
                    title={m.hint}
                    onClick={() => setActionMode(m.value)}
                    className={`px-2 py-0.5 text-[11px] transition-colors ${
                      actionMode === m.value
                        ? "bg-surface-4 text-white"
                        : "text-white-30 hover:text-white"
                    }`}
                  >
                    {m.label}
                  </button>
                ))}
              </div>
              <label className="flex items-center gap-1.5 ml-auto cursor-pointer">
                <Target size={12} className={goalMode ? "text-status-active" : "text-white-30"} />
                <span className="text-[11px] text-white-50">Goal</span>
                <button
                  type="button"
                  role="switch"
                  aria-checked={goalMode}
                  onClick={() => setGoalMode(!goalMode)}
                  className={`w-7 h-4 rounded-full transition-colors relative ${
                    goalMode ? "bg-status-active" : "bg-surface-4"
                  }`}
                >
                  <span
                    className={`absolute top-[2px] w-3 h-3 rounded-full bg-white transition-all ${
                      goalMode ? "left-[14px]" : "left-[2px]"
                    }`}
                  />
                </button>
              </label>
            </div>
          )}

          {/* Live tool activity feed (last 4, spec §4.1) while a turn runs */}
          {agentRequestId && activities.length > 0 && (
            <div className="mb-2 flex flex-wrap items-center gap-x-3 gap-y-1">
              {activities.slice(-4).map((a) => (
                <div key={a.id} className="flex items-center gap-1.5 text-[10px] text-white-30 min-w-0">
                  <Wrench size={10} className="shrink-0 text-white-15" />
                  <span
                    className={`shrink-0 font-medium ${
                      a.status === "ran"
                        ? "text-status-active"
                        : a.status === "denied"
                          ? "text-status-warning"
                          : "text-status-error"
                    }`}
                  >
                    {a.tool}
                  </span>
                  <span className="truncate max-w-[220px]">{a.summary}</span>
                </div>
              ))}
            </div>
          )}

          {/* Pending tool approval (spec §4.3): Allow / Deny gate */}
          {pendingApproval && (
            <div className="flex items-center gap-2.5 mb-2 bg-surface-2 border border-border-subtle rounded px-3 py-2 animate-fade-in">
              <ShieldCheck size={15} className="text-status-warning shrink-0" />
              <div className="flex-1 min-w-0">
                <div className="text-[12px] text-white font-medium truncate">{pendingApproval.name}</div>
                <div className="text-[11px] text-white-30 truncate">{pendingApproval.summary}</div>
              </div>
              <button
                onClick={() => respondApproval(true)}
                className="flex items-center gap-1 px-2.5 py-1 rounded bg-status-active text-black text-[11px] font-medium hover:opacity-90 transition-opacity"
                title="Allow this tool call"
              >
                <Check size={12} />
                Allow
              </button>
              <button
                onClick={() => respondApproval(false)}
                className="flex items-center gap-1 px-2.5 py-1 rounded bg-surface-4 text-white-50 text-[11px] hover:text-status-error transition-colors"
                title="Deny this tool call"
              >
                <X size={12} />
                Deny
              </button>
            </div>
          )}

          <div
            className="glass-mist relative flex items-end"
            onClick={handleComposerClick}
            title={composerTitle}
          >
            <MentionInput
              value={input}
              onChange={setInput}
              onSend={(text, mentions) => handleSubmit(text, mentions)}
              disabled={composerBlocked || isProcessing}
              placeholder={placeholder}
            />
            {composerState === "streaming" ? (
              <button
                onClick={handleCancel}
                className="glass-quartz glass-quartz--pill shrink-0 m-1.5 px-3.5 py-3 text-white"
                title="Stop"
              >
                <Square size={14} fill="currentColor" />
              </button>
            ) : composerState === "unreachable" ? (
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  retryKeyUnreachable();
                }}
                className="glass-quartz glass-quartz--pill shrink-0 m-1.5 px-3.5 py-3 text-white"
                title="Retry"
              >
                <RefreshCw size={16} />
              </button>
            ) : (
              <button
                onClick={() => handleSubmit()}
                disabled={!input.trim() || composerBlocked || isProcessing}
                className="glass-quartz glass-quartz--pill shrink-0 m-1.5 px-3.5 py-3 text-white"
                title="Send"
              >
                {isProcessing ? (
                  <Loader2 size={16} className="animate-spin" />
                ) : (
                  <Send size={16} />
                )}
              </button>
            )}
          </div>

          {/* Disabled-state helper row */}
          {composerBlocked && composerState !== "noActive" && (
            <div className="mt-2 text-center">
              {composerState === "noKey" || composerState === "auth" ? (
                <button
                  onClick={openSettingsForActive}
                  className="text-[12px] text-status-warning hover:text-white transition-colors inline-flex items-center gap-1.5"
                >
                  <Settings size={12} />
                  Add an API key in Settings
                </button>
              ) : composerState === "unreachable" ? (
                <button
                  onClick={retryKeyUnreachable}
                  className="text-[12px] text-status-warning hover:text-white transition-colors"
                >
                  API key unreachable — click to retry
                </button>
              ) : null}
            </div>
          )}

          {composerState === "noActive" && (
            <p className="text-[11px] text-white-15 mt-2 text-center">
              No active model.{" "}
              <button
                onClick={() => { useStore.getState().setSettingsOpen(true); }}
                className="text-white-30 hover:text-white underline underline-offset-2"
              >
                Add a model in Settings
              </button>
            </p>
          )}

          {composerState === "ready" && (
            <p className="text-[11px] text-white-15 mt-2 text-center">
              harness is an agent that can make mistakes and is not always correct with its decisions
            </p>
          )}
        </div>
      </div>
    </div>
  );
}

function MessageBubble({ message }: { message: SessionMessage }) {
  const isUser = message.role === "user";
  const isSystem = message.role === "system";

  if (isSystem) {
    return (
      <div className="text-center animate-fade-in">
        <span className="text-[12px] text-white-30 bg-surface-2 px-3 py-1 rounded-full border border-border-subtle">
          {message.content}
        </span>
      </div>
    );
  }

  return (
    <div className={`animate-fade-in ${isUser ? "flex justify-end" : "flex justify-start"}`}>
      <div className="max-w-[85%]">
        {!isUser && message.agentName && (
          <div className="text-[11px] font-medium text-white-30 mb-1 flex items-center gap-1.5">
            <span className="inline-block w-1.5 h-1.5 rounded-full bg-status-active" />
            {message.agentName}
          </div>
        )}
        <div className={`text-[14px] leading-relaxed ${isUser ? "bg-surface-3 text-white rounded-2xl rounded-br-sm px-4 py-2.5" : "text-white-50"}`}>
          <MessageRenderer content={message.content} />
        </div>
      </div>
    </div>
  );
}

function EmptyState({ variant }: { variant: "no-project" | "no-messages" }) {
  if (variant === "no-project") {
    return (
      <div className="flex flex-col items-center select-none">
        <div className="text-center space-y-4">
          <div className="text-[28px] font-light tracking-tight text-white">harness</div>
          <p className="text-[14px] text-white-30 max-w-xs leading-relaxed">
            get work done, fast
          </p>
        </div>
      </div>
    );
  }
  return (
    <div className="flex flex-col items-center select-none">
      <div className="text-center space-y-4">
        <div className="text-[28px] font-light tracking-tight text-white">harness</div>
        <p className="text-[14px] text-white-30 max-w-xs leading-relaxed">
          Start a conversation, spawn agents, or search the web.
        </p>
      </div>
    </div>
  );
}
