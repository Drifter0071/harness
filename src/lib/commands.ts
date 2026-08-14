/**
 * Slash-command registry (spec §2).
 *
 * PR2 — Slash commands (G1). PR3 wires the actual handlers
 * (compact, mode persistence, model switching) via CommandContext.
 */

import type { CommandMode } from "./ipc";
import type { Session } from "./store";

// ── Types ─────────────────────────────────────────────────────────────────────

export interface CommandArgSpec {
  name: string;
  values?: string[];
  required?: boolean;
}

export interface Command {
  name: string;
  summary: string;
  args?: CommandArgSpec[];
  execute: (ctx: CommandContext) => Promise<CommandResult>;
}

export interface CommandContext {
  session: Session | null;
  /** Raw text after the "/" — e.g. for "/mode Yolo" this is "mode Yolo". */
  input: string;
  /** Append a user/assistant/system message to the chat. */
  pushUserMessage: (text: string) => void;
  /** Switch the command mode. */
  setMode: (m: CommandMode) => void;
  /** Switch the active model by id. */
  setModel: (id: string) => void;
  /** Available model ids (for /model tab-completion). */
  availableModelIds: string[];
  /** Current mode label shown in /mode item. */
  currentModeLabel: string;
  /** Current model label shown in /model item. */
  currentModelLabel: string;
  /** Trigger a chat compaction. No-op if no active session. */
  compact: () => Promise<void>;
  /** Clear all messages after user confirmation. */
  clear: () => void;
  /** Show a transient toast notification. */
  showToast: (msg: string) => void;
}

export interface CommandResult {
  /** Text to show as a system note in the chat. */
  userMessage?: string;
  /** Toast message to show (for non-chat events like /clear). */
  toast?: string;
}

// ── Slash activation parser ───────────────────────────────────────────────────

/**
 * Detects whether the caret is inside an active slash-command token and
 * returns the query text (everything after the "/" up to the caret).
 *
 * Rules (spec §2.1):
 * - The "/" must be the first non-whitespace character of its line.
 * - There must be no whitespace between the "/" and the caret.
 *
 * @param value  The full textarea value.
 * @param caret  Current caret position (0-based index into value).
 */
export function parseSlashQuery(value: string, caret: number): {
  active: boolean;
  query: string;
} {
  // Find the start of the current line.
  const lineStart = value.lastIndexOf("\n", caret - 1) + 1;
  const lineBeforeCaret = value.slice(lineStart, caret);

  // Skip leading whitespace on the line to find the first non-whitespace char.
  const nonWsMatch = lineBeforeCaret.match(/\S/);
  if (!nonWsMatch || nonWsMatch[0] !== "/") {
    return { active: false, query: "" };
  }

  // Find the last "/" in the prefix and extract everything between it and the caret.
  // Whitespace between "/" and caret means the token has ended (close the popover).
  const lastSlash = lineBeforeCaret.lastIndexOf("/");
  const query = lineBeforeCaret.slice(lastSlash + 1, caret);
  if (/\s/.test(query)) {
    return { active: false, query: "" };
  }

  return { active: true, query };
}

// ── Built-in commands ─────────────────────────────────────────────────────────

async function executeCompact(ctx: CommandContext): Promise<CommandResult> {
  if (!ctx.session) {
    return { toast: "No active session to compact." };
  }
  ctx.showToast("Compacting chat…");
  await ctx.compact();
  return { toast: "Chat compacted." };
}

async function executeMode(ctx: CommandContext): Promise<CommandResult> {
  const raw = ctx.input.replace(/^mode\s*/i, "").trim().toLowerCase();
  const VALID = ["auto", "askrisky", "askall"] as const;
  if (!raw) {
    // No args: show the current mode.
    return {
      userMessage: `Current mode: **${ctx.currentModeLabel}**. Available: /mode commands | yolo`,
    };
  }
  if (raw === "commands") { ctx.setMode("commands"); return { userMessage: "Mode set to **Commands**." }; }
  if (raw === "yolo") { ctx.setMode("yolo"); return { userMessage: "Mode set to **Yolo**." }; }
  return { toast: `Unknown mode "${raw}". Use: /mode commands | yolo` };
}

async function executeModel(ctx: CommandContext): Promise<CommandResult> {
  const raw = ctx.input.replace(/^model\s*/i, "").trim();
  if (!raw) {
    return {
      userMessage: `Current model: **${ctx.currentModelLabel}**. Available: ${ctx.availableModelIds.map((id) => `\`${id}\``).join(" · ") || "(none)"}`,
    };
  }
  const match = ctx.availableModelIds.find((id) => id.toLowerCase() === raw.toLowerCase());
  if (!match) {
    return { toast: `Unknown model "${raw}". Use /model with a configured model id.` };
  }
  ctx.setModel(match);
  return { userMessage: `Model switched to **${match}**.` };
}

async function executeClear(ctx: CommandContext): Promise<CommandResult> {
  const raw = ctx.input.trim().toLowerCase();
  // /clear without args: show the clear message.
  if (!raw) {
    ctx.clear();
    return { userMessage: "Chat cleared." };
  }
  // /clear confirm or /clear yes: re-confirm.
  if (raw === "confirm" || raw === "yes") {
    return { toast: "Already confirmed." };
  }
  // Unknown arg: treat as a no-op with a hint.
  return { toast: `Unknown argument "${raw}". Use /clear or /clear confirm` };
}

async function executeHelp(_ctx: CommandContext): Promise<CommandResult> {
  const lines = [
    "**Available commands:**",
    "``/compact`` — Summarise chat history to save context",
    "``/mode`` — Switch action mode (auto / askRisky / askAll)",
    "``/model <id>`` — Switch the active model",
    "``/clear`` — Clear chat history",
    "``/help`` — Show this message",
    "",
    "Tip: mention files with ``@filename``",
  ].join("\n");
  return { userMessage: lines };
}

// ── Registry ─────────────────────────────────────────────────────────────────

export const COMMANDS: Command[] = [
  { name: "compact", summary: "Compact chat history", execute: executeCompact },
  { name: "mode", summary: "Switch between Auto, Ask risky, Ask all", execute: executeMode },
  { name: "model", summary: "Switch the active model", execute: executeModel },
  { name: "clear", summary: "Clear the current chat", execute: executeClear },
  { name: "help", summary: "Show all available commands", execute: executeHelp },
];

/** All command names (for filtering / tab-completion). */
export const COMMAND_NAMES = COMMANDS.map((c) => c.name);

/**
 * Find commands matching `query` (case-insensitive prefix match).
 * Returns all commands when query is empty.
 */
export function filterCommands(query: string): Command[] {
  const q = query.toLowerCase();
  return q ? COMMANDS.filter((c) => c.name.startsWith(q)) : COMMANDS;
}

/**
 * Execute a slash command by name.
 * @param name  Command name without the leading "/".
 * @param input  The raw text after the "/" (e.g. "mode Yolo").
 * @param ctx   Execution context.
 */
export async function executeCommand(
  name: string,
  input: string,
  ctx: CommandContext,
): Promise<CommandResult> {
  const cmd = COMMANDS.find((c) => c.name === name);
  if (!cmd) {
    return { toast: `Unknown command: /${name}` };
  }
  return cmd.execute({ ...ctx, input });
}
