# Building with the PM design system

PM is a local-first personal-manager desktop app. These are its **token-driven React primitives**.
The look is driven by CSS custom properties (OKLCH neutrals tinted by an accent hue) exposed as
Tailwind v4 utilities. Build screens by composing these primitives and using the token utilities
below for your own layout glue — never hardcode hex colors or px font sizes.

## Required: wrap the app in `ThemeProvider`

Every primitive reads theme from context — `Button`, `Input`, `Textarea`, `Select`,
`SegmentedControl`, `StatusBadge`, `Collapsible`, `NavItem`, `TitleBar` call `useTheme()` and
**throw `"useTheme must be used within <ThemeProvider>"` if it's missing.** Wrap the whole tree once:

```jsx
import { ThemeProvider } from "pm";          // exported from the bundle
import { Button, Card, ListRow, StatusBadge } from "pm";

function App() {
  return (
    <ThemeProvider>
      <Card style={{ padding: 16, maxWidth: 420 }}>
        <h2 className="font-head text-ink" style={{ fontSize: 18 }}>Q3 board deck</h2>
        <p className="font-ui text-ink3" style={{ fontSize: 13 }}>12 documents · due Tuesday</p>
        <ListRow title="Revenue model.xlsx" meta="updated 2h ago"
                 trailing={<StatusBadge status="quick_win" />} />
        <div className="border-t border-rule" style={{ marginTop: 12, paddingTop: 12 }}>
          <Button variant="primary">Open project</Button>
        </div>
      </Card>
    </ThemeProvider>
  );
}
```

`ThemeProvider` writes the active theme's tokens onto `<html>` on mount. The default is
**editorial · dark · orange accent**. It takes no required props.

## The styling idiom — token utilities (Tailwind v4)

Style your own elements with these utility classes; their values are CSS variables that flip when
the theme changes, so they stay on-brand. **Never use raw Tailwind palette colors** (`bg-gray-800`,
`text-blue-500`) or hex — only these token utilities:

| Family | Classes |
|---|---|
| Backgrounds | `bg-bg` `bg-panel` `bg-surface` `bg-accent` `bg-accent-soft` |
| Text | `text-ink` `text-ink2` `text-ink3` `text-ink4` `text-faint` `text-accent-text` `text-accent-ink` |
| Borders | `border-border` `border-border2` `border-rule` `border-accent` |
| Fonts | `font-head` (display) `font-ui` (body/controls) `font-mono` (numbers, ids, meta) |
| Radius | `rounded-[var(--radius)]` `rounded-[var(--radius-sm)]` |

Status colors are **semantic** (not accent-tied). There are no `text-st-*` utility classes — render
status with the `StatusBadge` component, or apply the raw variable inline:
`style={{ color: "var(--st-blocked)" }}`. Variables: `--st-due` `--st-blocked` `--st-quick`
`--st-look` `--st-part` `--st-track`.

Role intent: `bg` = window · `panel` = titlebar/sidebar · `surface` = cards/raised · `border` =
hairline · `border2` = control outline · `rule` = faint row divider · `ink`→`faint` = text primary
to faintest. Accent rule of thumb: fill/dot/border → `bg-accent`/`border-accent`; colored text →
`text-accent-text`; label on a filled accent → `text-accent-ink`; subtle tint → `bg-accent-soft`.

## Where the truth lives

- **`styles.css`** (and the `_ds_bundle.css` it imports) — the compiled token utilities, `:root`
  defaults, `@font-face`, and keyframes. Read it before inventing class names.
- **`components/<group>/<Name>/<Name>.prompt.md`** — per-component API + usage for each primitive.
- **`guidelines/`** — `DESIGN_TOKENS.md` (the full token tables, OKLCH math, per-System rules) and
  the design-language handoff README.

## Notes

- `Modal` and `ConfirmDialog` are fixed-position overlays over a scrim; render them at app root with
  `open` toggled. `ConfirmDialog` is built on `Modal` — use it to gate irreversible actions
  (`danger` tints the confirm; `busy` blocks dismissal).
- Three runtime "Systems" (`editorial` · `slate` · `terminal`) restyle everything; primitives branch
  internally (e.g. terminal wraps button labels in `[ … ]` and forces mono). You don't manage this —
  `ThemeProvider` does.
