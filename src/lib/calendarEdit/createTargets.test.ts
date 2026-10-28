// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Where a new event can go (#884, C10): only calendars the backend's gate would let PM create on,
// the accounts that can't named by why, the starting calendar, the account's own length, and the
// two notices before a save (outside the mirror, hidden here).

import { describe, expect, it } from "vitest";
import type { Calendar, CalendarAccount, CalendarOverview } from "../types";
import { formatDateLocal } from "../format";
import {
  accountLength,
  createdWhere,
  createNotes,
  createReasonFor,
  createTargets,
  defaultTarget,
  lengthFor,
  outsideMirror,
  targetById,
} from "./createTargets";

function account(id: string, provider = "google"): CalendarAccount {
  const email = id.includes(":") ? id.slice(id.indexOf(":") + 1) : null;
  return { id, provider, email, label: email ?? id, state: "ok", last_synced_at: null };
}

function calendar(source: string, remote: string, over: Partial<Calendar> = {}): Calendar {
  return {
    id: `${source}:${remote}`,
    source_id: source,
    provider: "google",
    remote_id: remote,
    name: remote,
    color: null,
    selected: true,
    is_primary: false,
    quiet: false,
    kind: null,
    access_role: "owner",
    time_zone: "Europe/London",
    default_reminders: [],
    conference_types: [],
    data_owner: null,
    ...over,
  };
}

const ME = "gcal:me@x.com";
const OFF = "gcal:off@x.com";
const PAUSED = "gcal:paused@x.com";
const CONSENT = "gcal:consent@x.com";

function overview(over: Partial<CalendarOverview> = {}): CalendarOverview {
  return {
    google_client_configured: true,
    microsoft_client_configured: false,
    accounts: [
      account(ME),
      account(OFF),
      account(PAUSED),
      account(CONSENT),
      account("ics:abc", "other"),
      account("outlook:me@y.com", "microsoft"),
    ],
    calendars: [
      calendar(ME, "zeta", { name: "Zeta" }),
      calendar(ME, "me@x.com", { name: "Me", is_primary: true }),
      calendar(ME, "team", { name: "Team", access_role: "writerWithoutPrivateAccess" }),
      calendar(ME, "alpha", { name: "Alpha", access_role: "writer" }),
      calendar(ME, "holidays", { name: "Holidays", access_role: "reader" }),
      calendar(ME, "busy", { name: "Busy", access_role: "freeBusyReader" }),
      calendar(ME, "odd", { name: "Odd", access_role: "someNewRole" }),
      calendar(ME, "none", { name: "None", access_role: null }),
      calendar(ME, "unsynced", { name: "Unsynced", selected: false }),
      calendar(OFF, "off@x.com", { name: "Off", is_primary: true }),
      calendar(PAUSED, "paused@x.com", { name: "Paused", is_primary: true }),
      calendar("ics:abc", "feed", { provider: "other", access_role: null }),
      calendar("outlook:me@y.com", "cal", { provider: "microsoft", access_role: null }),
    ],
    last_sync: null,
    window_days: 7,
    mirror_start: "2026-09-01T00:00:00Z",
    mirror_end: "2027-11-01T00:00:00Z",
    editing: { [ME]: "on", [PAUSED]: "paused", [CONSENT]: "needs_consent" },
    default_length: {},
    ...over,
  };
}

const names = (o: CalendarOverview) =>
  createTargets(o).groups.map((g) => [g.account.id, g.targets.map((t) => t.calendar.name)]);

describe("createTargets", () => {
  it("lists only writable, selected calendars grouped by account", () => {
    // Primary first, then by name; readers, free/busy, unknown roles, unsynced, editing-off
    // accounts, subscriptions and Outlook are all left out.
    expect(names(overview())).toEqual([[ME, ["Me", "Alpha", "Team", "Zeta"]]]);
    const team = createTargets(overview()).groups[0].targets.find(
      (t) => t.calendar.name === "Team",
    );
    expect(team?.noPrivate).toBe(true);
    expect(createTargets(overview()).groups[0].targets[0].noPrivate).toBe(false);
  });

  it("names editing-off accounts by status", () => {
    expect(createTargets(overview()).off.map((o) => [o.account.id, o.status])).toEqual([
      [OFF, "off"],
      [PAUSED, "paused"],
      [CONSENT, "needs_consent"],
    ]);
  });

  it("offers nothing when no account has editing on", () => {
    const t = createTargets(overview({ editing: {} }));
    expect(t.groups).toEqual([]);
    expect(t.off).toHaveLength(4);
  });
});

