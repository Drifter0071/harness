import { useEffect, useRef } from "react";

export interface ComposerPopoverProps {
  open: boolean;
  query: string;
  results: string[];
  selectedIndex: number;
  /** Label shown in the popover header, e.g. "Project files". */
  label: string;
  /** Total count when the results list is truncated (browse mode). */
  totalCount?: number;
  /** Footer hint text, e.g. "↑↓ navigate · Enter select · Esc close". */
  hint?: string;
  /** Extra count shown when query is empty and results are truncated, e.g. "+50 more". */
  truncatedCount?: number;
  /**
   * Renders one item in the list. The popover calls this; the parent
   * decides how to display the item string.
   */
  renderItem: (item: string, query: string, selected: boolean) => React.ReactNode;
  /** Called when the user selects an item (Enter / Tab / click). */
  onSelect: (item: string) => void;
  /** Called when the popover should dismiss (Escape, space, etc.). */
  onDismiss: () => void;
  /** Called when selectedIndex changes so the parent can persist it. */
  onSelectedIndexChange: (idx: number) => void;
}

/**
 * Shared popover primitive for the composer.
 *
 * Handles:
 * - Positioning (absolute, anchored to the bottom of the composer)
 * - Keyboard navigation (↑↓ to move, Enter/Tab to select, Escape to close)
 * - Mouse hover to track selection
 * - Scroll keeping the selected row visible
 *
 * The caller owns: how to detect when to open, what results to show,
 * what the query is, and how to commit a selection.
 */
export function ComposerPopover({
  open,
  query,
  results,
  selectedIndex,
  label,
  totalCount,
  hint = "↑↓ navigate · Enter select · Esc close",
  truncatedCount,
  renderItem,
  onSelect,
  onDismiss,
  onSelectedIndexChange,
}: ComposerPopoverProps) {
  const listRef = useRef<HTMLDivElement>(null);

  // Keep the keyboard-selected row in view while scrolling.
  useEffect(() => {
    if (!open) return;
    const el = listRef.current?.querySelector("[data-selected=\"true\"]");
    el?.scrollIntoView({ block: "nearest" });
  }, [selectedIndex, open]);

  if (!open) return null;

  const browseTruncated = !query && totalCount !== undefined && results.length < totalCount;

  return (
    <div className="absolute left-0 right-0 bottom-full z-50 glass-mist mention-popover animate-mention-in flex flex-col overflow-hidden">
      {/* Header */}
      <div className="px-2 py-1 text-[10px] text-white-15 uppercase tracking-wider border-b border-border-subtle flex items-center justify-between">
        <span>
          {query
            ? `Results for "${query}"`
            : label}
        </span>
        {!query && totalCount !== undefined && (
          <span className="font-mono normal-case">{totalCount}</span>
        )}
      </div>

      {/* List */}
      <div
        ref={listRef}
        className="max-h-[min(320px,45vh)] overflow-y-auto overscroll-contain"
      >
        {results.length === 0 ? (
          <div className="px-3 py-3 text-[11px] text-white-30">
            {query ? `No results for "${query}"` : "No results"}
          </div>
        ) : (
          results.map((item, i) => {
            const isSelected = i === selectedIndex;
            return (
              <div
                key={item}
                data-selected={isSelected || undefined}
                onMouseDown={(e) => {
                  // Prevent textarea blur so click-to-commit works.
                  e.preventDefault();
                  onSelect(item);
                }}
                onMouseEnter={() => onSelectedIndexChange(i)}
                className="cursor-pointer"
              >
                {renderItem(item, query, isSelected)}
              </div>
            );
          })
        )}
      </div>

      {/* Footer */}
      {(results.length > 0 || browseTruncated) && (
        <div className="px-2 py-1 border-t border-border-subtle text-[10px] text-white-15 flex items-center justify-between gap-3">
          <span className="whitespace-nowrap">{hint}</span>
          {browseTruncated && truncatedCount !== undefined && (
            <span className="truncate text-right font-mono">
              +{truncatedCount} more — keep typing
            </span>
          )}
        </div>
      )}
    </div>
  );
}
