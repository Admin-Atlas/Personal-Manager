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
/** Control density / touch-target size (Accessibility). `compact` is today's tight sizing (pinned
 *  for existing installs); `standard` meets WCAG 2.5.8 (24px) and is the fresh-install default;
 *  `comfortable` reaches the 44px AAA (2.5.5) target for lower motor precision. See {@link DENSITIES}
 *  and the density vars in tokens.ts. */
export type Density = "compact" | "standard" | "comfortable";
/** Contrast level (Accessibility). `legacy` is today's ramps (pinned for existing installs); `aa`
 *  is the fresh-install default and lifts the lowest text tier to WCAG 1.4.3 AA (4.5:1); `high`
 *  reaches AAA (7:1) for body text and firms up the faint text + borders. Applied by boost() in
 *  tokens.ts. See {@link CONTRASTS}. */
export type Contrast = "legacy" | "aa" | "high";
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
  | "faint";
export type StatusKey = "due" | "blocked" | "quick" | "look" | "part" | "track";

export const SYSTEMS: readonly System[] = ["editorial", "slate", "terminal"];
export const MODES: readonly Mode[] = ["dark", "light"];
// The four Mode *preferences* offered in Settings (see {@link ModePref}). Order is the picker order.
export const MODE_PREFS: readonly ModePref[] = ["light", "dark", "system", "auto"];
export const DEPTHS: readonly Depth[] = ["min", "standard", "power"];
export const DENSITIES: readonly Density[] = ["compact", "standard", "comfortable"];
export const CONTRASTS: readonly Contrast[] = ["legacy", "aa", "high"];

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

// Semantic status colours (NOT accent-tied). Order matches STATUS_KEYS; light is deepened for
// contrast on near-white.
export const STATUS: Record<System, Record<Mode, readonly string[]>> = {
  editorial: {
    dark: ["#e0856a", "#c789a4", "#9aab66", "#d2a24e", "#7fa3a0", "#9a8f80"],
    light: ["#c2553a", "#a8547a", "#6f7d3a", "#b07d2a", "#4f7a76", "#6f6457"],
  },
  slate: {
    dark: ["#ff8088", "#ff93b4", "#5fd6a0", "#ffc266", "#79c0ff", "#9aa0ad"],
    light: ["#d83a4a", "#c43a78", "#1f8a5b", "#b5781f", "#2f6fb5", "#5f6470"],
  },
  terminal: {
    dark: ["#f7768e", "#bb9af7", "#9ece6a", "#e0af68", "#7dcfff", "#82867f"],
    light: ["#c23a52", "#7a52c0", "#4a8a2a", "#9a6a1a", "#2a6a9a", "#5a5e57"],
  },
};

// Colour-blind-safe semantic status colours (Okabe–Ito-derived), swapped in by themeVars when the
// colour-blind axis is on. One set per Mode — System-independent, because CVD distinctness is
// universal — with order matching STATUS_KEYS (due, blocked, quick, look, part, track). The classic
// red/green confusion (due vs quick) is broken by pairing vermillion-orange with bluish-green, and
// each entry is tuned to read as text on the mode's --bg. Not accent-tied, like STATUS above.
export const STATUS_CVD: Record<Mode, readonly string[]> = {
  dark: ["#ef8a5c", "#e58fc4", "#3fc99b", "#eab44e", "#63abe6", "#a6adba"],
  light: ["#c24e12", "#a2497f", "#0e7c58", "#8a5a00", "#1f6fb5", "#5f6470"],
};