describe("createReasonFor", () => {
  const reason = (o: CalendarOverview) => createReasonFor(o, createTargets(o));
  const TURN_ON = "Turn on editing for a Google account in Settings › Connectors to create events.";
  const NONE = "None of your calendars can take new events.";

  it("says nothing while a calendar can take an event", () => {
    expect(reason(overview())).toBeNull();
  });

  it("points to the switch when editing is on nowhere, paused or not", () => {
    expect(reason(overview({ editing: {} }))).toBe(TURN_ON);
    expect(reason(overview({ editing: { [ME]: "paused", [OFF]: "needs_consent" } }))).toBe(TURN_ON);
    // A non-Google account's entry never counts as editing on.
    expect(reason(overview({ editing: { "outlook:me@y.com": "on" } }))).toBe(TURN_ON);
  });

  it("says none can when editing is on but no calendar there takes events", () => {
    const readOnly = overview({
      calendars: overview().calendars.map((c) =>
        c.source_id === ME ? { ...c, access_role: "reader" } : c,
      ),
    });
    expect(reason(readOnly)).toBe(NONE);
    // Every writable calendar unticked in Settings, likewise.
    const unticked = overview({
      calendars: overview().calendars.map((c) => ({ ...c, selected: false })),
    });
    expect(reason(unticked)).toBe(NONE);
  });

  it("without a Google account (no button at all), would point to the switch", () => {
    const none = overview({
      accounts: overview().accounts.filter((a) => a.provider !== "google"),
      editing: {},
    });
    expect(reason(none)).toBe(TURN_ON);
  });
});

describe("defaultTarget", () => {
  it("defaults to the last used, else the primary, else the first, else none", () => {
    const t = createTargets(overview());
    expect(defaultTarget(t, `${ME}:alpha`)?.calendar.name).toBe("Alpha");
    // A last calendar that can't take an event any more (read-only now, or gone) is passed over.
    expect(defaultTarget(t, `${ME}:holidays`)?.calendar.name).toBe("Me");
    expect(defaultTarget(t, "gone")?.calendar.name).toBe("Me");
    expect(defaultTarget(t, null)?.calendar.name).toBe("Me");
    const noPrimary = createTargets(
      overview({ calendars: overview().calendars.filter((c) => !c.is_primary) }),
    );
    expect(defaultTarget(noPrimary, null)?.calendar.name).toBe("Alpha");
    expect(defaultTarget(createTargets(overview({ editing: {} })), null)).toBeNull();
  });

  it("finds a calendar on offer by id, and only those", () => {
    const t = createTargets(overview());
    expect(targetById(t, `${ME}:team`)?.calendar.name).toBe("Team");
    expect(targetById(t, `${ME}:holidays`)).toBeNull();
  });
});

describe("lengthFor", () => {
  it("uses the account's Google default, and falls back to an hour", () => {
    const o = overview({ default_length: { [ME]: 30 } });
    const target = defaultTarget(createTargets(o), null);
    expect(lengthFor(o, target)).toBe(30 * 60_000);
    expect(lengthFor(overview(), target)).toBe(60 * 60_000);
    expect(lengthFor(o, null)).toBe(60 * 60_000);
    for (const odd of [0, -5, 1441, 2.5, Number.NaN]) {
      expect(lengthFor(overview({ default_length: { [ME]: odd } }), target)).toBe(60 * 60_000);
    }
  });

  // C16b: an all-day event dragged into the hours takes its own account's length.
  it("accountLength reads an account by its id, and an unknown one is an hour", () => {
    const o = overview({ default_length: { [ME]: 45 } });
    expect(accountLength(o, ME)).toBe(45 * 60_000);
    expect(accountLength(o, "gcal:other@x.com")).toBe(60 * 60_000);
    expect(accountLength(o, null)).toBe(60 * 60_000);
    expect(accountLength(overview({ default_length: { [ME]: 2.5 } }), ME)).toBe(60 * 60_000);
  });
});

describe("outsideMirror", () => {
  const o = overview({
    mirror_start: "2026-10-01T00:00:00Z",
    mirror_end: "2026-12-01T00:00:00Z",
  });
  const timed = (start: string, end: string) => ({
    kind: "timed" as const,
    start_date: start.slice(0, 10),
    start_time: start.slice(11, 16),
    start_zone: "UTC",
    end_date: end.slice(0, 10),
    end_time: end.slice(11, 16),
    end_zone: "UTC",
  });
  const allDay = (first: string, last: string) => ({
    kind: "all_day" as const,
    first_day: first,
    last_day: last,
  });

  it("outsideMirror reads both edges, and which side it's on", () => {
    // Ends exactly at the window's start: outside, before it. A minute into it: inside.
    expect(outsideMirror(timed("2026-09-30T23:00", "2026-10-01T00:00"), o)).toBe("before");
    expect(outsideMirror(timed("2026-09-30T23:00", "2026-10-01T00:01"), o)).toBeNull();
    // Starts exactly at the window's end: outside, after it. A minute before: inside.
    expect(outsideMirror(timed("2026-12-01T00:00", "2026-12-01T01:00"), o)).toBe("after");
    expect(outsideMirror(timed("2026-11-30T23:59", "2026-12-01T01:00"), o)).toBeNull();
    // All day: the last day before the end is in; the day the window ends on is out.
    expect(outsideMirror(allDay("2026-11-30", "2026-11-30"), o)).toBeNull();
    expect(outsideMirror(allDay("2026-12-01", "2026-12-01"), o)).toBe("after");
    expect(outsideMirror(allDay("2026-09-30", "2026-09-30"), o)).toBe("before");
    expect(outsideMirror(allDay("2026-09-30", "2026-10-01"), o)).toBeNull();
    // Far ahead: the probe's 13+ months out.
    expect(outsideMirror(allDay("2028-01-01", "2028-01-01"), o)).toBe("after");
  });

  it("says nothing about a time it can't read", () => {
    expect(
      outsideMirror(
        { ...timed("2028-01-01T10:00", "2028-01-01T11:00"), start_zone: "Nowhere/Here" },
        o,
      ),
    ).toBeNull();
    expect(
      outsideMirror(timed("2028-01-01T10:00", "2028-01-01T11:00"), { ...o, mirror_end: "" }),
    ).toBeNull();
  });
});

