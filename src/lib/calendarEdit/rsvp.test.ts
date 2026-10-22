// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Replies to invitations (C14): the user's own reply read as the backend reads it, and a note
// counted as the backend counts it (the same rows as `calendar_write::rsvp`'s tests).

import { describe, expect, it } from "vitest";

import type { Attendee } from "../types";
import {
  asRsvp,
  declinedByMe,
  myReply,
  NOTE_MAX,
  noteLength,
  RSVP_CHOICES,
  rsvpLabel,
  tidyNote,
} from "./rsvp";

const entry = (email: string | null, response: string | null, self = false): Attendee => ({
  name: null,
  email,
  response,
  optional: false,
  organizer: false,
  self,
});

describe("the user's own reply", () => {
  it("reads the entry marked as the calendar, never another guest's", () => {
    const ev = {
      attendees: [entry("a@x.org", "declined"), entry("me@x.org", "accepted", true)],
    };
    expect(myReply(ev)).toBe("yes");
    expect(declinedByMe(ev)).toBe(false);
    expect(myReply({ attendees: [entry("a@x.org", "declined")] })).toBeNull();
    expect(myReply({ attendees: [] })).toBeNull();
    expect(myReply({})).toBeNull();
    // Google's "tentative" is Maybe; no reply yet is awaiting.
    expect(myReply({ attendees: [entry("me@x.org", "tentative", true)] })).toBe("maybe");
    expect(myReply({ attendees: [entry("me@x.org", "needsAction", true)] })).toBe("awaiting");
  });

  it("with two marked, takes the calendar's own address in any case, else the first", () => {
    const ev = {
      attendees: [entry("Me+pm@X.org", "declined", true), entry("me@x.org", "accepted", true)],
    };
    expect(myReply(ev, "ME@x.org")).toBe("yes");
    expect(myReply(ev, "other@x.org")).toBe("no");
    expect(myReply(ev)).toBe("no");
    expect(declinedByMe(ev, "me@x.org")).toBe(false);
    expect(declinedByMe(ev, "me+pm@x.org")).toBe(true);
  });

  it("skips an entry marked as the calendar with no address, as the backend does", () => {
    const ev = { attendees: [entry(null, "declined", true), entry(" ", "declined", true)] };
    expect(myReply(ev)).toBeNull();
    expect(declinedByMe(ev)).toBe(false);
  });

  it("offers the three replies in Google's order, with Google's words", () => {
    expect(RSVP_CHOICES.map((c) => c.label)).toEqual(["Yes", "No", "Maybe"]);
    expect(RSVP_CHOICES.map((c) => c.value)).toEqual(["yes", "no", "maybe"]);
    expect(rsvpLabel("maybe")).toBe("Maybe");
    expect(asRsvp("yes")).toBe("yes");
    expect(asRsvp("maybe")).toBe("maybe");
    // No reply yet: none of the three is chosen.
    expect(asRsvp("awaiting")).toBeNull();
    expect(asRsvp(null)).toBeNull();
  });
});

describe("a note", () => {
  it("is tidied as the backend tidies it", () => {
    expect(tidyNote("a\r\nb\rc")).toBe("a\nb\nc");
    // The bell dropped, the tab trimmed at the end; tabs and new lines inside kept.
    expect(tidyNote("\u0007x\t")).toBe("x");
    expect(tidyNote(" a\tb\n\nc ")).toBe("a\tb\n\nc");
    // Rust's trim and controls, not JavaScript's: a C1 control goes, a zero-width no-break space
    // at the end stays (Rust doesn't count it as space).
    expect(tidyNote("x\u0085")).toBe("x");
    expect(tidyNote("x﻿")).toBe("x﻿");
  });

  it("is counted in characters after tidying, never in UTF-16 units", () => {
    expect(NOTE_MAX).toBe(1000);
    expect(noteLength("é".repeat(NOTE_MAX))).toBe(NOTE_MAX);
    expect(noteLength(`${"é".repeat(NOTE_MAX)}é`)).toBe(NOTE_MAX + 1);
    // Two UTF-16 units each, one character each.
    const faces = "😀".repeat(NOTE_MAX);
    expect(faces.length).toBe(2 * NOTE_MAX);
    expect(noteLength(faces)).toBe(NOTE_MAX);
    expect(noteLength(`  ${"a".repeat(NOTE_MAX)}\r\n`)).toBe(NOTE_MAX);
  });
});
