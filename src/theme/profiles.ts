// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Design-token data tables, ported verbatim from design-system-docs/DESIGN_TOKENS.md
// (§1 fonts/radii, §2 neutral ramps, §3 accent palettes, §4 status colours). Values are
// provisional per the V2 plan but structured so a palette swap is a data change *here*, never
// a component rewrite. Components never import these directly — they read the CSS custom
// properties that themeVars() (tokens.ts) computes from them.

export type System = "editorial" | "slate" | "terminal";
export type Mode = "dark" | "light";
/** What the user *picked* for Mode. Resolves to a concrete {@link Mode} at runtime:
 *  `light`/`dark` are explicit; `system` follows the OS light/dark setting; `auto` follows
 *  sunrise/sunset at the user's location (see resolveMode.ts). Only the resolved Mode ever
 *  reaches tokens/components — this preference is what's persisted and shown in Settings. */
export type ModePref = Mode | "system" | "auto";
export type Depth = "min" | "standard" | "power";
/** Control density / touch-target size (Accessibility). `standard` meets WCAG 2.5.8 (24px) and is the
 *  default; `comfortable` reaches the 44px AAA (2.5.5) target for lower motor precision. See
 *  {@link DENSITIES} and the density vars in tokens.ts.
 *
 *  A third level, `compact`, held PM's original tighter sizing so the accessibility epic wouldn't
 *  change an existing install's look under it. It is gone: it was a below-baseline default nobody
 *  chose on purpose, so the migration is simply to drop it — `oneOf` coerces a stored `compact` to
 *  `standard` on the next read (see ThemeContext). */
export type Density = "standard" | "comfortable";
/** Contrast level (Accessibility). `aa` is the default and lifts the lowest text tier to WCAG 1.4.3
 *  AA (4.5:1); `high` reaches AAA (7:1) for body text and firms up `faint` + the borders. Applied
 *  by boost() in tokens.ts. See {@link CONTRASTS}.
 *
 *  `legacy` (PM's original, softer ramp) is gone for the same reason `compact` is — see above. */
export type Contrast = "aa" | "high";
export type Role =
  | "bg"
  | "panel"
  | "surface"
  | "border"
  | "border2"
  | "rule"
  | "ink"
  | "ink2"
  | "ink3"
  | "ink4"
  /** DECORATIVE/DISABLED ONLY — separators, placeholder glyphs, `disabled:` control colour. It is
   *  the one role `aa` does not lift (see CONTRAST_SHIFT in tokens.ts), so it renders as low as
   *  1.67:1: the TEXT ramp is `ink`→`ink4`, and `designGuards.test.ts` keeps it that way. */
  | "faint";
export type StatusKey = "due" | "blocked" | "quick" | "look" | "part" | "track";

export const SYSTEMS: readonly System[] = ["editorial", "slate", "terminal"];
export const MODES: readonly Mode[] = ["dark", "light"];
// The four Mode *preferences* offered in Settings (see {@link ModePref}). Order is the picker order.
export const MODE_PREFS: readonly ModePref[] = ["light", "dark", "system", "auto"];
export const DEPTHS: readonly Depth[] = ["min", "standard", "power"];
export const DENSITIES: readonly Density[] = ["standard", "comfortable"];
export const CONTRASTS: readonly Contrast[] = ["aa", "high"];

// Order is load-bearing: themeVars maps these positionally onto each ramp / status row.
export const ROLES: readonly Role[] = [
  "bg",
  "panel",
  "surface",
  "border",
  "border2",
  "rule",
  "ink",
  "ink2",
  "ink3",
  "ink4",
  "faint",
];
export const STATUS_KEYS: readonly StatusKey[] = [
  "due",
  "blocked",
  "quick",
  "look",
  "part",
  "track",
];

export interface Fonts {
  head: string;
  ui: string;
  mono: string;
}

export const FONTS: Record<System, Fonts> = {
  editorial: {
    head: "'Newsreader',Georgia,serif",
    ui: "'Hanken Grotesk',system-ui,sans-serif",
    mono: "'JetBrains Mono',monospace",
  },
  slate: {
    head: "'Hanken Grotesk',system-ui,sans-serif",
    ui: "'Hanken Grotesk',system-ui,sans-serif",
    mono: "'JetBrains Mono',monospace",
  },
  terminal: {
    head: "'JetBrains Mono',monospace",
    ui: "'JetBrains Mono',monospace",
    mono: "'JetBrains Mono',monospace",
  },
};

