# DESIGN.md — Harness UI Design System

> Authoritative for visual decisions. For product context, see `PRODUCT.md`.
> For repo conventions, see `AGENTS.md`.
>
> This spec is generated from the code, not the other way around. When the
> code and this doc disagree, **the code wins** — fix this doc.
>
> Last reconciled: against `src/index.css`, `tailwind.config.js`, and the
> component tree under `src/components/`.

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
| `surface-2` | `#111111` | Cards, inputs, active surfaces, chat-user bubbles, "compact" surfaces. |
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
| `text-[13px]` | Settings labels, body copy in canvas sub-tiles, dialog copy |
| `text-[12px]` | Sub-canvas agent name, button labels, modal labels |
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
- **Card with a header and body, no glass →** plain `bg-surface-1` with `border-border-subtle` (this is what the DynamicCanvas sub-tiles do)
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

The ActiveJobBar sits at `top-3` with `left-1/2 -translate-x-1/2` and a
34×34 hit target for each button. That's the load-bearing floating position
in the app.

## Component patterns

### Chat message (`Chat.tsx`)

- **User:** right-aligned, `bg-surface-3`, `rounded-2xl rounded-br-sm`,
  `px-4 py-2.5`. Body is `text-white`.
- **Assistant:** left-aligned, no bubble. Body is `text-white-50`. Above
  the body, a label row with a `bg-status-active` 1.5×1.5 dot, the agent
  name in `text-white-30` at `text-[11px] font-medium`, and a 1.5px gap.
- **System:** centered pill, `bg-surface-2`, `text-white-30` at `text-[12px]`,
  `px-3 py-1`, `rounded-full`, `border-border-subtle`.
- **Animation:** every message is `animate-fade-in` (150ms `ease-out`).

### Active job bar (`ActiveJobBar.tsx`)

A single fixed pill at top-center, `glass-mist glass-mist--pill`, holding:

1. **Open folder** — 34×34 `glass-quartz glass-quartz--pill` button (`FolderPlus`).
2. A 1px vertical divider (`w-px h-4 bg-border-subtle`).
3. **Status indicator** — when idle: a `Circle` (6px) at `fill-white-15` plus
   "Idle" in `text-[11px] text-white-15`. When active: a tinted pill at
   `bg-accent-green/10 border border-accent-green/20` with a `Circle` (6px)
   at `fill-accent-green` and `animate-pulse`, plus the first activity label
   truncated to 160px.
4. A 1px vertical divider.
5. **Settings** — 34×34 `glass-quartz glass-quartz--pill` button (`Settings`).

Hovering the status pill reveals a `glass-mist` popover (`min-w-[280px]`,
`p-3`, `animate-fade-in`) with the full activity list and active agents.
The popover uses `top-full left-1/2 -translate-x-1/2 mt-2`.

The SidebarToggleFloating is a 34×34 `glass-quartz glass-quartz--pill` button
at `left-3 top-[18px]`, only visible when the sidebar is closed.

### Sidebar — two-panel floating column (`Sidebar.tsx`)

The sidebar is a single `position: fixed` flex column on the left edge,
holding two `glass-mist` panels: **Content** (top) and **Sessions**
(bottom). The chat / canvas fills the rest of the viewport. The gap
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
  - `..` row uses `FileText` icon (not `ChevronRight` rotated) — small
    affordance simplification.
- Rows enter with `animate-list-item` and the standard 25ms stagger
  capped at 8 items.
- Body cap: `maxHeight: CONTENT_BODY_MAX` (240px). A tall project tree
  scrolls inside this cap rather than pushing Sessions off the screen.

**Sessions panel** (bottom, `grow` — fills the remaining column space):
- Header: `History` icon (10px, `text-white-30`) + `SESSIONS` label +
  count badge on the right.
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
  Backend wiring (`listSessions`, `loadSessionById`, `forgetSession`) is
  owned by the LLM-support work in flight. The UI consumes the contract
  now so nothing has to be re-shaped later.

**Open / close flow:**
- When `sidebarOpen` is `false`: a single `SidebarToggleFloating` button
  at `left: 12px / top: 18px` (`left-3 top-[18px]`), 34×34
  `glass-quartz glass-quartz--pill`, `PanelLeftOpen` icon at 16px.
  Clicking it opens the column.
- When `sidebarOpen` is `true`: the floating open toggle disappears,
  the column slides in, and a **close button** appears at the top of
  the Content panel header (right side), 34×34, same material, with
  `PanelLeftClose` at 16px. Clicking it closes the column.
- The two buttons are at different positions on purpose: the open
  toggle is a fixed global anchor (always at the same place), while
  the close is contextual (next to the content it dismisses). They
  are the same size and material so they read as a pair.

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

### DynamicCanvas sub-tile (`DynamicCanvas.tsx`)

- A grid of 1 / 2 / 3 columns depending on panel count: `count <= 1 ? 1 : count <= 2 ? 2 : count <= 4 ? 2 : 3`.
- Each sub-tile is `bg-surface-1 border border-border-subtle` (no glass), with:
  - **Header** at `bg-surface-2/50`, `border-b border-border-subtle`,
    `px-3 py-1.5`. Contains a 6px `Circle` indicator (green/working, red/error,
    white-15/idle), the agent name in `text-[12px] font-medium text-white`,
    the role in `text-[11px] text-white-15`, and an X close button.
  - **Body** `flex-1 overflow-y-auto p-3 space-y-2.5`. User messages in
    `text-white-50`, assistant in `text-white-30`, both at `text-[13px]`.
  - **Footer** `border-t border-border-subtle p-2`, with a `bg-surface-2
    border border-border-subtle rounded` input, `bg-transparent text-[12px]
    text-white px-3 py-2`.

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

In use: `ChevronRight`, `Folder`, `FileText`, `X`, `FolderOpen`,
`PanelLeftClose`, `PanelLeftOpen`, `Send`, `Loader2`, `Settings`, `Circle`,
`FolderPlus`, `Eye`, `EyeOff`, `RefreshCw`, `XCircle`, `ChevronDown`.

## Open questions

These are real decisions the design hasn't made yet, called out so the
next pass doesn't punt on them.

1. **Token consolidation.** `status.active` and `--accent-green` are the
   same color via two paths. Same for `status.error` and `--accent-red`.
   Pick one and migrate. (My recommendation: keep `status.*` for Tailwind,
   `--accent-*` for the CSS-side material/shadow values, and document
   that they're aliases.)
2. **Reduced-motion coverage.** `animate-pulse` and `animate-spin` are
   not yet disabled when `prefers-reduced-motion: reduce` is on. Add it
   on next pass.
3. **Focus styles.** `:focus-visible` exists on `button.glass-quartz`
   (2px white outline at 60%, 2px offset) and on `glass-mist`
   (border becomes `border-strong`). Inputs rely on `focus:border-border-strong`.
   There's no shared focus ring utility yet.
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
