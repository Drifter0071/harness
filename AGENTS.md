# AGENTS.md

Ultra-lightweight Desktop & Web AI Agent Harness — Tauri v2 (Rust backend) + React/TypeScript (Vite frontend), with a workspace engine, multi-agent loop, dynamic canvas UI, and BYOK model setup.

## Setup commands

- Install deps: `npm install` (frontend); Rust toolchain via `rustup` is required for the desktop app
- Start dev (web only): `npm run dev` — Vite dev server on http://localhost:1420
- Start dev (desktop): `npm run tauri:dev` — Vite + `cargo run` (Rust IPC commands live)
- Windows dev helper: `.\run.ps1`
- Build: `npm run tauri:build` — exe at `src-tauri\target\release\harness.exe`; web build: `npm run build` (`tsc && vite build`)
- Typecheck: `npx tsc --noEmit`
- Rust quality: `cargo fmt` and `cargo clippy` inside `src-tauri`
- No JS test or lint suite is configured yet

## Project layout

- `src/` — React/TypeScript frontend: components, Zustand stores, global styles
- `src-tauri/` — Rust backend: Tauri v2 IPC commands, agent loop, DuckDuckGo web search
- `DESIGN.md` / `PRODUCT.md` — design and product briefs; authoritative for frontend and product decisions
- `.impeccable/` — live-mode tooling state (helper server, sessions, poll bridge) — dev-only, not app source
- `dist/` — Vite build output; `run.ps1` — Windows dev helper script

## Code style

- TypeScript strict mode; React 18 function components + hooks; Zustand for shared state
- Tailwind classes limited to the configured palette (`surface-0..4`, `white` / `white-50` / `30` / `15`, `border-subtle` / `DEFAULT` / `strong`, `status.*`). Unknown classes silently no-op in Tailwind v3 — do not add unconfigured colors
- Visual system: background strictly `#000`, pure white text on all non-placeholder UI, no gradients / glassmorphism / nested cards; Lucide icons only (no inline SVG)
- **Bip mascot** (`src/components/AgentMascot.tsx`): the one sanctioned non-Lucide illustration — a pill-shaped robot with mood states (idle / working / success / error) living in the top capsule bar plus tiny `TypingBuddy` cursors. Motion lives in `index.css` (`mascot-*` classes), respects `prefers-reduced-motion`. Do not convert it to an icon or remove it as a "rule violation"
- Cartoonish accents (`accent.*` in tailwind.config.js: coral/sun/mint/lilac/sky) are a small-dose palette on top of the black/white base — mascot, status glows, and tiny details only, never the majority
- Rust: async `tokio`, a serde struct per IPC command, `cargo fmt` before committing

## PR & commit conventions

- Branch from `main`; never push directly to it
- Commit message: conventional commits (`feat:` / `fix:` / `refactor:` / `docs:` / `chore:`)
- `npx tsc --noEmit` must pass and the Rust crate must compile clean before opening a PR

## Security

- BYOK only: model API keys live in the user's browser `localStorage` under `harness_model_entries` — never hardcode, embed, or log keys
- Never commit secrets or credentials of any kind