// [radius, radiusSm]
export const RADII: Record<System, readonly [string, string]> = {
  editorial: ["12px", "9px"],
  slate: ["10px", "8px"],
  terminal: ["2px", "2px"],
};

// Per role: [L, C] for oklch(L C H). H comes from the active accent's OKLab hue at runtime,
// which is why the whole neutral ramp warms/cools with the accent.
type Ramp = Record<Role, readonly [number, number]>;

export const PROFILES: Record<System, Record<Mode, Ramp>> = {
  editorial: {
    dark: {
      bg: [0.165, 0.016],
      panel: [0.145, 0.016],
      surface: [0.205, 0.018],
      border: [0.265, 0.016],
      border2: [0.315, 0.016],
      rule: [0.235, 0.012],
      ink: [0.915, 0.018],
      ink2: [0.845, 0.02],
      ink3: [0.685, 0.018],
      ink4: [0.575, 0.016],
      faint: [0.485, 0.014],
    },
    light: {
      bg: [0.985, 0.008],
      panel: [0.962, 0.01],
      surface: [0.944, 0.012],
      border: [0.884, 0.013],
      border2: [0.82, 0.015],
      rule: [0.918, 0.01],
      ink: [0.29, 0.03],
      ink2: [0.405, 0.026],
      ink3: [0.52, 0.022],
      ink4: [0.6, 0.018],
      faint: [0.7, 0.014],
    },
  },
  slate: {
    dark: {
      bg: [0.155, 0.013],
      panel: [0.135, 0.013],
      surface: [0.205, 0.015],
      border: [0.255, 0.013],
      border2: [0.305, 0.015],
      rule: [0.225, 0.011],
      ink: [0.925, 0.013],
      ink2: [0.835, 0.015],
      ink3: [0.665, 0.015],
      ink4: [0.565, 0.013],
      faint: [0.455, 0.011],
    },
    light: {
      bg: [0.992, 0.004],
      panel: [0.974, 0.005],
      surface: [0.962, 0.006],
      border: [0.902, 0.008],
      border2: [0.845, 0.01],
      rule: [0.935, 0.006],
      ink: [0.265, 0.018],
      ink2: [0.385, 0.016],
      ink3: [0.51, 0.015],
      ink4: [0.6, 0.012],
      faint: [0.71, 0.009],
    },
  },
  terminal: {
    dark: {
      bg: [0.135, 0.007],
      panel: [0.115, 0.007],
      surface: [0.165, 0.007],
      border: [0.235, 0.007],
      border2: [0.285, 0.009],
      rule: [0.205, 0.006],
      ink: [0.865, 0.011],
      ink2: [0.795, 0.011],
      ink3: [0.585, 0.009],
      ink4: [0.495, 0.008],
      faint: [0.345, 0.007],
    },
    light: {
      bg: [0.967, 0.01],
      panel: [0.945, 0.011],
      surface: [0.934, 0.011],
      border: [0.86, 0.013],
      border2: [0.805, 0.013],
      rule: [0.905, 0.008],
      ink: [0.3, 0.018],
      ink2: [0.395, 0.016],
      ink3: [0.52, 0.013],
      ink4: [0.6, 0.011],
      faint: [0.69, 0.008],
    },
  },
};

// The sentinel "accent" that selects the monochrome (Eigengrau) treatment instead of a hue.
// It is NOT a colour — tokens.ts special-cases it to a chroma-0 neutral ramp with white
// text/accents (dark) or near-black (light). Offered only in Slate (see ACCENTS below), and
// the app's default there. Its base dark background is Eigengrau, the perceptual "colour of
// darkness". Feature colours (the map palette, semantic status) are unaffected and stay in colour.
export const MONO_ACCENT = "mono";
/** Eigengrau — HEX #16161D / RGB (22,22,29). The exact base background for the dark monochrome
 *  theme; the rest of that ramp is a straight neutral greyscale up to white (no accent tint). */
