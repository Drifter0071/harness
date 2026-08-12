# Sessions, @-Mentions, Project Opening, and Rich Markdown

**Status:** Draft for review
**Date:** 2026-08-11
**Scope:** Phase 1.5 — UI + persistence layer for Harness

## 1. Summary

This spec adds four connected capabilities to the existing Harness app:

1. **Sessions** — one chat per project, persisted to `<project>/.harness/session.json`
2. **@-Mentions** — `MentionInput` component that turns `@` keystrokes into file chips in the chat input
3. **Project opening** — replace the dead `open_folder_dialog` IPC stub in `src-tauri/src/commands.rs` (returns `Ok(None)`) and the broken `ipc.openFolderDialog()` call site in `src/components/ActiveJobBar.tsx` (the "Open folder" button is currently non-functional). New `pick_project_folder` IPC uses the Rust dialog API; old command is removed from `lib.rs` and the `ipc` wrapper, the call site is updated.
4. **Rich markdown** — full content (math via KaTeX, Mermaid diagrams, image lightbox) on top of the existing `marked` + `highlight.js` pipeline, with the heavy renderers lazy-loaded

**Pre-existing bug fixed by this spec:** the "Open folder" button in the top bar is broken today. It calls the dead IPC stub and silently no-ops. This spec replaces that path with a real picker.

The goal is to make the chat actually usable as a daily tool — a user opens a project, sees prior context, drops file references inline, and reads rich agent output — without compromising the "ultra-lightweight" brand promise (≤20 MB binary, fast launch, low idle RAM).

## 2. Out of Scope

The following are explicitly deferred to future specs:

