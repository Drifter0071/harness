import { useEffect, useRef, useState, type CSSProperties } from "react";
import { createPortal } from "react-dom";
import { useStore } from "../lib/store";
import { ipc, chatEvents, agentEvents } from "../lib/ipc";
import { Settings, Circle, FolderPlus, PanelLeftOpen } from "lucide-react";
import { AgentMascot, ConfettiBurst, type MascotMood } from "./AgentMascot";

/** Halo tint behind the mascot, per mood — soft, never loud.
 *  Matches the body color the SVG body tints to (sky / lilac / sun / coral). */
const HALO: Record<MascotMood, string> = {
  idle:    "rgba(91, 201, 255, 0.30)",   // sky
  working: "rgba(178, 141, 255, 0.32)",  // lilac
  success: "rgba(255, 201, 77, 0.34)",   // sun
  error:   "rgba(239, 68, 68, 0.30)",    // coral / red
};

/** Accent border color per mood. Applied inline on the capsule so it
 *  survives the glass-mist cascade (unlayered custom CSS beats Tailwind
 *  utilities); the frost fill comes from glass-mist itself. */
const ACCENT_BORDER: Record<MascotMood, string> = {
  idle: "rgba(255, 255, 255, 0.07)",
  working: "rgba(84, 227, 168, 0.35)", // mint
  success: "rgba(255, 201, 77, 0.40)", // sun
  error: "rgba(239, 68, 68, 0.40)", // red
};

const fmt = (n: number) => n.toLocaleString("en-US");

