# PM — Design Tokens

Everything here is **framework-agnostic**. Port it into whatever theming mechanism the codebase
already uses (CSS variables + a provider, Tailwind theme, etc.). **Components must only ever read
the resulting CSS custom properties** (`var(--ink)`, `var(--accent)`, …) — never a hex literal.

The palette is **accent-driven OKLCH**: the accent's hue tints every neutral. Pick `{ system, mode,
accent }` → compute hue from the accent → build all neutrals as `oklch(L C H)` from a per-system,
per-mode `[L, C]` profile.

---

## 1. Fonts & radii (per System)

```
editorial:  head = "Newsreader, Georgia, serif"
            ui   = "Hanken Grotesk, system-ui, sans-serif"
            mono = "JetBrains Mono, monospace"
            radius = 12px   radiusSm = 9px

slate:      head = ui = "Hanken Grotesk, system-ui, sans-serif"
            mono = "JetBrains Mono, monospace"
            radius = 10px   radiusSm = 8px

terminal:   head = ui = mono = "JetBrains Mono, monospace"
            radius = 2px    radiusSm = 2px
```

Token names: `--head` (headings/display), `--ui` (body/controls), `--mono` (numbers, ids, code,
meta), `--radius`, `--radius-sm`.

---

## 2. Neutral ramps — `[L, C]` per role (hue comes from the accent)

Each value is `oklch(L C H)`. Roles, lightest-surface → text:
`bg, panel, surface, border, border2, rule, ink, ink2, ink3, ink4, faint`.

```
editorial.dark   bg[.165,.016] panel[.145,.016] surface[.205,.018] border[.265,.016] border2[.315,.016] rule[.235,.012]
                 ink[.915,.018] ink2[.845,.020] ink3[.685,.018] ink4[.575,.016] faint[.485,.014]
editorial.light  bg[.985,.008] panel[.962,.010] surface[.944,.012] border[.884,.013] border2[.820,.015] rule[.918,.010]
                 ink[.290,.030] ink2[.405,.026] ink3[.520,.022] ink4[.600,.018] faint[.700,.014]

slate.dark       bg[.155,.013] panel[.135,.013] surface[.205,.015] border[.255,.013] border2[.305,.015] rule[.225,.011]
                 ink[.925,.013] ink2[.835,.015] ink3[.665,.015] ink4[.565,.013] faint[.455,.011]
slate.light      bg[.992,.004] panel[.974,.005] surface[.962,.006] border[.902,.008] border2[.845,.010] rule[.935,.006]
                 ink[.265,.018] ink2[.385,.016] ink3[.510,.015] ink4[.600,.012] faint[.710,.009]

terminal.dark    bg[.135,.007] panel[.115,.007] surface[.165,.007] border[.235,.007] border2[.285,.009] rule[.205,.006]
                 ink[.865,.011] ink2[.795,.011] ink3[.585,.009] ink4[.495,.008] faint[.345,.007]
terminal.light   bg[.967,.010] panel[.945,.011] surface[.934,.011] border[.860,.013] border2[.805,.013] rule[.905,.008]
                 ink[.300,.018] ink2[.395,.016] ink3[.520,.013] ink4[.600,.011] faint[.690,.008]
```

Role intent: `bg` = window/main; `panel` = titlebar/sidebar; `surface` = cards/raised; `border` =
hairlines; `border2` = stronger border / control outline; `rule` = faint row dividers; `ink`→`ink4`
= text from primary to faintest.

`faint` is **not** a text tier. It is the decorative/disabled role — separators, placeholder glyphs,
and the `disabled:` colour of a control — and it is the one role no Contrast level lifts, so it
renders as low as 1.67:1 and must never carry text a reader is expected to read. Anything
informational stops at `ink4`, which every Contrast level holds to 4.5:1.
`src/theme/designGuards.test.ts` enforces this with a named allow-list.

---

## 3. Accent palettes (per System) — the picker options

```
editorial:  #d2825b  #c96f4c  #cda44e  #8f9a5b  #c789a4  #6f8bbf
slate:      mono     #5b8cff  #5bb5c0  #9b8cf0  #5fd6a0  #e0a86a  #ff93b4
terminal:   #9ece6a  #e0af68  #7dcfff  #bb9af7  #f7768e  #7fe0b0
```

The first entry is each System's default accent. Switching System resets the accent to that
System's default unless the user has chosen one for it.

### Monochrome accent (`mono` / Eigengrau) — Slate only, the app default

`mono` is a **sentinel, not a hex**: it selects a monochrome treatment instead of a hue, and is
Slate's default accent (so a fresh install is Eigengrau). `themeVars` special-cases it:

- **Neutral ramp** → the chroma-0 `MONO_RAMP` (a straight greyscale; **no** accent hue tints the
  cosmetics), *not* the §2 slate ramp. `--bg` is pinned to the exact **Eigengrau `#16161D`** in dark.
