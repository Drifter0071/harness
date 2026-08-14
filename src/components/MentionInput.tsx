import { useEffect, useRef, useState } from "react";
import { useStore } from "../lib/store";
import { parseMentionQuery, rankMentionResultsThrottled } from "../lib/mention-utils";
import { parseSlashQuery, filterCommands } from "../lib/commands";
import type { Command } from "../lib/commands";
import { FileTypeIcon } from "../lib/file-icons";
import type { Mention } from "../lib/store";
import { ComposerPopover } from "./ComposerPopover";
import { SlashCommandPopover } from "./SlashCommandPopover";

interface Props {
  value: string;
  onChange: (next: string) => void;
  onSend: (text: string, mentions: Mention[]) => void;
  disabled?: boolean;
  placeholder?: string;
  /** Slash popover data supplied by Chat.tsx. MentionInput owns the open
   *  state and keyboard nav; Chat supplies the command list and labels. */
  slashCommands?: {
    commands: Command[];
    currentModeLabel: string;
    currentModelLabel: string;
  };
}

interface MentionPopoverState {
  open: boolean;
  query: string;
  results: string[];
  selectedIndex: number;
}

interface SlashPopoverState {
  open: boolean;
  query: string;
  results: Command[];
  selectedIndex: number;
}

export function MentionInput({
  value,
  onChange,
  onSend,
  disabled,
  placeholder,
  slashCommands,
}: Props) {
  const { fileTree, workspacePath } = useStore();
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const [mentionPopover, setMentionPopover] = useState<MentionPopoverState>({
    open: false,
    query: "",
    results: [],
    selectedIndex: 0,
  });

  const [slashPopover, setSlashPopover] = useState<SlashPopoverState>({
    open: false,
    query: "",
    results: [],
    selectedIndex: 0,
  });

  // ── recomputePopover — runs on every keystroke (spec §2.7 arbitration) ──
  const recomputePopover = () => {
    const ta = textareaRef.current;
    if (!ta) return;
    const caret = ta.selectionStart ?? 0;

    // Whitespace before caret always closes the mention popover.
    if (caret > 0 && /\s/.test(value[caret - 1])) {
      setMentionPopover((p) => (p.open ? { ...p, open: false } : p));
    }

    // 1. Compute mention activation.
    const parsed = parseMentionQuery(value, caret);
    const mentionActive = parsed.active && !!workspacePath;

    // 2. Compute slash activation (only when slashCommands prop is provided).
    let slashActive = false;
    let slashQuery = "";
    if (slashCommands) {
      const slashParsed = parseSlashQuery(value, caret);
      slashActive = slashParsed.active;
      slashQuery = slashParsed.query;
    }

    // 3. Arbitration: at most one popover is open. Slash wins.
    //    (spec §2.7: slash is more "command-like"; @ mid-token is often a typo.)
    const slashIsOpen = slashActive;
    const mentionIsOpen = mentionActive && !slashActive;

    // Update mention popover.
    if (mentionIsOpen) {
      setMentionPopover((p) => ({
        ...p,
        open: true,
        query: parsed.query,
        selectedIndex: 0,
      }));
      rankMentionResultsThrottled(parsed.query, fileTree, (results) => {
        setMentionPopover((p) => ({ ...p, results }));
      });
    } else {
      setMentionPopover((p) => (p.open ? { ...p, open: false } : p));
    }

    // Update slash popover.
    if (slashCommands) {
      const filtered = filterCommands(slashQuery);
      if (slashIsOpen) {
        setSlashPopover({
          open: true,
          query: slashQuery,
          results: filtered,
          selectedIndex: 0,
        });
      } else {
        setSlashPopover((p) => (p.open ? { ...p, open: false } : p));
      }
    }
  };

  useEffect(() => {
    recomputePopover();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value, fileTree, workspacePath]);

  // ── Mention commit ──────────────────────────────────────────────────────────

  const commitMentionSelection = (path: string) => {
    const ta = textareaRef.current;
    if (!ta) return;
    const caret = ta.selectionStart ?? 0;
    const parsed = parseMentionQuery(value, caret);
    if (!parsed.active) return;
    const at = value.lastIndexOf("@", caret - 1);
    if (at < 0) return;
    const before = value.slice(0, at);
    const after = value.slice(caret);
    const insertion = `@${path} `;
    const next = before + insertion + after;
    onChange(next);
    requestAnimationFrame(() => {
      const pos = before.length + insertion.length;
      ta.focus();
      ta.setSelectionRange(pos, pos);
    });
    setMentionPopover((p) => ({ ...p, open: false }));
  };

  // ── Slash commit — insert command name + space into textarea (spec §2.4) ──

  const commitSlashSelection = (commandName: string) => {
    const ta = textareaRef.current;
    if (!ta) return;
    const caret = ta.selectionStart ?? 0;
    const lineStart = value.lastIndexOf("\n", caret - 1) + 1;
    // Replace the partial command token with the full name + space.
    const before = value.slice(0, lineStart);
    const after = value.slice(caret);
    const insertion = `/${commandName} `;
    const next = before + insertion + after;
    onChange(next);
    requestAnimationFrame(() => {
      const pos = before.length + insertion.length;
      ta.focus();
      ta.setSelectionRange(pos, pos);
    });
    setSlashPopover((p) => ({ ...p, open: false }));
  };

  // ── Shared keyboard handler for both popovers ──────────────────────────────

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // Unified arrow/enter/esc handler: whichever popover is open handles it.
    const mentionOpen = mentionPopover.open && mentionPopover.results.length > 0;
    const slashOpen = slashPopover.open && slashPopover.results.length > 0;

    const moveDown = () => {
      if (slashOpen) {
        setSlashPopover((p) => ({
          ...p,
          selectedIndex: (p.selectedIndex + 1) % Math.max(p.results.length, 1),
        }));
      } else if (mentionOpen) {
        setMentionPopover((p) => ({
          ...p,
          selectedIndex: (p.selectedIndex + 1) % p.results.length,
        }));
      }
    };

    const moveUp = () => {
      if (slashOpen) {
        setSlashPopover((p) => ({
          ...p,
          selectedIndex:
            (p.selectedIndex - 1 + Math.max(p.results.length, 1)) %
            Math.max(p.results.length, 1),
        }));
      } else if (mentionOpen) {
        setMentionPopover((p) => ({
          ...p,
          selectedIndex:
            (p.selectedIndex - 1 + p.results.length) % p.results.length,
        }));
      }
    };

    const commitCurrent = () => {
      if (slashOpen) {
        const cmd = slashPopover.results[slashPopover.selectedIndex];
        if (cmd) commitSlashSelection(cmd.name);
      } else if (mentionOpen) {
        commitMentionSelection(mentionPopover.results[mentionPopover.selectedIndex]);
      }
    };

    if (e.key === "ArrowDown") {
      e.preventDefault();
      moveDown();
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      moveUp();
      return;
    }
    if (e.key === "Enter" || e.key === "Tab") {
      if (slashOpen || mentionOpen) {
        e.preventDefault();
        commitCurrent();
        return;
      }
    }
    if (e.key === "Escape") {
      if (slashPopover.open || mentionPopover.open) {
        e.preventDefault();
        setSlashPopover((p) => ({ ...p, open: false }));
        setMentionPopover((p) => ({ ...p, open: false }));
        return;
      }
    }

    // Bug fix A (spec §3.1 Fix A): dismiss mention popover on space immediately.
    // Fires before the textarea value updates so the popover closes synchronously.
    if (mentionPopover.open && e.key === " ") {
      e.preventDefault();
      setMentionPopover((p) => ({ ...p, open: false }));
      return;
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
      mentions.push({ path: m[1] });
    }
    onSend(text, mentions);
    onChange("");
    setMentionPopover((p) => ({ ...p, open: false }));
    setSlashPopover((p) => ({ ...p, open: false }));
  };

  const browseTruncated =
    !mentionPopover.query && mentionPopover.results.length < fileTree.length;

  const activeSlash = slashPopover.open && slashCommands;

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
        style={{ minHeight: "44px", maxHeight: "200px", caretColor: "#5bc9ff" }}
      />

      {/* Slash command popover (spec §2 — wins arbitration over mention) */}
      {activeSlash && slashCommands && (
        <SlashCommandPopover
          open={slashPopover.open}
          query={slashPopover.query}
          results={slashPopover.results}
          selectedIndex={slashPopover.selectedIndex}
          currentModeLabel={slashCommands.currentModeLabel}
          currentModelLabel={slashCommands.currentModelLabel}
          onSelect={commitSlashSelection}
          onDismiss={() => setSlashPopover((p) => ({ ...p, open: false }))}
          onSelectedIndexChange={(idx) =>
            setSlashPopover((p) => ({ ...p, selectedIndex: idx }))
          }
        />
      )}

      {/* Mention popover (spec §2.7 — closes when slash is active) */}
      {!activeSlash && (
        <ComposerPopover
          open={mentionPopover.open}
          query={mentionPopover.query}
          results={mentionPopover.results}
          selectedIndex={mentionPopover.selectedIndex}
          label={mentionPopover.query ? `Files matching "${mentionPopover.query}"` : "Project files"}
          totalCount={fileTree.length}
          truncatedCount={fileTree.length - mentionPopover.results.length}
          renderItem={(path, query, isSelected) => {
            const slash = path.lastIndexOf("/");
            const base = slash >= 0 ? path.slice(slash + 1) : path;
            const dir = slash > 0 ? path.slice(0, slash) : "";
            return (
              <div
                className={`w-full text-left px-3 py-1.5 text-[12px] font-mono flex items-center gap-2 transition-colors ${
                  isSelected
                    ? "bg-white/[0.06] text-white"
                    : "text-white-50 hover:bg-white/[0.03]"
                }`}
              >
                <FileTypeIcon path={path} size={14} className="flex-shrink-0" />
                <span className="min-w-0 flex-1 truncate">
                  <HighlightedText
                    text={
                      base.toLowerCase().includes(query.toLowerCase())
                        ? base
                        : path
                    }
                    query={query}
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
              </div>
            );
          }}
          onSelect={commitMentionSelection}
          onDismiss={() => setMentionPopover((p) => ({ ...p, open: false }))}
          onSelectedIndexChange={(idx) =>
            setMentionPopover((p) => ({ ...p, selectedIndex: idx }))
          }
        />
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
