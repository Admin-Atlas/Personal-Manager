# Handoff: PM (Personal Manager) — Design System

## Overview
PM is a local-first "personal operating system" desktop app (Tauri + React/TypeScript). This
package documents its **visual design language**, not its features. The deliverable for you is a
**theming/presentation layer** — design tokens plus a small set of styled primitives — that the
existing app can adopt without changing what the app *does*.

The design has **four independent axes** the user can switch at runtime:

| Axis | Values | What it changes |
|------|--------|-----------------|
| **System** | `editorial` · `slate` · `terminal` | The whole visual language: layout structure, type, corner radius, density character |
| **Mode** | `dark` · `light` | Light/dark palette (same accent hue) |
| **Accent** | per-system palette | The primary color **and** the hue of every neutral (backgrounds warm/cool with it) |
| **Depth** | `min` · `standard` · `power` | How many features/fields are *shown* (not a layout change) |

All four are orthogonal and compose: e.g. *Slate · light · blue accent · power* is a valid state, as is *Terminal · dark · amber · minimal*.

---

## ⚠️ Read this first — how to use this handoff (and how NOT to)

This is the part the user cares about most. Please follow it literally.

### 1. The design is a presentation layer. It must never own functionality.
- Implement it as **theme tokens + reusable styled primitives** (`Button`, `Card`, `ListRow`,
  `Badge`, `Modal`, `Input`, `NavItem`, `Skeleton`, …). The rules in this doc describe how a
  *type* of element looks — **not** how one specific button on one specific screen looks.
- Features may be **added, changed, or removed** at any time. The design system must keep
  working when that happens. So: **do not** hardcode styling onto individual buttons/pages, and
  **do not** let the design layer decide which features exist, what routes there are, or what
  data is shown. A new button added next month should look right *for free* because it uses the
  `Button` primitive — not because someone restyled it.
- If a value in here ever conflicts with how a feature needs to behave, **behavior wins.** The
  design layer styles whatever the app renders; it does not gate or remove behavior.

### 2. There is no real content here. Everything is a sketch.
- Every name, project, date, email address, model id, dollar amount, and calendar event in the
  prototypes is **illustrative placeholder data to show layout** — e.g. "Bobby", "Atlas
  proposal", `events@example.com`, "claude-sonnet-4", "$0.04". **Do not copy any of it into the
  codebase.** Wire every surface to **real** data from the app.
