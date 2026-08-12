import { describe, expect, it } from "vitest";
import {
  parseMentionQuery,
  rankMentionResults,
  MENTION_BROWSE_LIMIT,
  filterAndSanitizeUrl,
  stripEventHandlers,
} from "./mention-utils";
import { extractMentions } from "../components/MentionInput";

describe("parseMentionQuery", () => {
  it("extracts the @query when the @ is preceded by whitespace or start-of-line", () => {
    expect(parseMentionQuery("Explain @auth", 13)).toEqual({ active: true, query: "auth" });
    expect(parseMentionQuery("@auth how does it work", 5)).toEqual({ active: true, query: "auth" });
    expect(parseMentionQuery("foo@bar", 7)).toEqual({ active: false, query: "" });
  });

  it("stops at the first whitespace or newline", () => {
    expect(parseMentionQuery("Explain @src/auth/login.ts please", 26)).toEqual({
      active: true,
      query: "src/auth/login.ts",
    });
    expect(parseMentionQuery("Explain @foo\nbar", 12)).toEqual({ active: true, query: "foo" });
  });

  it("treats a second @ inside the query as a hard boundary", () => {
    expect(parseMentionQuery("@@foo", 5)).toEqual({ active: false, query: "" });
  });
});

describe("rankMentionResults", () => {
  it("ranks exact filename matches above path-contains matches", () => {
    const ranked = rankMentionResults("auth", [
      "auth_helpers/login.ts",
      "auth.ts",
      "src/auth.ts",
    ]);
    expect(ranked[0]).toBe("auth.ts");
  });

  it("caps at 10 results", () => {
    const many = Array.from({ length: 25 }, (_, i) => `file_${i}.ts`);
    expect(rankMentionResults("file", many)).toHaveLength(10);
  });

  it("returns a browsable list when the query is empty (bare @)", () => {
    const files = ["src/a.ts", "src/b.ts", "README.md"];
    expect(rankMentionResults("", files)).toEqual(files);
  });

  it("caps the empty-query browse list", () => {
    const many = Array.from({ length: 150 }, (_, i) => `file_${i}.ts`);
    expect(rankMentionResults("", many)).toHaveLength(MENTION_BROWSE_LIMIT);
  });

  it("ranks path-prefix queries above filename matches", () => {
    const ranked = rankMentionResults("src/au", [
      "auth.ts",
      "src/auth/login.ts",
      "src/utils/auth.ts",
    ]);
    expect(ranked[0]).toBe("src/auth/login.ts");
  });
});

describe("filterAndSanitizeUrl", () => {
  it("preserves safe URL schemes", () => {
    expect(filterAndSanitizeUrl("https://example.com")).toBe("https://example.com");
    expect(filterAndSanitizeUrl("data:image/png;base64,abc")).toBe("data:image/png;base64,abc");
    expect(filterAndSanitizeUrl("mailto:foo@bar.com")).toBe("mailto:foo@bar.com");
    expect(filterAndSanitizeUrl("tel:+15551234567")).toBe("tel:+15551234567");
  });

  it("drops dangerous URL schemes", () => {
    expect(filterAndSanitizeUrl("javascript:alert(1)")).toBe("");
    expect(filterAndSanitizeUrl("vbscript:msgbox(1)")).toBe("");
    expect(filterAndSanitizeUrl("file:///etc/passwd")).toBe("");
  });

  it("rejects malformed URLs", () => {
    expect(filterAndSanitizeUrl("not a url")).toBe("");
    expect(filterAndSanitizeUrl("")).toBe("");
  });
});

describe("stripEventHandlers", () => {
  it("removes on* attributes from HTML", () => {
    expect(
      stripEventHandlers('<img onerror="x" src="https://x.com/y.png">')
    ).toBe('<img src="https://x.com/y.png">');
    expect(
      stripEventHandlers('<a href="https://x.com" onclick="hack()">link</a>')
    ).toBe('<a href="https://x.com">link</a>');
  });

  it("leaves other attributes untouched", () => {
    expect(
      stripEventHandlers('<img class="thumb" data-id="42" src="https://x.com/y.png">')
    ).toBe('<img class="thumb" data-id="42" src="https://x.com/y.png">');
  });
});

describe("extractMentions", () => {
  it("finds @path tokens in a message", () => {
    expect(extractMentions("Explain @src/auth/login.ts please")).toEqual([
      { path: "src/auth/login.ts" },
    ]);
  });

  it("finds multiple mentions", () => {
    expect(extractMentions("Compare @src/a.ts with @src/b.ts")).toEqual([
      { path: "src/a.ts" },
      { path: "src/b.ts" },
    ]);
  });

  it("stops at whitespace", () => {
    expect(extractMentions("@src/auth/login.ts please")).toEqual([
      { path: "src/auth/login.ts" },
    ]);
  });

  it("returns an empty array when there are no mentions", () => {
    expect(extractMentions("plain text")).toEqual([]);
  });
});
