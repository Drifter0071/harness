// Custom window title bar.
//
// Replaces the native Windows chrome (the window is undecorated — see
// `decorations: false` in tauri.conf.json). One slim 36px strip that
// holds everything the user needs at the top of the window:
//
//   left  — HARNESS wordmark, divider, sidebar/folder/settings/agent buttons
//   mid   — Bip (the agent mascot), centered
//   right — Minimize / Maximize / Close
//
// Bip lives here, not in a pill row below, because it's the face of
// the product — it should sit in the window chrome. The pill row
// below is gone; the stats header (Model / Last response / Context
// used) renders directly under the topbar in the ChatHeader component.
//
// Dragging: the strip is a Tauri drag region (`data-tauri-drag-region`)
// so it moves the window; double-click maximizes via the native Windows
// HTCAPTION behavior. The interactive cells (Bip, panel buttons, window
// controls) opt out via `onMouseDown stopPropagation` or by NOT
// carrying the drag attribute. Edge resizing keeps working — tao
// handles borderless window resizing for us.
//
// Outside the desktop runtime (plain `npm run dev` in a browser) the
// controls render but no-op, so the web-only dev flow keeps working.

import { useCallback, useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { getCurrentWindow, type Window } from "@tauri-apps/api/window";
import {
  Minus,
  Square,
  Copy,
  X,
  Settings,
  FolderPlus,
  PanelLeftOpen,
  PanelLeftClose,
  Bot,
} from "lucide-react";
import { AgentMascot, ConfettiBurst, type MascotMood } from "./AgentMascot";
import { BipThoughtsPopover } from "./ActiveJobBar";
import { useStore } from "../lib/store";
import { chatEvents, agentEvents, ipc } from "../lib/ipc";

/** Height of the title bar in px. Keep in sync with the `h-9` below. */
export const TITLEBAR_HEIGHT = 36;

/** Halo tint behind the mascot, per mood — soft, never loud.
 *  Matches the body color the SVG body tints to (sky / lilac / sun / coral). */
const HALO: Record<MascotMood, string> = {
  idle:    "rgba(91, 201, 255, 0.30)",   // sky
  working: "rgba(178, 141, 255, 0.32)",  // lilac
  success: "rgba(255, 201, 77, 0.34)",   // sun
  error:   "rgba(239, 68, 68, 0.30)",    // coral / red
};

function desktopWindow(): Window | null {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window
    ? getCurrentWindow()
    : null;
}

/** A small icon button used in the topbar. NOT a drag region so the
 *  user can click it without moving the window. The button itself
 *  sits inside a drag-bearing wrapper so the empty space around it
 *  still drags the window. */
function TopbarButton({
  onClick,
  title,
  ariaLabel,
  children,
}: {
  onClick: () => void;
  title: string;
  ariaLabel: string;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      onMouseDown={(e) => e.stopPropagation()}
      title={title}
      aria-label={ariaLabel}
      className="flex items-center justify-center w-7 h-7 rounded-md text-white-50 transition-colors hover:bg-white/[0.08] hover:text-white active:bg-white/[0.14]"
    >
      {children}
    </button>
  );
}

function TitleBarButton({
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
      onMouseDown={(e) => e.stopPropagation()}
      title={title}
      aria-label={label}
      className={`flex items-center justify-center w-[46px] h-full text-white-50 transition-colors duration-100 ${
        danger
          ? "hover:bg-status-error hover:text-white active:bg-status-error/80"
          : "hover:bg-white/[0.08] hover:text-white active:bg-white/[0.14]"
      }`}
    >
      {children}
    </button>
  );
}

export function TitleBar() {
  const [maximized, setMaximized] = useState(false);

  // Pull store hooks used by both the Bip and the panel buttons.
  const sidebarOpen = useStore((s) => s.sidebarOpen);
  const setSidebarOpen = useStore((s) => s.setSidebarOpen);
  const setSettingsOpen = useStore((s) => s.setSettingsOpen);
  const setAgentManagerOpen = useStore((s) => s.setAgentManagerOpen);
  const setWorkspacePath = useStore((s) => s.setWorkspacePath);

  // Track the maximized state via the Tauri window events.
  useEffect(() => {
    const win = desktopWindow();
    if (!win) return;

    let unlisten: (() => void) | undefined;
    let cancelled = false;

    const refresh = () => {
      win
        .isMaximized()
        .then((m) => {
          if (!cancelled) setMaximized(m);
        })
        .catch(() => {});
    };

    refresh();
    win
      .onResized(refresh)
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {});

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const minimize = useCallback(() => desktopWindow()?.minimize().catch(() => {}), []);
  const toggleMaximize = useCallback(
    () => desktopWindow()?.toggleMaximize().catch(() => {}),
    []
  );
  const close = useCallback(() => desktopWindow()?.close().catch(() => {}), []);

  const handleOpenFolder = async () => {
    try {
      const path = await ipc.pickProjectFolder();
      if (path) setWorkspacePath(path);
    } catch (err) {
      console.error("Failed to open folder:", err);
    }
  };

  /* Bip's mood: working whenever anything is in flight, a brief
   * success hop + confetti when a turn lands, a red "hmm" on errors. */
  const streamingRequestId = useStore((s) => s.streamingRequestId);
  const agentRequestId = useStore((s) => s.agentRequestId);
  const jobActivities = useStore((s) => s.jobActivities);
  const hasActivity = jobActivities.length > 0;
  const working = hasActivity || !!streamingRequestId || !!agentRequestId;
  const [celebrate, setCelebrate] = useState(false);
  const [errorFlash, setErrorFlash] = useState(false);

  /* Thoughts popover — anchored to the Bip element. The popover
   * itself is portaled to <body> by `BipThoughtsPopover`. */
  const bipRef = useRef<HTMLDivElement>(null);
  const [thoughtsOpen, setThoughtsOpen] = useState(false);
  const [thoughtsPos, setThoughtsPos] = useState<{ top: number; left: number } | null>(null);
  const showThoughts = () => {
    const el = bipRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    setThoughtsPos({ top: r.bottom + 8, left: r.left + r.width / 2 });
    setThoughtsOpen(true);
  };
  const hideThoughts = () => setThoughtsOpen(false);
  useEffect(() => {
    if (!thoughtsOpen) {
      setThoughtsPos(null);
      return;
    }
    const update = () => {
      const el = bipRef.current;
      if (!el) return;
      const r = el.getBoundingClientRect();
      setThoughtsPos({ top: r.bottom + 8, left: r.left + r.width / 2 });
    };
    window.addEventListener("scroll", update, true);
    window.addEventListener("resize", update);
    return () => {
      window.removeEventListener("scroll", update, true);
      window.removeEventListener("resize", update);
    };
  }, [thoughtsOpen]);

  /* One-shot entrance on first render. */
  const firstMountRef = useRef(true);
  const isFirstRender = firstMountRef.current;
  useEffect(() => {
    firstMountRef.current = false;
  }, []);

  useEffect(() => {
    const unsubs: Promise<() => void>[] = [];
    // In browser-only dev there's no Tauri event bus; swallow the
    // rejection instead of leaving an unhandled promise.
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

  return (
    <div
      data-tauri-drag-region
      className="relative h-9 flex-shrink-0 z-[60] flex items-center bg-surface-0 border-b border-border-subtle select-none"
    >
      {/* LEFT — wordmark + divider + panel buttons. The wordmark and
          the empty space around buttons drag the window; the buttons
          themselves stop propagation. */}
      <div className="flex items-center h-full pl-4 pr-2 gap-1">
        <span
          data-tauri-drag-region
          className="text-[9px] uppercase tracking-[0.14em] font-medium text-white-15 mr-2 select-none"
        >
          Harness
        </span>
        <span className="h-4 w-px bg-border-subtle mx-1" aria-hidden="true" />
        <TopbarButton
          onClick={() => setSidebarOpen(!sidebarOpen)}
          title={sidebarOpen ? "Hide sidebar" : "Show sidebar"}
          ariaLabel={sidebarOpen ? "Hide sidebar" : "Show sidebar"}
        >
          {sidebarOpen ? <PanelLeftClose size={14} /> : <PanelLeftOpen size={14} />}
        </TopbarButton>
        <TopbarButton
          onClick={handleOpenFolder}
          title="Open a folder as a workspace"
          ariaLabel="Open folder"
        >
          <FolderPlus size={14} />
        </TopbarButton>
        <TopbarButton
          onClick={() => setSettingsOpen(true)}
          title="Settings"
          ariaLabel="Settings"
        >
          <Settings size={14} />
        </TopbarButton>
        <TopbarButton
          onClick={() => setAgentManagerOpen(true)}
          title="Agent manager"
          ariaLabel="Agent manager"
        >
          <Bot size={14} />
        </TopbarButton>
      </div>

      {/* CENTER — Bip, the agent mascot. Not a drag region so the user
          can hover / click it (e.g. for the thoughts popover). */}
      <div
        ref={bipRef}
        className="absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 flex items-center justify-center w-8 h-8"
        title={`Bip — ${mood}`}
        onMouseEnter={showThoughts}
        onMouseLeave={hideThoughts}
      >
        <div
          className="mascot-halo"
          style={{ "--halo-color": HALO[mood] } as CSSProperties}
        />
        <div
          className={
            mood === "working"
              ? "bip-bob--fast"
              : mood === "success"
                ? "bip-celebrate"
                : mood === "error"
                  ? "bip-shake"
                  : "bip-bob"
          }
        >
          <AgentMascot mood={mood} size={24} firstMount={isFirstRender} />
        </div>
        {celebrate && <ConfettiBurst />}
      </div>

      {/* RIGHT — window controls (minimize / maximize / close). */}
      <div className="ml-auto flex items-stretch h-full">
        <TitleBarButton onClick={minimize} label="Minimize" title="Minimize">
          <Minus size={12} />
        </TitleBarButton>
        <TitleBarButton
          onClick={toggleMaximize}
          label={maximized ? "Restore" : "Maximize"}
          title={maximized ? "Restore" : "Maximize"}
        >
          {maximized ? <Copy size={10} /> : <Square size={10} />}
        </TitleBarButton>
        <TitleBarButton onClick={close} label="Close" title="Close" danger>
          <X size={12} />
        </TitleBarButton>
      </div>

      {/* Bip's hover thoughts popover — portaled to <body>. The
       * popover itself has its own hover handlers so the user can
       * move their cursor onto the popover (8px below the Bip)
       * without it closing mid-read. */}
      <BipThoughtsPopover
        open={thoughtsOpen}
        pos={thoughtsPos}
        mood={mood}
        onMouseEnter={showThoughts}
        onMouseLeave={hideThoughts}
      />
    </div>
  );
}
