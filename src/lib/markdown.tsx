// Markdown rendering pipeline. Lazy-loads KaTeX, Mermaid, and the image
// lightbox the first time they're needed.

import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react";
import type { RefObject } from "react";
import { marked } from "marked";
import { Check, Copy } from "lucide-react";
import { filterAndSanitizeUrl, stripEventHandlers } from "./mention-utils";
import { highlightCode } from "./highlight";

// ── Marked configuration ──

marked.setOptions({ gfm: true, breaks: true });

// ── Lazy renderers ──
//
// One-time module-level promise so multiple mermaid blocks in one
// message only fetch once.

let mermaidPromise: Promise<typeof import("mermaid").default> | null = null;
async function loadMermaid() {
  if (!mermaidPromise) {
    mermaidPromise = import("mermaid").then((m) => m.default);
  }
  return mermaidPromise;
}

let katexPromise: Promise<typeof import("katex")> | null = null;
async function loadKatex() {
  if (!katexPromise) {
    katexPromise = import("katex");
  }
  return katexPromise;
}

const MermaidBlock = lazy(async () => {
  const mermaid = await loadMermaid();
  mermaid.initialize({ startOnLoad: false, theme: "dark" });
  return import("./renderers/MermaidBlock").then((m) => ({ default: m.MermaidBlock }));
});

const MathBlock = lazy(async () => {
  const katex = await loadKatex();
  return import("./renderers/MathBlock").then((m) => ({ default: m.MathBlock(katex) }));
});

const InlineMath = lazy(async () => {
  const katex = await loadKatex();
  return import("./renderers/InlineMath").then((m) => ({ default: m.InlineMath(katex) }));
});

const LightboxClient = lazy(() =>
  import("./renderers/Lightbox").then((m) => ({ default: m.LightboxClient }))
);

// ── Render hosts ──
//
// The HTML produced by `marked` is post-processed to swap in placeholders
// for our lazy renderers. The placeholders are <div data-renderer="..."
// data-source="..."> elements; the renderer tree picks them up.

interface ProcessedHtml {
  html: string;
  needsMermaid: boolean;
  needsMathBlock: boolean;
  needsInlineMath: boolean;
  needsLightbox: boolean;
}

const PLACEHOLDER_ID = "harness-host";

// Inline `$...$` math: balance dollars, reject inside <code>/<pre>/<a>.
// This is a best-effort pass over the HTML output, not a full parser.
function detectInlineMath(html: string): { processed: string; found: boolean } {
  // Walk through tags and only scan text nodes. Simple state machine.
  let out = "";
  let i = 0;
  let found = false;
  while (i < html.length) {
    const lt = html.indexOf("<", i);
    if (lt < 0) {
      out += processTextNode(html.slice(i));
      break;
    }
    if (lt > i) {
      out += processTextNode(html.slice(i, lt));
    }
    const gt = html.indexOf(">", lt);
    if (gt < 0) {
      out += html.slice(lt);
      break;
    }
    out += html.slice(lt, gt + 1);
    i = gt + 1;
  }
  function processTextNode(text: string): string {
    // Split on $...$ pairs, but only outside of an "in code" context.
    // For simplicity we just balance dollars and replace balanced pairs.
    const parts: string[] = [];
    let buf = "";
    let inDollar = false;
    let start = 0;
    for (let j = 0; j < text.length; j++) {
      if (text[j] === "$") {
        if (!inDollar) {
          buf += text.slice(start, j);
          start = j + 1;
          inDollar = true;
        } else {
          const src = text.slice(start, j);
          if (src.length > 0) {
            const id = `${PLACEHOLDER_ID}-im-${parts.length}-${Math.random().toString(36).slice(2, 8)}`;
            parts.push(buf);
            parts.push(
              `<span class="math-inline-host" data-source="${escapeAttr(src)}" data-host-id="${id}"></span>`
            );
            buf = "";
            found = true;
            start = j + 1;
            inDollar = false;
          } else {
            // empty $...$ — not math
            buf += "$" + text.slice(start, j);
            start = j + 1;
            inDollar = false;
          }
        }
      }
    }
    buf += text.slice(start);
    if (inDollar) {
      // Unbalanced: emit the leading $ literally.
      buf = "$" + buf;
    }
    return parts.join("") + buf;
  }
  return { processed: out, found };
}

