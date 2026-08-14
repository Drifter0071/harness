/**
 * Slash-command popover (spec §2).
 *
 * PR2 — Slash commands (G1).
 * Uses the ComposerPopover primitive from PR1 to display filtered commands
 * and insert the selected command name into the textarea.
 *
 * The parent (MentionInput) owns the active/open state and keyboard nav;
 * this component only handles rendering.
 */

import { ComposerPopover } from "./ComposerPopover";
import type { Command } from "../lib/commands";

interface Props {
  open: boolean;
  query: string;
  results: Command[];
  selectedIndex: number;
  currentModeLabel: string;
  currentModelLabel: string;
  onSelect: (commandName: string) => void;
  onDismiss: () => void;
  onSelectedIndexChange: (idx: number) => void;
}

function currentAnnotation(
  cmdName: string,
  query: string,
  currentModeLabel: string,
  currentModelLabel: string,
): string | null {
  if (query !== "") return null; // Only show when query is empty.
  switch (cmdName) {
    case "mode":
      return `(current: ${currentModeLabel})`;
    case "model":
      return `(current: ${currentModelLabel})`;
    default:
      return null;
  }
}

export function SlashCommandPopover({
  open,
  query,
  results,
  selectedIndex,
  currentModeLabel,
  currentModelLabel,
  onSelect,
  onDismiss,
  onSelectedIndexChange,
}: Props) {
  return (
    <ComposerPopover
      open={open}
      query={query}
      results={results.map((c) => c.name)}
      selectedIndex={selectedIndex}
      label="Commands"
      renderItem={(name, _query, isSelected) => {
        const cmd = results.find((c) => c.name === name);
        if (!cmd) return null;
        const ann = currentAnnotation(name, query, currentModeLabel, currentModelLabel);

        return (
          <div
            className={`w-full text-left px-3 py-1.5 text-[12px] font-mono flex items-center gap-2 transition-colors ${
              isSelected
                ? "bg-white/[0.06] text-white"
                : "text-white-50 hover:bg-white/[0.03]"
            }`}
          >
            <span
              className={
                isSelected
                  ? "text-white font-medium"
                  : "text-white"
              }
            >
              /{name}
            </span>
            <span className={isSelected ? "text-white-50" : "text-white-30"}>
              {cmd.summary}
            </span>
            {ann && (
              <span className="ml-auto shrink-0 text-[10px] text-white-15">
                {ann}
              </span>
            )}
          </div>
        );
      }}
      onSelect={onSelect}
      onDismiss={onDismiss}
      onSelectedIndexChange={onSelectedIndexChange}
    />
  );
}
