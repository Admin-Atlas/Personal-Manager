// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Where a new event can go (#884, C10): only calendars the backend's gate would let PM create on,
// the accounts that can't named by why, the starting calendar, the account's own length, and the
// notice before a save (hidden here).

import { describe, expect, it } from "vitest";
import type { Calendar, CalendarAccount, CalendarOverview, GuestForEdit } from "../types";
import {
  accountLength,
  createdWhere,
  createNotes,
  createReasonFor,
  createTargets,
  defaultTarget,
  duplicateTarget,
  guestsOnto,
  lengthFor,
  meetDefault,
  meetOffered,
  meetOffText,
  meetOnCalendar,
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
    history_ready: true,
    editing: { [ME]: "on", [PAUSED]: "paused", [CONSENT]: "needs_consent" },
    default_length: {},
    auto_meet: {},
    ask_access: {},
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
  // Both surfaces (the full editor and quick create) say these, so they're worded once. A date says
  // nothing: PM keeps every event, so wherever it falls it shows here (#1047).
  it("says before Save when PM won't show the event: a calendar hidden here", () => {
    const o = overview();
    expect(createNotes(o, new Set(), `${ME}:alpha`)).toEqual([]);
    expect(createNotes(o, new Set([`${ME}:alpha`]), `${ME}:alpha`)).toEqual([
      "Alpha is hidden in your calendar list, so the new event won't show until you show that calendar again.",
    ]);
    // A twin on show will show it, so there's nothing to say.
    const twin = calendar(OFF, "alpha", { name: "Alpha (shared)" });
    const withTwin = overview({ calendars: [...o.calendars, twin] });
    expect(createNotes(withTwin, new Set([`${ME}:alpha`]), `${ME}:alpha`)).toEqual([]);
  });
});

// --- Duplicate (C18c) ---

describe("duplicateTarget", () => {
  it("starts a copy on the original's calendar when it takes events, else where a new one would", () => {
    const t = createTargets(overview());
    expect(duplicateTarget(t, `${ME}:alpha`, null)?.calendar.name).toBe("Alpha");
    // Read-only, or another account with editing off: where a new event starts.
    expect(duplicateTarget(t, `${ME}:holidays`, null)?.calendar.name).toBe("Me");
    expect(duplicateTarget(t, `${OFF}:off@x.com`, `${ME}:team`)?.calendar.name).toBe("Team");
    expect(
      duplicateTarget(createTargets(overview({ calendars: [] })), `${ME}:alpha`, null),
    ).toBeNull();
  });

  // Critique C1-L9: the calendar a copy starts on organises it, so its address isn't a guest.
  it("guestsOnto drops the calendar's own address from a copy's guests", () => {
    const t = createTargets(overview());
    const team = targetById(t, `${ME}:team`)!;
    const guests = [{ email: "a@y.org" }, { email: " Team " }, { email: "b@y.org" }];
    expect(guestsOnto(guests, team)).toEqual([{ email: "a@y.org" }, { email: "b@y.org" }]);
    expect(guestsOnto(guests, targetById(t, `${ME}:alpha`)!)).toEqual(guests);
  });
});

// --- a video call on new events (C18b) ---

describe("the Meet default", () => {
  const meetCal = calendar(ME, "me@x.com", {
    name: "Me",
    is_primary: true,
    conference_types: ["addOn", "hangoutsMeet"],
  });
  const plainCal = calendar(ME, "alpha", { name: "Alpha", conference_types: ["addOn"] });
  const o = (auto: Record<string, boolean>) =>
    overview({ calendars: [meetCal, plainCal], auto_meet: auto });
  const on = (cal: Calendar, auto: Record<string, boolean>) =>
    targetById(createTargets(o(auto)), cal.id);
  const guest = (email: string, over: Partial<GuestForEdit> = {}): GuestForEdit => ({
    email,
    name: null,
    response: null,
    optional: false,
    organizer: false,
    self: false,
    room: false,
    ...over,
  });
  const other = [guest("a@y.com")];

  it("meetOffered reads the calendar's kinds of call", () => {
    expect(meetOffered(on(meetCal, {}))).toBe(true);
    expect(meetOffered(on(plainCal, {}))).toBe(false);
    expect(meetOffered(null)).toBe(false);
  });

  // Critique C2-H2: Google adds Meet "when you add invitees", so the setting turns it on only with
  // a guest who isn't the user.
  it("is on only with the setting, a calendar with Meet and a guest", () => {
    const auto = { [ME]: true };
    expect(meetDefault(o(auto), on(meetCal, auto), other)).toBe(true);
    expect(meetDefault(o(auto), on(meetCal, auto), []), "no guest").toBe(false);
    expect(meetDefault(o(auto), on(plainCal, auto), other), "no Meet there").toBe(false);
    expect(meetDefault(o({ [ME]: false }), on(meetCal, { [ME]: false }), other)).toBe(false);
    expect(meetDefault(o({}), on(meetCal, {}), other), "never read").toBe(false);
    expect(meetDefault(o(auto), null, other)).toBe(false);
    // The user's own address, the calendar's own entry or a room is nobody Google invites.
    for (const self of [
      [guest("me@x.com")],
      [guest("someone@x.com", { self: true })],
      [guest("room@resource.calendar.google.com")],
    ]) {
      expect(meetDefault(o(auto), on(meetCal, auto), self), self[0].email).toBe(false);
    }
  });

  it("another calendar decides an untouched call afresh, and turns a touched one off without Meet", () => {
    const auto = { [ME]: true };
    const ov = o(auto);
    const draft = (meet: boolean) => ({ meet, guests: other });
    expect(meetOnCalendar(ov, on(meetCal, auto), draft(false), false)).toEqual({
      meet: true,
      off: false,
    });
    expect(meetOnCalendar(ov, on(plainCal, auto), draft(true), false)).toEqual({
      meet: false,
      off: true,
    });
    // Touched: kept where Meet is offered, either way.
    expect(meetOnCalendar(ov, on(meetCal, auto), draft(false), true)).toEqual({
      meet: false,
      off: false,
    });
    expect(meetOnCalendar(ov, on(meetCal, auto), draft(true), true)).toEqual({
      meet: true,
      off: false,
    });
    expect(meetOnCalendar(ov, on(plainCal, auto), draft(true), true)).toEqual({
      meet: false,
      off: true,
    });
    expect(meetOffText("Alpha")).toBe("Alpha doesn't offer Google Meet, so the video call is off.");
  });
});
