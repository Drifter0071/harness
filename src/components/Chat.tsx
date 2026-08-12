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
import { Send, Loader2, Square, Settings, RefreshCw } from "lucide-react";
import type { SessionMessage, Mention } from "../lib/store";

const nowIso = () => new Date().toISOString();

type ComposerState =
  | "noActive" // no active model, or id not in the list
  | "noKey"
  | "unreachable"
  | "auth" // last chat failed with 401/403 → re-add the key
  | "streaming"
  | "ready";

export function Chat() {
  const { messages, appendMessage, setJobActivities, workspacePath } = useStore();
  const [input, setInput] = useState("");
  const [isProcessing, setIsProcessing] = useState(false); // non-LLM commands
  const [authBlockedFor, setAuthBlockedFor] = useState<string | null>(null);
  const messagesEndRef = useRef<HTMLDivElement>(null);

  // Keep the latest streaming requestId without re-subscribing.
  const streamingRequestId = useStore((s) => s.streamingRequestId);
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
    if (!text || isProcessing || streamingRequestId) return;
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
    if (st.streamingRequestId) {
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
  const composerState: ComposerState = streamingRequestId
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
      ? "Streaming..."
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