- Keep the *design rules* (e.g. the date format below, the status taxonomy, the "nothing acts
  without you" approval pattern). Drop the *example values*.
- Where the design shows a list/empty area, build the real data path and the **real empty
  state** — never ship lorem-style filler.

### 3. This is an open-source repo. No secrets, no personal data — from the first commit.
- **Never commit secrets.** The mock shows an "OpenRouter API key" field; in the real app the key
  must live in the **OS keychain** (Tauri: use the `keyring`/secure-storage APIs), never in code,
  `.env` committed to git, localStorage, or logs.
- Add/verify a `.gitignore` for `.env*`, secret files, and local config **before** the first
  commit. Don't bake model lists, endpoints, or account ids into source if they're meant to be
  user/runtime config.
- The fictional personal data above (a person's name, the `@example.com` email) is for layout
  only — replace with real app data or neutral examples; don't seed the repo with it.

---

## About the design files
The files in this bundle are **design references created as a single HTML prototype** — they show
intended look, structure, and behavior. They are **not production code to copy**. Your job is to
**recreate this design language in the app's existing React/TypeScript environment**, using its
established patterns (component library, styling approach, state). The prototype is built with
inline styles + CSS custom properties; in the real app, express the same tokens however the
codebase already does theming (CSS variables, a `ThemeProvider`, Tailwind config, etc.).

- `PM.dc.html` — the current, complete design system (all four axes, all surfaces, all in-use states). **This is the source of truth.**
- `support.js` — runtime needed to open `PM.dc.html` in a browser. Not part of the design.
- `DESIGN_TOKENS.md` — the precise token recipe (OKLCH ramps, accent math, status colors, fonts, radii) + a framework-agnostic `themeVars()` function. **Start here when implementing theming.**

> The design rules that used to live in a drop-in `CLAUDE.md` here now live in the repo's
> [`AGENTS.md`](../AGENTS.md#design-system-v2) (`## Design system`), so they stay in context on
> every task. The earlier three-System exploration prototype has been removed now that the
> direction is settled — `PM.dc.html` is the single source of truth.

## Fidelity
**High-fidelity.** Final colors (as an OKLCH system), typography, spacing, radii, and interaction
patterns are all specified. Recreate the UI faithfully using the codebase's libraries — but treat
the *layout of each screen* as the intended target for that surface, not a pixel contract that
overrides product decisions.

---

## The token system (summary — full values in `DESIGN_TOKENS.md`)

The whole palette is **OKLCH-based and accent-driven**:

1. The active **accent** hex is converted to an OKLab **hue** `H`.
2. Every neutral (`--bg`, `--surface`, `--ink`, `--border`, …) is `oklch(L C H)` where `L`/`C`
   come from a per-**System**, per-**Mode** profile and `H` is the accent's hue. → This is why the
   background subtly warms/cools with the accent instead of being a fixed tint.
3. Accent roles:
   - `--accent` = the chosen hex (fills, dots, borders, active indicators).
   - `--accent-text` = accent used as **colored text**. Dark mode: `= --accent`. Light mode:
     `oklch(0.52 min(C,0.17) H)` (deepened so it stays legible on white — important for light
     accents like phosphor green).
   - `--accent-ink` = `oklch(0.16 0.024 H)` — text/icon **on** an accent fill.
   - `--accent-soft` = `rgba(accent, 0.15)` (dark) / `0.14` (light) — tint backgrounds.
4. **Status colors** are a separate semantic set (due / blocked / quick-win / take-a-look /
   part-of / on-track), with distinct **dark** and **light** values per System (light versions are
   deepened for contrast). They are **not** tied to the accent.

Implement components against **CSS custom properties only** (`var(--ink)`, `var(--accent)`, …).
Set the properties on a root element from a `themeVars(system, mode, accent)` function. Never put a
hex literal in a component.

---

## Systems (layout languages)

Each System is a genuinely different layout treatment of the **same surfaces and data**. Keep all
three as shippable options.

- **Editorial** — serif headlines (Newsreader), Hanken Grotesk UI, JetBrains Mono for
  numbers/ids. Hairline rules instead of boxes; generous, "set like a page." Soft 12 px corners.
- **Slate** — calm, card-based command centre. Hanken Grotesk throughout, mono for meta. 10 px
  corners, surfaces/cards with hairline borders.
- **Terminal** — monospace everything. Status-bar chrome (no traffic-light titlebar), `❯`
  prompts, bracket nav (`f focus`), dense tables, ANSI-style status dots, square (2 px) corners.

`--head` / `--ui` / `--mono` font tokens and `--radius` carry most of this automatically; the
structural differences (rules vs cards vs tables, prompt chrome) are real markup variants and are
documented per surface below.

## Mode
`dark` (default) and `light`. Same accent hue drives both; only the neutral L/C ramp and the status
set swap. Light = near-white system-tinted bg, dark ink; softer shadow. See `DESIGN_TOKENS.md`.

## Depth (feature reveal — **not** a layout change)
Implement as flags that **show/hide optional content** within the same layout:
- **`min`** — hide meta lines, model footers, keyboard hints, secondary columns; larger type, more air.
- **`standard`** — the everyday view.
- **`power`** — reveal everything: cost, token counts, timestamps, extra table columns, keybind hints, command affordances.

Drive it with a single `depth` value; gate optional blocks on it. Don't fork layouts per depth.

---

## Surfaces (purpose + layout intent)

> Content shown is placeholder. Recreate the *layout/treatment*; wire real data and real empty/loading states.

- **Window shell** — title/status bar + left sidebar (nav) + main content. Editorial/Slate use a
  traffic-light titlebar with a `⌘K` search affordance; Terminal uses a `● pm [alpha] ~/pm/<view>`
  status bar. Sidebar = nav items + (Depth-gated) model footer + Settings.
- **Chat / Home (dashboard)** — greeting + daily briefing + (Depth-gated) Upcoming and To-do +
  a prominent chat/voice composer pinned to the bottom. Editorial sets it as a typeset page; Slate
  uses cards; Terminal renders it as a prompt session with a briefing block.
- **Focus** — every project with one honest status (the status taxonomy). Editorial = rule rows;
  Slate = cards; Terminal = table. Depth scales it from name+status only → +meta → full table with cost/active.
- **Project** — file list beside a project-scoped chat thread (two-pane). Treatment per System.
- **Documents** — ingestion: a drop zone + recent items with type/date/index-state. Rules / cards / mono table per System.
- **Calendar** — read-only week/agenda. Serif agenda / day cards / mono list per System.
- **Review** — queue of proposed actions as inline approval cards (Approve / Edit / Dismiss).
- **Settings** — appearance (System / Accent / Depth / Mode), models & keys (key field → keychain), help-mode toggle. Editorial = ruled sections, Slate = card panel, Terminal = key/value list.
- **Command palette** (`⌘K`) — overlay with a query line + result rows.

## In-use states (reusable patterns — token-based)
Build these as shared primitives so any surface can enter them:
- **Streaming / loading** — a spinner + shimmer **skeleton** rows + a response that **types out**
  with a blinking caret (`▋`). Skeleton shimmer = a 200%-wide `--surface → --border → --surface`
  gradient animated horizontally.
- **Empty** — a calm rest state (mark + headline + one line). Build the *real* empty state per surface.
- **Approval** — a **blocking modal** showing the actual action it wants to take (e.g. the email
  body) with **Approve / Edit / Cancel**. Reinforces "nothing acts without you." This is the
  app's core trust pattern — keep it for any side-effecting action.
- **Permission** — a capability request modal (what + why + "read-only / stays on device") with
  Always allow / Allow once / Not now.

## Interactions & behavior
- Nav switches the main view; clicking a project opens the Project surface; `⌘K` toggles the
  palette; `Esc` closes overlays.
- View transitions: a 0.25 s ease fade-up (`opacity` + 4 px `translateY`). Caret blink ~1 s steps. Spinner 0.8 s linear. Shimmer ~1.4 s linear.
- Hover/active/focus belong on the primitives (see component rules in `DESIGN_TOKENS.md`), not on instances.

## State management (presentation only)
The only state the **design layer** owns is `{ system, mode, accent, depth }` (persist per user)
plus transient UI (`paletteOpen`, current view, which modal is open). Everything else — projects,
documents, messages, calendar, models, costs — is **app state from real sources**. Keep the theme
state separate from feature state.

## Design tokens
See **`DESIGN_TOKENS.md`** for the complete, exact recipe: OKLCH neutral ramps (per System ×
Mode), accent palettes, accent math (`--accent-text` / `--accent-ink` / `--accent-soft`), status
colors (dark + light), fonts, radii, the OKLab hue function, and a ready-to-port
`themeVars(system, mode, accent)`.

### One hard design rule to keep (not placeholder)
**All user-facing dates render `DD-MM-YYYY`, with the `-YYYY` dropped when the date is in the
current year** (e.g. `21-06` this year, `21-06-2027` otherwise). This is a real rule; the specific
dates in the mock are not.

## Assets
No raster/vector brand assets are required. Fonts: **Newsreader**, **Hanken Grotesk**, **JetBrains
Mono** (Google Fonts) — self-host or use the codebase's font pipeline. The only inline SVG is a
simple mic glyph in the composer; replace with the codebase's icon set. Status/empty marks are CSS
shapes.

## Files in this bundle
- `PM.dc.html` — current full design system (source of truth)
- `DESIGN_TOKENS.md` — token recipe + `themeVars()`
- `support.js` — runtime to open `PM.dc.html`
- The design rules also live in the repo's [`AGENTS.md`](../AGENTS.md#design-system-v2).
