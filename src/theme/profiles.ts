// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Design-token data tables, ported verbatim from design-system-docs/DESIGN_TOKENS.md
// (§1 fonts/radii, §2 neutral ramps, §3 accent palettes, §4 status colours). Values are
// provisional per the V2 plan but structured so a palette swap is a data change *here*, never
// a component rewrite. Components never import these directly — they read the CSS custom
// properties that themeVars() (tokens.ts) computes from them.

export type System = "editorial" | "slate" | "terminal";
export type Mode = "dark" | "light";
export type Depth = "min" | "standard" | "power";
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
export const DEPTHS: readonly Depth[] = ["min", "standard", "power"];

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

// Picker palettes; index 0 is each System's default accent.
export const ACCENTS: Record<System, readonly string[]> = {
  editorial: ["#d2825b", "#c96f4c", "#cda44e", "#8f9a5b", "#c789a4", "#6f8bbf"],
  slate: ["#5b8cff", "#5bb5c0", "#9b8cf0", "#5fd6a0", "#e0a86a", "#ff93b4"],
  terminal: ["#9ece6a", "#e0af68", "#7dcfff", "#bb9af7", "#f7768e", "#7fe0b0"],
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