export const EIGENGRAU = "#16161d";

// Picker palettes; index 0 is each System's default accent. Slate leads with the monochrome
// sentinel, so a fresh Slate install is Eigengrau; the coloured hues remain selectable after it.
export const ACCENTS: Record<System, readonly string[]> = {
  editorial: ["#d2825b", "#c96f4c", "#cda44e", "#8f9a5b", "#c789a4", "#6f8bbf"],
  slate: [MONO_ACCENT, "#5b8cff", "#5bb5c0", "#9b8cf0", "#5fd6a0", "#e0a86a", "#ff93b4"],
  terminal: ["#9ece6a", "#e0af68", "#7dcfff", "#bb9af7", "#f7768e", "#7fe0b0"],
};

/** Human names for the accent swatches, shown on hover so every colour is legible — not just the
 *  monochrome sentinel. Keyed by the same hex/sentinel used in {@link ACCENTS}; theme-neutral colour
 *  words. Data, not code, so a palette tweak is a one-line change. {@link accentName} falls back to
 *  the raw value for anything unlisted. */
export const ACCENT_NAMES: Record<string, string> = {
  [MONO_ACCENT]: "Monochrome (Eigengrau)",
  // editorial
  "#d2825b": "Terracotta",
  "#c96f4c": "Sienna",
  "#cda44e": "Ochre",
  "#8f9a5b": "Olive",
  "#c789a4": "Mauve",
  "#6f8bbf": "Slate blue",
  // slate
  "#5b8cff": "Blue",
  "#5bb5c0": "Teal",
  "#9b8cf0": "Lavender",
  "#5fd6a0": "Mint",
  "#e0a86a": "Amber",
  "#ff93b4": "Rose",
  // terminal
  "#9ece6a": "Lime",
  "#e0af68": "Gold",
  "#7dcfff": "Sky",
  "#bb9af7": "Violet",
  "#f7768e": "Coral",
  "#7fe0b0": "Aquamarine",
};

/** The display name for an accent value (a hex or the mono sentinel), falling back to the value. */
export function accentName(accent: string): string {
  return ACCENT_NAMES[accent] ?? accent;
}

// The monochrome ramp for the {@link MONO_ACCENT} treatment: per-role Lightness only, rendered at
// chroma 0 (pure neutral — no accent hue fans through it). tokens.ts pins --bg to the exact
// Eigengrau hex in dark; every other role is a straight grey. Dark = Eigengrau base + white ink;
// light = paper base + near-black ink. Kept here so a palette tweak is a data change, never code.
export const MONO_RAMP: Record<Mode, Record<Role, number>> = {
  dark: {
    bg: 0.168, // pinned to EIGENGRAU at apply time; this L is the greyscale sibling for the ramp
    panel: 0.138,
    surface: 0.222,
    border: 0.3,
    border2: 0.36,
    rule: 0.258,
    ink: 0.94, // soft off-white (~#ECECEC), NOT pure white — avoids halation/glare on the dark bg
    ink2: 0.86,
    ink3: 0.66,
    ink4: 0.555,
    faint: 0.44,
  },
  light: {
    bg: 0.992,
    panel: 0.968,
    surface: 0.951,
    border: 0.884,
    border2: 0.82,
    rule: 0.916,
    ink: 0.2, // near-black text
    ink2: 0.34,
    ink3: 0.5,
    ink4: 0.6,
    faint: 0.72,
  },
};