- File watcher / live tree refresh (refresh happens on project open and on `Ctrl+Shift+R`)
- Multi-session per project (we deliberately chose one chat per project root)
- Real agent behavior that consumes `mentions[]` (the agent stays a stub; this spec is about UI + persistence)
- Streaming responses (no agent to stream from)
- Encrypted secret storage for API keys (separate spec; the prior audit flagged plaintext localStorage, but that fix is not this spec's job)
- Telemetry / analytics
- Mobile targets — `tauri.conf.json` has no mobile config; this spec does not add or test them
- **Two windows on the same project** — Tauri is single-process per window, so opening the same project in two windows concurrently writes to the same `session.json` and `recent.json` with last-writer-wins. Acceptable for v1; a future spec will add per-project file locking (`fs2`/`flock`) if needed.

## 3. Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  React frontend                                             │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐  │
│  │ Sidebar      │  │ MentionInput │  │ MessageRenderer  │  │
│  │  + Recent    │  │   (chat      │  │  + lazy KaTeX    │  │
│  │  + FileTree  │  │    input)    │  │  + lazy Mermaid  │  │
│  └──────┬───────┘  └──────┬───────┘  │  + Lightbox      │  │
│         │                 │          └────────┬─────────┘  │
│         ▼                 ▼                   │            │
│  ┌──────────────────────────────────────┐     │            │
│  │ Zustand store (extended)             │     │            │
│  │  + session, recentProjects,          │     │            │
│  │    mentionState                      │     │            │
│  └──────────────────┬───────────────────┘     │            │
│                     │ debounced                │            │
└─────────────────────┼─────────────────────────┼────────────┘
                      ▼                         │
┌─────────────────────────────────────────────────────────────┐
│  Tauri IPC (new commands)                                   │
│  load_session / save_session                                │
│  list_recent_projects / forget_recent_project               │
│  pick_project_folder  (replaces dead open_folder_dialog)    │
│  list_files_recursive                                      │
└──────────────────────┬──────────────────────────────────────┘
                       ▼
┌─────────────────────────────────────────────────────────────┐
│  Rust: src-tauri/src/session.rs (new)                       │
│  - per-project: <project>/.harness/session.json             │
│  - global recents:  <app_data>/recent.json                  │
│  - normalization boundary for paths (forward-slash,         │
│    project-relative)                                        │
└─────────────────────────────────────────────────────────────┘
```

**Component boundaries**

| Unit | What it does | Depends on |
|---|---|---|
| `session.rs` (Rust) | Read / write / corrupt-recovery / atomic-rename for both files; path normalization for IPC | `tokio::fs`, `dirs` |
| `Sidebar` (TS) | Recent Projects list, file tree container, project open affordance | `useStore`, `ipc` |
| `MentionInput` (TS) | Textarea + popover, query parsing, key handling, chip rendering | `useStore`, `ipc.listFilesRecursive` |
| `MessageRenderer` (TS) | Markdown → HTML → lazy renderer hosts (mermaid/math/lightbox) | `marked`, `highlight.js`, dynamic imports |
| `useStore` (TS) | Holds `session`, `recentProjects`, `mentionState`, `fileTree` | `zustand` |

Each unit can be understood and modified without reading the others. The contract between them is the `ipc` namespace and the store shape.

## 4. Data Model

### 4.1 Per-project session

**Location:** `<projectPath>/.harness/session.json`

**Shape:**

```json
{
  "version": 1,
  "projectPath": "C:/code/myapp",
  "createdAt": "2026-08-11T13:04:00Z",
  "updatedAt": "2026-08-11T15:05:00Z",
  "messages": [
    {
      "id": "abc123",
      "role": "user",
      "content": "Explain @src/auth/login.ts",
      "timestamp": "2026-08-11T15:00:00Z",
      "mentions": [{ "path": "src/auth/login.ts" }]
    }
  ]
}
```

- `.harness/` is created on first save. **We do not touch the project's `.gitignore`** — the user is responsible for excluding `.harness/` if they share the repo. (Auto-editing `.gitignore` is a footgun: scanning existing patterns reliably is non-trivial and a sloppy regex can clobber a curated file.)
- On first open of a folder with no `.harness/`, we silently initialize it. No prompt, no friction.
- `version: 1` lets us migrate later. The loader checks version and starts fresh on unknown versions, copying the old file to `session.v1.archived-<unix_ts>.json` (version-keyed so a v2 migration does not collide).

### 4.2 Global recents

**Location:** `<app_data>/recent.json` (via `dirs::data_dir()` — `%APPDATA%/Harness/` on Windows, `~/.local/share/Harness/` on Linux, `~/Library/Application Support/Harness/` on macOS)

**Shape:**

```json
{
  "version": 1,
  "projects": [
    { "path": "C:/code/myapp", "name": "myapp", "lastOpened": "2026-08-11T15:00:00Z" }
  ]
}
```

- Capped at 20 entries. LRU-ish: opening a project bumps it to the top; oldest falls off.
- `name` is derived from `Path::file_name(&path).to_string_lossy()` at recents-write time on the Rust side. The frontend may also compute the basename from `projectPath` as a fallback (same `path.split(/[/\\]/).pop()` pattern as `FileExplorer.tsx:31`).
- `lastOpened` is an ISO 8601 string in UTC (see §4.4).
- "Forget" is a hover-revealed `×` button on each recent row, mirroring the `FileExplorer.tsx:79` `group-hover:opacity-100` pattern (not right-click — that fights the "no nested cards" design and is not keyboard-accessible). Includes `role="button"` and `aria-label="Forget project"`.
- **Path comparison is normalized:** on Windows, the Rust side compares paths case-insensitively (`Path::new(p).to_string_lossy().to_lowercase()`) so a Forget call with `C:/code/MyApp` deletes a stored `c:/code/myapp`.

### 4.3 Mention state (in-memory, ephemeral)

```ts
type MentionState = {
  open: boolean;            // popover visible
  query: string;            // substring between @ and caret, post-§8.2 rules
  results: string[];        // up to 10 ranked paths from fileTree
  selectedIndex: number;    // ↑/↓ selection, Enter/Tab commits
};
```

The store also caches `fileTree: string[]` — a flat list of project-relative forward-slash paths, seeded from `listFilesRecursive` on project open. Re-fetched on `Ctrl+Shift+R`.

### 4.4 Time format convention

**All persisted timestamps are ISO 8601 strings in UTC** (`"2026-08-11T15:05:00Z"`). The `Session.createdAt` and `Session.updatedAt` are ISO 8601. `RecentProject.lastOpened` is ISO 8601. The per-message `timestamp` is **also ISO 8601** (not epoch millis, even though the current `ChatMessage.timestamp: number` in `src/lib/store.ts:9` is epoch millis — the v1 spec migrates the in-memory shape to `string` to match the on-disk shape; the change is a one-line type update and any formatter downstream is updated in the same change set). Rationale: a single human-grep-friendly format, no `new Date(millis)` mental tax when reading the JSON.

### 4.5 Mention chip in message

A mention is recorded as:

```ts
type Mention = {
  path: string;          // project-relative, forward-slashes, no leading ./
  lineStart?: number;    // reserved for future "mention a range" feature
  lineEnd?: number;
}
```

The chip is rendered inline in the textarea (visual only); the on-disk message stores both `content` (the user's plain-text representation, e.g. `Explain @src/auth/login.ts`) and `mentions[]` (the structured form for the agent).

**In-memory `ChatMessage` is extended:** the existing `ChatMessage` interface in `src/lib/store.ts:6` (`{ id, role, content, timestamp, agentId?, agentName? }`) gains an optional `mentions?: Mention[]` field. The store's `addMessage` signature is unchanged (callers populate `mentions` before calling). `MessageBubble` and the canvas's `SubCanvas` ignore `mentions` for now (no agent to consume them); the data rides along.

## 5. IPC Surface

All new commands live in `src-tauri/src/commands.rs` and are registered in `src-tauri/src/lib.rs`. Session-specific logic is in a new `src-tauri/src/session.rs` module.

### 5.1 `pick_project_folder() -> Option<String>`

Replaces the dead `open_folder_dialog`. Uses `tauri-plugin-dialog`'s Rust API (`app.dialog().file().pick_folder()`) instead of the JS plugin. Returns the absolute path or null on cancel.

### 5.2 `load_session(project_path: String) -> Option<Session>`

Reads `<project>/.harness/session.json`. If absent → `null`. If corrupt JSON → rename the file to `session.v1.archived-<unix_ts>.json`, log to stderr, return `null` (start fresh).

On Windows, `tokio::fs::rename` may fail with `ERROR_SHARING_VIOLATION` if the file is open in another tool (VS Code, Notepad). Treated as a save/load error with a toast — no special handling.

### 5.3 `save_session(project_path: String, session: Session) -> Result<(), String>`

Atomic write:
1. `tokio::fs::create_dir_all(<project>/.harness)` (idempotent; private helper inside the command).
2. Write to `session.json.tmp`.
3. Rename to `session.json` (atomic on the same filesystem).
4. Errors (disk full, permission, sharing violation) bubble up; the frontend shows a non-blocking toast and keeps the in-memory session.

### 5.4 `list_recent_projects() -> Vec<RecentProject>`

Reads `<app_data>/recent.json`. If absent → `[]`. Never errors.

### 5.5 `forget_recent_project(path: String) -> ()`

Removes the entry by path. Idempotent. Path comparison is normalized (see §4.2).

### 5.6 `list_files_recursive(project_path: String) -> Vec<String>`

Returns a flat list of **project-relative, forward-slash, no-leading-`./`** paths for every regular file under the project, depth-limited to 8 (matches `search_files`'s existing depth). Used by the mention popover as a source of truth — the existing `search_files` command is not a substitute because it filters by `name.contains(pattern)` and would return only entries whose **name** matches, not the full file set. Symlinks are not followed (avoid loops). Skips `.git/`, `node_modules/`, `target/`, `dist/`, `.harness/` (the implementation uses the same set as the existing `walk` helper plus these additions; the spec is just "use gitignore-like heuristics" — concrete list finalized in implementation).

**Why a new command instead of reusing `search_files(path, "")`:** `search_files` lowercases the pattern and matches against `name.to_lowercase().contains(pattern)`, so an empty pattern would match every entry — but it returns paths, not names, and recurses depth-first while only checking the basename. We need a flat list of all files to filter and rank for mentions, so a dedicated command is the right tool.

### 5.7 Removal of the dead command

- Remove the `open_folder_dialog` entry from the `invoke_handler!` macro in `src-tauri/src/lib.rs`.
- Remove `openFolderDialog` from `src/lib/ipc.ts`.
- Update `src/components/ActiveJobBar.tsx` to call `ipc.pickProjectFolder()`.

### 5.8 Frontend `ipc` (in `src/lib/ipc.ts`)

TypeScript wrappers for each of the above, mirroring the existing style. The frontend uses normalized forward-slash paths everywhere; the Rust filesystem layer is the single normalization boundary.

## 6. State Machine — Project Lifecycle

```
   ┌─────────────┐  pick_project_folder / click recent   ┌──────────────────┐
   │ noProject   │ ───────────────────────────────────────▶│ loadingSession   │
   └─────────────┘                                        └────────┬─────────┘
                                                                  │
                                            load_session success  │
                                                                  ▼
                                                       ┌──────────────────┐
                                                       │ projectActive    │
                                                       └────────┬─────────┘
                                                                │
                                          pick another folder   │
                                                                ▼
                                                       ┌──────────────────┐
                                                       │ loadingSession   │  (old session
                                                       │ (switching)      │   was saved)
                                                       └──────────────────┘
```

- `noProject`: sidebar shows recents only; chat input disabled; "Choose a folder to start" hint.
- `loadingSession`: sidebar shows recents + a thin skeleton for the file tree; chat input disabled.
- `projectActive`: full UI active. Saving the session is debounced — every `appendMessage` resets a 500 ms timer; flush on app `beforeunload` (best-effort — Tauri's webview does not always fire `beforeunload` reliably on app close; the project-switch flush in the next bullet is the real safety net, and the in-memory session is the source of truth until the next save).
- **Project switch flush:** when transitioning `projectActive → loadingSession` (picking another folder or clicking a recent), the pending debounced save for the outgoing project is **flushed synchronously** before the new `load_session` call begins. The flush awaits `save_session` for the outgoing project; if the save fails, the in-memory session is preserved, a toast warns, and the switch still proceeds (in-memory is the source of truth; the smoke test in §11 verifies no message loss). The smoke test covers the 100 ms-after-edit case so we don't have a silent data-loss window.
- Failure paths: any load error → `projectActive` with empty messages + a single non-blocking toast. Any save error → toast + keep going (in-memory is source of truth until next successful save).

## 7. Sidebar Layout

**There is currently no sidebar container.** The existing `FileExplorer` (`src/components/FileExplorer.tsx:35`) is a `fixed left-3 top-[52px] w-[200px] max-h-[280px]` floating popover; the `sidebarOpen` boolean in the store is toggled by `ActiveJobBar.tsx:23` but has no consumer. This spec introduces a real sidebar container in `App.tsx`, absorbs the popover into it, and adds the recents section above the file tree.

The sidebar is a 220px column on the left edge of the chat area (next to the existing 34px sidebar toggle pill in `ActiveJobBar.tsx`). Recents on top, file tree on the bottom, separated by a 1px divider.

```
┌─── Sidebar (220px) ────┐
│  RECENT                │   <- 10px label, white-15, uppercase tracking
│  ● myapp        2m ago │   <- current project, green dot indicator
│    C:/code/myapp       │   <- path, mono 10px, white-15
│  ─────────────────     │
│    webapp       3h ago │
│    C:/code/webapp      │
│  ─────────────────     │
│    notes        2d ago │
│  ─────────────────     │
│  + Open folder…        │
│                        │
│  FILES (myapp)         │
│  📁 src                │
│  📁 docs               │
│  📄 README.md          │
│  📄 package.json       │
│  ↳ ..                  │
└────────────────────────┘
```

**Behaviors**
- Click recent → `setWorkspacePath(path)` → triggers `load_session`.
- "Open folder…" → `pick_project_folder` → on success, behaves like a recent click.
- Hover-revealed `×` on each recent row → `forget_recent_project(path)`.
- File tree section: existing `FileExplorer` is **rewritten from a fixed popover to a sidebar section** — remove the `absolute left-3 top-[52px] w-[200px] max-h-[280px]` wrapper, drop the parent-up `..` button (recents replaces the need to navigate up), and inherit the sidebar's height instead of a hard cap. The tree gets a `FILES (myapp)` header above it (the header shows the project basename; hides when no project is open).
- Width 220px (slightly wider than the current 200px to fit paths comfortably). No nested cards, no shadows — flat hierarchy.

**Empty state (no project)**: the recents section expands; file tree section is hidden; chat shows "Choose a folder to start" in place of the existing `EmptyState`. The existing `EmptyState` (`Chat.tsx:155`, the "harness" branding + tagline) is preserved and shown when `workspacePath != null && messages.length === 0` (i.e. a project is open but the chat is empty). The new "Choose a folder to start" state is shown only when `workspacePath == null`.

## 8. @-Mention Input

### 8.1 Trigger

The mention popover opens when the caret in the textarea is preceded by `@` and the character before that is whitespace or start-of-line. Closes on `Esc`, on commit, on whitespace typed after the query, or on focus loss.

### 8.2 Query parsing

The query is the substring from `@` to the caret, stopping at the first whitespace, newline, or another `@`.

### 8.3 Ranking (top 10)

1. **Filename matches** the query (case-insensitive substring) — e.g. `auth` → `auth.ts` ranks above `auth_helpers/login.ts`
2. **Path contains** the query
3. **Fuzzy match** against filename (so `alh` → `auth_login_helpers.ts`)

Source: client-side filter over the cached file tree (`useStore.fileTree`, seeded from `ipc.listFilesRecursive` on project open — see §5.6). The list is normalized to project-relative forward-slash paths at the Rust boundary; the frontend never has to normalize. Cached until the user re-opens the project or hits `Ctrl+Shift+R`.

### 8.4 Key handling

| Key | Action |
|---|---|
| `↑` / `↓` | Move selection in popover |
| `Enter` / `Tab` | Insert mention chip for selected file |
| `Esc` | Close popover, leave `@query` in textarea (don't eat the user's typing) |
| `Backspace` at start of a chip | Delete the whole chip |
| Any other key | Passes through to the textarea |

### 8.5 Chip representation

- **Visual:** a 1px-bordered pill inside the textarea line, monospace, with a file-type icon. Achieved with a transparent textarea overlaid by a styled `<div>` that mirrors the textarea content and renders chip spans. Two elements are kept in sync via the same `value` prop. The textarea is the source of truth; the div is purely visual.
- **Plain text in the message:** the chip renders as `@<path>` so plain text round-trips.
- **Immutability:** after insertion, a chip is a single immutable text token. The user cannot edit characters inside it. `Backspace` at the chip's start position deletes the whole chip; inserting characters in the middle is not supported in v1.
- **Re-trigger:** after the popover closes (Esc, focus loss, commit), typing another `@` reopens it normally at the new caret.
- **Structured in the message payload:** `mentions[]` contains the parsed path.
- **Validation:** on send and on `load_session`, each `mentions[].path` is checked against the current `fileTree`. Missing paths render the warning icon from §8.7. No data is rewritten — the chip keeps the original path even if missing (preserves round-trip).

### 8.6 Popover positioning

**v1: fixed-position popover anchored to the textarea's bottom edge** (top of the popover sits ~6px below the textarea's bottom border, horizontally aligned to the textarea's left edge minus 8px). Clamped to the viewport edges so it can't fall off-screen. Hidden when the project is closed.

Caret-anchored positioning (counting newlines × line height + offset) is **deferred** — it is fiddly to get right, and the v1 "below the textarea" position is robust and good enough for a single-line-of-results popover. See §12.

### 8.7 Edge cases

- Mentioned file no longer exists at send time: chip shows a small warning icon (a triangle with `!`). Send still goes through; the agent decides what to do.
- Path outside the project root: not allowed. The popover only lists project-relative paths, so this is structurally prevented.
- Long file names: truncate with ellipsis in the popover row, full path on hover via `title`.

## 9. Rich Markdown + Lazy Loading

### 9.1 Pipeline

```
content string
   ↓
preprocess:  detect code-fence language `mermaid` and `math`
             (the preprocessor does not try to find inline `$...$`
             because marked will mangle `$` outside code fences if
             we touch the text before parsing)
             leave everything else alone
   ↓
marked.parse → HTML (synchronous, async: false)
   ↓
post-process:
  - <pre><code class="language-mermaid"> → <div class="mermaid-host" data-source="...">
  - <pre><code class="language-math"> → <div class="math-host" data-source="..."> (display mode)
  - inline $...$ math: scan HTML output for $…$ inside <p>/<li> tags,
    NOT inside <code>, <pre>, <a>. Replace with <span class="math-host" data-source="...">
    (catches phrases like "$5 and $10" → no match because the parser is
    dollar-balanced and rejects mismatched counts; "let $x = 5$" → match)
  - <img src> → <button class="lightbox-trigger" data-src="...">
  - <a href> and <img src>: filter to https: / data: / mailto: / tel: schemes only (drop javascript:, vbscript:, file:)
  - strip on*="..." event handler attributes (defense in depth; marked
    does not generate these by default, but a custom renderer or user
    raw HTML in markdown could)
   ↓
mount: MutationObserver finds unmounted hosts, dynamic-imports the relevant renderer, hands off the source
```

### 9.2 Lazy loading

The current `dist/assets/index-*.js` is ~211 KB raw / ~63 KB gzipped and includes React, ReactDOM, Zustand, Lucide, the app code, marked, and highlight.js together. The breakdown below is **gzipped** (what the user actually downloads) and matches what `npm run build` reports for the lazy chunks:

| Renderer | Approx gzipped | Loads on |
|---|---|---|
| `marked` + `highlight.js` (already shipped) | ~27 KB | first message render |
| Mermaid | ~200 KB | first ` ```mermaid ` block |
| KaTeX | ~90 KB | first math expression |
| Lightbox | ~2 KB | first image click |

The first-use download for a message with both math and a Mermaid diagram is therefore ~290 KB gz on top of the baseline. The `tauri:build` exe embeds the lazy chunks in the binary, so this affects the install size (~290 KB heavier exe) but not network or load time, since they're only fetched into the webview on first use.

Each heavy renderer is `React.lazy(() => import('./renderers/XxxHost'))` with a single module-level `import()` promise so we only fetch once even with multiple diagrams in one message.

A small `<Suspense>` boundary shows a 1px-bordered skeleton (matches `surface-2` / `border-subtle`) during the ~200 ms first-load.

### 9.3 Renderers

- **MermaidHost**: render to SVG in-place. No zoom UI (the SVG is already crisp at the chat's max width).
- **MathHost**: KaTeX `renderToString` into the host. Supports inline (`$x$`) and display (`$$x$$`) modes.
- **Lightbox**: full-viewport overlay using the `DESIGN.md` modal pattern (`bg-black/60` + `backdrop-blur-sm`, not the `Settings.tsx:92` `bg-black/70`-without-blur variant — `DESIGN.md` is the authoritative design system per `AGENTS.md`), image centered, max `90vw × 85vh`. `Esc` and backdrop-click dismiss.

### 9.4 Safety

`marked` output is still `dangerouslySetInnerHTML` (the existing renderer already does this). The post-processor adds:

- **URL scheme filter** for `<a href>` and `<img src>` — `https:`, `data:`, `mailto:`, `tel:` are preserved; `javascript:`, `vbscript:`, `file:` are dropped silently. The §11 test suite exercises this allowlist.
- **Event handler stripper** — regex strips any `on\w+="..."` attribute, applied to the **post-`marked.parse` HTML** (the only point in the pipeline where the HTML is a known, structured string). Defense in depth, even though `marked` doesn't generate these by default.

If the user wants stricter sanitization (e.g. DOMPurify), that's a 30-min add but a different shape. Flagged in risks.

## 10. Dependencies

| Package | Type | Why |
|---|---|---|
| `mermaid` | runtime, lazy | Diagrams in chat |
| `katex` | runtime, lazy | Math rendering |
| `vitest` | devDep | TS unit tests |
| `happy-dom` | devDep | DOM environment for chip round-trip and event-strip tests |

**No new runtime React deps.** The existing textarea autosize (roll-our-own via `min/maxHeight` inline style in `Chat.tsx:91`) stays as-is. `react-textarea-autosize` is **not** used — it would conflict with the chip-overlay approach in §8.5 (the library owns the height measurement and won't know about the mirror `<div>` that renders chip spans).

**No new Rust deps.** `dirs` (already in `src-tauri/Cargo.toml:26`) provides `data_dir()` for `<app_data>/recent.json`. `tokio::fs` and `uuid` are already in.

## 11. Testing

The repo has no JS test framework today. This spec adds the minimum infrastructure to verify the high-risk bits.

**Test infrastructure added**
- `devDependencies`: `vitest` (the existing Vite config reuses its resolver — zero extra config), `happy-dom` (smaller than jsdom, fine for the small DOM needs below).
- `vitest.config.ts` extending `vite.config.ts` with the `test` block.
- `package.json` adds a `test` script: `vitest run`.

**Rust unit tests (in `session.rs`, `#[cfg(test)] mod tests`)**
- `save_session` is atomic: write to tmp, rename, no half-written file on simulated failure (mocked `tokio::fs` rename returning an error → no `session.json` written).
- Corrupt JSON load: file is renamed with the version-keyed suffix and the call returns `None`.
- Recents cap at 20: insert 25, only 20 remain, oldest are dropped.
- Recents path comparison is case-insensitive on Windows.

**TS unit tests (Vitest, split by environment)**

Pure-logic tests (no DOM, run on `node` environment):
- Mention query parser: `@foo bar` → query = `foo`; `@@foo` → no trigger; `@src/auth/login.ts please` → query = `src/auth/login.ts`.
- Mention ranking: `auth` against `["auth.ts", "auth_helpers/login.ts", "src/auth_helpers.ts"]` returns `auth.ts` first.
- URL filter: `<a href="javascript:alert(1)">` → href removed; `<img src="data:image/png;base64,...">` → preserved; `<a href="mailto:x@y.com">` → preserved; `<a href="file:///etc/passwd">` → removed.

DOM-touching tests (`happy-dom` environment):
- Chip ↔ text round-trip: render a chip with path `src/auth/login.ts` into a test textarea, read `.value`, parse → same path.
- Event handler strip: `<img onerror="x" src="https://x.com/y.png">` → onerror removed after post-process.

**happy-dom scope:** the current DOM tests need only `Element.innerHTML`, `Element.getAttribute`, and basic event dispatch. happy-dom covers that. If a future test needs `getBoundingClientRect` (e.g. popover positioning), happy-dom is insufficient and the test must move to the smoke-test list or to jsdom. Flagged here so a future maintainer doesn't get caught.

**Smoke test script (manual + a small `scripts/smoke.sh` for the headless flow)**
1. Open folder A.
2. Send message "A1".
3. Open folder B.
4. Send message "B1".
5. Reopen folder A → verify A1 is present, B1 is not.
6. Reopen folder B → verify B1 is present, A1 is not.
7. Verify recents list has both, current is on top.
8. Quit and relaunch the app → repeat steps 5–7 (verifies disk round-trip).

The smoke script can drive the binary via the existing `npm run tauri:dev` headless mode, or be done manually.

## 12. Risks

- **`dangerouslySetInnerHTML` is preserved.** The current renderer already uses it. The design adds URL + event-handler filtering, but if you'd rather have a sanitized path (DOMPurify or `marked-sanitize`), say so before we ship — it's a different shape.
- **Mermaid at ~200 KB gz lazy-loaded once is fine.** Multiple diagrams in one message only trigger one fetch (cached promise). The first one feels ~200 ms slower than the rest.
- **Mention popover positioning is fiddly.** Caret-to-pixel mapping via `selectionStart` + line-height math. **v1 ships a fixed-position popover anchored below the textarea** (not caret-anchored). Less precise but robust; upgrade to caret-anchored only if a user explicitly requests it.
- **`.harness/` inside the project folder** is convenient but couples the app to the project. If a user wants sessions to be machine-local even when the project is shared, we'd need a different location. Out of scope for this spec; flagged for a future "machine-local override" toggle.
- **No file watcher.** The file tree is stale until the user re-opens the project or hits `Ctrl+Shift+R`. A small `notify` watcher would fix this; flagged for a follow-up.
- **Two windows on the same project** race on `session.json` and `recent.json` with last-writer-wins. Out of scope for v1; mitigation is the per-project locking follow-up mentioned in §2.
- **On Windows, `session.json` may be locked by an external editor** (VS Code, Notepad) and the rename may fail with `ERROR_SHARING_VIOLATION`. Treated as a save error with a toast — no special handling.
- **Inline math parser is regex-based** and may misclassify edge cases (e.g. a chat message containing unbalanced `$` characters from code samples). The dollar-balance check mitigates this but is not bulletproof; a future v2 could use a proper marked extension to do context-aware math tokenization.

## 13. Implementation Order (for the eventual plan)

1. **Rust session module + IPC** (foundation everything else depends on) — adds `session.rs`, registers `pick_project_folder`, `load_session`, `save_session`, `list_recent_projects`, `forget_recent_project`, `list_files_recursive`. Also: remove the dead `open_folder_dialog` from `lib.rs` (invoke handler), `ipc.ts` (the wrapper), and `ActiveJobBar.tsx` (the call site) in the same change set — otherwise the top bar is broken between this step and step 5.
2. **Test infrastructure** — `vitest` + `happy-dom`, the `test` script, the Rust `#[cfg(test)]` skeleton.
3. **Store extensions** (session + recents + fileTree + mentionState; `ChatMessage.mentions?`).
4. **Project lifecycle wiring** (load on set, debounced save with flush-on-switch, in-memory as source of truth).
5. **Sidebar rewrite** — add the real sidebar container in `App.tsx`, absorb the floating `FileExplorer` into it, add the recents section above the tree, write the hover-revealed `×` for Forget.
6. **Mention input** (parser, popover, chip rendering, key handling) — `list_files_recursive` powers the popover source.
7. **Markdown extensions** (mermaid/math/lightbox, lazy-loaded) + URL + event-handler safety pass.
8. **Smoke test** (open → message → switch → relaunch → verify).

The order is: data layer first, UI second, polish third. The first three are invisible to the user and need to be solid before the visible features are useful.
