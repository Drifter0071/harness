import { useEffect, useState, useCallback } from "react";
import { useStore } from "../lib/store";
import { ipc } from "../lib/ipc";
import type { DirEntry, SessionSummary } from "../lib/ipc";
import {
  Folder,
  FileText,
  PanelLeftClose,
  PanelLeftOpen,
  History,
  X,
} from "lucide-react";

// ── Layout constants ─────────────────────────────────────────────────────
//
// Two panels inside a single fixed flex column. The column floats just
// below the chat header strip (chatHeaderHeight + SIDEBAR_GAP) down to
// near the bottom of the viewport, as an overlay over the chat area.
// Content takes the remaining space (flex-1), Sessions sizes to its
// content (flex-shrink-0), and the gap between them is fixed.
//
// Why a column instead of two truly independent panels? Because we want
// Sessions to "hug" the bottom of Content with a fixed gap, and that
// requires both panels to know about each other's heights. Putting them
// in one flex container is the simplest way to guarantee it.

export const SIDEBAR_WIDTH = 280;
const PANEL_GAP = 16; // px between Content (bottom) and Sessions (top)
const SIDEBAR_GAP = 8; // px of breathing room between the chat header and the column
const BOTTOM_ANCHOR = 16;
const SIDE_ANCHOR = 12;
const CONTENT_BODY_MAX = 240; // file tree body cap (px) — keeps the tree from crowding Sessions

// ── Reusable bits ────────────────────────────────────────────────────────

function ActiveDot({ visible }: { visible: boolean }) {
  return (
    <span
      className={`absolute left-1.5 top-1/2 -translate-y-1/2 w-1.5 h-1.5 rounded-full transition-all duration-200 ${
        visible
          ? "bg-accent-green shadow-[0_0_8px_rgba(34,197,94,0.7)]"
          : "bg-transparent group-hover:bg-white/20 shadow-none"
      }`}
      aria-hidden
    />
  );
}

// Single panel chrome — header (section label + optional trailing slot)
// + body. The trailing slot is where Content renders the column's
// close button (next to the "Content" label); Sessions leaves it empty.
function PanelShell({
  label,
  icon,
  count,
  trailing,
  children,
  grow = false,
}: {
  label: string;
  icon?: React.ReactNode;
  count?: number;
  trailing?: React.ReactNode;
  children: React.ReactNode;
  grow?: boolean;
}) {
  return (
    <div
      className={`glass-mist flex flex-col overflow-hidden ${
        grow ? "flex-1 min-h-0" : "flex-shrink-0"
      }`}
    >
      <div className="flex items-center px-3 pt-2.5 pb-1.5 border-b border-border-subtle min-w-0">
        <div className="flex items-center gap-1.5 min-w-0">
          {icon}
          <span className="text-[9px] text-white-15 uppercase tracking-[0.14em] font-medium">
            {label}
          </span>
        </div>
        {(typeof count === "number" && count > 0) || trailing ? (
          <div className="ml-auto flex items-center gap-2 pl-3">
            {typeof count === "number" && count > 0 && (
              <span className="text-[10px] text-white-30 font-mono">
                {count}
              </span>
            )}
            {trailing}
          </div>
        ) : null}
      </div>
      {children}
    </div>
  );
}

// ── Content panel (project files) ────────────────────────────────────────
//
// Natural size — sizes to its content (header + path + file rows) with a
// soft cap so it never crowds Sessions. Sessions is the one that grows.