export function ActiveJobBar() {
  const {
    jobActivities,
    activeAgents,
    setSettingsOpen,
    setWorkspacePath,
    sidebarOpen,
    setSidebarOpen,
  } = useStore();
  const hasActivity = jobActivities.length > 0;

  // Thoughts popover — portaled to <body> at a high z-index so it always
  // floats above chat content, modals and the title bar. Hover state is
  // shared between the capsule and the popover itself, so moving across
  // the 8px gap between them doesn't close it mid-read.
  const capsuleRef = useRef<HTMLDivElement>(null);
  const [thoughtsOpen, setThoughtsOpen] = useState(false);
  const [thoughtsPos, setThoughtsPos] = useState<{ top: number; left: number } | null>(null);
  const showThoughts = () => setThoughtsOpen(true);
  const hideThoughts = () => setThoughtsOpen(false);
  useEffect(() => {
    if (!thoughtsOpen) {
      setThoughtsPos(null);
      return;
    }
    const update = () => {
      const el = capsuleRef.current;
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

  // Bip's mood: working whenever anything is in flight, a brief
  // success hop + confetti when a turn lands, a red "hmm" on errors.
  const streamingRequestId = useStore((s) => s.streamingRequestId);
  const agentRequestId = useStore((s) => s.agentRequestId);
  const working = hasActivity || !!streamingRequestId || !!agentRequestId;
  const [celebrate, setCelebrate] = useState(false);
  const [errorFlash, setErrorFlash] = useState(false);
  // First-render flag — drives the one-shot entrance reveal on the
  // main Bip. The mini Bip in the thoughts popover skips the entrance
  // (it's a passive context icon, not a performative character).
  const firstMountRef = useRef(true);
  const isFirstRender = firstMountRef.current;
  // After the first commit we drop the flag so subsequent re-renders
  // skip the entrance. We flip it post-render via a layout effect so
  // React commits the first paint with `firstMount` still true.
  useEffect(() => {
    firstMountRef.current = false;
  }, []);

  // Thoughts panel telemetry — the "in depth" view of the session.
  const tokenStats = useStore((s) => s.tokenStats);
  const savedModels = useStore((s) => s.savedModels);
  const activeModelId = useStore((s) => s.activeModelId);

  // Terminal events from both the plain-chat and agent loops drive the
  // celebration / error expressions.
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

  const label = hasActivity
    ? jobActivities[0]
    : working
      ? "Working…"
      : celebrate
        ? "Done!"
        : errorFlash
          ? "Hmm…"
          : "Idle";

  // Bip's inner monologue for the hover panel.
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

  const activeModel = savedModels.find((m) => m.id === activeModelId) ?? null;
  const lastIn = tokenStats?.lastIn ?? null;
  const lastOut = tokenStats?.lastOut ?? null;
  const cumulativeIn = tokenStats?.cumulativeIn ?? null;
  const cumulativeOut = tokenStats?.cumulativeOut ?? null;
  const sw = tokenStats?.sessionWindowUsed ?? null;

  const handleOpenFolder = async () => {
    try {
      const path = await ipc.pickProjectFolder();
      if (path) setWorkspacePath(path);
    } catch (err) {
      console.error("Failed to open folder:", err);
    }
  };

  return (
    // Sits in row 1 of the app-level 3-col grid (left / center / right).
    // The 3 pills are direct children of the parent grid; this component
    // only contributes the pill content, not its own column track. The
    // Bip thoughts popover is portaled to <body> and stays above all.
    <>
      {/* Pill 1 — left: sidebar + open project */}
        {/* Pill 1 — left: sidebar + open project */}
        <div className="glass-mist glass-mist--pill inline-flex items-center gap-0.5 px-1.5 py-1 justify-self-start">
          {!sidebarOpen && (
            <button
              onClick={() => setSidebarOpen(true)}
              className="glass-quartz glass-quartz--pill flex items-center justify-center w-[34px] h-[34px] text-white-50 hover:text-white"
              title="Show sidebar"
              aria-label="Show sidebar"
            >
              <PanelLeftOpen size={16} />
            </button>
          )}
          <button
            onClick={handleOpenFolder}
            className="glass-quartz glass-quartz--pill flex items-center justify-center w-[34px] h-[34px] text-white-50 hover:text-white"
            title="Open a folder as a workspace"
            aria-label="Open folder"
          >
            <FolderPlus size={16} />
          </button>
        </div>

        {/* Pill 2 — center: Bip alone. Hover him to read his thoughts.
            The popover is portaled to <body> so its z-index escapes the
            pill bar's stacking context and floats above everything. */}
        <div
          ref={capsuleRef}
          className="relative justify-self-center"
          onMouseEnter={showThoughts}
          onMouseLeave={hideThoughts}
        >
          <div
            className="glass-mist glass-mist--pill flex items-center px-2 py-0.5 rounded-full cursor-default transition-colors"
            aria-label={`Agent status: ${label}`}
            style={{ borderColor: ACCENT_BORDER[mood] }}
          >
            <div
              className="relative flex items-center justify-center w-[40px] h-[40px] select-none"
              title={`Bip — ${mood}`}
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
                <AgentMascot mood={mood} size={36} firstMount={isFirstRender} />
              </div>
              {celebrate && <ConfettiBurst />}
            </div>
          </div>

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
              {/* Bip's thoughts — header with mini Bip + inner monologue */}
              <div className="flex items-start gap-2">
                <div className="relative mt-0.5 flex-shrink-0">
                  <AgentMascot mood={mood} size={20} />
                </div>
                <div className="min-w-0">
                  <div className="text-[10px] text-white-30 uppercase tracking-wider">
                    Bip's thoughts
                  </div>
                  <div className="text-[12px] text-white-50 leading-snug">{thought}</div>
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
                        <Circle size={4} className="fill-accent-green text-accent-green flex-shrink-0" />
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

              {/* Turn state */}
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

              {/* Session telemetry */}
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
        </div>

        {/* Pill 3 — right: settings alone */}
        <div className="glass-mist glass-mist--pill inline-flex items-center px-1 py-1 justify-self-end">
          <button
            onClick={() => setSettingsOpen(true)}
            className="glass-quartz glass-quartz--pill flex items-center justify-center w-[34px] h-[34px] text-white-50 hover:text-white"
            title="Settings"
          >
            <Settings size={16} />
          </button>
        </div>
    </>
  );
}