describe("createdWhere", () => {
  it("a hidden calendar keeps it out of sight, unless a visible twin shows it", () => {
    const o = overview();
    expect(createdWhere(o, new Set(), `${ME}:alpha`)).toEqual({ name: "Alpha", hidden: false });
    expect(createdWhere(o, new Set([`${ME}:alpha`]), `${ME}:alpha`)).toEqual({
      name: "Alpha",
      hidden: true,
    });
    // The same Google calendar through a second account, ticked and shown: it'll show there.
    const twin = calendar(OFF, "alpha", { name: "Alpha (shared)" });
    const withTwin = overview({ calendars: [...o.calendars, twin] });
    expect(createdWhere(withTwin, new Set([`${ME}:alpha`]), `${ME}:alpha`).hidden).toBe(false);
    // A twin that is hidden too, or not synced, shows nothing.
    expect(createdWhere(withTwin, new Set([`${ME}:alpha`, twin.id]), `${ME}:alpha`).hidden).toBe(
      true,
    );
    const unsyncedTwin = overview({ calendars: [...o.calendars, { ...twin, selected: false }] });
    expect(createdWhere(unsyncedTwin, new Set([`${ME}:alpha`]), `${ME}:alpha`).hidden).toBe(true);
  });

  it("a calendar unticked while the event was being made, or gone, reads as hidden", () => {
    expect(createdWhere(overview(), new Set(), `${ME}:unsynced`).hidden).toBe(true);
    expect(createdWhere(overview(), new Set(), "gone").hidden).toBe(true);
  });
});

describe("createNotes", () => {
  // Both surfaces (the full editor and quick create) say these, so they're worded once.
  it("says before Save when PM won't show the event: outside the months it syncs, or hidden", () => {
    const o = overview();
    const day = (d: string) => ({ kind: "all_day" as const, first_day: d, last_day: d });
    expect(createNotes(day("2026-10-12"), false, o, new Set(), `${ME}:alpha`)).toEqual([]);
    // The window's ends as this computer's dates, DD-MM.
    const range = `${formatDateLocal(new Date(o.mirror_start))} – ${formatDateLocal(new Date(o.mirror_end))}`;
    expect(createNotes(day("2028-01-01"), false, o, new Set(), `${ME}:alpha`)).toEqual([
      `This starts outside the months PM syncs (${range}), so it'll be saved in Google but won't show here.`,
    ]);
    expect(
      createNotes(day("2026-10-12"), false, o, new Set([`${ME}:alpha`]), `${ME}:alpha`),
    ).toEqual([
      "Alpha is hidden in your calendar list, so the new event won't show until you show that calendar again.",
    ]);
    // A twin on show will show it, so there's nothing to say.
    const twin = calendar(OFF, "alpha", { name: "Alpha (shared)" });
    const withTwin = overview({ calendars: [...o.calendars, twin] });
    expect(
      createNotes(day("2026-10-12"), false, withTwin, new Set([`${ME}:alpha`]), `${ME}:alpha`),
    ).toEqual([]);
  });

  // The backend lands a repeating event's occurrences inside the months PM syncs, whenever it
  // starts; only one that starts after them has none there.
  it("a repeating event from before the months PM syncs shows inside them", () => {
    const o = overview();
    const day = (d: string) => ({ kind: "all_day" as const, first_day: d, last_day: d });
    const range = `${formatDateLocal(new Date(o.mirror_start))} – ${formatDateLocal(new Date(o.mirror_end))}`;
    expect(createNotes(day("2020-01-06"), true, o, new Set(), `${ME}:alpha`)).toEqual([
      `This starts before the months PM syncs (${range}), so only its occurrences within them will show here.`,
    ]);
    // Once, it won't show at all.
    expect(createNotes(day("2020-01-06"), false, o, new Set(), `${ME}:alpha`)).toEqual([
      `This starts outside the months PM syncs (${range}), so it'll be saved in Google but won't show here.`,
    ]);
    // After them, repeating or not, none of it does.
    expect(createNotes(day("2028-01-03"), true, o, new Set(), `${ME}:alpha`)).toEqual([
      `This starts outside the months PM syncs (${range}), so it'll be saved in Google but won't show here.`,
    ]);
  });
});
