// Syntax highlighting for reply code blocks — highlight.js, core + a
// curated language set (the full bundle registers ~190 languages we'd
// never use). Vite tree-shakes the per-language ESM imports.
//
// Styling lives in `index.css` (monochrome theme, no hues) — the design
// system forbids colored tokens, so weight/opacity/italic carry the
// hierarchy instead.

import hljs from "highlight.js/lib/core";
import javascript from "highlight.js/lib/languages/javascript";
import typescript from "highlight.js/lib/languages/typescript";
import rust from "highlight.js/lib/languages/rust";
import python from "highlight.js/lib/languages/python";
import bash from "highlight.js/lib/languages/bash";
import powershell from "highlight.js/lib/languages/powershell";
import json from "highlight.js/lib/languages/json";
import sql from "highlight.js/lib/languages/sql";
import xml from "highlight.js/lib/languages/xml";
import css from "highlight.js/lib/languages/css";
import markdown from "highlight.js/lib/languages/markdown";
import yaml from "highlight.js/lib/languages/yaml";
import ini from "highlight.js/lib/languages/ini";
import go from "highlight.js/lib/languages/go";
import java from "highlight.js/lib/languages/java";
import cpp from "highlight.js/lib/languages/cpp";
import csharp from "highlight.js/lib/languages/csharp";
import dockerfile from "highlight.js/lib/languages/dockerfile";
import diff from "highlight.js/lib/languages/diff";
import plaintext from "highlight.js/lib/languages/plaintext";

// registerLanguage also registers each language's aliases (e.g. "ts",
// "py", "sh") from its definition.
hljs.registerLanguage("javascript", javascript);
hljs.registerLanguage("typescript", typescript);
hljs.registerLanguage("rust", rust);
hljs.registerLanguage("python", python);
hljs.registerLanguage("bash", bash);
hljs.registerLanguage("powershell", powershell);
hljs.registerLanguage("json", json);
hljs.registerLanguage("sql", sql);
hljs.registerLanguage("xml", xml);
hljs.registerLanguage("css", css);
hljs.registerLanguage("markdown", markdown);
hljs.registerLanguage("yaml", yaml);
// No TOML grammar ships in this hljs version; the ini grammar covers
// `[section]` + `key = value`, which is the shape TOML output takes.
hljs.registerLanguage("toml", ini);
hljs.registerLanguage("go", go);
hljs.registerLanguage("java", java);
hljs.registerLanguage("cpp", cpp);
hljs.registerLanguage("csharp", csharp);
hljs.registerLanguage("dockerfile", dockerfile);
hljs.registerLanguage("diff", diff);
hljs.registerLanguage("plaintext", plaintext);

/**
 * Highlight one `code` element in place. Uses its `language-*` class when
 * present, auto-detects when absent, and skips unknown declared tags
 * (e.g. `tsx`, `batch`) instead of letting hljs throw and break the whole
 * decoration pass. Idempotent — guarded by hljs's own `data-highlighted`
 * marker, so re-running after a streamed chunk is a cheap no-op on
 * already-decorated nodes.
 */
export function highlightCode(code: HTMLElement): void {
  if (code.dataset.highlighted === "yes") return;
  const lang = code.className.match(/language-([\w#+-]+)/)?.[1];
  if (lang && !hljs.getLanguage(lang)) return;
  hljs.highlightElement(code);
}
