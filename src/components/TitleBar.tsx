// Custom window title bar.
//
// Replaces the native Windows chrome (the window is undecorated — see
// `decorations: false` in tauri.conf.json). Deliberately minimal so it
// matches the app's design language: a pure-black 36px strip with a
// hairline bottom border, a quiet uppercase wordmark on the left (same
// recipe as the SESSIONS / CONTENT panel labels), and Windows 11-style
// window controls on the right that brighten on hover — Close turns the
// status red.
//
// Dragging: the strip is a Tauri drag region (`data-tauri-drag-region`),
// so it moves the window; double-click maximizes via the native Windows
// HTCAPTION behavior. Edge resizing keeps working — tao handles
// borderless window resizing for us.
//
// Outside the desktop runtime (plain `npm run dev` in a browser) the
// controls render but no-op, so the web-only dev flow keeps working.

import { useCallback, useEffect, useState, type ReactNode } from "react";
import { getCurrentWindow, type Window } from "@tauri-apps/api/window";
import { Minus, Square, Copy, X } from "lucide-react";

/** Height of the title bar in px. Keep in sync with the `h-9` below and
 *  with the ActiveJobBar offset (`top-[52px]` = 36 + 16 gap). */
export const TITLEBAR_HEIGHT = 36;

function desktopWindow(): Window | null {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window
    ? getCurrentWindow()
    : null;
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

  return (
    <div
      data-tauri-drag-region
      className="relative h-9 flex-shrink-0 z-[60] flex items-center justify-between bg-surface-0 border-b border-border-subtle select-none"
    >
      {/* Wordmark — quiet, at the panel-label brightness. The inner
          elements carry the drag attribute too so the whole left side
          is draggable, not just the strip's background. */}
      <div data-tauri-drag-region className="flex items-center h-full pl-4 pr-3">
        <span
          data-tauri-drag-region
          className="text-[9px] uppercase tracking-[0.14em] font-medium text-white-15"
        >
          Harness
        </span>
      </div>

      {/* Window controls — 46px hit targets, the Windows convention. */}
      <div className="flex items-stretch h-full">
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
    </div>
  );
}
