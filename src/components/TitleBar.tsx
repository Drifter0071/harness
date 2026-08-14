// Custom window title bar.
//
// Replaces the native Windows chrome (the window is undecorated — see
// `decorations: false` in tauri.conf.json). Hosts the four app-bar
// actions (show/hide sidebar, open folder, settings, agent manager) on
// the left, the **Bip** mascot in the center (no separate pill — he
// lives in this bar per AGENTS.md, "the top capsule bar"), and the
// minimize / maximize / close window controls on the right.
//
// Dragging: the strip is a Tauri drag region
// (`data-tauri-drag-region="deep"`, Tauri ≥ 2.11). Clicks on any
// non-button pixel start a native window drag. Buttons and the Bip
// element carry `data-tauri-drag-region="false"` to opt out so their
// clicks / hovers fire. Double-click on the strip maximizes (native
// HTCAPTION).
//
// Window-control permissions live in
// `src-tauri/capabilities/default.json` — without those, every
// `core:window:*` invoke is blocked and min/max/close silently do
// nothing. Buttons no-op in plain browser dev (`isTauri()` false).

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { getCurrentWindow, type Window } from "@tauri-apps/api/window";
import { isTauri } from "@tauri-apps/api/core";
import {
  Minus,
  Square,
  Copy,
  X,
  PanelLeftOpen,
  PanelLeftClose,
  FolderPlus,
  Settings,
  Bot,
  Circle,
} from "lucide-react";
import { useStore } from "../lib/store";
import { ipc, chatEvents, agentEvents } from "../lib/ipc";
import { AgentMascot, ConfettiBurst, type MascotMood } from "./AgentMascot";

/** Height of the title bar in px. */
export const TITLEBAR_HEIGHT = 36;

// ── Tauri runtime guard ────────────────────────────────────────────────

function isDesktop(): boolean {
  if (typeof window === "undefined") return false;
  if ("__TAURI_INTERNALS__" in window) return true;
  return false;
}

async function safeWindow(): Promise<Window | null> {
  if (!isDesktop() && !(await isTauri())) return null;
  return getCurrentWindow();
}

// ── Bip mood maps ─────────────────────────────────────────────────────

/** Halo tint behind the mascot, per mood — soft, never loud. */
const HALO: Record<MascotMood, string> = {
  idle: "rgba(255, 255, 255, 0.10)",
  working: "rgba(84, 227, 168, 0.22)",
  success: "rgba(255, 201, 77, 0.28)",
  error: "rgba(239, 68, 68, 0.22)",
};

const fmt = (n: number) => n.toLocaleString("en-US");

// ── Bar button ────────────────────────────────────────────────────────
//
// 36px hit target, full strip height, hover lifts to white on white-08.
// Close is the only "danger" button. All buttons opt out of the drag
// region so their clicks fire.

function BarButton({
  onClick,
  label,
  title,
  danger = false,
  children,
}: {
  onClick: () => void;
  label: string;
  title: string;
  danger?: boolean;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      aria-label={label}
      data-tauri-drag-region="false"
      className={`flex items-center justify-center w-[36px] h-full text-white-50 transition-colors duration-100 ${
        danger
          ? "hover:bg-status-error hover:text-white"
          : "hover:bg-white/[0.08] hover:text-white active:bg-white/[0.14]"
      }`}
    >
      {children}
    </button>
  );
}

function BarDivider() {
  return (
    <div
      aria-hidden
      data-tauri-drag-region="false"
      className="self-center w-px h-4 bg-border-subtle mx-0.5"
    />
  );
}

// ── Title bar component ───────────────────────────────────────────────