// Semantic status colours (NOT accent-tied). Order matches STATUS_KEYS.
//
// The light rows are CALIBRATED, not eyeballed. Every value clears WCAG 1.4.3 AA (4.5:1) *as text*
// against the worst background it can land on, and is built to 4.6:1 so hex quantisation or a later
// ramp nudge cannot silently re-break it. The worst background is `surface` — the DARKEST of
// bg/panel/surface in light mode, so the one that gives dark text the least to work against — taken
// under whichever accent hue drives that System's surface lowest. Not `bg`: bg is the *lightest*
// surface and therefore the most forgiving. contrast.test.ts measures every accent, so the choice is
// pinned rather than trusted.
//
// Why this cannot be left to the contrast axis: boost() is applied to the neutral ramp only, and
// themeVars emits --st-* verbatim, so `aa` and `high` produce byte-identical status colours. High
// contrast rescues nothing here — a status colour that fails is failing at every setting the app
// offers, and these render at text-xs so the 3:1 large-text exemption never applies either.
//
// Deepening holds each colour's OKLab HUE (max drift across the whole table is 0.9°) and holds its
// chroma except where sRGB clips it. The second, competing constraint is that the six stay
// distinguishable from EACH OTHER: a row pushed uniformly toward one lightness would pass AA and
// destroy the taxonomy, which is the worse outcome. So each row's minimum pairwise OKLab ΔE is held
// at or above where it started — editorial 0.0727 → 0.0743, terminal 0.1095 unchanged. `part` is the
// visible consequence of that rule: deepening it at constant chroma would have made editorial's
// already-tightest pair (part/track) 14% tighter, so it gains chroma (0.048 → 0.060) instead.
//
// The one exception, measured and accepted: slate's due/blocked narrows 0.0793 → 0.0756 (−4.7%),
// because `due` had to drop 0.040 L to reach AA while `blocked` needed only 0.017, converging their
// lightness. Buying it back meant deepening `blocked` a further 0.025 L — twice the move AA asked
// for — to gain 0.0037 ΔE between two colours already 23° apart in hue at chroma 0.18+. Declined:
// the cost is a visible colour change, the gain is below any perceptual threshold.
//
// Dark rows are deliberately untouched: their worst case is already 5.20:1.
export const STATUS: Record<System, Record<Mode, readonly string[]>> = {
  editorial: {
    dark: ["#e0856a", "#c789a4", "#9aab66", "#d2a24e", "#7fa3a0", "#9a8f80"],
    light: ["#b2472c", "#a14e74", "#626f2c", "#8f6000", "#3a726e", "#6f6457"],
  },
  slate: {
    dark: ["#ff8088", "#ff93b4", "#5fd6a0", "#ffc266", "#79c0ff", "#9aa0ad"],
    light: ["#ca2a3f", "#be3473", "#007b4d", "#965f01", "#2d6db3", "#5f6470"],
  },
  terminal: {
    dark: ["#f7768e", "#bb9af7", "#9ece6a", "#e0af68", "#7dcfff", "#82867f"],
    light: ["#bc344d", "#7950be", "#36750e", "#8d5e03", "#2a6a9a", "#5a5e57"],
  },
};

// Colour-blind-safe semantic status colours (Okabe–Ito-derived), swapped in by themeVars when the
// colour-blind axis is on. One set per Mode — System-independent, because CVD distinctness is
// universal — with order matching STATUS_KEYS (due, blocked, quick, look, part, track). The classic
// red/green confusion (due vs quick) is broken by pairing vermillion-orange with bluish-green. Not
// accent-tied, like STATUS above.
//
// The light row follows the same AA calibration as STATUS.light, with one extra move that needs its
// reason recorded. Being System-independent, it is measured against the worst surface across ALL
// three Systems. `due` sits at hue 42.5°, where sRGB runs out of chroma before 4.6:1 is reachable
// above L 0.535 — so deepening it to pass AA necessarily walks it toward `look`, and that pair is
// the tightest in the row *and* the orange-vs-amber pair CVD users are most likely to confuse.
// Leaving it there cost 17% of that gap (ΔE 0.1081 → 0.0897). `look` is therefore deepened too
// (0.508 → 0.471 L, same hue) even though it already passed at 4.86:1, purely to hold the gap: the
// row's whole purpose is distinguishability, so trading it away inside a contrast fix would be a
// regression on the one axis this table exists to serve. Its chroma was already at the sRGB cusp
// (zero headroom at L 0.508), so lightness was the only lever. Net: ΔE 0.1081 → 0.1090.
export const STATUS_CVD: Record<Mode, readonly string[]> = {
  dark: ["#ef8a5c", "#e58fc4", "#3fc99b", "#eab44e", "#63abe6", "#a6adba"],
  light: ["#b44300", "#a1487e", "#007653", "#7c5100", "#186ab0", "#5f6470"],
};
