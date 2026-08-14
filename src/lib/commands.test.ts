/**
 * Slash-command registry tests (spec §2).
 *
 * PR2 — Slash commands (G1).
 */

import { describe, expect, it, vi } from "vitest";
import {
  parseSlashQuery,
  filterCommands,
  executeCommand,
  COMMAND_NAMES,
  COMMANDS,
} from "./commands";
import type { CommandContext } from "./commands";

function makeCtx(overrides: Partial<CommandContext> = {}): CommandContext {
  return {
    session: null,
    input: "",
    pushUserMessage: vi.fn(),
    setMode: vi.fn(),
    setModel: vi.fn(),
    availableModelIds: ["gpt-4o", "claude-3-5-sonnet"],
    currentModeLabel: "Auto",
    currentModelLabel: "gpt-4o",
    compact: vi.fn().mockResolvedValue(undefined),
    clear: vi.fn(),
    showToast: vi.fn(),
    ...overrides,
  };
}

// ── parseSlashQuery (spec §2.1 trigger rule) ──────────────────────────────────

describe("parseSlashQuery", () => {
  it("opens with bare /", () => {
    const r = parseSlashQuery("/", 1);
    expect(r).toEqual({ active: true, query: "" });
  });

  it("opens with query after /", () => {
    const r = parseSlashQuery("/co", 3);
    expect(r).toEqual({ active: true, query: "co" });
  });

  it("opens with multi-char query", () => {
    const r = parseSlashQuery("/compact", 8);
    expect(r).toEqual({ active: true, query: "compact" });
  });

  it("closes when space follows the token", () => {
    const r = parseSlashQuery("/compact ", 9);
    expect(r).toEqual({ active: false, query: "" });
  });

  it("closes when / is mid-sentence", () => {
    // "hello /co" — "/" is not at line start, so it's not a slash command.
    const r = parseSlashQuery("hello /co", 9);
    expect(r).toEqual({ active: false, query: "" });
  });

  it("opens with leading whitespace then /", () => {
    // Leading whitespace is skipped; first non-whitespace char is "/".
    const r = parseSlashQuery("  /mo", 5);
    expect(r).toEqual({ active: true, query: "mo" });
  });

  it("opens with // (strip leading slashes)", () => {
    const r = parseSlashQuery("//compact", 9);
    expect(r).toEqual({ active: true, query: "compact" });
  });

  it("closes when caret is before the /", () => {
    const r = parseSlashQuery("/compact", 0);
    expect(r).toEqual({ active: false, query: "" });
  });

  it("handles newlines — / on second line only activates on that line", () => {
    // "first line\n/com" — caret=15 is after the "m" of "/com".
    const r = parseSlashQuery("first line\n/com", 15);
    expect(r).toEqual({ active: true, query: "com" });
  });

  it("ignores / after other text on the same line", () => {
    const r = parseSlashQuery("hello /there", 11);
    expect(r).toEqual({ active: false, query: "" });
  });

  it("closes on whitespace in query", () => {
    const r = parseSlashQuery("/comp act", 10);
    expect(r).toEqual({ active: false, query: "" });
  });
});

// ── filterCommands ───────────────────────────────────────────────────────────

describe("filterCommands", () => {
  it("returns all commands when query is empty", () => {
    expect(filterCommands("")).toHaveLength(COMMANDS.length);
  });

  it("filters by prefix", () => {
    const r = filterCommands("co");
    expect(r.map((c) => c.name)).toEqual(["compact"]);
    // "cl" is a prefix of "clear".
    const r2 = filterCommands("cl");
    expect(r2.map((c) => c.name)).toEqual(["clear"]);
  });

  it("is case-insensitive", () => {
    expect(filterCommands("COMP")).toHaveLength(1); // /compact
    // "Mo" matches both /mode and /model (case-insensitive "mo" prefix).
    expect(filterCommands("Mo")).toHaveLength(2);
  });
});

// ── executeCommand ───────────────────────────────────────────────────────────

