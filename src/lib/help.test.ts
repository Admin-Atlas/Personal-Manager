// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The help registry is prose, so nothing compiled ever disagreed with it — which is how two entries
// went on teaching the retired "Part of" status for ~250 commits after #278 deleted it. The app's
// own help documented a state `ProjectStatus` cannot produce, and no type, lint or test could see it.
//
// These bind the two entries that ENUMERATE statuses to `STATUS_LABEL`, the single source for the
// strings a user actually sees. Both directions are checked on purpose: a missing label means a new
// status shipped undocumented, and an extra one means a retired status is still being taught. The
// extra-label direction is the one that was failing, and it is asserted WITHOUT naming "Part of" —
// the test derives the truth from the map rather than from a list of ghosts someone has to maintain.

import { describe, expect, it } from "vitest";

import { STATUS_LABEL } from "../components/ui/StatusBadge";
import { HELP } from "./help";

const LABELS = Object.values(STATUS_LABEL);

describe("help copy agrees with the statuses the app can render", () => {
  it("focus-status-badge defines every status, and only statuses that exist", () => {
    // The body is a run of "<Label> = <explanation>." clauses, so the labels parse straight out.
    const body = HELP["focus-status-badge"].body;
    const defined = [...body.matchAll(/([A-Z][A-Za-z ]*?) = /g)].map((m) => m[1]);
    expect([...defined].sort()).toEqual([...LABELS].sort());
  });

  it("nav-focus lists every status, and only statuses that exist", () => {
    // "…answers 'should I look at this now?' — Due soon, Quick win, …, or On track. Click a project…"
    const body = HELP["nav-focus"].body;
    const enumeration = body.match(/—\s*(.+?)\./)?.[1];
    expect(enumeration, "nav-focus no longer contains an em-dashed status list").toBeDefined();
    const listed = enumeration!
      .split(",")
      .map((s) => s.replace(/^\s*or\s+/, "").trim())
      .filter(Boolean);
    expect([...listed].sort()).toEqual([...LABELS].sort());
  });
});

describe("help registry hygiene", () => {
  it("gives every entry a title and a body", () => {
    for (const [id, entry] of Object.entries(HELP)) {
      expect(entry.title.trim(), `${id} has no title`).not.toBe("");
      expect(entry.body.trim(), `${id} has no body`).not.toBe("");
    }
  });
});
