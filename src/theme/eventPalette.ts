// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// DOCUMENTED EXCEPTION to the "no hex literals" rule — a sibling of sourcePalette.ts and
// graphPalette.ts. Google Calendar lets you give one event its own colour, from a fixed set of
// eleven ("Lavender", "Tomato" …), and the event keeps it as a `colorId` "1"–"11" (#884, F4). PM
// fills the event with that colour, solid, as Google does, and keeps the calendar's own colour for
// the stripe down its edge. These are colours the user chose by name in Google, not PM chrome, so
// they're the same in every System and Mode, and they don't follow the colour-blind axis: the name is
// what carries the meaning there (the event's details say "Colour: Lavender").
//
// Solid, not the faint tint a calendar's colour gets (14–22%): at that strength an event's own colour
// barely changed the card (Lavender on a blue calendar was a shift too small to see), and no single
// stronger tint kept text readable on every colour in every look. A solid colour is the same
// everywhere, so each one carries the text colour that reads on it (`ink`, 4.5:1 or better), as
// Google draws dark text on its light colours and white on its dark ones.
//
// The names are Google's own (the Apps Script `EventColor` reference lists ids 1–11 under them). The
// shades are the ones Google Calendar on the web draws; Google publishes no official values for them
// (its Colors API returns an older, paler set), so they were matched by eye.

import type { CSSProperties } from "react";

export interface EventColour {
  /** Google's name for it, as its colour picker says it. */
  name: string;
  hex: string;
  /** The text colour that reads on it: 4.5:1 or better (eventPalette.test.ts). */
  ink: string;
}

const DARK = "#1f1f1f";
const WHITE = "#ffffff";

const EVENT_COLOURS: Readonly<Record<string, EventColour>> = Object.freeze({
  "1": { name: "Lavender", hex: "#7986cb", ink: DARK },
  "2": { name: "Sage", hex: "#33b679", ink: DARK },
  "3": { name: "Grape", hex: "#8e24aa", ink: WHITE },
  "4": { name: "Flamingo", hex: "#e67c73", ink: DARK },
  "5": { name: "Banana", hex: "#f6bf26", ink: DARK },
  "6": { name: "Tangerine", hex: "#f4511e", ink: DARK },
  "7": { name: "Peacock", hex: "#039be5", ink: DARK },
  "8": { name: "Graphite", hex: "#616161", ink: WHITE },
  "9": { name: "Blueberry", hex: "#3f51b5", ink: WHITE },
  "10": { name: "Basil", hex: "#0b8043", ink: WHITE },
  "11": { name: "Tomato", hex: "#d50000", ink: WHITE },
});

/** The colour an event was given in Google, or `null` when it has none of its own (it shows in its
 *  calendar's colour) or an id PM doesn't know. */
export function eventColour(colorId: string | null | undefined): EventColour | null {
  if (!colorId) return null;
  return Object.prototype.hasOwnProperty.call(EVENT_COLOURS, colorId)
    ? EVENT_COLOURS[colorId]
    : null;
}

/** An event's background: its own colour, solid, when it has one; else its calendar's `calendarColour`
 *  at the surface's usual `calendarPct`, mixed into `base` (`var(--surface)` for an opaque card,
 *  `transparent` for a tint over the grid). A surface that doesn't tint an event without its own
 *  colour passes `calendarPct` null, and gets no background for it. */
export function eventFill(
  own: EventColour | null,
  calendarColour: string,
  calendarPct: number | null,
  base = "transparent",
): string | undefined {
  if (own) return own.hex;
  if (calendarPct === null) return undefined;
  return `color-mix(in oklab, ${calendarColour} ${calendarPct}%, ${base})`;
}

/** The text colour for text drawn on an event's own colour, as an inline style (it outranks the
 *  span's `text-ink…` class, which stays for an event without one). */
export function inkOn(own: EventColour | null): CSSProperties | undefined {
  return own ? { color: own.ink } : undefined;
}

/** An event's hover cue: brighter, except on an own colour with white text, which darkens instead.
 *  `brightness()` lifts the fill while white text can't get any whiter, which took Basil from 5.02:1 to
 *  4.27:1; darkening keeps every colour at 4.5:1 or better (eventPalette.test.ts). Whole class names,
 *  so Tailwind finds them here. */
export function eventHover(own: EventColour | null): string {
  return own?.ink === WHITE ? "hover:brightness-90" : "hover:brightness-110";
}
