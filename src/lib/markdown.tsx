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
  // Markdown re-parse + post-process + DOM re-injection is the
  // single heaviest cost in the subagent window under parallel
  // dispatch (16 windows streaming simultaneously). Re-parsing on
  // every chunk is wasteful because the *displayed* text is only
  // visible at 60fps to the eye; a ~75ms idle window still feels
  // live. The `debouncedContent` value lags the live `content` by
  // that window — `useDeferredValue` would do similar, but we need
  // a *fixed* delay tied to the rendering clock so behavior is
  // deterministic across machines.
  const debouncedContent = useDebouncedValue(content, 75);

  const processed = useMemo<ProcessedHtml>(() => {
    try {
      const raw = marked.parse(debouncedContent, { async: false }) as string;
      return postProcessHtml(raw);
    } catch {
      return {
        html: `<p>${escapeAttr(debouncedContent)}</p>`,
        needsMermaid: false,
        needsMathBlock: false,
        needsInlineMath: false,
        needsLightbox: false,
      };
    }
  }, [debouncedContent]);

  // The lazy renderer host selectors re-mount their React roots on
  // every `tick` increment in the old design, which thrashes the DOM
  // under parallel streams (16 subagent windows × per-chunk tick
  // bump = hundreds of root teardowns per second). Instead we keep
  // the host components mounted and key them on a *count* of the
  // placeholder elements — that only changes when a new block opens
  // or closes, not on every streamed character.
  //
  // `proseRef` points at the freshly-injected HTML; counting after
  // mount (next microtask) is enough for the placeholders to be in
  // the DOM. The counts are stable for "still streaming into the
  // same block" which is the common case under parallel dispatch.
  const proseRef = useRef<HTMLDivElement>(null);
  const [mermaidCount, setMermaidCount] = useState(0);
  const [mathBlockCount, setMathBlockCount] = useState(0);
  const [inlineMathCount, setInlineMathCount] = useState(0);
  useEffect(() => {
    // Defer to a microtask so the placeholders injected by
    // `dangerouslySetInnerHTML` are queryable. `queueMicrotask` runs
    // before the next paint, so the user never sees an un-mounted
    // placeholder for more than a frame.
    const id = queueMicrotask(() => {
      const scope = proseRef.current;
      if (!scope) return;
      setMermaidCount(scope.querySelectorAll(".mermaid-host").length);
      setMathBlockCount(scope.querySelectorAll(".math-block-host").length);
      setInlineMathCount(scope.querySelectorAll(".math-inline-host").length);
    });
    return () => id;
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
      {/* Lazy renderers — mounted once per placeholder via the
       *  data-mounted guard inside `HostMounts`. Keyed on the
       *  placeholder *count* (not a per-render tick) so they
       *  never remount on plain text streams. `children` is a
       *  stable render-fn reference so it doesn't churn the
       *  `HostMounts` effect dep on every MessageBody render. */}
      {processed.needsMermaid && mermaidCount > 0 && (
        <HostMounts selector={`.mermaid-host`} dep={mermaidCount} render={renderMermaid} />
      )}
      {processed.needsMathBlock && mathBlockCount > 0 && (
        <HostMounts
          selector={`.math-block-host`}
          dep={mathBlockCount}
          render={renderMathBlock}
        />
      )}
      {processed.needsInlineMath && inlineMathCount > 0 && (
        <HostMounts
          selector={`.math-inline-host`}
          dep={inlineMathCount}
          render={renderInlineMath}
        />
      )}
      {processed.needsLightbox && <LightboxClient />}
    </>
  );
}

// Stable render callbacks for the lazy hosts. Pulled out of the
// `MessageBody` JSX so the `children` reference doesn't change on
// every render — otherwise `HostMounts`'s effect would re-run on
// every content chunk just because of a fresh closure.
const renderMermaid = (el: HTMLElement) => (
  <MermaidBlock source={el.dataset.source ?? ""} />
);
const renderMathBlock = (el: HTMLElement) => (
  <MathBlock source={el.dataset.source ?? ""} />
);
const renderInlineMath = (el: HTMLElement) => (
  <InlineMath source={el.dataset.source ?? ""} />
);

/** Trailing-edge debounce. Returns `value` unchanged for `delayMs`
 *  after the last change, then commits the latest. Used by
 *  `MessageBody` to coalesce rapid streaming chunks into a single
 *  markdown re-parse. */
function useDebouncedValue<T>(value: T, delayMs: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const handle = setTimeout(() => setDebounced(value), delayMs);
    return () => clearTimeout(handle);
  }, [value, delayMs]);
  return debounced;
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
  render,
  /** A value that changes only when a NEW placeholder appears or an
   *  existing one disappears. Re-runs the mount pass; existing
   *  placeholders are skipped via the `data-mounted` guard. */
  dep,
}: {
  selector: string;
  render: (el: HTMLElement) => React.ReactNode;
  dep?: number | string;
}) {
  // `roots` lives in a ref so freshly-mounted React roots survive the
  // re-render that follows a `dep` change — without this the cleanup
  // would unmount the new roots before they finish their first render.
  const rootsRef = useRef<Root[]>([]);
  useEffect(() => {
    const elements = Array.from(
      document.querySelectorAll<HTMLElement>(selector)
    );
    const fresh: Root[] = [];
    for (const el of elements) {
      if (el.dataset.mounted === "1") continue;
      el.dataset.mounted = "1";
      const root = createRoot(el);
      fresh.push(root);
      root.render(
        <Suspense
          fallback={
            <div className="border border-border-subtle rounded p-3 text-[11px] text-white-15">
              Loading renderer…
            </div>
          }
        >
          {render(el)}
        </Suspense>
      );
    }
    rootsRef.current = [...rootsRef.current, ...fresh];
    // No cleanup that re-creates `data-mounted`: placeholders stay
    // mounted for the lifetime of the host element. When the message
    // is removed from the tree, the entire DOM subtree (including
    // the placeholders) is garbage-collected, and React 18 detaches
    // the roots on its own when the parent unmounts.
  }, [selector, dep, render]);
  return null;
}
