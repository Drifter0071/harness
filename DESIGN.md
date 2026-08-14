# DESIGN.md — Harness UI Design System

> Authoritative for visual decisions. For product context, see `PRODUCT.md`.
> For repo conventions, see `AGENTS.md`.
>
> This spec is generated from the code, not the other way around. When the
> code and this doc disagree, **the code wins** — fix this doc.
>
> Last reconciled: 2026-08-14 against `src/index.css`, `tailwind.config.js`,
> and the component tree under `src/components/` — chat message hierarchy
> (assistant = brightest), custom title bar (4 app buttons + 3 window
> controls in one row), Bip capsule + thought popover, Agent Manager slider,
> subagent tiling (read-only viewers, equal-grid layout, locked chat column),
> session header buttons, file-type icons, ChatHeader telemetry.

## Philosophy

Ultra-minimal dark UI on a pure-black canvas. The product is a workspace —
the chrome should disappear so the work is what's visible. Every pixel must
serve information; the only flourishes are the **glass-quartz / glass-mist
material** and a small motion vocabulary that signals state, never decoration.

The previous version of this doc forbade glassmorphism. The code shipped
glass anyway, deliberately, as the project's signature material. That is now
the design.

## Color system

All values are pulled from `tailwind.config.js` (the source of truth) and
mirrored as CSS custom properties in `src/index.css` for cases where Tailwind
isn't enough (pseudo-elements, the `glass-quartz` material).

### Surfaces — `surface-0..4`

| Token | Hex | Role |
|---|---|---|
| `surface-0` | `#000000` | App canvas — the void. Body, between-pane gutters. |
| `surface-1` | `#0a0a0a` | Sidebar background, modal panels, canvas sub-tiles. |
| `surface-2` | `#111111` | Cards, inputs, active surfaces, "compact" surfaces. |
| `surface-3` | `#1a1a1a` | Hover state, **user message bubble background**, "elevated" cards. |
| `surface-4` | `#222222` | Active inputs, focused elements. Reserved; rarely used in the current tree. |

> The previous version of this doc listed `#222222` as a row in the table
> but never explained its role. Treat it as the "focused-input" surface.

### Text — `white` family

| Token | Value | Role |
|---|---|---|
| `white` (DEFAULT) | `#ffffff` | Primary text, headings, hero strings ("harness") |
| `white-50` | `rgba(255,255,255,0.50)` | Secondary text, label icons at rest |
| `white-30` | `rgba(255,255,255,0.30)` | Tertiary text, helper copy, agent body, default body copy on assistant messages |
| `white-15` | `rgba(255,255,255,0.15)` | Quaternary text, placeholders, "Files" section labels |
| `white-08` | `rgba(255,255,255,0.08)` | Scrollbar thumb, very subtle dividers (used directly in CSS) |

The previous version of this doc referenced zinc-200 / 400 / 500 / 700. The
code does not use zinc utilities. Use the `white` family.

### Borders

| Token | Value | Role |
|---|---|---|
| `border-subtle` | `rgba(255,255,255,0.06)` | Dividers, inactive card edges, file-tree row separators |
| `border` (DEFAULT) | `rgba(255,255,255,0.10)` | Default card border, modal border |
| `border-strong` | `rgba(255,255,255,0.18)` | Focused inputs, active state |

> The previous version of this doc said `0.08 / 0.12 / 0.20`. The Tailwind
> config and the CSS variables both ship `0.06 / 0.10 / 0.18`. **Code wins.**

### Status

| Token | Hex | Role |
|---|---|---|
| `status.active` | `#22c55e` | Active/working — green dot, agent working state, recent-project active row |
| `status.warning` | `#eab308` | Reserved — not yet used in components |
| `status.error` | `#ef4444` | Error — used on hover for destructive actions (e.g. forget-project X) |
| `status.info` | `#3b82f6` | Reserved — not yet used in components |
| `--accent-green` | `#22c55e` | Mirror of `status.active` for CSS contexts; used directly on glow shadows |
| `--accent-red` | `#ef4444` | Mirror of `status.error` for CSS contexts |

Two parallel systems. Consolidate to `status.*` in new code; the CSS
variables exist for the `glass-quartz` material's tint and for shadow values
that Tailwind can't express inline.

### The black-on-black trap

Almost every active row, hover row, and panel uses `bg-white/[0.04..0.06]`
as a fill. This is deliberate: on the `#000` canvas, white at 4–6% reads as
a faint surface lift without introducing a new color. New components should
use the same opacities, not invent grays.

## Typography

| Role | Stack | Used for |
|---|---|---|
| Display / Sans | `Inter → system-ui → -apple-system → sans-serif` | Headings, UI labels, body copy, hero strings |
| Mono | `SF Mono → JetBrains Mono → Fira Code → Cascadia Code → monospace` | Code blocks, terminal output, file paths, timestamps, the workspace path strip in the sidebar |

Two text roles, no Display, no Serif. The previous version listed "Display"
but the code only uses Sans and Mono.

### Sizing — the actual scale the code uses

There is no documented type scale. The code uses ad-hoc Tailwind sizes. The
frequencies below come from grepping the components. **New components should
stay in this range; if you need outside it, propose the new size and we add
it to the scale.**

| Size | Used for |
|---|---|
| `text-[28px]` | Hero wordmark ("harness") in empty states — `font-light tracking-tight` |
| `text-[16px]` | Modal titles ("Model Setup") |
| `text-[14px]` | Primary body copy in chat messages |
| `text-[13px]` | Settings labels, body copy in subagent viewer bodies, dialog copy |
| `text-[12px]` | Subagent window name, button labels, modal labels |
| `text-[11px]` | Section labels (uppercase), recent-project row, agent body in main chat |
| `text-[10px]` | Active-root pill, helper copy in tight rows |
| `text-[9px]`  | Section headings ("RECENT", "FILES") — uppercase, tracking `[0.14em]` |

> The `text-2xs` token (`0.625rem / 14px`) is defined in `tailwind.config.js`
> but unused in the current tree. Drop it or use it.

### Section labels

