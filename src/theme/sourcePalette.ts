// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// DOCUMENTED EXCEPTION to the "no hex literals" rule — the sibling of graphPalette.ts. The unified
// calendar view (card 8) colours each source (calendar) categorically: one distinct hue per
// calendar, which the single-accent token system (one accent + tinted neutrals) cannot express.
// Rather than a fresh hardcoded set, the source hues are the active System's *accent picker palette*
// (ACCENTS[system]) with the active accent removed — the accent stays reserved for chrome (today /
// now / active view) so a source is never confusable with "now", and the whole set re-tints
// coherently when the System changes. Everything else around an event (card, border, rules, tints)
// still uses tokens.

import { ACCENTS, MONO_ACCENT, type System } from "./profiles";

// Colour-blind-safe (Okabe–Ito) source hues, used when the colour-blind axis is on in place of the
// accent-derived set. System/accent-independent — under the axis, CVD distinctness matters more than
// re-tinting with the accent. Six mutually distinguishable chromatic hues that read on both the light
// and dark --bg (Okabe–Ito's yellow is dropped, being invisible in light mode); the milestone and
// pinboard overlays keep their own distinct hues (they're single, labelled overlays, not sources).
const CVD_SOURCES: readonly string[] = [
  "#56b4e9",
  "#e69f00",
  "#009e73",
  "#0072b2",
  "#d55e00",
  "#cc79a7",
];

/** The categorical source hues for a System: its accent picker palette minus the `accent` currently
 *  reserved for chrome (~5 hues). The monochrome sentinel is never a source hue (it's not a colour),
 *  so it's always dropped too. If the active accent isn't one of the picker hues (a custom accent),
 *  the full palette stands — there's nothing to reserve-and-remove. When the colour-blind axis is on,
 *  the CVD-safe set above replaces all of it. */
export function sourcePalette(system: System, accent: string, colorblind = false): string[] {
  if (colorblind) return [...CVD_SOURCES];
  const active = accent.trim().toLowerCase();
  const hues = ACCENTS[system].filter((h) => h !== MONO_ACCENT);
  const rest = hues.filter((h) => h.toLowerCase() !== active);
  return rest.length > 0 ? rest : hues;
}

/** A stable, collision-free colour for each calendar id: sort the ids deterministically and walk the
 *  palette (`palette[i % len]`), so with the usual handful of calendars every source gets a distinct
 *  hue. Assignment is a pure function of the *set* of ids — it only shifts when a calendar is
 *  connected/removed, never on a list re-sort or render — and every surface that builds the map from
 *  the same calendar list (the view, and later the focus agenda) resolves the same calendar to the
 *  same hue. Switching System keeps each calendar's slot; only the hue it resolves to changes. */
export function sourceColors(
  calendarIds: string[],
  system: System,
  accent: string,
  colorblind = false,
): Map<string, string> {
  const palette = sourcePalette(system, accent, colorblind);
  const ordered = [...new Set(calendarIds)].sort();
  const map = new Map<string, string>();
  ordered.forEach((id, i) => map.set(id, palette[i % palette.length]));
  return map;
}

/** The stable slot index for each calendar id — the SAME sorted-slot assignment {@link sourceColors}
 *  uses, exposed so a surface can pick a redundant SHAPE per source that tracks its colour (the
 *  colour-blind axis, so a source is distinguishable without relying on hue). Pure function of the id
 *  SET, like sourceColors, so the shape only shifts when a calendar is connected/removed. Overlays
 *  (milestones/pinboard) aren't calendars, so they're absent here and fall back to the plain circle. */
export function sourceShapeIndex(calendarIds: string[]): Map<string, number> {
  const ordered = [...new Set(calendarIds)].sort();
  const map = new Map<string, number>();
  ordered.forEach((id, i) => map.set(id, i));
  return map;
}

/** The distinct hue for project-milestone events on the calendar (card 7 overlay) — one per System,
 *  chosen from OUTSIDE that System's accent picker (`ACCENTS`) so it can never land on a source slot
 *  and never reads as the reserved "today/now" accent. A single mid-tone hue (like the source hues),
 *  which carries in both light and dark via the same `color-mix` tints the event parts already apply.
 *  DOCUMENTED hex exception, like the source palette above. */
const MILESTONE_HUES: Record<System, string> = {
  editorial: "#3f8f86", // deep teal — cool against the warm earth-tone sources
  slate: "#b8559e", // magenta — distinct from the blue/teal/lavender/rose picker
  terminal: "#ff9e64", // orange — distinct from the green/violet/pink picker
};

export function milestoneColor(system: System): string {
  return MILESTONE_HUES[system];
}

/** The distinct hue for the pinboard overlay (freeform timeline entries) — same rules as
 *  {@link MILESTONE_HUES}: outside the System's accent picker so it can never take a source slot or
 *  read as "today/now", and distinct from the milestone hue so the two overlays never blur together.
 *  DOCUMENTED hex exception, like the source palette above. */
const PINBOARD_HUES: Record<System, string> = {
  editorial: "#7a5aa8", // violet — off the warm earth-tone sources and the teal milestones
  slate: "#d1603d", // rust — the one warm hue among slate's cool picker + magenta milestones
  terminal: "#7aa2f7", // blue — deeper than the picker's cyan, clear of the orange milestones
};

export function pinboardColor(system: System): string {
  return PINBOARD_HUES[system];
}
