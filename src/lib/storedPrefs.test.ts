// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Moving the three user-content pref blobs out of the webview's plaintext localStorage and into the
// encrypted `settings` table. What is worth pinning is not the storage call but the ONE-TIME
// ADOPTION, because every way it can go wrong loses data silently:
//
//   - resolving both copies the wrong way round would revive a stale value on every launch;
//   - clearing localStorage before the write lands loses the only copy there is if it rejects;
//   - re-running (StrictMode double-invokes every effect in dev) would re-adopt what was just moved;
//   - the backup keys are dynamically named per cloud account, so a prefix scan that mis-handles an
//     empty or dotted account segment orphans a dismissal into a plaintext key nothing enumerates;
//   - over-migrating would drag genuine view state — the calendar's view mode, cursor, panel bounds
//     — behind an IPC round trip it has no business being behind.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { getPref, setPref } from "./ipc";
import {
  readCursorDay,
  readDayCount,
  readHidden,
  readOpenOn,
  readRange,
  readRangeBounds,
  readRosterOpen,
  readView,
  readZones,
} from "./calendarPrefs";
import {
  __resetStoredPrefs,
  hydrateStoredPrefs,
  readStored,
  storedPrefsHydrated,
  writeStored,
} from "./storedPrefs";

vi.mock("./ipc", () => ({
  getPref: vi.fn(async () => null),
  setPref: vi.fn(async () => undefined),
}));

const get = vi.mocked(getPref);
const set = vi.mocked(setPref);

/** Make `getPref` answer from a fixed map of stored rows; anything absent reads as `null`. */
function storeHolds(rows: Record<string, unknown>) {
  get.mockImplementation(async (key: string) => (key in rows ? JSON.stringify(rows[key]) : null));
}

beforeEach(() => {
  __resetStoredPrefs();
  localStorage.clear();
  get.mockReset();
  set.mockReset();
  get.mockImplementation(async () => null);
  set.mockImplementation(async () => undefined);
});

describe("hydration", () => {
  it("lets the store win over a leftover localStorage copy", async () => {
    // Both copies exist only because adoption already ran once, and every write since then went to
    // the store — so localStorage is the stale one. Resolving this the other way would silently
    // revive an old sort on every launch.
    localStorage.setItem("pm.calendar.hidden", JSON.stringify(["stale@example.com"]));
    storeHolds({ calendar_ui: { hidden: ["current@example.com"] } });

    await hydrateStoredPrefs();

    expect(readStored("calendar_ui").hidden).toEqual(["current@example.com"]);
    expect(set).not.toHaveBeenCalledWith("calendar_ui", expect.anything());
  });

  it("adopts the localStorage copy exactly once, then clears it", async () => {
    localStorage.setItem("pm.calendar.hidden", JSON.stringify(["a@example.com"]));

    await hydrateStoredPrefs();

    const writes = set.mock.calls.filter(([k]) => k === "calendar_ui");
    expect(writes).toHaveLength(1);
    expect(JSON.parse(writes[0][1] as string)).toEqual({ hidden: ["a@example.com"] });
    expect(localStorage.getItem("pm.calendar.hidden")).toBeNull();
    expect(readStored("calendar_ui").hidden).toEqual(["a@example.com"]);
  });

  it("writes nothing for a key with no localStorage copy to adopt", async () => {
    await hydrateStoredPrefs();
    expect(set).not.toHaveBeenCalled();
    expect(storedPrefsHydrated()).toBe(true);
  });

  it("does NOT clear localStorage when the write rejects", async () => {
    // The whole point of clearing in the resolve path. If the store is full, read-only or shut, that
    // localStorage copy is still the only copy of data this change exists to protect — losing it
    // here would be the change causing exactly the loss it was written to prevent.
    localStorage.setItem("pm.calendar.hidden", JSON.stringify(["a@example.com"]));
    set.mockRejectedValue(new Error("session unavailable"));

    await hydrateStoredPrefs();

    expect(localStorage.getItem("pm.calendar.hidden")).toBe(JSON.stringify(["a@example.com"]));
    // The session still behaves normally, and the next boot re-attempts the adoption.
    expect(readStored("calendar_ui").hidden).toEqual(["a@example.com"]);
  });

  it("is a no-op the second time (StrictMode double-invokes every effect)", async () => {
    localStorage.setItem("pm.calendar.hidden", JSON.stringify(["a@example.com"]));

    await hydrateStoredPrefs();
    const readsAfterFirst = get.mock.calls.length;
    set.mockClear();

    await hydrateStoredPrefs();

    expect(get.mock.calls.length).toBe(readsAfterFirst);
    expect(set).not.toHaveBeenCalled();
  });

  it("shares one in-flight attempt between concurrent callers", async () => {
    await Promise.all([hydrateStoredPrefs(), hydrateStoredPrefs(), hydrateStoredPrefs()]);
    // One read per key, not three: two roots and a StrictMode remount must not triple the boot cost.
    expect(get.mock.calls.length).toBe(3);
  });

  it("stays un-hydrated when the store is not open, so a later call retries", async () => {
    // `AppState::conn()` errors while the vault is locked, which is exactly what a boot-time
    // hydration hits on a passphrase vault. Latching "done" here would spend the whole session on
    // defaults AND start persisting them.
    get.mockRejectedValue(new Error("session unavailable"));
    await expect(hydrateStoredPrefs()).rejects.toThrow();
    expect(storedPrefsHydrated()).toBe(false);

    get.mockImplementation(async () => null);
    await hydrateStoredPrefs();
    expect(storedPrefsHydrated()).toBe(true);
  });

  it("reads defaults and drops writes before hydration lands", async () => {
    // A component's mount effect fires with its DEFAULT (ProjectView's `showCompleted` does exactly
    // that). Pointed at the store rather than at localStorage, persisting it would stamp `true` over
    // a stored `false` before the real value ever arrived.
    expect(readStored("milestone_ui")).toEqual({});
    writeStored("milestone_ui", { showCompleted: true });
    expect(set).not.toHaveBeenCalled();

    storeHolds({ milestone_ui: { showCompleted: false } });
    await hydrateStoredPrefs();
    expect(readStored("milestone_ui").showCompleted).toBe(false);
  });

  it("degrades a corrupt stored row to the defaults rather than throwing", async () => {
    get.mockImplementation(async (key: string) => (key === "calendar_ui" ? "{not json" : null));
    await hydrateStoredPrefs();
    expect(readStored("calendar_ui")).toEqual({});
  });
});