`RECENT`, `FILES`, `WORKSPACE`, `ACTIVE TASKS` — every section label in the
app uses the same recipe:

```tsx
<span className="text-[9px] text-white-15 uppercase tracking-[0.14em] font-medium">
  Recent
</span>
```

The `tracking-[0.14em]` is the load-bearing detail. Don't fudge it.

## Glass material system

The signature material. Two variants, defined in `src/index.css`. Both are
deliberate, both are documented anti-patterns in most design systems, and
both are part of this product's identity.

### `.glass-quartz` — "Vitreous Liquid Quartz"

A frosted, light-tinted slab with an animated conic-gradient edge ring and
a slow drifting sheen. Sits on `surface-0`. **Quiet at rest, blooms on
hover.**

| Property | Value |
|---|---|
| Body fill | `radial-gradient(120% 90% at 12% 0%, rgba(255,255,255,0.08) → 0.02 → transparent) + --q-tint` |
| `--q-tint` (default) | `rgba(255,255,255,0.035)` — barely-there |
| `--q-tint` (strong) | `rgba(255,255,255,0.08)` — promoted |
| `--q-tint` (button rest) | `rgba(255,255,255,0.012)` — even quieter for buttons |
| `--q-tint` (button hover) | `rgba(255,255,255,0.09)` |
| `--q-radius` (default) | `12px` |
| `--q-radius` (pill) | `999px` |
| `--q-radius` (compact) | `8px` |
| `--q-blur` | `16px` |
| `--q-speed` (edge ring) | `16s` |
| Edge ring | Conic gradient at `--quartz-angle`, 85% opacity, 16s loop |
| Sheen | Two soft radial highlights, `mix-blend-mode: screen`, 18s alternate |
| Box shadow | `inset 0 1px 0 rgba(255,255,255,0.06)` + drop shadow over the void |
| Reduced motion | Both `::before` and `::after` animations disabled; transitions disabled |

**Variants** (compose by adding classes):

- `glass-quartz` — base
- `glass-quartz--pill` — radius `999px`
- `glass-quartz--compact` — radius `8px`
- `glass-quartz--strong` — `--q-tint` promoted