export function TitleBar() {
  const {
    sidebarOpen,
    setSidebarOpen,
    setSettingsOpen,
    setAgentManagerOpen,
    setWorkspacePath,
    jobActivities,
    activeAgents,
  } = useStore();
  const hasActivity = jobActivities.length > 0;

  const [maximized, setMaximized] = useState(false);
  const hasMounted = useRef(false);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    safeWindow().then((win) => {
      if (cancelled || !win) return;
      const refresh = () =>
        win
          .isMaximized()
          .then((m) => {
            if (!cancelled) setMaximized(m);
          })
          .catch(() => {});
      refresh();
      win
        .onResized(refresh)
        .then((fn) => {
          unlisten = fn;
        })
        .catch(() => {});
    });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const minimize = useCallback(
    () => void safeWindow().then((w) => w?.minimize().catch(() => {})),
    []
  );
  const toggleMaximize = useCallback(
    () => void safeWindow().then((w) => w?.toggleMaximize().catch(() => {})),
    []
  );
  const close = useCallback(
    () => void safeWindow().then((w) => w?.close().catch(() => {})),
    []
  );

  const handleOpenFolder = useCallback(async () => {
    try {
      const path = await ipc.pickProjectFolder();
      if (path) setWorkspacePath(path);
    } catch (err) {
      console.error("Failed to open folder:", err);
    }
  }, [setWorkspacePath]);

  // ── Bip: mood engine + thought popover ─────────────────────────────

  const streamingRequestId = useStore((s) => s.streamingRequestId);
  const agentRequestId = useStore((s) => s.agentRequestId);
  const tokenStats = useStore((s) => s.tokenStats);
  const savedModels = useStore((s) => s.savedModels);
  const activeModelId = useStore((s) => s.activeModelId);

  const working = hasActivity || !!streamingRequestId || !!agentRequestId;
  const [celebrate, setCelebrate] = useState(false);
  const [errorFlash, setErrorFlash] = useState(false);

  useEffect(() => {
    const unsubs: Promise<() => void>[] = [];
    const reg = (p: Promise<() => void>) => {
      p.catch(() => {});
      unsubs.push(p);
    };
    const celebrateNow = () => {
      setCelebrate(true);
      window.setTimeout(() => setCelebrate(false), 3200);
    };
    const errorNow = () => {
      setErrorFlash(true);
      window.setTimeout(() => setErrorFlash(false), 5000);
    };
    reg(chatEvents.onDone(celebrateNow));
    reg(chatEvents.onError(errorNow));
    reg(agentEvents.onDone(celebrateNow));
    reg(agentEvents.onError(errorNow));
    return () => {
      unsubs.forEach((p) => {
        p.then((unlisten) => unlisten()).catch(() => {});
      });
    };
  }, []);

  const mood: MascotMood = celebrate
    ? "success"
    : errorFlash
      ? "error"
      : working
        ? "working"
        : "idle";

  const label = hasActivity
    ? jobActivities[0]
    : working
      ? "Working…"
      : celebrate
        ? "Done!"
        : errorFlash
          ? "Hmm…"
          : "Idle";

  const thought =
    mood === "working"
      ? agentRequestId && streamingRequestId
        ? "Two things at once — an agent turn and a stream. Juggling."
        : agentRequestId
          ? "Agent turn in flight — tools are moving."
          : streamingRequestId
            ? "Streaming a reply — words incoming."
            : hasActivity
              ? `On it — ${jobActivities.length} task${jobActivities.length === 1 ? "" : "s"} in flight.`
              : "Busy, actually."
      : mood === "success"
        ? "That one landed clean. Confetti earned."
        : mood === "error"
          ? "Something went sideways. Letting the red glow show it."
          : "Nothing on my plate. Dozing by the capsule.";

  // Popover position. The bar is `fixed` and the popover is portaled
  // to <body> at z-[90] so it floats above everything (Settings, Agent
  // Manager, modals).
  const bipRef = useRef<HTMLDivElement>(null);
  const [thoughtsOpen, setThoughtsOpen] = useState(false);
  const [thoughtsPos, setThoughtsPos] = useState<{ top: number; left: number } | null>(
    null
  );
  const showThoughts = () => setThoughtsOpen(true);
  const hideThoughts = () => setThoughtsOpen(false);

  useEffect(() => {
    if (!thoughtsOpen) {
      setThoughtsPos(null);
      return;
    }
    hasMounted.current = true;
    const update = () => {
      const el = bipRef.current;
      if (!el) return;
      const r = el.getBoundingClientRect();
      setThoughtsPos({ top: r.bottom + 8, left: r.left + r.width / 2 });
    };
    update();
    window.addEventListener("scroll", update, true);
    window.addEventListener("resize", update);
    return () => {
      window.removeEventListener("scroll", update, true);
      window.removeEventListener("resize", update);
    };
  }, [thoughtsOpen]);

  const activeModel = savedModels.find((m) => m.id === activeModelId) ?? null;
  const lastIn = tokenStats?.lastIn ?? null;
  const lastOut = tokenStats?.lastOut ?? null;
  const cumulativeIn = tokenStats?.cumulativeIn ?? null;
  const cumulativeOut = tokenStats?.cumulativeOut ?? null;
  const sw = tokenStats?.sessionWindowUsed ?? null;

  return (
    <>
      <div
        data-tauri-drag-region="deep"
        className="relative h-9 flex-shrink-0 z-[60] flex items-stretch justify-between bg-surface-0 border-b border-border-subtle select-none"
      >
        {/* Left cluster: wordmark + app actions. */}
        <div className="flex items-stretch h-full">
          <div className="flex items-center pl-4 pr-3">
            <span className="text-[9px] uppercase tracking-[0.14em] font-medium text-white-15">
              Harness
            </span>
          </div>

          <BarDivider />

          <BarButton
            onClick={() => setSidebarOpen(!sidebarOpen)}
            label={sidebarOpen ? "Hide sidebar" : "Show sidebar"}
            title={sidebarOpen ? "Hide sidebar" : "Show sidebar"}
          >
            {sidebarOpen ? (
              <PanelLeftClose size={16} />
            ) : (
              <PanelLeftOpen size={16} />
            )}
          </BarButton>

          <BarButton
            onClick={handleOpenFolder}
            label="Open folder"
            title="Open a folder as a workspace"
          >
            <FolderPlus size={16} />
          </BarButton>

          <BarDivider />

          <BarButton
            onClick={() => setSettingsOpen(true)}
            label="Settings"
            title="Settings"
          >
            <Settings size={16} />
          </BarButton>

          <BarButton
            onClick={() => setAgentManagerOpen(true)}
            label="Agent Manager"
            title="Agent Manager"
          >
            <Bot size={16} />
          </BarButton>
        </div>

        {/* Center cluster: Bip mascot, no pill — he lives in the bar
            per AGENTS.md (the top capsule bar). Absolutely positioned
            at the bar's true horizontal center so he's not biased by
            the width of the left / right clusters. Hovers trigger the
            thought popover (portaled below). */}
        <div
          ref={bipRef}
          data-tauri-drag-region="false"
          className="absolute top-0 left-1/2 -translate-x-1/2 h-full flex items-center justify-center w-[44px]"
          onMouseEnter={showThoughts}
          onMouseLeave={hideThoughts}
          aria-label={`Agent status: ${label}`}
          title={`Bip — ${mood}`}
        >
          <div className="relative flex items-center justify-center w-[40px] h-[40px] select-none">
            <div
              className="mascot-halo"
              style={{ "--halo-color": HALO[mood] } as CSSProperties}
            />
            <div
              className={
                mood === "working"
                  ? "mascot-bob--fast"
                  : mood === "success"
                    ? "mascot-bounce"
                    : mood === "error"
                      ? "mascot-shake"
                      : "mascot-bob"
              }
            >
              <AgentMascot mood={mood} size={32} />
            </div>
            {celebrate && <ConfettiBurst />}
          </div>
        </div>

        {/* Right cluster: window controls. */}
        <div className="flex items-stretch h-full">
          <BarButton onClick={minimize} label="Minimize" title="Minimize">
            <Minus size={12} />
          </BarButton>
          <BarButton
            onClick={toggleMaximize}
            label={maximized ? "Restore" : "Maximize"}
            title={maximized ? "Restore" : "Maximize"}
          >
            {maximized ? <Copy size={10} /> : <Square size={10} />}
          </BarButton>
          <BarButton onClick={close} label="Close" title="Close" danger>
            <X size={12} />
          </BarButton>
        </div>
      </div>

      {/* Bip's thoughts — portaled to <body> at high z-index so it
          floats above the chat, modals, and any other overlay. Shared
          hover state with the Bip element so the 8px gap doesn't
          close it mid-read. */}
      {thoughtsOpen &&
        thoughtsPos &&
        createPortal(
          <div
            className="fixed z-[90] glass-mist p-3 min-w-[300px] max-h-[60vh] overflow-y-auto animate-fade-in"
            style={{
              top: thoughtsPos.top,
              left: thoughtsPos.left,
              transform: "translateX(-50%)",
            }}
            onMouseEnter={showThoughts}
            onMouseLeave={hideThoughts}
          >
            <div className="flex items-start gap-2">
              <div className="relative mt-0.5 flex-shrink-0">
                <AgentMascot mood={mood} size={20} />
              </div>
              <div className="min-w-0">
                <div className="text-[10px] text-white-30 uppercase tracking-wider">
                  Bip's thoughts
                </div>
                <div className="text-[12px] text-white-50 leading-snug">
                  {thought}
                </div>
              </div>
            </div>

            {hasActivity && (
              <div className="border-t border-border-subtle mt-2.5 pt-2.5">
                <div className="text-[11px] text-white-30 uppercase tracking-wider mb-1.5">
                  Active Tasks
                </div>
                <div className="space-y-1.5">
                  {jobActivities.map((activity, i) => (
                    <div
                      key={i}
                      className="flex items-center gap-2 text-[13px] text-white-50"
                    >
                      <Circle
                        size={4}
                        className="fill-accent-green text-accent-green flex-shrink-0"
                      />
                      {activity}
                    </div>
                  ))}
                </div>
              </div>
            )}

            {activeAgents.length > 0 && (
              <div className="border-t border-border-subtle mt-2.5 pt-2.5">
                <div className="text-[11px] text-white-30 uppercase tracking-wider mb-1.5">
                  Agents
                </div>
                <div className="space-y-1.5">
                  {activeAgents.map((agent) => (
                    <div
                      key={agent.id}
                      className="flex items-center gap-2 text-[13px] text-white-30"
                    >
                      <Circle
                        size={4}
                        className={`flex-shrink-0 ${
                          agent.status === "Working"
                            ? "fill-accent-green text-accent-green"
                            : "fill-white-15 text-white-15"
                        }`}
                      />
                      <span className="truncate">{agent.name}</span>
                      <span className="text-[11px] text-white-15 ml-auto">
                        {agent.role}
                      </span>
                    </div>
                  ))}
                </div>
              </div>
            )}

            <div className="border-t border-border-subtle mt-2.5 pt-2.5">
              <div className="text-[11px] text-white-30 uppercase tracking-wider mb-1.5">
                Turn
              </div>
              <div className="flex items-center gap-2 text-[12px] text-white-50">
                <Circle
                  size={4}
                  className={
                    working
                      ? "fill-accent-mint text-accent-mint animate-pulse"
                      : "fill-white-15 text-white-15"
                  }
                />
                {working
                  ? agentRequestId
                    ? "Agent turn running"
                    : "Streaming response"
                  : "Idle"}
              </div>
            </div>

            {(activeModel || tokenStats) && (
              <div className="border-t border-border-subtle mt-2.5 pt-2.5">
                <div className="text-[11px] text-white-30 uppercase tracking-wider mb-1.5">
                  Session
                </div>
                <div className="space-y-1 text-[12px]">
                  <div className="flex items-baseline justify-between gap-4">
                    <span className="text-white-30 flex-shrink-0">Model</span>
                    <span className="text-white-50 truncate">
                      {activeModel ? activeModel.label : "—"}
                    </span>
                  </div>
                  {lastIn !== null && (
                    <div className="flex items-baseline justify-between gap-4">
                      <span className="text-white-30 flex-shrink-0">Last response</span>
                      <span className="text-white-50">
                        {fmt(lastIn)} in · {fmt(lastOut ?? 0)} out
                      </span>
                    </div>
                  )}
                  {cumulativeIn !== null && (
                    <div className="flex items-baseline justify-between gap-4">
                      <span className="text-white-30 flex-shrink-0">This session</span>
                      <span className="text-white-50">
                        {fmt(cumulativeIn)} peak in · {fmt(cumulativeOut ?? 0)} out
                      </span>
                    </div>
                  )}
                  {sw !== null && (
                    <div className="flex items-baseline justify-between gap-4">
                      <span className="text-white-30 flex-shrink-0">Session window</span>
                      <span className="text-white-50">{Math.round(sw * 100)}% used</span>
                    </div>
                  )}
                </div>
              </div>
            )}
          </div>,
          document.body
        )}
    </>
  );
}
