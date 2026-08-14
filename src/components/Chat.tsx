import { useEffect, useRef, useState } from "react";
import { useStore } from "../lib/store";
import { ipc, chatEvents } from "../lib/ipc";
import type { ChatError } from "../lib/ipc";
import { nanoid } from "nanoid";
import { MessageRenderer } from "./MessageRenderer";
import { MentionInput, extractMentions } from "./MentionInput";
import { ModelSelect } from "./ModelSelect";
import { TypingBuddy, type BuddyMode } from "./AgentMascot";
import { formatMessagesForProvider } from "../lib/models";
import { capabilitiesFor } from "../lib/model-capabilities";
import { chatErrorActions } from "../lib/chat-errors";
import { showToast } from "../lib/toast";
import { startAgentTurn, cancelAgentTurn, respondApproval, flushPendingSave } from "../lib/session-manager";
import { Loader2, Square, Settings, RefreshCw, ShieldCheck, Target, Check, X, Wrench, Ban, AlertTriangle, ListPlus, Pencil, Zap } from "lucide-react";
import type { SessionMessage, Mention, ToolCallRecord, QueuedMessage } from "../lib/store";
import type { ActionMode, ReasoningEffort } from "../lib/ipc";

const nowIso = () => new Date().toISOString();

/** Stable empty list so the per-message tool-call selector returns a
 *  constant reference for messages without tool calls (avoids re-render
 *  churn on every store update). */