> **CSS gotcha:** `.glass-quartz` declares `position: relative` so the
> `::before` / `::after` pseudo-elements can anchor. The custom CSS loads
> after Tailwind utilities, so a plain `fixed` class on the same element
> is overridden. **For any `glass-quartz` element that needs to be
> `position: fixed`, use `!fixed` (Tailwind's `!` important prefix).**
> In normal flow (flex row, grid cell, etc.) the `position: relative`
> is harmless — leave it alone.

**Button recipes:**

```tsx
// Quiet at-rest
<button className="glass-quartz glass-quartz--pill">…</button>

// Promoted primary action
<button className="glass-quartz glass-quartz--compact glass-quartz--strong">…</button>
```

Buttons animate to `translateY(-1px)` and lift the inner shadow on hover,
and to `scale(0.96)` on `:active`. `:focus-visible` draws a 2px white outline
at 60% opacity with 2px offset. `:disabled` is 35% opacity with no animations.

### `.glass-mist` — companion material

Same frosted family, but barely-there: 2% white fill, static hairline edge,
no animated ring. **Keeps `::before` and `::after` free for consumers** —
use it when you want the material without claiming the pseudo-elements.

| Property | Value |
|---|---|
| `--m-tint` (default) | `rgba(255,255,255,0.02)` |
| `--m-radius` (default) | `12px` |
| `--m-radius` (pill) | `999px` |
| `--m-blur` | `14px` |
| Border | `1px solid rgba(255,255,255,0.07)` |
| Hover border | `rgba(255,255,255,0.11)` |
| Focus-within border | `rgba(255,255,255,0.18)` |

**Used for:** the chat input bar, the sidebar (`--m-radius: 0`), the
ActiveJobBar pill, the activity popover, any "container" that should
feel like glass without being a button.

### When to use which

- **Primary action / button →** `glass-quartz` (one of the three variants)
- **Container / panel that holds content →** `glass-mist`
- **Card with a header and body, no glass →** plain `bg-surface-1` with `border-border-subtle` (this is what the SubagentWindow tiles do)
- **Never:** nested glass inside glass. One layer of the material per surface.

## Spacing

The Tailwind default scale is in use; there is no custom scale. The
recurring 4/8/12/16/24/32 ladder from the previous version of this doc
still applies. Practical notes from the code:

- `gap-1` (4px) — between related icons, between an icon and its label
- `gap-1.5` (6px) — between icon and label inside a row
- `gap-2.5` (10px) — between message bubbles, between activity items
- `px-3` (12px) — section padding, list-row left padding
- `px-4` (16px) — chat column gutters
- `py-1` (4px) — list-row vertical padding (tight)
- `py-1.5` (6px) — list-row vertical padding (default)
- `py-3` (12px) — section break inside a sidebar pane

The Bip capsule (the `ActiveJobBar` component) floats at `top-[52px]`
(36px custom title bar + 16px gap) with `left-1/2 -translate-x-1/2`.
It anchors the agent mascot and is the only floating control on the
top of the app — the four app-bar actions (Show sidebar / Open folder
/ Settings / Agent Manager) live in the title bar instead, so the bar
reads as a single row. The offset is paired with `TITLEBAR_HEIGHT` in
`TitleBar.tsx` — if the title bar height changes, update this to match.

## Component patterns

### Plan (todo list) — `PlanStrip.tsx` + `PlanPopover.tsx`

The session-scoped todo list (spec `2026-08-14--agent-todo-list.md`).
The agent's plan, visible to the user at the top of the chat as a
compact menu.

- **Layout.** `PlanStrip` lives in the ChatHeader row, rendered as a
  third centered row beneath the existing model/tokens/usage cluster
  + progress bar (row 1) and the conditional cumulative/session-
  window row (row 2). Hidden entirely when `items.length === 0`.
- **Strip (rest).** `glass-mist` pill, `--m-radius: 999px`,
  `text-[11px] text-white-50 font-medium gap-1.5`. Content: `Plan`
  wordmark (section-label recipe) + progress fraction `n/m` in
  `text-white` + `next: <text>` in `text-white-30` (or `· done` in
  `text-status-active` when the list is finished) + chevron
  (`ChevronDown` 12px). All animations respect
  `prefers-reduced-motion`.
- **Popover (hover/focus).** Portaled to `<body>` at `z-[85]` (just
  under Bip's thought popover at `z-[90]`). Anchored 12px below
  the strip, 320px wide, max-height 360px (internal scroll for
  >10 items). Uses the same shared-hover state trick as Bip's
  thought popover so the 8px gap doesn't close the popover
  mid-read. `Esc` and outside-click close.
- **Items.** 32px row, `px-3 hover:bg-white/[0.04]`. Status pill
  (24×24 left, clickable, cycles `pending → in_progress → done`),
  text (`text-[13px] text-white` for pending/in_progress,
  `text-white-30 line-through` for done; click → contenteditable
  for inline rename, `Enter` commits, `Esc` reverts, blur commits,
  empty text on commit = delete), hover affordances on the right
  (`GripVertical` for drag, `X` for delete).
- **`+ Add step` row** at the bottom: `Plus` icon + `text-white-30
  text-[12px]`. Click → inline input (same recipe as item text),
  `Enter` commits.
- **Footer.** `Clear plan` button (`text-white-15 text-[11px]`,
  hover `text-status-error`).
- **Drag-reorder.** Native HTML5 drag. `order` values are sparse
  step-1000 so future inserts between items don't require full
  re-numbering.
- **"All done" transition.** When `isFinished(list) === true`, the
  strip flips to `Plan · done` in `text-status-active` and the
  popover header gets a 2px `status.active` left rule. The
  transition only fires on the *transition* from non-finished to
  finished; loading a session that was already finished does not
  re-trigger it.
- **Drag region.** The strip sits in the ChatHeader row, outside
  the title bar's `data-tauri-drag-region` strip — no opt-out
  needed.

### Chat message (`Chat.tsx`)

- **User:** right-aligned, `bg-surface-3`, `rounded-2xl rounded-br-sm`,
  `px-4 py-2.5`. Body is `text-white-50` — the prompt is the input, not
  the substance.
- **Assistant:** left-aligned, no bubble. Body is `text-white` — the
  output is the substance and reads brightest, matching the subagent
  tile hierarchy. Above the body, a label row with a
  `bg-status-active` 1.5×1.5 dot, the agent name in `text-white-30` at
  `text-[11px] font-medium`, and a 1.5px gap.
- **Reply prose** (`MessageBody` → `.prose-harness`, typography in
  `src/index.css`): markdown renders with real hierarchy inside the
  black/white world. Body inherits the bubble's color and size (14px
  chat / 13px subagent threads), line-height 1.7, `overflow-wrap: break-word`.
  Headings are 600-weight steps (1.15 / 1.07 / 1em) with more air above
  than below. Lists restore disc/decimal markers (Preflight strips
  them), marker color `white-50`. Inline code is a `white-08` chip with
  a hairline border; fenced code blocks are `surface-2` panels at
  `0.875em` mono, `pre-wrap` (no horizontal scroll), with a hover-revealed
  toolbar (`CodeBlockDecorator` in `markdown.tsx`) showing the language
  label (`white-30`, 10px, uppercase, tracked) and a 24×24 `surface-3`
  copy button (`Copy`/`Check` Lucide, clipboard + textarea fallback);
  always visible on touch devices (`@media (hover: none)`). Fenced code
  gets real syntax highlighting via `highlight.js` (curated language set
  in `highlight.ts`) — code tokens are the one sanctioned use of hue: a
  muted, desaturated editor palette (soft blue keywords, green strings,
  orange numbers, cyan types, red built-ins on gray comments) so code
  reads like a real editor while UI chrome stays monochrome. Links
  underline in `white-30` and brighten to white on hover — no blue, the
  world is monochrome (code tokens excepted). Blockquotes get a 2px
  `border-strong` left rule
  and `white-50` body; tables get hairline rows, a `border-strong`
  header rule, and 0.93em type; `hr` is `border-default`. Images round
  to 8px, take a hairline, and open in the lightbox via the existing
  `button.lightbox-trigger` wrapper.
- **System:** centered pill, `bg-surface-2`, `text-white-30` at `text-[12px]`,
  `px-3 py-1`, `rounded-full`, `border-border-subtle`.
- **Animation:** every message is `animate-fade-in` (150ms `ease-out`).

### Title bar (`TitleBar.tsx`)

The window is undecorated (`decorations: false` in `tauri.conf.json`,
plus `shadow: false` so Windows 11 doesn't force a 1px white border or
rounded corners onto the black canvas, `acceptFirstMouse: true` so the
first drag on a not-yet-focused window moves it, and `backgroundColor:
"#000000"` so there's no white flash on launch). `TitleBar.tsx`
replaces the native chrome with a single 36px strip that hosts **the
four app actions on the left, the Bip mascot in the center, and the
three window controls on the right** so the top of the app reads as
one row.

- **Layout:** `bg-surface-0`, 36px tall, hairline `border-border-subtle`
  bottom edge, `select-none`. Sits above every overlay (`z-[60]`, above
  the Settings / Agent Manager `z-50`) so the window can always be
  dragged, minimized, maximized or closed — like native chrome.
- **Drag region:** the strip carries `data-tauri-drag-region="deep"`
  (Tauri ≥ 2.11). Clicks on any non-button, non-Bip pixel — wordmark,
  dividers, empty space — start a native window drag. Buttons and the
  Bip element carry `data-tauri-drag-region="false"` to opt out so
  their clicks / hovers fire. Double-click on the strip maximizes
  (native HTCAPTION). Edge resizing still works (tao's borderless
  resize handles).
- **Left cluster (app actions):** "HARNESS" wordmark (`text-[9px]
  uppercase tracking-[0.14em] font-medium text-white-15`, the
  panel-label recipe) → divider → **Show/Hide sidebar** (`PanelLeftOpen`
  / `PanelLeftClose`, 16px, toggles `sidebarOpen`) → **Open folder**
  (`FolderPlus`, 16px, calls `ipc.pickProjectFolder()` and sets
  `workspacePath`) → divider → **Settings** (`Settings`, 16px) →
  **Agent Manager** (`Bot`, 16px).
- **Center cluster (Bip):** a 44×35 wrapper holding a 40×40 Bip mascot
  rendered at `size={32}` (so the SVG fits the 36px strip with a 2px
  halo bleed). No `glass-mist` pill around him — he lives directly in
  the bar per AGENTS.md ("the top capsule bar"). `data-tauri-drag-region="false"`
  on the wrapper so the bar still drags from the rest of its surface.
  `onMouseEnter` opens the thought popover (portaled below).
- **Right cluster (window controls):** Minimize (`Minus`, 12px),
  Maximize/Restore (`Square` / `Copy`, 10px, swapped live via the resize
  listener), Close (`X`, 12px).
- **Bar button language:** every button on the bar is the same
  `BarButton` helper — 36px full-strip-height target,
  `text-white-50` at rest, hover lifts to white on `white-08` (active
  `white-14`). Close is the only "danger" button: hover goes to
  `status-error` on the same hover state, matching the rest of the
  app's destructive language.
- **Runtime guard:** `isTauri()` from `@tauri-apps/api/core` (with a
  sync `__TAURI_INTERNALS__` shortcut) gates every `getCurrentWindow`
  call. In plain-browser dev the buttons render but no-op without
  throwing, so `npm run dev` keeps working.
- **Maximized state:** tracked via `isMaximized()` + `onResized`.

### Bip mascot (lives in `TitleBar.tsx`)

Bip is the agent mascot, "the one sanctioned non-Lucide illustration"
(AGENTS.md). He lives in the **center of the custom title bar** — no
floating pill, no separate row. The four app actions flank him on the
left, the three window controls on the right.

- **Mood engine:** driven by `streamingRequestId`, `agentRequestId` and
  `jobActivities` from the store. The motion class follows the mood:
  `mascot-bob` (idle), `mascot-bob--fast` (working),
  `mascot-bounce` + `<ConfettiBurst />` for 3.2s on terminal `chat-done`
  / `agent-done`, `mascot-shake` for 5s on `chat-error` / `agent-error`.
  Aria label reads `Agent status: <label>`. A soft `mascot-halo` div
  behind Bip (tint per mood, see the `HALO` map) gives him an ambient
  aura.
- **Thought popover (Bip's thoughts):** portaled to `<body>` at
  `z-[90]` so it floats above every overlay including the title bar.
  Shared hover state between the Bip element and the popover so the
  8px gap doesn't close it mid-read. Content:
  - **Header:** mini Bip + "BIP'S THOUGHTS" label (10px uppercase
    `text-white-30 tracking-wider`) + an inner monologue string that
    varies by mood ("Streaming a reply — words incoming.", "That one
    landed clean. Confetti earned.", "Something went sideways. Letting
    the red glow show it.", etc.).
  - **Active Tasks** section (when `jobActivities.length > 0`): list
    of activities, each row a 4px green dot + label.
  - **Agents** section (when `activeAgents.length > 0`): per-agent
    row with role-tinted dot + name + role chip.
  - **Turn** section: working state with `fill-accent-mint animate-pulse`
    dot + "Agent turn running" / "Streaming response" / "Idle".
  - **Session** section (when `activeModel` or `tokenStats` exists):
    model label, last response tokens, cumulative session totals,
    session-window usage %. Uses the same `text-[12px] text-white-30
    flex-shrink-0` label / `text-white-50` value row recipe as the
    ChatHeader telemetry.
- **Why Bip moved into the title bar:** the original `ActiveJobBar`
  floated below the bar as a glass-mist pill. With the title bar now
  carrying the four app actions anyway, the floating pill was
  redundant chrome — Bip now occupies the bar's center cell directly,
  which honors AGENTS.md's "lives in the top capsule bar" instruction
  more literally and saves vertical real estate.
- **Why the capsule is the only floating control:** the title bar is
  the load-bearing row for the four app actions and the three window
  controls; the capsule's job is the mascot + status + telemetry
  hover-panel. Keeping Bip in his own floating element (instead of
  trying to fit him into the title bar) preserves the "mascot, status
  glows, and tiny details only" rule in AGENTS.md and gives him room
  to bob / bounce / shake without competing for chrome real estate.

### Agent controls (`Chat.tsx`, `AgentManager.tsx`)

Agent turns replace plain chat streaming whenever a project + session is
open (`workspacePath && activeSessionId`). The controls live in the composer
block, stacked above the input:

- **Composer options row** (only when a session is open, below the model
  selector row): a segmented action-mode control (`Auto` / `Ask risky` /
  `Ask all`; `bg-surface-2` shell, active segment `bg-surface-4 text-white`,
  inactive `text-white-30 hover:text-white`, `text-[11px]`) plus a right-side
  **Goal** toggle — the same `w-7 h-4` switch recipe as the Thinking toggle,
  with a `Target` icon (12px) that turns `text-status-active` when on. Both
  switches share the focus ring recipe —
  `focus-visible:outline-2 outline-white/60 outline-offset-2`.
- **Pending approval card** — transient, shown above the input while the
  Rust loop waits on a tool call: `bg-surface-2 border-border-subtle rounded`,
  `ShieldCheck` (15px, `text-status-warning`), tool name in
  `text-[12px] text-white font-medium`, one-line args summary
  (`text-[11px] text-white-30 truncate`), and Allow / Deny buttons
  (Allow: `bg-status-active text-black`; Deny: `bg-surface-4`, hover
  `text-status-error`). Card clears itself on either response.
- **Tool-call cards in the message stream** — every tool call the LLM makes
  renders as a compact card on its assistant message (keyed by the
  `agent-<requestId>` placeholder), so the chat timeline shows what the agent
  did — live and after reload. Card: `bg-surface-2 border-border-subtle
  rounded px-2.5 py-1.5`; a status icon (running → `Loader2` 11px spinning
  `text-white-50`; `ran` → `Check` `text-status-active`; `denied` → `Ban`
  `text-status-warning`; `error` → `AlertTriangle` `text-status-error`),
  `Wrench` (11px, `text-white-15`), tool name (`text-[12px] font-medium
  text-white`), args in mono (`text-[11px] text-white-30 font-mono truncate`),
  and the result summary below once finished (`text-[11px] text-white-30
  truncate`). Live entries come from `agent-tool-start` / `agent-tool-result`;
  persisted `activities` (now carrying `requestId` + `arguments`) rebuild the
  cards on session load. This replaces the old transient composer strip.
- **Pending message queue** — while a generation is in flight the composer
  is still live: sending (Enter or the send button) **queues the draft**
  instead of dropping it, and the head of the queue auto-sends the moment
  the current stream/agent turn ends (done, error, or cancelled). Queued
  chips render above the input (`bg-surface-2 border-border-subtle rounded`,
  truncated `text-[12px] text-white-50` preview) with inline **edit**
  (pencil → text input, Enter saves / Esc cancels) and **discard** (×).
  The queue is ephemeral — never persisted, cleared on session/project
  switch. While streaming with a draft, the trailing buttons are:
  **queue** (`ListPlus`), **stop & send** (`Zap` — cancels the current
  generation and jumps the draft to the front of the queue), and **stop**
  (`Square` — keeps the partial response; an empty placeholder is removed).
- **Cancellation keeps partial output** — `chat-error cancelled` /
  `agent-error cancelled` no longer delete the streaming message: whatever
  content already streamed stays in the chat (the optimistic bubble is only
  removed when nothing was generated).
- **Agent Manager** (`AgentManager.tsx`) — modal opened from the Sessions
  panel's Bot button. Max-agents-per-session is a **custom `slider-quartz`
  range** (1–12): a 4px two-tone track filled by an inline `--slider-fill`
  percentage, a 16px white disc thumb, a mono value readout, and a focus
  ring mirroring `button.glass-quartz:focus-visible` (Firefox parity via
  `::-moz-*`, reduced-motion respected). Save is a
  `glass-quartz glass-quartz--pill px-4 py-1.5 text-[12px] text-white`
  button → `saveAgentRoster` (backend normalizes; the builtin `harness`
  agent is editable but not deletable).

### Chat header telemetry (`ChatHeader.tsx`)

A quiet readout strip above the message list; it also anchors the sidebar
column via the live-measured `chatHeaderHeight` (`useLayoutEffect` +
`ResizeObserver`). Each row is a `text-[12px]` label/value pair — labels
`text-white-30`, values `text-white-50`, `justify-between`:

- **Model** — the active model's label, static text (`truncate`); model
  switching lives in the composer's `ModelSelect`.
- **Last response** — `N in · M out` tokens for the most recent turn.
- **Context used** — `used / window` plus a 1px context bar (`h-px
  bg-white-15` track) filled by fraction: `bg-status-active` below 60%,
  `bg-status-warning` ≤ 85%, `bg-status-error` above.
- **This session** — cumulative peak-in / total-out across the session
  (shown only when the backend reports totals).
- **Session window** — `N% used · resets in …` (hidden unless the
  provider sent rate-limit headers).

### Sidebar — two-panel floating column (`Sidebar.tsx`)

The sidebar is a single `position: fixed` flex column on the left edge,
holding two `glass-mist` panels: **Content** (top) and **Sessions**
(bottom). The chat / subagent region fills the rest of the viewport. The gap
between the two panels is fixed at `PANEL_GAP` regardless of how tall
Content gets, because they share one flex container.

**Why one column, not two truly independent floating panels?** The brief
asks for Sessions to "hug" the bottom of Content with a small gap.
Sharing a flex column is the simplest way to guarantee that — Sessions
fills the remaining vertical space (`flex-1`), Content is natural-sized
with an internal cap, the gap stays constant.

**Which panel grows?** Sessions does. The file tree (Content) is a
capped list of small rows; a tall project can have hundreds of files
and shouldn't dominate the layout. Sessions is a list of past work
that the user wants to see at a glance, so it gets the rest of the
column.

**Layout constants** (in `Sidebar.tsx`):

| Constant | Value | Role |
|---|---|---|
| `SIDEBAR_WIDTH` | 280px | Both panels and the outer column share this width. |
| `TOP_ANCHOR` | 64px | The column pins `top: 64px`. Clears the `ActiveJobBar` (12px `top-3` + 36px pill + 16px breathing room). |
| `BOTTOM_ANCHOR` | 16px | Column pins `bottom: 16px`. |
| `SIDE_ANCHOR` | 12px | Column pins `left: 12px`. |
| `PANEL_GAP` | 16px | Flex `gap` between Content and Sessions. |
| `CONTENT_BODY_MAX` | 240px | Inner cap on the Content body. The file tree is natural-sized; this stops a giant tree from pushing Sessions off the screen. |

> `TOP_ANCHOR` is tied to the `ActiveJobBar`'s current height (34px button
> + 1px border on each side = 36px pill, sitting at `top: 12px`). If the
> job bar changes height, update this constant to match — they're a pair.

**Outer column (`Sidebar` component):**
- `position: fixed`, `z-30`, `display: flex`, `flex-direction: column`,
  `gap: PANEL_GAP`.
- Animated `translate-x` + `opacity` + `pointer-events` at 200ms
  `ease-out` for open/close. Inert + `aria-hidden` when closed.

**Panel chrome (`PanelShell` helper):**
- Single `glass-mist` container, 12px radius.
- Header row: optional icon + section label using the standard recipe
  (`text-[9px] text-white-15 uppercase tracking-[0.14em] font-medium`).
  Optional count badge (`text-[10px] text-white-30 font-mono`) and
  optional `trailing` slot (a React node, typically the close button).
- The right-hand side of the header is a `flex items-center gap-2`
  block that holds the count and the trailing slot, pushed to the
  right by `ml-auto`. The two never collide because they live in the
  same `gap-2` group.
- `grow` prop: when true, the panel uses `flex-1 min-h-0` and fills the
  remaining column space. Sessions uses this; Content doesn't.
- When `grow` is false, the panel is `flex-shrink-0` and sizes to its
  content.

**Content panel** (top, natural size, capped):
- Header: `Folder` icon (10px, `text-white-30`) + `CONTENT` label +
  **close button on the right** (34×34 `glass-quartz glass-quartz--pill`,
  `PanelLeftClose` at 16px). The close lives here, not floating
  outside the column, so it's right next to the content it dismisses.
- Active workspace pill lives *below* the header strip (in its own
  `px-3 pb-1.5` row) so the header itself stays uncluttered. Pill
  recipe: `text-[10px] text-white px-2 py-0.5 rounded-full bg-white/[0.06]
  border border-white/[0.08] font-mono truncate max-w-[110px]`.
- Path strip: `text-[9px] text-white-15 font-mono truncate` showing
  the full path with a `title` attribute for the full string on hover.
- Empty state (no workspace): centered two-line message in `text-[11px]
  text-white-15` — "No workspace open. / Use the folder icon to pick one."
- File rows:
  - Inactive: `text-white-50`, hover `text-white bg-white/[0.04]
    hover:translate-x-0.5`, left border `border-transparent`.
  - Active: `text-white bg-white/[0.05] border-l border-accent-green/60`,
    1.5×1.5 `bg-accent-green` dot with `shadow-[0_0_8px_rgba(34,197,94,0.7)]`
    on the left.
  - **File-type icons:** every file row renders `FileTypeIcon`
    (`react-file-icon`, size 14 — same as the @-mention popover) with a
    `ml-1.5` gutter; folder icons are 12px. `FileText` is retained only
    for the `..` parent row (a small affordance simplification, not a
    rotated `ChevronRight`).
- Rows enter with `animate-list-item` and the standard 25ms stagger
  capped at 8 items.
- Body cap: `maxHeight: CONTENT_BODY_MAX` (240px). A tall project tree
  scrolls inside this cap rather than pushing Sessions off the screen.

**Sessions panel** (bottom, `grow` — fills the remaining column space):
- Header: `History` icon (10px, `text-white-30`) + `SESSIONS` label +
  count badge and two 34×34 `glass-quartz glass-quartz--pill` buttons on
  the right: **New session** (`Plus`, only when a workspace is open) and
  **Agent Manager** (`Bot`). The pair mirrors the topbar's pill buttons.
- Body uses `flex-1 overflow-y-auto min-h-0` so it fills whatever
  vertical space remains after Content and the close button row.
- Empty state: "No sessions yet. Start a conversation." in
  `text-[11px] text-white-15`, centered.
- Row label: `<title> — <projectName>` (e.g. `Fix login redirect — harness`).
  Two lines:
  - Primary: `text-[11px] truncate` with the label.
  - Meta: `text-[9px] text-white-15 font-mono` showing
    `<relative time> · <N> msg` (e.g. `8m ago · 14 msg`).
- Active row state: matches Content (same recipe: green left border +
  dot with glow + bg lift + bold text).
- Each row has a hover-revealed forget button (`X` icon, 10px) that
  fades in with `opacity-0 group-hover:opacity-100 scale-90
  group-hover:scale-100`, turning `text-status-error` with a
  `hover:shadow-[0_0_6px_rgba(239,68,68,0.5)]` glow on hover.
- The active session id lives in the Zustand store as `activeSessionId`.
- Sessions data shape (in `src/lib/ipc.ts`):
  ```ts
  interface SessionSummary {
    id: string;
    projectPath: string;
    projectName: string;
    createdAt: string;
    updatedAt: string;
    messageCount: number;
    title: string;       // first user message, truncated to ~40 chars
    agentId?: string;
  }
  ```
  Sessions persist per project in `.harness/sessions/<id>.json` (version 2,
  camelCase wire format; atomic tmp+rename writes; corrupt files archived).
  Backend commands: `listSessions(projectPath)`, `createSession(projectPath)`,
  `loadSession(projectPath, id)`, `loadActiveSession(projectPath)` (marker →
  newest fallback), `saveSession`, `renameSession(projectPath, id, title)`,
  `deleteSession(projectPath, id)`, `setActiveSession(projectPath, id)`.
  Agent turns run through `runAgentTurn` / `respondApproval` / `cancelAgent`
  and stream `agent-*` events routed by `requestId`.

**Open / close flow:**
- When `sidebarOpen` is `false`: the **Show sidebar** button inside the
  ActiveJobBar pill (leftmost position, 34×34 `glass-quartz
  glass-quartz--pill`, `PanelLeftOpen` at 16px) opens the column. It is
  only rendered while the sidebar is closed, so the pill never carries a
  dead button.
- When `sidebarOpen` is `true`: the Show sidebar button disappears from
  the pill, the column slides in, and a **close button** appears at the
  top of the Content panel header (right side), 34×34, same material,
  with `PanelLeftClose` at 16px. Clicking it closes the column.
- The two buttons are at different positions on purpose: the open sits
  in the global top pill (a fixed anchor), while the close is contextual
  (next to the content it dismisses). They are the same size and material
  so they read as a pair.

**Time formatting** (`formatRelativeTime` helper in `Sidebar.tsx`):
- `< 1m` → `now`
- `< 60m` → `Nm ago`
- `< 24h` → `Nh ago`
- `< 7d` → `Nd ago`
- `< 4w` → `Nw ago`
- else → `toLocaleDateString()`

> The previous spec described two independent floating panels pinned to
> top and bottom of the viewport. That layout left a large empty gap
> between Content (top) and Sessions (bottom) when Content was short,
> which failed the "barely a gap" requirement. The current single-column
> flex layout makes the gap exact.

### Subagent window manager (`WindowManager.tsx` + `SubagentWindow.tsx`)

The Orchestrator (the chat agent) can dispatch subagent turns
(Coder / Architect / Researcher / HealthMonitor) via the
`delegate_subagent` tool. Each dispatch opens a **read-only** tiled
viewer in the right-hand region of the app — the user cannot
message subagents. Only the Health Agent can, via the
`subagent_message` tool (a role-gated channel whose inbox the
subagent's loop drains at the top of every round).

**Tiling model (user-specified):**

- The main chat window **never changes size** once a subagent spawns
  — it just shifts left. Width is locked at `chatRatio` (default 0.5)
  the moment the first subagent opens and never auto-resizes after.
- The right-hand region opens for subagent windows.
- **All subagent windows are always the same size regardless of
  count** (i3-style equal grid): `cols = ceil(sqrt(n))`,
  `rows = ceil(n / cols)`. A partially filled last row simply leaves
  empty cells — no resizing the live windows.
- The chat column is clamped to `MIN_CHAT_WIDTH` (320px) so the
  Orchestrator conversation stays readable even in a narrow window;
  the subagent region absorbs the remainder and the subagent tiles
  shrink together.

**Math lives in `src/lib/wm.ts`** (pure, UI-agnostic) with a
`vitest` cover: `gridDims(count) → { cols, rows }`,
`computeTiles(count, w, h, gap) → Tile[]`, `chatSplit(containerW,
ratio, minChatW) → { chatWidth, subagentWidth }`. The
`WindowManager` component just calls `computeTiles` against the
measured region size; the components are dumb renderers.

**Per-window structure (`SubagentWindow.tsx`):**

- **Header** — `bg-surface-2/50 border-b border-border-subtle
  px-3 py-1.5`. 6px `Circle` status dot (green + `animate-pulse` while
  working, red on error, yellow on cancelled, white-15 on done),
  name in `text-[12px] font-medium text-white`, role in `text-[10px]
  text-white-30`. Trailing cluster: 24×24 stop button (working
  only) + 24×24 close (X). Closing a working subagent cancels it;
  closing a finished one just removes the window.
- **Task line** — `text-[11px] text-white-30` on `bg-surface-1`,
  border-b. `Task:` label in `text-white-15` + the delegation text
  in `text-white-50`. Locks in the context the Orchestrator handed
  off.
- **Body** — `flex-1 overflow-y-auto px-3 py-2 space-y-2`. Stack
  order: tool call cards (if any) → Health Monitor inbox bubbles
  (read-only, surfaced as the only "incoming" message a subagent
  viewer shows) → the streamed content via `MessageRenderer`
  (with collapsible Thinking block, same recipe as the chat) →
  typing indicator while working. Terminal empty-state: italic
  `(no output)` / `(cancelled)` / `{error}` depending on status.
- **Approval footer** — `border-t border-border-subtle bg-surface-2`.
  Only when the subagent loop emits an `agent-approval-request` for
  this request id. Renders the `ShieldCheck` + tool name + summary
  + Allow (green) / Deny (red) buttons. Routes through
  `respondSubagentApproval` which clears the window's own approval
  card (separate from the Orchestrator's `pendingApproval`) and
  dispatches the answer via the shared `respond_approval` IPC.

**Lifecycle / event routing** (`src/lib/subagent-manager.ts`):

- All `agent-*` events (turn-start, chunk, reasoning, tool-start,
  tool-result, approval-request, approval-result, done, error) are
  routed by request-id prefix. `sub-<uuid>` → subagent slice;
  everything else → Orchestrator chat. `session-manager.ts` adds a
  defensive `isOrchestratorRequestId` guard on every handler so a
  stray `sub-` event can never clobber the chat.
- `agent-turn-start` for `sub-` ids carries a `subagent: { name,
  role, task, personalityId }` payload (no registry round-trip
  needed) and **locks the chat ratio** to the default the moment
  the first subagent spawns.
- The chat **reflows back to its original full-width / centered
  position the moment every subagent reaches a terminal state**
  (done / error / cancelled). The trigger is "no working
  subagents left" (not "no subagents in the registry"): a finished
  record collapses its viewer, the chat column regains the full
  content width, and the `chatRatio` lock is cleared so the next
  spawn starts from a fresh default. Final answers are preserved
  in the Orchestrator's chat via the `delegate_subagent` tool
  result — the subagent viewer doesn't need to stay open for the
  user to see the result.
- The `subagent-message` event (Health Agent → subagent inbox) is
  the only way a subagent viewer can receive text while running.
  The user CANNOT type into a subagent window — there is no
  composer.
- On WebView mount, `list_subagents` resyncs the registry mirror
  so a reload reopens every still-tracked subagent window with the
  final answer + status (live content for in-flight subagents is
  gone — the user sees the eventual terminal content if the agent
  loop reports a fresh done for the same request id). If the
  resync returns only terminal records, the chat reflows
  immediately (no live work → no tiling lock).

The legacy `DynamicCanvas.tsx` (per-panel composer + 1/2/2/3
column heuristic) is retired — the file remains as an empty stub
so legacy imports resolve. `canvasPanels` / `CanvasPanel` /
`addCanvasPanel` etc. are gone from the store.

### Settings modal (`Settings.tsx`)

- Backdrop: `bg-black/70`, full screen, dismisses on click.
- Panel: `bg-surface-1 border border-border-subtle rounded-lg`,
  `max-w-[520px] max-h-[85vh]`, `animate-fade-in`. No glass.
- Header: `px-6 pt-6 pb-4`. Title is `text-[16px] font-medium text-white`.
- Body: `flex-1 overflow-y-auto px-6 pb-4 space-y-6`. Each model form is
  a `space-y-4` block. Multiple models show a "Model N" label and a
  "Remove" link that turns `text-status-error` on hover.
- Form fields use the shared `inputCls`:
  ```
  w-full px-3 py-2 bg-surface-2 border border-border-subtle rounded
  text-[13px] text-white outline-none focus:border-border-strong
  transition-colors placeholder:text-white-15
  ```
- Footer: `flex items-center justify-end gap-3 px-6 py-4 border-t border-border-subtle`.
  Cancel is `glass-quartz glass-quartz--compact`; Save is the same with
  `glass-quartz--strong` and `font-medium`.

### Empty state

Two variants in `Chat.tsx`. Both centered, `select-none`, hero wordmark
"harness" in `text-[28px] font-light tracking-tight text-white`, tagline
below in `text-[14px] text-white-30 max-w-xs leading-relaxed`:

- **No project:** "get work done, fast"
- **No messages:** "Start a conversation, spawn agents, or search the web."

The helper text under the chat input is fixed: *"harness is an agent that
can make mistakes and is not always correct with its decisions"*. Don't
rewrite that copy casually — it's the user-facing safety note.

## Motion

| Token | Used for |
|---|---|
| `animate-fade-in` | 150ms `fadeIn` `ease-out` — message enter, modal enter, popover enter |
| `animate-slide-up` | 200ms `slideUp` `ease-out` (4px translate) — reserved for future use |
| `animate-list-item` | 180ms `listItemEnter` `ease-out both` — sidebar/file tree rows |
| `cursor-blink` | 1s `step-end` infinite — typing indicator |
| `animate-pulse` | Tailwind default — used on the active-job status dot |
| `quartz-edge` | `--q-speed` (16s) linear infinite — `glass-quartz` conic edge |
| `quartz-sheen` | 18s `ease-in-out infinite alternate` — `glass-quartz` interior sheen |

### Stagger

`animate-list-item` rows in the sidebar use `style={{ animationDelay: \`${Math.min(i, 7) * 25}ms\` }}`.
The cap at 8 items × 25ms is deliberate — beyond that, the stagger becomes
a noticeable delay rather than a hint.

### Easing

- Enter: `ease-out` (all of them)
- Exit: not used; the app doesn't unmount with motion. (Open question for
  the next pass.)
- No springs, no bouncy easings. The glass material's edge ring is
  `linear` because it's a continuous travel, not a state change.

### Reduced motion

Every animated surface respects `prefers-reduced-motion: reduce`:

- `.animate-list-item` is disabled.
- `.glass-quartz` transitions and both `::before` / `::after` animations are disabled.
- Hover and active transforms on `button.glass-quartz` are disabled.

Other surfaces (the `animate-pulse` on the active dot, `animate-spin` on
the loading indicator) currently do **not** have a reduced-motion override.
Add one when you touch them.

## Anti-patterns (the project-specific list)

These are not generic AI-slop warnings — they're the things that have
actually been tried in this codebase and rejected.

1. **No purple/blue gradients.** Hard rule, unchanged.
2. **No nested cards in cards.** Hard rule, unchanged. If you need a panel
   inside a panel, use background lift (`bg-white/[0.04]`), not a new card.
3. **No box-shadow for elevation.** Use border-color change or background
   lift. The `glass-quartz` material is the *one* place shadows are
   permitted, and that's a deliberate signature.
4. **No rounded-full on large elements.** Use `rounded-lg` (6–8px) or
   `rounded-2xl` for the chat bubble. Pilling is reserved for the
   `glass-quartz--pill` family and true small affordances.
5. **No inline SVG.** Lucide icons only.
6. **No emoji in UI text.** The hero wordmark and taglines are
   text-only by design.
7. **No animated gradient backgrounds.** The `glass-quartz` material's
   conic-gradient and sheen are the *one* permitted motion-of-color.
8. **No "✨ magic ✨" copy.** The product copy is quiet and operational.
9. **No glassmorphism from third-party libraries.** The `glass-quartz` and
   `glass-mist` classes in `src/index.css` are the only ones; don't import
   "frosted glass" CSS from elsewhere.
10. **No unconfigured Tailwind colors.** The classes outside
    `surface / white / border / status` silently no-op. Don't add new
    colors without updating this doc and the Tailwind config together.

## Iconography

Lucide only. Sized in `9 / 10 / 12 / 14 / 16 / 18` pixels. Stroke width
defaults. Color follows the text role (`text-white-15` for inactive,
`text-white-50` for hover, `text-white` for active).

In use: `Settings`, `Circle`, `FolderPlus`, `PanelLeftOpen`,
`PanelLeftClose`, `Folder`, `FileText`, `History`, `Bot`, `Plus`, `X`,
`Send`, `Loader2`, `Square`, `RefreshCw`, `ShieldCheck`, `Target`,
`Check`, `Wrench`, `Trash2`, `Brain`, `Lock`, `AlertTriangle`,
`ChevronDown`, `ChevronRight`, `Eye`, `EyeOff`, `XCircle`,
`CheckCircle2`, `CircleDot`, `Play`.

File-type glyphs are **not** Lucide: the content tree and @-mention
popover render `FileTypeIcon` from `react-file-icon` (see
`src/lib/file-icons.tsx` for the curated `DEV_STYLES` palette and
`THEME_OVERRIDES`).

## Open questions

These are real decisions the design hasn't made yet, called out so the
next pass doesn't punt on them.

1. **Token consolidation.** `status.active` and `--accent-green` are the
   same color via two paths. Same for `status.error` and `--accent-red`.
   Pick one and migrate. (My recommendation: keep `status.*` for Tailwind,
   `--accent-*` for the CSS-side material/shadow values, and document
   that they're aliases.)
2. **Reduced-motion coverage.** The `slider-quartz` thumb transition is
   disabled, but `animate-pulse` and `animate-spin` still are not when
   `prefers-reduced-motion: reduce` is on. Add it on next pass.
3. **Focus styles.** `:focus-visible` exists on `button.glass-quartz`
   (2px white outline at 60%, 2px offset), the Thinking/Goal switches and
   the `slider-quartz` thumb reuse the same ring via utilities, and
   `glass-mist` (border becomes `border-strong`). Inputs rely on
   `focus:border-border-strong`. There's no shared focus-ring utility yet —
   the ring recipe is now repeated in three places.
4. **Modal vs. side panel for Settings.** Settings is a centered modal
   with a backdrop. As the form grows (model picker, defaults, advanced),
   a side panel may serve better. Not urgent.
5. **Empty workspace state.** The sidebar's file tree shows "No files"
   when the directory is empty. There's no "drag a folder here" or
   "create a file" affordance yet. Wire that in when the empty-state
   pattern is decided for the rest of the app.
6. **Type scale.** The ad-hoc `text-[9..28px]` ladder works but is
   undocumented. Convert to a `text-3xs .. text-2xl` scale in
   `tailwind.config.js` and migrate components when there's a quiet
   moment.
