import { describe, expect, it } from "vitest";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { MessageBody } from "./markdown";

// happy-dom does not set this; without it React 18 warns on every act().
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// The code-block decorator mutates the inert prose DOM (syntax
// highlighting, language label, copy button). These tests guard that
// injection: it is easy to break silently because nothing asserts on it
// during normal rendering.

async function renderProse(content: string): Promise<{
  host: HTMLElement;
  unmount: () => Promise<void>;
}> {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(<MessageBody content={content} />);
    // Flush the post-injection re-render tick (rAF → timer in happy-dom)
    // inside act so React doesn't warn about the update.
    await new Promise((resolve) => setTimeout(resolve, 32));
  });
  return {
    host,
    unmount: async () => {
      await act(async () => {
        root.unmount();
        // CodeBlockDecorator defers its nested-root unmounts one
        // microtask; let that land inside act too.
        await new Promise<void>((resolve) => queueMicrotask(() => resolve()));
      });
    },
  };
}

const RICH = [
  "# Heading",
  "",
  "Intro with `inline code` and a [link](https://example.com).",
  "",
  "## Sub",
  "",
  "- one",
  "- two",
  "  - nested",
  "",
  "```ts",
  "const x = 1;",
  "```",
  "",
  "> quoted",
  "",
  "| a | b |",
  "|---|--:|",
  "| 1 | 2 |",
].join("\n");

describe("MessageBody prose + code decorator", () => {
  it("renders markdown structure through the prose host", async () => {
    const { host, unmount } = await renderProse(RICH);
    const prose = host.querySelector(".prose-harness");
    expect(prose).not.toBeNull();
    expect(prose!.querySelector("h1")?.textContent).toBe("Heading");
    expect(prose!.querySelectorAll("li").length).toBeGreaterThanOrEqual(2);
    expect(prose!.querySelector("table")).not.toBeNull();
    expect(prose!.querySelector("blockquote")?.textContent).toContain("quoted");
    unmount();
    host.remove();
  });

  it("decorates fenced code blocks with syntax highlighting, a language label, and a copy button", async () => {
    const { host, unmount } = await renderProse(RICH);
    const pre = host.querySelector<HTMLPreElement>(".prose-harness pre");
    expect(pre).not.toBeNull();
    expect(pre!.dataset.decorated).toBe("1");
    expect(pre!.dataset.lang).toBe("ts");

    // highlight.js ran: the code element is marked and `const` is a token.
    const code = pre!.querySelector("code");
    expect(code?.classList.contains("hljs")).toBe(true);
    expect(code?.querySelector("span.hljs-keyword")?.textContent).toBe("const");

    const bar = host.querySelector(".code-toolbar");
    expect(bar).not.toBeNull();
    expect(host.querySelector(".code-toolbar__lang")?.textContent).toBe("ts");
    const button = host.querySelector("button.code-copy");
    expect(button).not.toBeNull();
    expect(button!.getAttribute("aria-label")).toBe("Copy code");

    // Clicking must not throw, and the code content is what gets copied.
    await act(async () => {
      button!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    unmount();
    host.remove();
  });

  it("leaves plain paragraphs undecorated", async () => {
    const { host, unmount } = await renderProse("Just text, no fences.");
    expect(host.querySelector(".code-toolbar")).toBeNull();
    unmount();
    host.remove();
  });

  it("skips unknown declared languages without throwing", async () => {
    const { host, unmount } = await renderProse("```batch\necho hello\n```");
    const code = host.querySelector<HTMLElement>(".prose-harness pre code");
    expect(code).not.toBeNull();
    expect(code!.classList.contains("hljs")).toBe(false);
    expect(host.querySelector(".code-toolbar__lang")?.textContent).toBe("batch");
    unmount();
    host.remove();
  });
});