function ContentPanel({ onClose }: { onClose: () => void }) {
  const { workspacePath } = useStore();
  const [entries, setEntries] = useState<DirEntry[]>([]);
  const [currentPath, setCurrentPath] = useState<string>("");

  useEffect(() => {
    if (!workspacePath) {
      setEntries([]);
      setCurrentPath("");
      return;
    }
    loadDirectory(workspacePath);
  }, [workspacePath]);

  const loadDirectory = useCallback(async (path: string) => {
    try {
      const result = await ipc.listDirectory(path);
      setEntries(result);
      setCurrentPath(path);
    } catch {
      setEntries([]);
    }
  }, []);

  const parentPath =
    currentPath && workspacePath
      ? currentPath.split(/[/\\]/).slice(0, -1).join("/") || "/"
      : "";

  const activeRoot = workspacePath ? workspacePath.split(/[/\\]/).pop() : null;

  return (
    <PanelShell
      label="Content"
      icon={<Folder size={10} className="text-white-30 flex-shrink-0" />}
      trailing={
        <button
          onClick={onClose}
          className="glass-quartz glass-quartz--pill flex items-center justify-center w-[34px] h-[34px] text-white-50 hover:text-white"
          title="Hide sidebar"
          aria-label="Hide sidebar"
        >
          <PanelLeftClose size={16} />
        </button>
      }
    >
      {/* Active workspace pill */}
      {activeRoot && (
        <div className="px-3 pb-1.5">
          <span className="inline-block text-[10px] text-white px-2 py-0.5 rounded-full bg-white/[0.06] border border-white/[0.08] font-mono truncate max-w-[110px] align-middle">
            {activeRoot}
          </span>
        </div>
      )}

      {/* Path strip */}
      {workspacePath && (
        <div
          className="text-[9px] text-white-15 font-mono truncate px-3 pt-1.5 pb-1"
          title={currentPath}
        >
          {currentPath}
        </div>
      )}

      {/* Body — natural height, capped so a giant tree doesn't push
          Sessions off the screen. Scrolls inside the cap. */}
      {!workspacePath ? (
        <div className="px-3 py-6 text-center">
          <div className="text-[11px] text-white-15 leading-relaxed">
            No workspace open.
            <br />
            Use the folder icon to pick one.
          </div>
        </div>
      ) : (
        <div className="overflow-y-auto" style={{ maxHeight: CONTENT_BODY_MAX }}>
          {currentPath !== workspacePath && (
            <button
              onClick={() => loadDirectory(parentPath)}
              className="group w-full flex items-center gap-1.5 px-3 py-1 hover:bg-white/[0.04] transition-all text-left"
            >
              <FileText
                size={10}
                className="text-white-15 group-hover:text-white-50 flex-shrink-0"
              />
              <span className="text-[11px] text-white-30 group-hover:text-white-50">
                ..
              </span>
            </button>
          )}
          {entries.map((entry, i) => {
            const isActive = entry.name === activeRoot;
            const delay = `${Math.min(i, 7) * 25}ms`;
            return (
              <div
                key={entry.path}
                style={{ animationDelay: delay }}
                className={`group relative flex items-center gap-1.5 pl-3 pr-2 py-1 transition-all cursor-pointer animate-list-item ${
                  isActive
                    ? "text-white bg-white/[0.05] border-l border-accent-green/60"
                    : "text-white-50 hover:text-white hover:bg-white/[0.04] hover:translate-x-0.5 border-l border-transparent"
                }`}
                onClick={() => {
                  if (entry.is_dir) loadDirectory(entry.path);
                }}
              >
                <ActiveDot visible={isActive} />
                {entry.is_dir ? (
                  <Folder
                    size={10}
                    className={`flex-shrink-0 ml-1.5 ${
                      isActive
                        ? "text-accent-green"
                        : "text-white-30 group-hover:text-white-50"
                    }`}
                  />
                ) : (
                  <FileText
                    size={10}
                    className="text-white-15 flex-shrink-0 ml-1.5"
                  />
                )}
                <span className="text-[11px] truncate">{entry.name}</span>
              </div>
            );
          })}
          {entries.length === 0 && (
            <div className="px-3 py-4 text-center">
              <span className="text-[11px] text-white-15">No files</span>
            </div>
          )}
        </div>
      )}
    </PanelShell>
  );
}

// ── Sessions panel ───────────────────────────────────────────────────────

function SessionsPanel() {
  const {
    sessions,
    setSessions,
    activeSessionId,
    setActiveSessionId,
    workspacePath,
  } = useStore();

  useEffect(() => {
    if (!workspacePath) {
      setSessions([]);
      return;
    }
    ipc.listSessions(workspacePath)
      .then(setSessions)
      .catch(() => setSessions([]));
  }, [workspacePath, setSessions]);

  const handleForget = async (e: React.MouseEvent, id: string) => {
    e.stopPropagation();
    if (!workspacePath) return;
    try {
      await ipc.deleteSession(workspacePath, id);
      setSessions(sessions.filter((s) => s.id !== id));
      if (activeSessionId === id) setActiveSessionId(null);
    } catch (err) {
      console.error("Failed to delete session:", err);
    }
  };

  return (
    <PanelShell
      label="Sessions"
      icon={<History size={10} className="text-white-30 flex-shrink-0" />}
      count={sessions.length}
      grow
    >
      {/* Body — Sessions is the panel that grows to fill the column.
          Scrolls inside if the list is long. */}
      <div className="flex-1 overflow-y-auto min-h-0 pb-1">
        {sessions.length === 0 ? (
          <div className="px-3 py-4 text-center">
            <span className="text-[11px] text-white-15">
              No sessions yet. Start a conversation.
            </span>
          </div>
        ) : (
          sessions.map((session, i) => {
            const isActive = session.id === activeSessionId;
            const delay = `${Math.min(i, 7) * 25}ms`;
            return (
              <SessionRow
                key={session.id}
                session={session}
                isActive={isActive}
                delay={delay}
                onActivate={() => setActiveSessionId(session.id)}
                onForget={(e) => handleForget(e, session.id)}
              />
            );
          })
        )}
      </div>
    </PanelShell>
  );
}

