import { useEffect, useRef, useState } from "react";
import { useStore } from "../lib/store";
import { parseMentionQuery, rankMentionResults } from "../lib/mention-utils";
import { FileTypeIcon } from "../lib/file-icons";
import type { Mention } from "../lib/store";

interface Props {
  value: string;
  onChange: (next: string) => void;
  onSend: (text: string, mentions: Mention[]) => void;
  disabled?: boolean;
  placeholder?: string;
}

interface PopoverState {
  open: boolean;
  query: string;
  results: string[];
  selectedIndex: number;
  top: number;
  left: number;
}

export function MentionInput({
  value,
  onChange,
  onSend,
  disabled,
  placeholder,
}: Props) {
  const { fileTree, workspacePath } = useStore();
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const [popover, setPopover] = useState<PopoverState>({
    open: false,
    query: "",
    results: [],
    selectedIndex: 0,
    top: 0,
    left: 0,
  });

  // Update popover position + results whenever the textarea caret moves
  // and the mention is "active".
  const recomputePopover = () => {
    const ta = textareaRef.current;
    if (!ta) return;
    const caret = ta.selectionStart ?? 0;
    const parsed = parseMentionQuery(value, caret);
    if (!parsed.active || !workspacePath) {
      setPopover((p) => (p.open ? { ...p, open: false } : p));
      return;
    }
    // A space right before the caret means the mention token ended — the
    // user is typing the next word. Dismiss like an editor would.
    if (caret > 0 && /\s/.test(value[caret - 1])) {
      setPopover((p) => (p.open ? { ...p, open: false } : p));
      return;
    }
    const results = rankMentionResults(parsed.query, fileTree);
    // Position the popover just below the textarea's bottom edge.
    const rect = ta.getBoundingClientRect();
    setPopover({
      open: true,
      query: parsed.query,
      results,
      selectedIndex: 0,
      top: rect.bottom + 6,
      left: rect.left,
    });
  };

  useEffect(() => {
    recomputePopover();
    // We intentionally depend on `value` so each keystroke re-evaluates.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value, fileTree, workspacePath]);

  // Keep the keyboard-selected row in view while scrolling the list.
  useEffect(() => {
    if (!popover.open) return;
    const el = listRef.current?.querySelector('[data-selected="true"]');
    el?.scrollIntoView({ block: "nearest" });
  }, [popover.selectedIndex, popover.open]);

  const commitSelection = (path: string) => {
    const ta = textareaRef.current;
    if (!ta) return;
    const caret = ta.selectionStart ?? 0;
    // Find the `@` that started this mention.
    const parsed = parseMentionQuery(value, caret);
    if (!parsed.active) return;
    const at = value.lastIndexOf("@", caret - 1);
    if (at < 0) return;
    // Replace the @query with @<path> and add a trailing space.
    const before = value.slice(0, at);
    const after = value.slice(caret);
    const insertion = `@${path} `;
    const next = before + insertion + after;
    onChange(next);
    // Restore caret right after the insertion.
    requestAnimationFrame(() => {
      const pos = before.length + insertion.length;
      ta.focus();
      ta.setSelectionRange(pos, pos);
    });
    setPopover((p) => ({ ...p, open: false }));
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (popover.open && popover.results.length > 0) {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setPopover((p) => ({
          ...p,
          selectedIndex: (p.selectedIndex + 1) % p.results.length,
        }));
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setPopover((p) => ({
          ...p,
          selectedIndex:
            (p.selectedIndex - 1 + p.results.length) % p.results.length,
        }));
        return;
      }
      if (e.key === "Enter" || e.key === "Tab") {
        e.preventDefault();
        commitSelection(popover.results[popover.selectedIndex]);
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setPopover((p) => ({ ...p, open: false }));
        return;
      }
    }
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  const handleSend = () => {
    const text = value.trim();
    if (!text || disabled) return;
    const mentions: Mention[] = [];
    const re = /@([^\s@]+)/g;
    let m: RegExpExecArray | null;
    while ((m = re.exec(text)) !== null) {
      const path = m[1];
      mentions.push({ path });
    }
    onSend(text, mentions);
    onChange("");
    setPopover((p) => ({ ...p, open: false }));
  };

  const browseTruncated =
    !popover.query && popover.results.length < fileTree.length;

  return (
    <>
      <textarea
        ref={textareaRef}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={handleKeyDown}
        onClick={recomputePopover}
        onSelect={recomputePopover}
        placeholder={placeholder}
        disabled={disabled}
        rows={1}
        className="flex-1 bg-transparent text-[14px] leading-relaxed text-white px-4 py-3 my-1.5 resize-none outline-none placeholder:text-white-30 disabled:opacity-50 disabled:pointer-events-none"
        style={{ minHeight: "44px", maxHeight: "200px" }}
      />
      {popover.open && (
        <div
          className="fixed z-50 bg-surface-2 border border-border-default rounded-lg min-w-[300px] max-w-[440px] shadow-lg animate-fade-in flex flex-col overflow-hidden"
          style={{ top: popover.top, left: popover.left }}
        >
          <div className="px-2 py-1 text-[10px] text-white-15 uppercase tracking-wider border-b border-border-subtle flex items-center justify-between">
            <span>
              {popover.query
                ? `Files matching "${popover.query}"`
                : "Project files"}
            </span>
            {!popover.query && (
              <span className="font-mono normal-case">{fileTree.length}</span>
            )}
          </div>

          <div
            ref={listRef}
            className="max-h-[240px] overflow-y-auto overscroll-contain"
          >
            {popover.results.length === 0 ? (
              <div className="px-3 py-3 text-[11px] text-white-30">
                {popover.query
                  ? `No files match "${popover.query}"`
                  : "No files in this project"}
              </div>
            ) : (
              popover.results.map((path, i) => {
                const isSelected = i === popover.selectedIndex;
                const slash = path.lastIndexOf("/");
                const base = slash >= 0 ? path.slice(slash + 1) : path;
                const dir = slash > 0 ? path.slice(0, slash) : "";
                return (
                  <button
                    key={path}
                    data-selected={isSelected || undefined}
                    onMouseDown={(e) => {
                      e.preventDefault();
                      commitSelection(path);
                    }}
                    onMouseEnter={() =>
                      setPopover((p) => ({ ...p, selectedIndex: i }))
                    }
                    className={`w-full text-left px-3 py-1.5 text-[12px] font-mono flex items-center gap-2 transition-colors ${
                      isSelected
                        ? "bg-white/[0.06] text-white"
                        : "text-white-50 hover:bg-white/[0.03]"
                    }`}
                  >
                    <FileTypeIcon path={path} size={14} className="flex-shrink-0" />
                    <span className="min-w-0 flex-1 truncate">
                      <HighlightedText
                        text={base.toLowerCase().includes(popover.query.toLowerCase())
                          ? base
                          : path}
                        query={popover.query}
                        selected={isSelected}
                      />
                      {dir && (
                        <span
                          className={
                            isSelected ? "text-white-30" : "text-white-15"
                          }
                        >
                          {" "}
                          · {dir}
                        </span>
                      )}
                    </span>
                  </button>
                );
              })
            )}
          </div>

          {popover.results.length > 0 && (
            <div className="px-2 py-1 border-t border-border-subtle text-[10px] text-white-15 flex items-center justify-between gap-3">
              <span className="whitespace-nowrap">
                ↑↓ navigate · Enter select · Esc close
              </span>
              {browseTruncated && (
                <span className="truncate text-right font-mono">
                  +{fileTree.length - popover.results.length} more — keep typing
                </span>
              )}
            </div>
          )}
        </div>
      )}
    </>
  );
}

/**
 * Renders `text` with the first case-insensitive occurrence of `query`
 * brightened. The matched span stays white in every state so the user can
 * see *why* a result ranked; the rest dims to signal "selected" state.
 */
function HighlightedText({
  text,
  query,
  selected,
}: {
  text: string;
  query: string;
  selected: boolean;
}) {
  const dim = selected ? "text-white-50" : "text-white-30";
  if (!query) {
    return <span className={selected ? "text-white" : dim}>{text}</span>;
  }
  const idx = text.toLowerCase().indexOf(query.toLowerCase());
  if (idx < 0) {
    return <span className={selected ? "text-white" : dim}>{text}</span>;
  }
  return (
    <>
      <span className={dim}>{text.slice(0, idx)}</span>
      <span className="text-white font-medium">
        {text.slice(idx, idx + query.length)}
      </span>
      <span className={dim}>{text.slice(idx + query.length)}</span>
    </>
  );
}

/**
 * Extract mentions from a message body. Public helper so the same
 * parsing logic is available to other call sites (and to tests).
 */
export function extractMentions(text: string): Mention[] {
  const mentions: Mention[] = [];
  const re = /@([^\s@]+)/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    mentions.push({ path: m[1] });
  }
  return mentions;
}
