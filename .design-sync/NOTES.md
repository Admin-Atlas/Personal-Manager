# design-sync notes — PM Design System

Repo-specific gotchas for syncing `src/components/ui/` to claude.ai/design. Append a bullet
whenever a sync teaches something new.

## Shape & entry
- **Package shape, synth-entry mode.** This repo is a Tauri **app**, not a published component
  library — `dist/` is the app bundle (one hashed `index.js` + `index.css`), and `package.json`
  has no `exports`/`module`/`main`. The design system is the 16 primitives barrel-exported from
  `src/components/ui/index.ts`. The converter bundles JS straight from that source entry
  (`--entry ./src/components/ui/index.ts`), not from a dist lib.
- `cn` is a utility (className merge), not a component — excluded via `componentSrcMap: {"cn": null}`.

## Provider — required
- Components call `useTheme()` (e.g. `Button` reads `system` to branch the terminal look), so every
  preview must be wrapped in a provider that supplies `ThemeProvider` (`src/theme/ThemeContext.tsx`).
- **`cfg.provider` is `PreviewSurface`** (`.design-sync/preview-provider.tsx`), NOT `ThemeProvider`
  directly. The primitives use light-on-dark text designed for the app's dark canvas, but the
  upstream preview-card body is hardcoded white (`emit.mjs` — a contract file, do not fork), so plain
  `ThemeProvider` previews render light-on-white (unreadable). `PreviewSurface` = `ThemeProvider` +
  a `var(--bg)` dark surface, so cards look like the app. It's wired through
  `extraEntries: ["./src/theme/index.ts", "./.design-sync/preview-provider.tsx"]`. `ThemeProvider`
  itself stays a bundle export (first extraEntries entry) because the conventions header tells the
  design agent to import it from `'pm'`.
- `ThemeProvider` is browser-safe: `@tauri-apps/api` import is side-effect-free, and its `getPref`/
  `setPref` IPC calls run inside async effects with `.catch`, so they no-op outside Tauri.
- On mount, `applyTheme(document.documentElement, …)` writes the OKLCH token custom-properties onto
  `<html>` and stamps `data-system`/`data-mode`/`data-depth`. Default theme = editorial · dark ·
  orange accent (#d2825b, H=46.3).

## Styling / CSS
- Tailwind v4. Utilities like `bg-surface`, `text-ink2`, `font-mono` are declared `@theme inline`
  in `src/index.css`, so they compile to `var(--token)` refs that flip live when `applyTheme`
  rewrites the custom properties. The `:root` block in `src/index.css` provides themed first paint.
- `cfg.cssEntry` points at the **compiled** app CSS (`dist/assets/index-<hash>.css`) — only the
  compiled output contains the real utility rules + `:root` defaults + `@font-face` + keyframes.
- **Build with a relative base** (`buildCmd = "tsc && npx vite build --base=./"`). Vite's default
  absolute base emits `url(/assets/x.woff2)` `@font-face` paths the converter can't resolve relative
  to the stylesheet → all fonts dangle ([FONT_DANGLING]) and only 1/29 copies. `--base=./` makes them
  `url(./x.woff2)`, resolvable from `dist/assets/`, so all woff2 copy into `fonts/`.

## Guidelines — exclude internal docs
- The default `guidelinesGlob` pulls `docs/*.md`, but this repo's `docs/` holds **internal,
  never-published** files (DECISIONS.md, PM_Project_Spec.md, board-ops-runbook.md, PUBLISH.md).
  `guidelinesGlob` is pinned to `["design-system-docs/DESIGN_TOKENS.md"]` — the pure, PII-free token
  reference. The sibling `README.md` is deliberately excluded: it carries placeholder names ("Bobby",
  "Atlas") and a sample email, and the owner prefers those not ship. Never ship `docs/` either.

## Fonts
- Self-hosted via `@fontsource` (Newsreader = `--head`, Hanken Grotesk = `--ui`, JetBrains Mono =
  `--mono`), imported as side-effects in `src/theme/fonts.ts`. Vite fingerprints the woff2 into
  `dist/assets/`. If validate prints `[FONT_MISSING]`/`[FONT_DANGLING]`, point `cfg.extraFonts` at
  the `@fontsource/*/{weight}.css` files instead (stable, non-hashed url()s).

## Previews
- 15 of 16 primitives have authored previews in `.design-sync/previews/`. Each export is a
  zero-arg PascalCase component (the harness does `createElement(Export)` with no props) importing
  from `'pm'`.
- **TitleBar ships the floor card on purpose.** It calls `getCurrentWindow()` from
  `@tauri-apps/api/window` inside its effect, unguarded — outside Tauri that throws (no
  `__TAURI_INTERNALS__`), blanking any render. It's also app window-chrome, not a compositional
  primitive. Authoring a real preview would need a Tauri shim; left as floor card.
- `Modal` and `ConfirmDialog` are fixed-position overlays → `cfg.overrides` gives them
  `cardMode: single` + a `600x420` viewport so the open dialog renders inside the card.
- Render check was skipped this run (`--no-render-check`; no Chromium) — previews verified by the
  user's `.review.html` eyeball, not machine screenshots. No automated grades exist in `.cache/`.

## Re-sync risks
- **`cssEntry` is a hashed path.** `dist/assets/index-<hash>.css` changes every app build. On
  re-sync: run `cfg.buildCmd` (`npm run build`), then re-glob `dist/assets/*.css` and update
  `cfg.cssEntry` before running the converter.
- The committed `dist/` can be stale relative to `src/` (package.json version lagged HEAD at first
  sync). Always rebuild before syncing so the CSS matches the current primitives.
