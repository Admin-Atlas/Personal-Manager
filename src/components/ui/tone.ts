// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The ONE tone→recipe map (DESIGN_TOKENS.md §7). A "tone" is the semantic weight of a message —
// informational, cautionary, or something went wrong — and this file is the only place in `src/`
// that says what colour that is and how strongly it is mixed.
//
// Why a data map and not a component: three different primitives need the same recipe but render
// nothing alike — `Callout` is an inline container with a live region, `Button variant="danger"` is
// an interactive control, and a dialog's tone is chrome. A shared base component would couple an
// error strip to focus-trap semantics; a shared *map* couples nothing and still leaves exactly one
// line to edit when a ratio changes.
//
// The history this replaces: 45 callout-shaped blocks had hand-typed the danger tint in FIVE rival
// ratios (border 35/40/45/50% × background 12/15%) with the radius drifting alongside, because each
// instance owned a private copy. Style component TYPES, never instances — that rule is the whole
// point of this file.
//
// `color-mix` over `var(--…)` is the sanctioned technique, not a token violation: the mix is
// computed against whatever the four runtime axes resolved the token to, so a tint follows System,
// Mode, Accent and Depth for free. Never write a hex here.

import type { CSSProperties } from "react";

/** The semantic weight of a message. `success` is deliberately absent — `--st-quick` appears only
 *  as a badge in PM, never as a callout, and an unused tone is a speculative API. */
export type Tone = "info" | "warning" | "danger";

/** The token a tone's *surface* (background + border) is mixed from. */
export const TONE_TOKEN: Record<Tone, string> = {
  info: "--accent",
  warning: "--st-look",
  danger: "--st-due",
};

/** The token a tone's own *text* takes. Info diverges: `--accent` is the raw brand hue and can be
 *  too light to read as body text, while `--accent-text` is the contrast-corrected sibling
 *  `tokens.ts` derives for exactly this use. The status tokens are already text-calibrated. */
export const TONE_TEXT_TOKEN: Record<Tone, string> = {
  info: "--accent-text",
  warning: "--st-look",
  danger: "--st-due",
};

/**
 * The tint ratios, as percentages mixed into `transparent`. Four numbers, one home.
 *
 * `border`/`surface` at 40/12 is the median of the five danger recipes that were live in the tree,
 * so collapsing to it is the smallest possible visual move across ~45 sites (and it was already
 * exactly what the chat error strip used). `fill` is the stronger wash a *control* needs to read as
 * a button rather than a note, and `fillHover` is its hover step — a translucent tint barely moves
 * under `brightness-*`, so the danger button deepens the mix instead.
 */
export const TONE_MIX = {
  /** Border of a bordered callout. */
  border: 40,
  /** Background of a callout. */
  surface: 12,
  /** Background of a tinted control (`Button variant="danger"`). */
  fill: 15,
  /** Hover background of a tinted control. */
  fillHover: 24,
} as const;

/** The one place the `color-mix()` string itself is spelled. */
export function toneMix(token: string, percent: number): string {
  return `color-mix(in oklab, var(${token}) ${percent}%, transparent)`;
}

/**
 * The surface recipe for a tone: background, border colour, and (unless the caller is wrapping
 * neutral prose or controls) the text colour.
 *
 * Returned as inline style rather than utility classes on purpose. `cn()` is a plain joiner, not
 * tailwind-merge, so a colour utility emitted by a primitive and a colour utility passed by a call
 * site would BOTH survive and stylesheet order would silently pick the winner (#469). An inline
 * style has no such ambiguity, and it keeps the entire recipe in one object a test can assert on.
 */
export function toneSurface(tone: Tone, body: "tone" | "ink" = "tone"): CSSProperties {
  return {
    background: toneMix(TONE_TOKEN[tone], TONE_MIX.surface),
    borderColor: toneMix(TONE_TOKEN[tone], TONE_MIX.border),
    ...(body === "tone" ? { color: `var(${TONE_TEXT_TOKEN[tone]})` } : {}),
  };
}
