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

/** The categorical source hues for a System: its accent picker palette minus the `accent` currently
 *  reserved for chrome (~5 hues). The monochrome sentinel is never a source hue (it's not a colour),
 *  so it's always dropped too. If the active accent isn't one of the picker hues (a custom accent),
 *  the full palette stands — there's nothing to reserve-and-remove. */
export function sourcePalette(system: System, accent: string): string[] {
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
): Map<string, string> {
  const palette = sourcePalette(system, accent);
  const ordered = [...new Set(calendarIds)].sort();
  const map = new Map<string, string>();
  ordered.forEach((id, i) => map.set(id, palette[i % palette.length]));
  return map;
}
