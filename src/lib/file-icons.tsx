// ── Per-file-type icons for the @-mention popover ─────────────────────
//
// Backed by `react-file-icon` (pure inline SVG, no runtime assets, MIT).
// The library ships a `defaultStyles` map that covers media/office types,
// but not modern dev languages (tsx, rs, go, yaml, vue, …), so we extend
// it with a curated palette below.
//
// Visual rules (match the app chrome): the sheets are flattened (no
// gradient wash) and use a slightly softer radius. Colors are chosen to
// stay scannable on the pure-black UI without screaming.

import type { CSSProperties } from "react";
import { FileIcon, defaultStyles } from "react-file-icon";
import type { FileIconProps } from "react-file-icon";

/** Theme overrides applied to every icon, last so they win. */
const THEME_OVERRIDES: Partial<FileIconProps> = {
  gradientOpacity: 0,
  radius: 3,
};

/**
 * Dev-language styles missing from the library's default map.
 * Keys are lowercase extensions, or exact special filenames (no dot).
 * `labelColor` is the band behind the extension text; `labelUppercase`
 * renders e.g. "tsx" as "TSX" on the sheet.
 */
const DEV_STYLES: Record<string, Partial<FileIconProps>> = {
  // TypeScript / JavaScript family
  ts: { color: "#3178c6", type: "code", labelColor: "#2a69ab", labelUppercase: true },
  mts: { color: "#3178c6", type: "code", labelColor: "#2a69ab", labelUppercase: true },
  cts: { color: "#3178c6", type: "code", labelColor: "#2a69ab", labelUppercase: true },
  tsx: { color: "#3178c6", type: "code2", labelColor: "#2a69ab", labelUppercase: true },
  js: { color: "#c9b458", type: "code", labelColor: "#9a8232", labelUppercase: true },
  mjs: { color: "#c9b458", type: "code", labelColor: "#9a8232", labelUppercase: true },
  cjs: { color: "#c9b458", type: "code", labelColor: "#9a8232", labelUppercase: true },
  jsx: { color: "#61dafb", type: "code", labelColor: "#3fa8c4", labelUppercase: true },

  // Other languages
  rs: { color: "#dea584", type: "code", labelColor: "#a9714f", labelUppercase: true },
  go: { color: "#00add8", type: "code", labelColor: "#0a8ba8", labelUppercase: true },
  py: { color: "#4b8bbe", type: "code", labelColor: "#3a6f99", labelUppercase: true },
  rb: { color: "#cc342d", type: "code", labelColor: "#99271f", labelUppercase: true },
  php: { color: "#777bb4", type: "code", labelColor: "#5c5f8c", labelUppercase: true },
  java: { color: "#e76f00", type: "code", labelColor: "#b2570a", labelUppercase: true },
  c: { color: "#6a737d", type: "code", labelColor: "#4f565e", labelUppercase: true },
  h: { color: "#6a737d", type: "code", labelColor: "#4f565e", labelUppercase: true },
  cpp: { color: "#6a737d", type: "code", labelColor: "#4f565e", labelUppercase: true },
  hpp: { color: "#6a737d", type: "code", labelColor: "#4f565e", labelUppercase: true },
  cs: { color: "#6a737d", type: "code", labelColor: "#4f565e", labelUppercase: true },
  kt: { color: "#a97bff", type: "code", labelColor: "#7d56c9", labelUppercase: true },
  swift: { color: "#f05138", type: "code", labelColor: "#c03a24", labelUppercase: true },

  // Web
  css: { color: "#519aba", type: "code2", labelColor: "#3d7a94", labelUppercase: true },
  scss: { color: "#519aba", type: "code2", labelColor: "#3d7a94", labelUppercase: true },
  sass: { color: "#519aba", type: "code2", labelColor: "#3d7a94", labelUppercase: true },
  less: { color: "#519aba", type: "code2", labelColor: "#3d7a94", labelUppercase: true },
  html: { color: "#e44d26", type: "code", labelColor: "#b23a1c", labelUppercase: true },
  htm: { color: "#e44d26", type: "code", labelColor: "#b23a1c", labelUppercase: true },
  vue: { color: "#41b883", type: "code", labelColor: "#338f66", labelUppercase: true },
  svelte: { color: "#ff3e00", type: "code", labelColor: "#cc3204", labelUppercase: true },
  astro: { color: "#ff5d01", type: "code", labelColor: "#c74a04", labelUppercase: true },

  // Data / config
  json: { color: "#cbcb41", type: "code", labelColor: "#9c9c31", labelUppercase: true },
  jsonc: { color: "#cbcb41", type: "code", labelColor: "#9c9c31", labelUppercase: true },
  yml: { color: "#c25450", type: "settings", labelColor: "#93403c", labelUppercase: true },
  yaml: { color: "#c25450", type: "settings", labelColor: "#93403c", labelUppercase: true },
  toml: { color: "#a074c4", type: "settings", labelColor: "#7d5b98", labelUppercase: true },
  ini: { color: "#a074c4", type: "settings", labelColor: "#7d5b98", labelUppercase: true },
  env: { color: "#a074c4", type: "settings", labelColor: "#7d5b98", labelUppercase: true },
  conf: { color: "#a074c4", type: "settings", labelColor: "#7d5b98", labelUppercase: true },
  cfg: { color: "#a074c4", type: "settings", labelColor: "#7d5b98", labelUppercase: true },
  lock: { color: "#cbcb41", type: "settings", labelColor: "#9c9c31", labelUppercase: true },
  xml: { color: "#e37933", type: "code", labelColor: "#b05d26", labelUppercase: true },
  sql: { color: "#e38c00", type: "settings", labelColor: "#b06f04", labelUppercase: true },
  gql: { color: "#e10098", type: "code", labelColor: "#ae0576", labelUppercase: true },
  graphql: { color: "#e10098", type: "code", labelColor: "#ae0576", labelUppercase: true },

  // Docs / misc
  md: { color: "#6a737d", type: "document", labelColor: "#4f565e", labelUppercase: true },
  mdx: { color: "#6a737d", type: "document", labelColor: "#4f565e", labelUppercase: true },
  txt: { color: "#8a8f98", type: "document", labelColor: "#656a72", labelUppercase: true },

  // Shell / scripts
  sh: { color: "#4eaa25", type: "code", labelColor: "#3c8220", labelUppercase: true },
  bash: { color: "#4eaa25", type: "code", labelColor: "#3c8220", labelUppercase: true },
  zsh: { color: "#4eaa25", type: "code", labelColor: "#3c8220", labelUppercase: true },
  fish: { color: "#4eaa25", type: "code", labelColor: "#3c8220", labelUppercase: true },
  ps1: { color: "#2f7cd6", type: "code", labelColor: "#265f9f", labelUppercase: true },
  bat: { color: "#6a737d", type: "code", labelColor: "#4f565e", labelUppercase: true },
  cmd: { color: "#6a737d", type: "code", labelColor: "#4f565e", labelUppercase: true },
};