describe("executeCommand", () => {
  it("returns unknown for a nonexistent command", async () => {
    const ctx = makeCtx();
    const result = await executeCommand("nonexistent", "", ctx);
    expect(result.toast).toContain("Unknown command");
  });

  describe("/compact", () => {
    it("calls compact and shows toast when session is null", async () => {
      const ctx = makeCtx({ session: null });
      const result = await executeCommand("compact", "", ctx);
      expect(result.toast).toBe("No active session to compact.");
    });

    it("calls compact() and shows toast when session exists", async () => {
      const ctx = makeCtx({ session: { id: "s1" } as never });
      const result = await executeCommand("compact", "", ctx);
      expect(ctx.compact).toHaveBeenCalled();
      expect(result.toast).toBe("Chat compacted.");
    });
  });

  describe("/mode", () => {
    it("shows current mode when called with no args", async () => {
      const ctx = makeCtx({ currentModeLabel: "Commands" });
      const result = await executeCommand("mode", "", ctx);
      expect(result.userMessage).toContain("Commands");
    });

    it("sets mode to commands", async () => {
      const ctx = makeCtx();
      await executeCommand("mode", "commands", ctx);
      expect(ctx.setMode).toHaveBeenCalledWith("commands");
    });

    it("sets mode to yolo", async () => {
      const ctx = makeCtx();
      await executeCommand("mode", "yolo", ctx);
      expect(ctx.setMode).toHaveBeenCalledWith("yolo");
    });

    it("rejects unknown mode", async () => {
      const ctx = makeCtx();
      const result = await executeCommand("mode", "foo", ctx);
      expect(result.toast).toContain("Unknown mode");
    });
  });

  describe("/model", () => {
    it("shows current model when called with no args", async () => {
      const ctx = makeCtx({ currentModelLabel: "claude-3-5-sonnet" });
      const result = await executeCommand("model", "", ctx);
      expect(result.userMessage).toContain("claude-3-5-sonnet");
    });

    it("switches to a known model", async () => {
      const ctx = makeCtx({ availableModelIds: ["gpt-4o", "claude-3-5-sonnet"] });
      await executeCommand("model", "claude-3-5-sonnet", ctx);
      expect(ctx.setModel).toHaveBeenCalledWith("claude-3-5-sonnet");
    });

    it("rejects an unknown model", async () => {
      const ctx = makeCtx({ availableModelIds: ["gpt-4o"] });
      const result = await executeCommand("model", "unknown-model", ctx);
      expect(result.toast).toContain("Unknown model");
    });

    it("is case-insensitive", async () => {
      const ctx = makeCtx({ availableModelIds: ["Claude-3-5-Sonnet"] });
      await executeCommand("model", "claude-3-5-sonnet", ctx);
      expect(ctx.setModel).toHaveBeenCalledWith("Claude-3-5-Sonnet");
    });
  });

  describe("/clear", () => {
    it("clears immediately when called with no args", async () => {
      const ctx = makeCtx();
      const result = await executeCommand("clear", "", ctx);
      expect(ctx.clear).toHaveBeenCalled();
      expect(result.userMessage).toContain("cleared");
    });

    it("shows 'already confirmed' for /clear confirm", async () => {
      const ctx = makeCtx();
      const result = await executeCommand("clear", "confirm", ctx);
      expect(ctx.clear).not.toHaveBeenCalled();
      expect(result.toast).toContain("Already confirmed");
    });

    it("shows 'already confirmed' for /clear yes", async () => {
      const ctx = makeCtx();
      const result = await executeCommand("clear", "yes", ctx);
      expect(ctx.clear).not.toHaveBeenCalled();
      expect(result.toast).toContain("Already confirmed");
    });
  });

  describe("/help", () => {
    it("returns a formatted help message", async () => {
      const ctx = makeCtx();
      const result = await executeCommand("help", "", ctx);
      expect(result.userMessage).toContain("/compact");
      expect(result.userMessage).toContain("/mode");
      expect(result.userMessage).toContain("/model");
      expect(result.userMessage).toContain("/clear");
      expect(result.userMessage).toContain("/help");
    });
  });
});

// ── Arbitration helpers (spec §2.7 — tested via MentionInput) ─────────────────

describe("COMMAND_NAMES", () => {
  it("contains all expected commands", () => {
    expect(COMMAND_NAMES).toContain("compact");
    expect(COMMAND_NAMES).toContain("mode");
    expect(COMMAND_NAMES).toContain("model");
    expect(COMMAND_NAMES).toContain("clear");
    expect(COMMAND_NAMES).toContain("help");
    expect(COMMAND_NAMES).toHaveLength(5);
  });
});