const NO_TOOL_CALLS: ToolCallRecord[] = [];

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

  // Bip animates inside the send button while the user drafts the
  // message: tapping keys while the draft grows, whipping out a tiny
  // broom on backspace. Mirrors the previous MentionInput behaviour;
  // it lives here now because the send button is the visual host.
  const [buddyMode, setBuddyMode] = useState<BuddyMode>("typing");
  const prevInputLen = useRef(0);
  useEffect(() => {
    const len = input.length;
    if (len === 0) {
      prevInputLen.current = 0;
      return;
    }
    if (prevInputLen.current > len) {
      setBuddyMode("sweeping");
      const t = setTimeout(() => setBuddyMode("typing"), 800);
      prevInputLen.current = len;
      return () => clearTimeout(t);
    }
    prevInputLen.current = len;
    setBuddyMode("typing");
  }, [input]);

  // Keep the latest streaming requestId without re-subscribing.
  const streamingRequestId = useStore((s) => s.streamingRequestId);
  const agentRequestId = useStore((s) => s.agentRequestId);
  const pendingApproval = useStore((s) => s.pendingApproval);
  const actionMode = useStore((s) => s.actionMode);
  const setActionMode = useStore((s) => s.setActionMode);
  const goalMode = useStore((s) => s.goalMode);
  const setGoalMode = useStore((s) => s.setGoalMode);
  const sessionLoaded = useStore((s) => s.sessionLoaded);
  const activeSessionId = useStore((s) => s.activeSessionId);
  const pendingMessages = useStore((s) => s.pendingMessages);
  const savedModels = useStore((s) => s.savedModels);
  const activeModelId = useStore((s) => s.activeModelId);
  const modelPrefs = useStore((s) => s.modelPrefs);

  const activeModel = savedModels.find((m) => m.id === activeModelId) ?? null;
  // Default to "none" when no model is active so the conditional
  // renders below can read `caps.reasoning` unconditionally.
  const caps = activeModel
    ? capabilitiesFor(activeModel)
    : { contextWindow: 0, reasoning: "none" as const, reasoningLevels: ["low", "medium", "high"] as const, source: "default" as const };

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
        // eslint-disable-next-line no-console
        console.log("[chat] chunk", e.requestId, "len", e.delta.length, "streaming", st.streamingRequestId);
        if (st.streamingRequestId && e.requestId === st.streamingRequestId) {
          st.appendToMessage(e.requestId, e.delta);
        }
      })
    );

    reg(
      chatEvents.onReasoning((e) => {
        const st = useStore.getState();
        if (st.streamingRequestId && e.requestId === st.streamingRequestId) {
          st.appendReasoning(e.requestId, e.delta);
        }
      })
    );

    reg(
      chatEvents.onDone((e) => {
        const st = useStore.getState();
        // eslint-disable-next-line no-console
        console.log("[chat] done", e.requestId, "content_len", e.content.length);
        // Bail only when a *different* request is streaming. The
        // disk-watcher fallback (applySession) can clear
        // `streamingRequestId` first when the agent's terminal write
        // hits the disk before the in-process event reaches the
        // WebView. The watcher already copied the final content into
        // the placeholder in that case, but we still re-apply the
        // event's authoritative content + reasoning so any
        // final-round additions land in the UI.
        if (
          st.streamingRequestId !== null &&
          e.requestId !== st.streamingRequestId
        ) {
          return;
        }
        const requestId = e.requestId;
        st.updateMessage(requestId, {
          content: e.content,
          // Prefer the explicit final reasoning if the backend
          // bundled it; otherwise leave whatever the live stream
          // accumulated.
          reasoning: e.reasoning && e.reasoning.length > 0
            ? e.reasoning
            : undefined,
        });

        // Flush the debounced session save so the response is on disk
        // even if the user reloads immediately. Without this, the chat
        // streaming path in Rust bypasses persistence and the message
        // only exists in the in-memory store.
        flushPendingSave().catch(() => {});

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
    if (actions.bubble === undefined) {
      // `cancelled`: keep the already-streamed segment in the chat.
      // Only drop the optimistic bubble when nothing was generated.
      const msg = st.messages.find((m) => m.id === err.requestId);
      if (msg && (msg.content ?? "").length === 0) {
        st.removeMessage(err.requestId);
      }
    } else if (actions.bubble === null) {
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
    if (!text || isProcessing) return;
    const mentions = mentionsOverride ?? extractMentions(text);

    // A generation is in flight: the draft goes into the queue instead
    // of being dropped. The auto-advance effect sends the head of the
    // queue the moment the current stream/agent turn ends. Slash
    // commands stay immediate-only — they can't run mid-stream anyway.
    const st = useStore.getState();
    if (st.streamingRequestId || st.agentRequestId) {
      if (text.startsWith("/")) return;
      st.enqueueMessage({
        id: nanoid(),
        content: text,
        mentions: mentions.length > 0 ? mentions : undefined,
      });
      if (!textOverride) setInput("");
      return;
    }

    if (!textOverride) setInput("");

    // System commands stay on the old path (no LLM involved).
    if (text.startsWith("/")) {
      await handleCommand(text);
      return;
    }

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
    // eslint-disable-next-line no-console
    console.log("[chat] send", requestId, "model", model.id);
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
      // Resolve the model's effective capabilities so we only send
      // the reasoning param(s) the model actually supports. Sending
      // a field the model rejects triggers a 400 on OpenAI-compat
      // gateways — the OpenAI path already retries without effort,
      // but the cleanest answer is to not send it in the first place.
      //
      // `thinkingOn` is a user-facing "this model can reason, ask it
      // to" toggle. On the Anthropic wire it maps to
      // `body.thinking = { type: "enabled", ... }`. On OpenAI-compat
      // it maps to `reasoning_effort` — there is no "thinking" toggle,
      // so we mirror the intent through the effort knob. Without
      // this, a "thinking"-capable model proxied through an
      // OpenAI-compat gateway (Kilo, OpenRouter, etc.) would never
      // return reasoning content.
      const caps = capabilitiesFor(model);
      const reasoningRequested =
        caps.reasoning === "thinking" || caps.reasoning === "effort";
      const thinkingOn =
        caps.reasoning === "thinking" ? st.modelPrefs.thinkingOn : false;
      const reasoningEffort: ReasoningEffort | undefined = reasoningRequested
        ? caps.reasoning === "effort"
          ? st.modelPrefs.reasoningEffort
          : // For "thinking"-capability models on OpenAI-compat we
            // still send an effort so the gateway relays reasoning.
            // If the user hasn't picked one, default to medium.
            st.modelPrefs.reasoningEffort ?? "medium"
        : undefined;
      await ipc.chatComplete({
        requestId,
        modelId: model.id,
        messages: formatMessagesForProvider(history, model.format),
        thinkingOn,
        reasoningEffort,
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

  // ── Queue auto-advance ──
  // The moment a generation ends (done / error / cancelled), send the
  // head of the pending-message queue so queued drafts flow without
  // any extra click.
  const prevBusyRef = useRef(false);
  useEffect(() => {
    const busy = streamingRequestId !== null || agentRequestId !== null;
    if (prevBusyRef.current && !busy) {
      const st = useStore.getState();
      const next = st.pendingMessages[0];
      if (next) {
        st.removeQueuedMessage(next.id);
        void handleSubmit(next.content, next.mentions);
      }
    }
    prevBusyRef.current = busy;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [streamingRequestId, agentRequestId]);

  /** Force-send: stop the in-flight generation and send `text` now.
   *  The draft is pushed to the FRONT of the queue (ahead of anything
   *  already waiting); the auto-advance effect picks it up the moment
   *  the cancellation lands. Enqueuing before cancelling keeps the
   *  ordering deterministic regardless of how the terminal event races
   *  the cancel IPC. */
  const handleForceSend = async (textOverride?: string, mentionsOverride?: Mention[]) => {
    const text = (textOverride ?? input).trim();
    if (!text) return;
    const mentions = mentionsOverride ?? extractMentions(text);
    const st = useStore.getState();
    const busy = st.streamingRequestId !== null || st.agentRequestId !== null;
    if (!busy) {
      // Nothing to interrupt — behaves like a normal send.
      await handleSubmit(textOverride, mentionsOverride);
      return;
    }
    st.enqueueMessage(
      {
        id: nanoid(),
        content: text,
        mentions: mentions.length > 0 ? mentions : undefined,
      },
      true
    );
    if (!textOverride) setInput("");
    await handleCancel();
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
        ? "Agent working… type to queue a message"
        : "Streaming… type to queue a message"
      : composerState === "noActive"
        ? "Add a model in Settings to start"
        : composerState === "noKey" || composerState === "auth"
          ? "Add an API key in Settings"
          : composerState === "unreachable"
            ? "API key unreachable — click to retry"
            : "input your instructions here";

  return (
    <div className="flex-1 flex flex-col overflow-hidden">
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
              {activeModel && caps.reasoning === "thinking" && (
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
                    className={`w-7 h-4 rounded-full transition-colors relative focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-white/60 ${
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
              {activeModel && caps.reasoning === "effort" && (
                <label className="flex items-center gap-1.5 ml-auto cursor-pointer">
                  <span className="text-[11px] text-white-30">Reasoning</span>
                  <select
                    value={modelPrefs.reasoningEffort}
                    onChange={(e) => {
                      const next = { ...useStore.getState().modelPrefs, reasoningEffort: e.target.value as ReasoningEffort };
                      useStore.getState().setModelPrefs(next);
                      if (activeModel) ipc.setModelPrefs(activeModel.id, next).catch(() => {});
                    }}
                    className="bg-surface-2 border border-border-subtle rounded px-2 py-0.5 text-[11px] text-white outline-none cursor-pointer focus:border-border-strong"
                  >
                    {(["low", "medium", "high"] as const).map((lvl) => {
                      const enabled = caps.reasoningLevels.includes(lvl);
                      return (
                        <option
                          key={lvl}
                          value={lvl}
                          disabled={!enabled}
                          className="bg-surface-2 text-white"
                        >
                          {lvl[0].toUpperCase() + lvl.slice(1)}{!enabled ? " (not supported)" : ""}
                        </option>
                      );
                    })}
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
              <div className="flex items-center gap-px rounded-md bg-surface-2 border border-border-subtle p-0.5">
                {ACTION_MODES.map((m) => (
                  <button
                    key={m.value}
                    type="button"
                    title={m.hint}
                    onClick={() => setActionMode(m.value)}
                    className={`px-2.5 py-1 text-[11px] font-medium rounded-[5px] transition-all duration-150 ${
                      actionMode === m.value
                        ? "bg-white/[0.08] text-white border border-border-strong"
                        : "text-white-30 hover:text-white-50 hover:bg-white/[0.04] border border-transparent"
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
                  className={`w-7 h-4 rounded-full transition-colors relative focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-white/60 ${
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

          {/* Pending message queue: drafts wait here while a generation
              runs. The head auto-sends when the current response ends. */}
          {pendingMessages.length > 0 && (
            <div className="mb-2">
              <div className="flex items-center gap-1.5 mb-1.5 text-[11px] text-white-30 select-none">
                <ListPlus size={12} className="shrink-0" />
                <span>
                  Queued ({pendingMessages.length}) — sent in order when the
                  current response ends
                </span>
              </div>
              <div className="space-y-1.5">
                {pendingMessages.map((qm) => (
                  <QueuedMessageItem key={qm.id} item={qm} />
                ))}
              </div>
            </div>
          )}

          <div
            className="glass-mist glass-mist--composer relative flex items-center"
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
              <div className="flex items-center shrink-0 m-1.5 gap-1">
                {input.trim() && (
                  <>
                    <button
                      onClick={() => void handleSubmit()}
                      className="glass-quartz glass-quartz--pill px-3 py-3 text-white"
                      title="Queue message — auto-sent when the current response finishes"
                      aria-label="Queue message"
                    >
                      <ListPlus size={15} />
                    </button>
                    <button
                      onClick={() => void handleForceSend()}
                      className="glass-quartz glass-quartz--pill px-3 py-3 text-white"
                      title="Stop the current response and send this now"
                      aria-label="Send now, stopping the current response"
                    >
                      <Zap size={14} />
                    </button>
                  </>
                )}
                <button
                  onClick={handleCancel}
                  className="glass-quartz glass-quartz--pill px-3.5 py-3 text-white"
                  title="Stop — keeps the partial response"
                >
                  <Square size={14} fill="currentColor" />
                </button>
              </div>
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
                // The send button IS Bip. `disabled:opacity-40
                // disabled:grayscale` keeps Bip visible when the user
                // hasn't typed anything yet (no text → can't send →
                // button is disabled) so the user still has a
                // constant visual anchor in the composer. When they
                // start typing, Bip wakes up and animates.
                className="glass-quartz glass-quartz--pill shrink-0 m-1.5 px-3 py-2.5 text-white disabled:opacity-40 disabled:grayscale disabled:saturate-0 transition-opacity"
                title="Send"
                aria-label="Send message"
              >
                {isProcessing ? (
                  <Loader2 size={16} className="animate-spin" />
                ) : (
                  <TypingBuddy mode={buddyMode} size={22} />
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
  // Tool calls attached to this assistant message (keyed by
  // `agent-<requestId>`), live from events or rebuilt from disk.
  const toolCalls = useStore((s) => s.toolCalls[message.id] ?? NO_TOOL_CALLS);

  // Is this the message the agent is currently streaming into?
  // Streamed messages are created with `id === requestId`, so the
  // in-flight request id identifies the live bubble.
  const streamingRequestId = useStore((s) => s.streamingRequestId);
  const isStreamingMessage = !isUser && !isSystem && streamingRequestId === message.id;

  // When the agent rewrites its stream (content shrinks between
  // chunks), Bip breaks out the broom — "sweeping up" the deleted text.
  const [sweeping, setSweeping] = useState(false);
  const prevContentLen = useRef(0);
  useEffect(() => {
    if (!isStreamingMessage) return;
    const len = message.content.length;
    if (prevContentLen.current > len) {
      setSweeping(true);
      const t = setTimeout(() => setSweeping(false), 900);
      prevContentLen.current = len;
      return () => clearTimeout(t);
    }
    prevContentLen.current = len;
  }, [message.content.length, isStreamingMessage]);

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
            <span className="inline-block w-1.5 h-1.5 rounded-full bg-accent-coral" />
            {message.agentName}
          </div>
        )}
        <div className={`text-[14px] leading-relaxed ${isUser ? "bg-surface-3 text-white-50 rounded-2xl rounded-br-sm px-4 py-2.5" : "text-white"}`}>
          {!isUser && toolCalls.length > 0 && (
            <div className="space-y-1.5 mb-2">
              {toolCalls.map((call) => (
                <ToolCallBlock key={call.id} call={call} />
              ))}
            </div>
          )}
          <MessageRenderer content={message.content} reasoning={message.reasoning} />
          {isStreamingMessage && (
            <span className="inline-flex items-center gap-1.5 mt-1">
              <TypingBuddy mode={sweeping ? "sweeping" : "typing"} size={17} />
              <span className="text-[10px] text-white-30">
                {sweeping ? "tidying up…" : "typing…"}
              </span>
            </span>
          )}
        </div>
      </div>
    </div>
  );
}

/** Compact tool-call card: status icon, tool name, arguments, and the
 *  result summary once the call finishes. */
function ToolCallBlock({ call }: { call: ToolCallRecord }) {
  const running = call.status === "running";
  return (
    <div className="rounded border border-border-subtle bg-surface-2 px-2.5 py-1.5 animate-fade-in">
      <div className="flex items-center gap-1.5 min-w-0">
        {running ? (
          <Loader2 size={11} className="shrink-0 animate-spin text-white-50" />
        ) : call.status === "ran" ? (
          <Check size={11} className="shrink-0 text-status-active" />
        ) : call.status === "denied" ? (
          <Ban size={11} className="shrink-0 text-status-warning" />
        ) : (
          <AlertTriangle size={11} className="shrink-0 text-status-error" />
        )}
        <Wrench size={11} className="shrink-0 text-white-15" />
        <span className="text-[12px] font-medium text-white truncate">{call.name}</span>
        {call.arguments && (
          <span className="text-[11px] text-white-30 font-mono truncate" title={call.arguments}>
            {call.arguments}
          </span>
        )}
        {running && <span className="text-[10px] text-white-30 shrink-0">running…</span>}
      </div>
      {!running && call.summary && (
        <div className="text-[11px] text-white-30 mt-0.5 truncate" title={call.summary}>
          {call.summary}
        </div>
      )}
    </div>
  );
}

/** One queued draft chip: truncated preview, inline edit, discard.
 *  Editing happens in place so the composer stays free to draft the
 *  next message while earlier ones wait in line. */
function QueuedMessageItem({ item }: { item: QueuedMessage }) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(item.content);

  const commit = () => {
    const text = draft.trim();
    if (!text) return;
    const mentions = extractMentions(text);
    useStore.getState().updateQueuedMessage(item.id, {
      content: text,
      mentions: mentions.length > 0 ? mentions : undefined,
    });
    setEditing(false);
  };

  if (editing) {
    return (
      <div className="flex items-center gap-2 bg-surface-2 border border-border-strong rounded px-2.5 py-1.5 animate-fade-in">
        <input
          autoFocus
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              commit();
            } else if (e.key === "Escape") {
              setDraft(item.content);
              setEditing(false);
            }
          }}
          className="flex-1 min-w-0 bg-transparent text-[12px] text-white outline-none"
          placeholder="Message…"
          aria-label="Edit queued message"
        />
        <button
          onClick={commit}
          disabled={!draft.trim()}
          className="text-white-50 hover:text-status-active disabled:opacity-40 transition-colors shrink-0"
          title="Save"
          aria-label="Save queued message"
        >
          <Check size={13} />
        </button>
        <button
          onClick={() => {
            setDraft(item.content);
            setEditing(false);
          }}
          className="text-white-30 hover:text-white transition-colors shrink-0"
          title="Cancel edit"
          aria-label="Cancel edit"
        >
          <X size={13} />
        </button>
      </div>
    );
  }

  return (
    <div className="flex items-center gap-2 bg-surface-2 border border-border-subtle rounded px-2.5 py-1.5 animate-fade-in">
      <span className="flex-1 min-w-0 text-[12px] text-white-50 truncate" title={item.content}>
        {item.content}
      </span>
      <button
        onClick={() => {
          setDraft(item.content);
          setEditing(true);
        }}
        className="text-white-30 hover:text-white transition-colors shrink-0"
        title="Edit queued message"
        aria-label="Edit queued message"
      >
        <Pencil size={12} />
      </button>
      <button
        onClick={() => useStore.getState().removeQueuedMessage(item.id)}
        className="text-white-30 hover:text-status-error transition-colors shrink-0"
        title="Discard queued message"
        aria-label="Discard queued message"
      >
        <X size={13} />
      </button>
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