describe("writeStored", () => {
  it("patches the blob rather than replacing it", async () => {
    // The co-tenancy hazard that keeps these three OUT of `project_ui`: useSidebarSplit writes that
    // key as a whole blob on every divider drag, so a co-tenant would be deleted by a mouse gesture.
    // Within a key, a patch must not do the same to its neighbours.
    storeHolds({ milestone_ui: { sort: { Roof: { key: "manual", dir: "asc" } } } });
    await hydrateStoredPrefs();

    writeStored("milestone_ui", { showCompleted: false });

    expect(readStored("milestone_ui")).toEqual({
      sort: { Roof: { key: "manual", dir: "asc" } },
      showCompleted: false,
    });
    const last = set.mock.calls[set.mock.calls.length - 1];
    expect(JSON.parse(last[1])).toEqual({
      sort: { Roof: { key: "manual", dir: "asc" } },
      showCompleted: false,
    });
  });

  it("never throws out of a click handler when the store rejects", async () => {
    await hydrateStoredPrefs();
    set.mockRejectedValue(new Error("disk full"));
    expect(() => writeStored("calendar_ui", { hidden: ["x"] })).not.toThrow();
    // The cache still reflects the click, so the surface does not appear to ignore the user.
    expect(readStored("calendar_ui").hidden).toEqual(["x"]);
  });
});

describe("the backup dismissal prefix scan", () => {
  it("finds every dynamically-named key, empty and dotted account segments included", async () => {
    // `pm.backup.reconcileDismissed.${destination}.${account ?? ""}` — so the account may be absent
    // and, being an email address, may itself contain dots. The composite that follows the prefix IS
    // the stored id: splitting it back apart on the LAST dot (or on every dot) would mangle both.
    localStorage.setItem("pm.backup.reconcileDismissed.proton.", "true");
    localStorage.setItem("pm.backup.reconcileDismissed.gdrive.first.last@example.co.uk", "true");
    localStorage.setItem("pm.backup.reconcileDismissed.gdrive.plain@example.com", "true");
    // A key written by an older build and then un-dismissed: present, but not "true".
    localStorage.setItem("pm.backup.reconcileDismissed.proton.other@example.com", "false");
    // A neighbour that must not be swept up.
    localStorage.setItem("pm.backup.lastRun", "2026-07-31");

    await hydrateStoredPrefs();

    expect(readStored("backup_ui").reconcileDismissed).toEqual([
      "proton.",
      "gdrive.first.last@example.co.uk",
      "gdrive.plain@example.com",
    ]);
    // Every scanned key goes, including the "false" one — its meaning is carried by absence now.
    expect(localStorage.getItem("pm.backup.reconcileDismissed.proton.")).toBeNull();
    expect(
      localStorage.getItem("pm.backup.reconcileDismissed.proton.other@example.com"),
    ).toBeNull();
    expect(localStorage.getItem("pm.backup.lastRun")).toBe("2026-07-31");
  });
});

describe("what deliberately stays in localStorage", () => {
  it("moves only pm.calendar.hidden — the other eight calendar keys are untouched", async () => {
    // calendarPrefs holds nine keys and exactly ONE is user content (a calendar id is routinely the
    // account's email). The rest are view state, and dragging them behind an IPC round trip would be
    // against the decision this change implements, not for it.
    const stays: Record<string, string> = {
      "pm.calendar.view": "week",
      "pm.calendar.range": "work",
      "pm.calendar.rosterOpen": "false",
      "pm.calendar.zones": JSON.stringify(["Europe/London"]),
      "pm.calendar.rangeBounds": JSON.stringify({ work: { startHour: 9, endHour: 17 } }),
      "pm.calendar.dayCount": "3",
      "pm.calendar.openOn": "last",
      // The borderline call, decided deliberately: a date the user was looking at is view position,
      // not user content.
      "pm.calendar.cursor": "2026-07-31",
    };
    for (const [k, v] of Object.entries(stays)) localStorage.setItem(k, v);
    localStorage.setItem("pm.calendar.hidden", JSON.stringify(["a@example.com"]));

    await hydrateStoredPrefs();

    for (const [k, v] of Object.entries(stays)) expect(localStorage.getItem(k)).toBe(v);
    expect(localStorage.getItem("pm.calendar.hidden")).toBeNull();

    // And they still round-trip through their own readers — the assertion above only proves the
    // bytes survived, not that the module still looks for them where they are.
    expect(readView(["month", "week"], "month")).toBe("week");
    expect(readRange()).toBe("work");
    expect(readRosterOpen()).toBe(false);
    expect(readZones()).toEqual(["Europe/London"]);
    expect(readRangeBounds().work).toEqual({ startHour: 9, endHour: 17 });
    expect(readDayCount()).toBe(3);
    expect(readOpenOn()).toBe("last");
    expect(readCursorDay()?.getDate()).toBe(31);
    // The one that moved reads from the store instead.
    expect([...readHidden()]).toEqual(["a@example.com"]);
  });
});