function escapeAttr(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/"/g, "&quot;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

// Code fences for `mermaid` and `math` become host placeholders.
// Inline `<img>` and `<a href>` get URL filtering. Event handlers get
// stripped.
function postProcessHtml(html: string): ProcessedHtml {
  let out = html;
  let needsMermaid = false;
  let needsMathBlock = false;

  // Mermaid code fences
  out = out.replace(
    /<pre><code class="language-mermaid">([\s\S]*?)<\/code><\/pre>/g,
    (_m, src) => {
      needsMermaid = true;
      const id = `${PLACEHOLDER_ID}-mermaid-${Math.random().toString(36).slice(2, 8)}`;
      return `<div class="mermaid-host" data-source="${escapeAttr(decodeHtml(src))}" data-host-id="${id}"></div>`;
    }
  );

  // Math code fences
  out = out.replace(
    /<pre><code class="language-math">([\s\S]*?)<\/code><\/pre>/g,
    (_m, src) => {
      needsMathBlock = true;
      const id = `${PLACEHOLDER_ID}-math-${Math.random().toString(36).slice(2, 8)}`;
      return `<div class="math-block-host" data-source="${escapeAttr(decodeHtml(src))}" data-host-id="${id}"></div>`;
    }
  );

  // URL filter on <a href> and <img src>
  out = out.replace(/<a\s+([^>]*?)href="([^"]*)"([^>]*)>/gi, (full, pre, href, post) => {
    const safe = filterAndSanitizeUrl(href);
    if (!safe) return "";
    return `<a ${pre}href="${escapeAttr(safe)}"${post}>`;
  });
  out = out.replace(/<img\s+([^>]*?)src="([^"]*)"([^>]*)>/gi, (full, pre, src, post) => {
    const safe = filterAndSanitizeUrl(src);
    if (!safe) return "";
    // Wrap in a button for the lightbox
    return `<button class="lightbox-trigger" data-src="${escapeAttr(safe)}"><img ${pre}src="${escapeAttr(safe)}"${post}></button>`;
  });

  // Strip event handler attributes
  out = stripEventHandlers(out);

  // Inline math (after URL filter so we don't munge $ inside <a>).
  const { processed, found } = detectInlineMath(out);
  return {
    html: processed,
    needsMermaid,
    needsMathBlock,
    needsInlineMath: found,
    needsLightbox: /class="lightbox-trigger"/.test(processed),
  };
}

function decodeHtml(s: string): string {
  return s
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"');
}

// ── Public component ──

export function MessageBody({ content }: { content: string }) {
  const processed = useMemo<ProcessedHtml>(() => {
    try {
      const raw = marked.parse(content, { async: false }) as string;
      return postProcessHtml(raw);
    } catch {
      return {
        html: `<p>${escapeAttr(content)}</p>`,
        needsMermaid: false,
        needsMathBlock: false,
        needsInlineMath: false,
        needsLightbox: false,
      };
    }
  }, [content]);

  // Mount lazy renderers in response to the placeholders appearing in
  // the DOM. The `useEffect` runs after `dangerouslySetInnerHTML` has
  // injected the elements.
  const [tick, setTick] = useState(0);
  const proseRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    // Force a re-render after the HTML is injected so the children
    // findDOMNode() picks up the freshly-mounted placeholders.
    const id = requestAnimationFrame(() => setTick((t) => t + 1));
    return () => cancelAnimationFrame(id);
  }, [processed.html]);

  return (
    <>
      <div
        ref={proseRef}
        className="prose-harness"
        dangerouslySetInnerHTML={{ __html: processed.html }}
      />
      {/* Decorate code blocks with a language label + copy button. */}
      <CodeBlockDecorator scopeRef={proseRef} html={processed.html} />
      {/* tick: re-render trigger; mount lazy hosts whenever needed */}
      {processed.needsMermaid && (
        <HostMounts selector={`.mermaid-host`} key={`mermaid-${tick}`}>
          {(el) => <MermaidBlock source={el.dataset.source ?? ""} />}
        </HostMounts>
      )}
      {processed.needsMathBlock && (
        <HostMounts selector={`.math-block-host`} key={`math-${tick}`}>
          {(el) => <MathBlock source={el.dataset.source ?? ""} />}
        </HostMounts>
      )}
      {processed.needsInlineMath && (
        <HostMounts selector={`.math-inline-host`} key={`im-${tick}`}>
          {(el) => <InlineMath source={el.dataset.source ?? ""} />}
        </HostMounts>
      )}
      {processed.needsLightbox && (
        <LightboxClient key={`light-${tick}`} />
      )}
    </>
  );
}

// Walk the DOM and instantiate a lazy renderer for each placeholder.
// We don't actually mount React children into the placeholder (that would
// require portals); instead we render into a side effect that replaces
// the placeholder's content after the lazy module loads.
import { createRoot, type Root } from "react-dom/client";
import { createElement } from "react";

