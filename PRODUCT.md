# PRODUCT.md — Harness

> Authoritative for product decisions. For visual decisions, see `DESIGN.md`.
> For repo conventions, see `AGENTS.md`.

## What it is

**Harness** is an ultra-lightweight, local-first AI agent workspace. It runs as a native
desktop app (Tauri v2) and in the browser, gives the user a real filesystem-backed
workspace, and orchestrates multiple LLM agents in parallel against that workspace —
with the user watching on a dynamic canvas, not in a single chat thread.

The bet: coding agents have outgrown chat. The next leg of value comes from a
**workspace engine** that owns the project's working state and a **multi-agent loop**
that drives it — and a UI that surfaces what's actually happening, not just the final
text.

## The spine (do not lose this)

Everything else in the product should serve these two:

1. **Workspace engine** — first-class project state: a folder, its files, the live
   conversation, the canvas surfaces, and the active agent graph. The workspace is
   the unit of work. Sessions live inside a workspace. Files are not decoration —
   they are inputs and outputs the agents can read and write.

2. **Multi-agent loop** — agents are not a single LLM call. There is an
   `Orchestrator` that breaks goals into tasks and dispatches them to `Coder`,
   `Architect`, `Researcher`, and `HealthMonitor` sub-agents (see
   `src-tauri/src/agent.rs`). Agents run concurrently, share state via the
   workspace, and are observable from the canvas.

Every product question — features, copy, visuals, even bug priorities — gets
weighed against: *does this strengthen the workspace engine or the multi-agent
loop?* If it doesn't, it's noise.

## Stack

- **Runtime:** Tauri v2 (Rust backend) + React 18 / TypeScript (Vite frontend)
- **State:** Zustand store, `localStorage` for model config, OS keyring for secrets
- **Target:** Standalone Windows `.exe` under 20MB; same UI runs as a web app
- **Styling:** Tailwind on a strict dark palette (see `DESIGN.md`)
- **IPC:** ~20 typed commands across Workspace, CLI, Agents, Search, Settings

## Audience

Developers and engineers who want a local AI coding environment that:

- Runs as a **native desktop app**, not Electron, not a tab
- Lets them **bring their own keys** (BYOK) — keys never leave the machine
- Treats the **filesystem as the source of truth**, not a hidden vector store
- Surfaces **agent activity visually** — code diffs, file trees, web previews —
  on a canvas, not buried in a chat scroll

The user is technical. Optimize for clarity and power, not hand-holding.

## Core surfaces

| Surface | Role in the workspace |
|---|---|
| **Workspace** (folder + file explorer) | The project's working set; agents read/write here |
| **Chat** | Where the user gives goals and where agents reply back |
| **Dynamic Canvas** | Real-time view of agent work: diffs, file trees, previews, terminal output |
| **Active Job Bar** | Floating status pill — what's running, who owns it, how to stop it |
| **Settings** | BYOK model entries (provider, base URL, key, model name); multi-entry support |

The canvas and the chat are peers, not a hierarchy. A user might run a long
agent task and only watch the canvas; another might keep both open.

## What works today (pre-launch honesty)

**Real and working:**
- Tauri v2 shell, full React UI, all five surfaces wired up
- Workspace engine v1: native folder picker, read/write/list/search/create, CLI exec
- Multi-agent loop v1: typed agent graph (`Orchestrator`, `Coder`, `Architect`,
  `Researcher`, `HealthMonitor`), spawn / message / status / stop, health checks
- BYOK settings: multiple model entries, OS keyring-backed secrets
- Web search via DuckDuckGo
- Build pipeline produces a working `.exe` (`npm run tauri:build`)

**Stubbed / partial — call this out in copy and design:**
- The agent loop has the **shape** of orchestration but the dispatching logic
  between Orchestrator and sub-agents is still thin. Most "agent" interactions
  today are still single-LLM calls routed through the typed agent struct.
- The canvas has the surfaces but auto-tiling and live updates from the agent
  graph are still being wired.
- No persistence across launches of the workspace state (only model config
  persists via `localStorage`).
- No JS test/lint suite; Rust has clippy + fmt.

**Not built yet (and that's fine for pre-launch):**
- Real diff preview engine on the canvas
- Plan-mode / approval flow for destructive writes
- Multi-workspace tab model
- Telemetry / crash reporting (intentionally off — privacy stance)

## Design principles

See `DESIGN.md` for the visual contract. The product principles behind them:

- **Workspace is the hero.** The folder, the files, the agent graph — these are
  primary. The chat is one pane among many, not the whole product.
- **Show the work.** If an agent is doing something, the canvas should show it
  in progress, not just the final message.
- **No AI design clichés.** No purple gradients, no glassmorphism, no nested
  cards, no "✨ magic ✨" copy. The user is technical; respect that.
- **Local-first, always.** Keys in the OS keyring, workspace on disk, no
  server-side model calls. If a feature would break that, it doesn't ship.
- **One codebase, two surfaces.** The web build and the Tauri build share the
  same React UI. No separate desktop rewrite.

## Anti-references

Things we are explicitly not building towards:

- ChatGPT's sidebar-heavy, conversation-list-first layout
- Cursor's dense multi-panel IDE look
- Any product whose brand is a purple/blue gradient
- Electron-based desktop apps (too heavy; undermines the local-first story)
- "AI workspace" products that hide the filesystem behind a vector store

## Open questions (for the next sprint)

These are real decisions, not nice-to-haves:

1. **Workspace persistence model.** Per-folder `.harness/` state? A central
   `~/harness/workspaces/` registry? File-watcher based or manual save?
2. **Agent dispatch protocol.** How does the Orchestrator actually hand work to
   sub-agents — explicit tool calls, shared message bus, or a graph runtime?
3. **Canvas composition.** Static surfaces or free-form blocks the user arranges?
4. **Approval gates.** Where do we ask the user before an agent writes/deletes?

If you change one of these, update this file.

## Current state

Pre-launch prototype. All five surfaces are wired. Workspace engine and
multi-agent loop have the right shape but are not yet the full bet. Next
milestone: real Orchestrator → sub-agent dispatch and a canvas that
auto-tiles from the agent graph.