- **Accent-derived tokens** → dark: `--accent`/`--accent-text` = white, `--accent-ink` = Eigengrau,
  `--accent-soft` = `rgba(255,255,255,.12)`. light: near-black text/accents on paper.
- **Feature colours are unaffected** — the §4 semantic status set and the map palette
  (`graphPalette.ts`) still render in colour; only cosmetic hue is removed.

`sourcePalette` always drops `mono` (it's not a colour), so calendar source hues are never affected.

### Accent-derived tokens
Given the active accent's OKLab `{ L, C, H }`:
```
--accent       = <accent hex>                     (fills, dots, borders, active indicators)
--accent-text  = dark:  <accent hex>
                 light: oklch(0.52  min(C, 0.17)  H)   (deepened for legibility on white)
--accent-ink   = oklch(0.16  0.024  H)            (text/icon ON an accent fill)
--accent-soft  = rgba(accent, 0.15) dark / rgba(accent, 0.14) light   (tint backgrounds)
```
Rule of thumb in components: **fill / dot / border → `--accent`; colored text → `--accent-text`;
label on a filled accent → `--accent-ink`; subtle tint behind something → `--accent-soft`.**

---

## 4. Status colors (semantic — NOT accent-tied)

Order: `due, blocked, quick, look, part, track`. Separate dark/light sets per System. Tokens:
`--st-due`, `--st-blocked`, `--st-quick`, `--st-look`, `--st-part`, `--st-track`.

These are **text** colours (error messages, `role="alert"` copy, status chips) as much as they are
fills, so the light rows are calibrated to clear WCAG AA (4.5:1) — built to 4.6:1 — against the
worst background they can land on. That is `surface`, the *darkest* of `bg/panel/surface` in light
mode, under whichever accent hue drives it lowest; `bg` is the lightest and therefore the most
forgiving. Note that a contrast axis cannot save them: the status row is emitted verbatim and is
byte-identical at every contrast level.

Deepening holds each colour's OKLab hue, and its chroma except where sRGB clips it. The competing
rule is that the six stay distinguishable from **each other** — a row pushed uniformly toward one
lightness passes AA and destroys the taxonomy — so each row's minimum pairwise OKLab ΔE is held at
or above where it started, with one measured exception (slate's `due`/`blocked`, −4.7%). Dark rows
already clear comfortably (worst 5.20:1) and are untouched.

```
editorial.dark   #e0856a #c789a4 #9aab66 #d2a24e #7fa3a0 #9a8f80
editorial.light  #b2472c #a14e74 #626f2c #8f6000 #3a726e #6f6457
slate.dark       #ff8088 #ff93b4 #5fd6a0 #ffc266 #79c0ff #9aa0ad
slate.light      #ca2a3f #be3473 #007b4d #965f01 #2d6db3 #5f6470
terminal.dark    #f7768e #bb9af7 #9ece6a #e0af68 #7dcfff #82867f
terminal.light   #bc344d #7950be #36750e #8d5e03 #2a6a9a #5a5e57
```

Status taxonomy (labels are the design's vocabulary; map to your real states):
`due soon · blocked · quick win · take a look · part of · on track`.

---

## 5. OKLab hue/L/C from a hex (port verbatim)

```js
// sRGB hex -> OKLab. Returns { L, C, H } (H in degrees 0..360).
function oklabLCH(hex) {
  const h = hex.replace('#', '');
  const lin = (v) => { v = parseInt(v, 16) / 255; return v <= 0.04045 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4); };
  const r = lin(h.slice(0, 2)), g = lin(h.slice(2, 4)), b = lin(h.slice(4, 6));
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  const L = 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s;
  const A = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s;
  const B = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s;
  let H = Math.atan2(B, A) * 180 / Math.PI; if (H < 0) H += 360;
  return { L: +L.toFixed(3), C: +Math.hypot(A, B).toFixed(3), H: +H.toFixed(1) };
}
```

---

## 6. `themeVars(system, mode, accent)` — build the CSS custom properties

```js
const FONTS = {
  editorial: { head: "'Newsreader',Georgia,serif", ui: "'Hanken Grotesk',system-ui,sans-serif", mono: "'JetBrains Mono',monospace" },
  slate:     { head: "'Hanken Grotesk',system-ui,sans-serif", ui: "'Hanken Grotesk',system-ui,sans-serif", mono: "'JetBrains Mono',monospace" },
  terminal:  { head: "'JetBrains Mono',monospace", ui: "'JetBrains Mono',monospace", mono: "'JetBrains Mono',monospace" },
};
const RADII = { editorial: ['12px','9px'], slate: ['10px','8px'], terminal: ['2px','2px'] };

// PROFILES[system][mode] = the [L,C] table from §2. STATUS[system][mode] = the array from §4.
// ACCENTS[system] = palette from §3.
const ROLES = ['bg','panel','surface','border','border2','rule','ink','ink2','ink3','ink4','faint'];
const ST = ['due','blocked','quick','look','part','track'];

function hexA(hex, a) {
  const h = hex.replace('#',''); 
  return `rgba(${parseInt(h.slice(0,2),16)},${parseInt(h.slice(2,4),16)},${parseInt(h.slice(4,6),16)},${a})`;
}

function themeVars(system, mode, accent) {
  const ramp = PROFILES[system][mode];
  const stat = STATUS[system][mode];
  const { C, H } = oklabLCH(accent);
  const ok = ([L, c]) => `oklch(${L} ${c} ${H})`;
  const v = {};
  ROLES.forEach(r => v[`--${r}`] = ok(ramp[r]));
  ST.forEach((k, i) => v[`--st-${k}`] = stat[i]);
  v['--head'] = FONTS[system].head; v['--ui'] = FONTS[system].ui; v['--mono'] = FONTS[system].mono;
  v['--radius'] = RADII[system][0]; v['--radius-sm'] = RADII[system][1];
  v['--accent'] = accent;
  v['--accent-text'] = mode === 'light' ? `oklch(0.52 ${Math.min(C, 0.17)} ${H})` : accent;
  v['--accent-ink'] = `oklch(0.16 0.024 ${H})`;
  v['--accent-soft'] = hexA(accent, mode === 'light' ? 0.14 : 0.15);
  return v; // spread onto a root element's style, or emit as a :root rule
}
```

> The recipe above is the **coloured** path only. The shipped `themeVars` adds the `mono` branch
> from §3 (chroma-0 ramp, white/near-black accents, Eigengrau `--bg`).

### `mode` is *resolved* before it reaches `themeVars`

`themeVars` always receives a concrete `dark | light`. What the user actually picks is a **Mode
preference** — `light | dark | system | auto` — which a resolver (`resolveMode.ts`) collapses to that
concrete value before styling. This is the single place four options become two; everything
downstream still only ever sees `dark | light`.

- `system` → the OS `prefers-color-scheme` (kept live via a media-query listener).
- `auto` → sunrise/sunset at the user's location, computed **offline** (`solar.ts`) from
  timezone-derived coordinates (`timezones.ts`) or a manual `lat, lon` override; re-resolved at each
  transition and whenever the app regains focus. With no location it degrades to `system`.

The preference (not the resolved value) is what's persisted in localStorage + the appearance blob.

---

## 7. Component rules (apply to the **type**, never an instance)

All values below are tokens. Build primitives; let every concrete button/card/etc. inherit.

- **Button**
  - *Primary*: `background var(--accent)`, `color var(--accent-ink)`, `border-radius var(--radius-sm)`, weight 600. Hover: lighten accent ~6%. Active: darken ~8%. Disabled: `background var(--surface)`, `color var(--faint)`.
  - *Secondary*: transparent bg, `color var(--ink2)`, `1px solid var(--border2)`. Hover: `background var(--surface)`.
  - *Tertiary*: text-only `color var(--ink4)`; hover `color var(--ink2)`.
  - *Terminal flavor*: wrap label in `[ … ]`, square corners, mono.
- **Switch (Toggle)**: track ON `background var(--accent)` + `box-shadow inset 0 0 0 1px var(--accent)`, OFF `background var(--surface)` + `inset 0 0 0 1px var(--border2)`, knob inset `2px`. The outline is an **inset shadow, never a border**: a border is part of the box, so under `border-box` it eats a pixel off the padding box that the knob's `left`/percentage `top` resolve against — it shrinks the knob's surround from 2px to 1px on all four sides and lands the ON and OFF edges on different device-pixel boundaries at a fractional DPR, which reads as the dot changing size when you flip it. A shadow paints over the track's edge and takes part in no layout. A fill alone is not an edge: `--surface` sits one step off `--panel`, which reads as a raised area behind text and not as the boundary of a control, so an un-bordered OFF switch was a lone knob on apparently empty page — and because no neutral in it responds to the Contrast axis, turning **High** on changed switches not at all. `--border2` is the ramp's strong edge, mode-relative through `boost()` (darkens on a light page, lightens on a dark one) and firmed at `high` by +0.18 L light / +0.20 L dark. Knob ON `background var(--accent-ink)`, OFF `background var(--ink4)`. The knob's two states use **different** tokens on purpose: `--accent-ink` is calibrated only against the accent fill it sits on when ON, and has no contrast contract anywhere else — painted unconditionally it drew the OFF knob at 1.0–1.15:1, and under the `mono` accent `--accent-ink` and `--bg` are the same literal. Any control whose state changes the surface beneath a part must re-pick that part's token with it. Disabled: `opacity` on the wrapper — the switch has no disabled colour branch, so unlike Button this alpha is its only inert cue and cannot simply be dropped.
- **Input**: `background var(--surface)`, `1px solid var(--border2)`, `var(--radius-sm)`, text `var(--ink2)`, placeholder `var(--ink4)`. Focus: border `var(--accent)`. Terminal: prepend `❯`, caret in `var(--accent-text)`.
- **Card / surface**: `background var(--surface)`, `1px solid var(--border)`, `var(--radius)`. Editorial often replaces cards with a top `1px solid var(--border)` rule + breathing room.
- **List row**: divider `1px solid var(--rule)`; title `var(--head)`/`var(--ink)`; meta `var(--mono)`/`var(--ink4)`.
- **Status badge/dot**: dot = `background var(--st-*)`. Slate badge = `color var(--st-*)`, `background rgba(st, .12)`, `border rgba(st, .28)`. Editorial = colored dot + italic label in the status color. Terminal = `● label` in the status color.
- **Nav item (active)**: Editorial/Terminal = `border-left 2px var(--accent)` + `background var(--surface)`; Slate = `background var(--surface)`, `color var(--ink)`. Inactive = `var(--ink3)`.
- **Window chrome**: titlebar/sidebar use `var(--panel)` + `1px solid var(--border)`. Editorial/Slate: traffic-light dots in `var(--border2)`. Terminal: `● pm [alpha] ~/pm/<view>` status bar.
- **Modal / scrim**: scrim `rgba(8,6,4,.5)`; dialog = `var(--surface)`, `1px solid var(--border2)`, `var(--radius)`, large soft shadow. `Esc` / scrim-click closes.
- **Skeleton**: block `var(--surface)`; shimmer = `linear-gradient(90deg, var(--surface), var(--border), var(--surface))` at `background-size:200% 100%`, animate position 200%→-200% over ~1.4 s.

## 8. Type scale (approximate — from the prototype)
- Display/greeting `var(--head)` ~30–31px / 600 · Surface title ~20–22px · Card/row title ~16–19px ·
  Body ~13–18px (Editorial leans larger serif) · Meta/labels `var(--mono)` 10–11px, ~0.12em tracking, uppercase for section labels.
- Mobile/responsive isn't in scope (desktop app); keep min hit targets ≥ 32px for dense controls.

## 9. Motion
`view transition` 0.25s ease (opacity + 4px translateY) · `caret blink` 1s steps · `spinner` 0.8s linear · `shimmer` 1.4s linear. Respect `prefers-reduced-motion`.
