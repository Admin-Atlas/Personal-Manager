// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

/** The Pinboard's note-tint palette — the single source of truth for WHICH design-token
 *  colours are offered as note/widget tints, in what order, and under what names. The colour
 *  VALUES themselves live once as the global `--st-*` design tokens in `src/index.css` (they
 *  are theme-adaptive, so we never hard-code hex); this module only fixes the *offered set*,
 *  its order, and the human labels, so the tint colours stay consistent everywhere on the
 *  board (swatches, tooltips, default new-note colour). Add or reorder a tint here and every
 *  consumer follows. */

export interface TintOption {
  /** A design-token name (`st-quick` …) → the CSS custom property `--st-quick`. Never hex. */
  token: string;
  /** The colour name shown in the swatch tooltip / aria-label. */
  name: string;
}

/** The ordered tint options (this is also the left-to-right swatch order in a note footer). */
export const TINT_PALETTE: readonly TintOption[] = [
  { token: "st-quick", name: "Sage" },
  { token: "st-due", name: "Coral" },
  { token: "st-look", name: "Amber" },
  { token: "st-track", name: "Stone" },
  { token: "st-part", name: "Teal" },
];

/** The tint token names, in palette order — the set of swatches a note offers. */
export const NOTE_COLORS: readonly string[] = TINT_PALETTE.map((t) => t.token);

/** token → display name, for a swatch's tooltip and aria-label. */
export const TINT_NAME: Record<string, string> = Object.fromEntries(
  TINT_PALETTE.map((t) => [t.token, t.name]),
);

/** The default tint applied to a freshly-added note (first in the palette). */
export const DEFAULT_TINT = TINT_PALETTE[0].token;
