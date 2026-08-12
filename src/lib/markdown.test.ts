import { describe, expect, it } from "vitest";
import { extractMentions } from "../components/MentionInput";
import { filterAndSanitizeUrl, stripEventHandlers } from "./mention-utils";

// Integration-style smoke test for the data flow that powers a chat
// message: extract mentions, filter URLs, strip event handlers. These
// are the three pure-logic operations the MessageRenderer pipeline
// composes in order — verifying them together catches regressions in
// any of the three.

describe("chat message pipeline", () => {
  it("extracts mentions, filters unsafe URLs, and strips event handlers", () => {
    const text = "See @src/auth/login.ts and the [unsafe link](javascript:alert(1)) plus an inline https://example.com image <img src='https://x.com/y.png' onerror='hack()'>";

    const mentions = extractMentions(text);
    expect(mentions).toEqual([{ path: "src/auth/login.ts" }]);

    // The pipeline would call these on the marked HTML output; simulate
    // that surface here.
    const raw = `<a href="javascript:alert(1)">bad</a> <a href="https://ok.com">ok</a> <img src="data:image/png;base64,abc">`;
    const filtered = raw
      .replace(/<a\s+([^>]*?)href="([^"]*)"([^>]*)>/gi, (_m, pre, href, post) => {
        const safe = filterAndSanitizeUrl(href);
        if (!safe) return "";
        return `<a ${pre}href="${safe}"${post}>`;
      });
    expect(filtered).not.toContain("javascript:");
    expect(filtered).toContain("https://ok.com");
    expect(filtered).toContain("data:image/png;base64,abc");

    const html = '<img onerror="x" src="https://x.com/y.png">';
    const stripped = stripEventHandlers(html);
    expect(stripped).not.toContain("onerror=");
    expect(stripped).toContain('src="https://x.com/y.png"');
  });
});