/** Exact special filenames (no extension) → style + icon label. */
const SPECIAL_FILES: Record<string, Partial<FileIconProps>> = {
  dockerfile: { color: "#2496ed", labelColor: "#1e78b8", labelUppercase: true },
  makefile: { color: "#6a737d", labelColor: "#4f565e", labelUppercase: true },
  license: { color: "#8a8f98", labelColor: "#656a72", labelUppercase: true },
  readme: { color: "#8a8f98", labelColor: "#656a72", labelUppercase: true },
  changelog: { color: "#8a8f98", labelColor: "#656a72", labelUppercase: true },
  gitignore: { color: "#a074c4", labelColor: "#7d5b98", labelUppercase: true },
  gitattributes: { color: "#a074c4", labelColor: "#7d5b98", labelUppercase: true },
  editorconfig: { color: "#a074c4", labelColor: "#7d5b98", labelUppercase: true },
  "package-lock": { color: "#cbcb41", labelColor: "#9c9c31", labelUppercase: true },
  "yarn.lock": { color: "#cbcb41", labelColor: "#9c9c31", labelUppercase: true },
  "pnpm-lock": { color: "#cbcb41", labelColor: "#9c9c31", labelUppercase: true },
  "bun.lock": { color: "#cbcb41", labelColor: "#9c9c31", labelUppercase: true },
};

/** Neutral sheet for extensions we don't have a style for. */
const FALLBACK_STYLE: Partial<FileIconProps> = { color: "#5c6370" };

function basenameOf(path: string): string {
  const parts = path.split("/");
  return parts[parts.length - 1] ?? path;
}

function extensionOf(base: string): string {
  // Dotfiles (.gitignore, .env) have no extension.
  if (base.startsWith(".")) return "";
  const dot = base.lastIndexOf(".");
  return dot > 0 ? base.slice(dot + 1) : "";
}

/**
 * Resolve the full icon props (sheet color, glyph, label) for a
 * project-relative path. Dev-language and special-filename styles win
 * over the library defaults, then the theme overrides apply last.
 */
export function getFileIconProps(path: string): FileIconProps {
  const base = basenameOf(path).toLowerCase();
  const ext = extensionOf(base);
  const key = ext || base;

  const dev = DEV_STYLES[key];
  const special = SPECIAL_FILES[key];
  const defaults = (defaultStyles as Record<string, Partial<FileIconProps> | undefined>)[key];

  const style: Partial<FileIconProps> = {
    ...FALLBACK_STYLE,
    ...defaults,
    ...(dev ?? special),
    ...THEME_OVERRIDES,
  };

  // Label shown on the sheet — the extension (or basename for special
  // files), truncated so long names never overflow the tiny glyph.
  const label = (ext || (special ? key : "")).slice(0, 4);
  return { ...style, extension: label };
}

/**
 * Renders a 40×48 file sheet scaled to `size` px wide. Wrap sizing is
 * handled here so callers just say `<FileTypeIcon path="src/a.ts" />`.
 */
export function FileTypeIcon({
  path,
  size = 16,
  className,
}: {
  path: string;
  size?: number;
  className?: string;
}) {
  const style: CSSProperties = {
    width: size,
    height: size * 1.2,
    display: "inline-flex",
  };
  return (
    <span className={className} style={style} aria-hidden>
      <FileIcon {...getFileIconProps(path)} />
    </span>
  );
}