// ── Code block toolbar (language label + copy) ──
//
// The prose HTML is inert from React's perspective, so we decorate each
// `pre` block by direct DOM mutation and mount the copy control with its
// own root. Scoped to this message's own prose container — every message
// runs its own pass, and the whole subtree is rebuilt on every streamed
// chunk (html change → fresh decorator run).

function CodeBlockDecorator({
  scopeRef,
  html,
}: {
  scopeRef: RefObject<HTMLDivElement | null>;
  html: string;
}) {
  useEffect(() => {
    const scope = scopeRef.current;
    if (!scope) return;
    const roots: Root[] = [];
    for (const pre of Array.from(scope.querySelectorAll<HTMLPreElement>("pre"))) {
      if (pre.dataset.decorated === "1") continue;
      pre.dataset.decorated = "1";
      const code = pre.querySelector("code");
      // Read the declared language first — highlightAuto mutates the
      // class with its guess, which we don't want to show as a label.
      const lang = code?.className.match(/language-([\w#+-]+)/)?.[1];
      if (code) highlightCode(code);
      if (lang) pre.dataset.lang = lang;
      const bar = document.createElement("div");
      bar.className = "code-toolbar";
      const label = document.createElement("span");
      label.className = "code-toolbar__lang";
      label.textContent = lang ?? "";
      const host = document.createElement("span");
      host.className = "code-toolbar__copy";
      bar.append(label, host);
      pre.prepend(bar);
      const root = createRoot(host);
      roots.push(root);
      root.render(
        <CopyButton getText={() => code?.textContent ?? pre.textContent ?? ""} />
      );
    }
    return () => {
      // Defer the nested-root unmounts one microtask: unmounting a root
      // synchronously while React is committing the parent's unmount
      // triggers a React 18.3 dev warning (and can race the fresh pass).
      const pending = roots;
      queueMicrotask(() => {
        pending.forEach((r) => r.unmount());
      });
    };
  }, [html, scopeRef]);
  return null;
}

/** Copy control mounted inside each fenced code block. */
function CopyButton({ getText }: { getText: () => string }) {
  const [copied, setCopied] = useState(false);
  const resetTimer = useRef<number | undefined>(undefined);
  const onCopy = async () => {
    const ok = await copyTextToClipboard(getText());
    if (!ok) return;
    setCopied(true);
    window.clearTimeout(resetTimer.current);
    resetTimer.current = window.setTimeout(() => setCopied(false), 1600);
  };
  useEffect(() => {
    return () => window.clearTimeout(resetTimer.current);
  }, []);
  return (
    <button
      type="button"
      className="code-copy"
      onClick={onCopy}
      aria-label="Copy code"
      title="Copy code"
    >
      {copied ? <Check size={12} /> : <Copy size={12} />}
    </button>
  );
}

/** Clipboard write with a legacy fallback for WebView permission gaps. */
async function copyTextToClipboard(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    // fall through to the textarea path
  }
  const ta = document.createElement("textarea");
  ta.value = text;
  ta.style.position = "fixed";
  ta.style.opacity = "0";
  document.body.appendChild(ta);
  try {
    ta.select();
    return document.execCommand("copy");
  } catch {
    return false;
  } finally {
    document.body.removeChild(ta);
  }
}

function HostMounts({
  selector,
  children,
}: {
  selector: string;
  children: (el: HTMLElement) => React.ReactNode;
}) {
  useEffect(() => {
    const elements = Array.from(
      document.querySelectorAll<HTMLElement>(selector)
    );
    if (elements.length === 0) return;
    const roots: Root[] = [];
    for (const el of elements) {
      if (el.dataset.mounted === "1") continue;
      el.dataset.mounted = "1";
      const root = createRoot(el);
      roots.push(root);
      root.render(
        <Suspense
          fallback={
            <div className="border border-border-subtle rounded p-3 text-[11px] text-white-15">
              Loading renderer…
            </div>
          }
        >
          {children(el)}
        </Suspense>
      );
    }
    return () => {
      // Deferred unmounts — see CodeBlockDecorator above.
      const pending = roots;
      const els = elements;
      queueMicrotask(() => {
        pending.forEach((r) => r.unmount());
        els.forEach((el) => {
          delete el.dataset.mounted;
        });
      });
    };
    // children is a fresh closure per render; re-running when tick changes
    // is fine because data-mounted guards against double-mounts.
  }, [selector]);
  return null;
}