function SessionRow({
  session,
  isActive,
  delay,
  onActivate,
  onForget,
}: {
  session: SessionSummary;
  isActive: boolean;
  delay: string;
  onActivate: () => void;
  onForget: (e: React.MouseEvent) => void;
}) {
  const label = `${session.title} — ${session.projectName}`;

  return (
    <div
      style={{ animationDelay: delay }}
      onClick={onActivate}
      className={`group relative flex items-center gap-1.5 pl-3 pr-2 py-1.5 cursor-pointer transition-all animate-list-item ${
        isActive
          ? "text-white bg-white/[0.05] border-l border-accent-green/60"
          : "text-white-50 hover:text-white hover:bg-white/[0.04] hover:translate-x-0.5 border-l border-transparent"
      }`}
      role="button"
      tabIndex={0}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onActivate();
        }
      }}
    >
      <ActiveDot visible={isActive} />
      <div className="flex-1 min-w-0 ml-1.5">
        <div className="text-[11px] truncate" title={label}>
          {label}
        </div>
        <div className="text-[9px] text-white-15 font-mono">
          {formatRelativeTime(session.updatedAt)} · {session.messageCount} msg
        </div>
      </div>
      <button
        onClick={onForget}
        className="opacity-0 group-hover:opacity-100 scale-90 group-hover:scale-100 text-white-15 hover:text-status-error hover:shadow-[0_0_6px_rgba(239,68,68,0.5)] transition-all duration-150 p-0.5 flex-shrink-0 rounded"
        title="Forget session"
        aria-label="Forget session"
      >
        <X size={10} />
      </button>
    </div>
  );
}

// ── Helpers ──────────────────────────────────────────────────────────────

function formatRelativeTime(iso: string): string {
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "—";
  const diffMs = Date.now() - then;
  const min = Math.floor(diffMs / 60_000);
  if (min < 1) return "now";
  if (min < 60) return `${min}m ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h ago`;
  const day = Math.floor(hr / 24);
  if (day < 7) return `${day}d ago`;
  const wk = Math.floor(day / 7);
  if (wk < 4) return `${wk}w ago`;
  return new Date(iso).toLocaleDateString();
}

// ── Toggle button (shown when both panels are closed) ────────────────────

export function SidebarToggleFloating({ onToggle }: { onToggle: () => void }) {
  const chatHeaderHeight = useStore((s) => s.chatHeaderHeight);
  return (
    <button
      onClick={onToggle}
      // `!fixed` (Tailwind's `!` important prefix) is required because
      // `.glass-quartz` sets `position: relative` for its ::before/::after
      // pseudo-elements, and the custom CSS loads after Tailwind utilities.
      className="glass-quartz glass-quartz--pill !fixed left-3 z-50 flex items-center justify-center w-[34px] h-[34px] text-white-50 hover:text-white"
      style={{ top: chatHeaderHeight + SIDEBAR_GAP }}
      title="Show sidebar"
      aria-label="Show sidebar"
    >
      <PanelLeftOpen size={16} />
    </button>
  );
}

// ── Close button (lives inside the column at the top of the Content
//    panel header, next to the "Content" label) ───────────────────────────
//
// Same 34×34 as the open toggle so the two read as a pair, just on
// opposite sides of the open/close flow. The close lives inside the
// column on purpose: it's next to the content it dismisses, not
// floating at a global anchor like the open toggle.

// ── Public component (mounts the column + the open toggle) ──────────────

export function Sidebar() {
  const { sidebarOpen, setSidebarOpen, chatHeaderHeight } = useStore();
  const close = () => setSidebarOpen(false);
  // Anchor just below the chat header (height measured live by ChatHeader),
  // as a floating overlay — the chat content does not reflow around it.
  const top = chatHeaderHeight + SIDEBAR_GAP;

  return (
    <>
      {!sidebarOpen && <SidebarToggleFloating onToggle={() => setSidebarOpen(true)} />}
      <div
        {...(!sidebarOpen ? { inert: "" } : {})}
        aria-label="Sidebar"
        aria-hidden={!sidebarOpen}
        className={`fixed z-30 flex flex-col overflow-hidden will-change-transform ${
          sidebarOpen
            ? "translate-x-0 opacity-100 pointer-events-auto"
            : "-translate-x-full opacity-0 pointer-events-none"
        } transition-[transform,opacity,visibility] duration-200 ease-out`}
        style={{
          top,
          bottom: BOTTOM_ANCHOR,
          left: SIDE_ANCHOR,
          width: SIDEBAR_WIDTH,
          gap: PANEL_GAP,
        }}
      >
        <ContentPanel onClose={close} />
        <SessionsPanel />
      </div>
    </>
  );
}

// Re-export for the deprecation shim at src/components/FileExplorer.tsx.
export { Sidebar as FileExplorer };
