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

import { TUNING_TITLE } from "../components/localai/locate";
import { LOCALAI_SECTIONS } from "../components/localai/sections";
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

  it("has an entry for the On battery section", () => {
    // HelpOverlay renders nothing at all for an id it can't find, so a section whose `data-help`
    // points nowhere is silently unexplained — and nothing but this would notice.
    const entry = HELP["settings-localai-power"];
    expect(entry, "no help entry for settings-localai-power").toBeDefined();
    expect(entry?.title).toBe("On battery");
    expect(entry?.body).toMatch(/reduces power use by not running inference on your GPU/);
  });
});

describe("the Local AI tab's help", () => {
  it("has an entry for every section, titled with the section's own name", () => {
    // Each section's wrapper points its `data-help` at its row's id; an entry titled with an old name
    // would explain a heading the reader can't find.
    for (const { id, label, help } of LOCALAI_SECTIONS) {
      expect(HELP[help], `${id} → ${help}`).toBeDefined();
      expect(HELP[help].title).toBe(label);
    }
  });

  it("says nothing the redesign made untrue", () => {
    const bodies = LOCALAI_SECTIONS.map((s) => HELP[s.help].body).join(" ");
    // The speeds are ceilings or rough guides now, never "conservative".
    expect(bodies).not.toContain("The numbers are conservative estimates.");
    // Sections are named, never pointed at by direction.
    expect(bodies).not.toMatch(/\b(above|below)\b/);
    // The fold it names is the one Model server shows.
    expect(HELP["settings-localai-endpoint"].body).toContain(`“${TUNING_TITLE}”`);
  });

  it("calls a speed a ceiling only on a graphics card PM recognises", () => {
    // On a card it doesn't recognise PM uses a typical 400 GB/s, and the tab says the real figure
    // "could be half that, or double" — so an unscoped "on a graphics card they're ceilings" told
    // the reader the opposite of the line beside the number. System-memory speeds are a typical
    // 40 GB/s, not a published one, so they get no ceiling either.
    for (const { help } of LOCALAI_SECTIONS) {
      const body = HELP[help].body;
      for (const m of body.matchAll(/[^.]*\bceilings?\b[^.]*\./g)) {
        expect(m[0], help).toMatch(/graphics card PM recognises/);
        expect(m[0], help).toMatch(/rough guide/);
      }
    }
  });

  it("says the limit on unloading is PM's, not the servers'", () => {
    // LM Studio can eject a model (in the app, `lms unload`, or its REST API since 0.4.0). What is
    // true is that PM only drives Ollama's unload.
    const body = HELP["settings-localai-lifecycle"].body;
    expect(body).not.toMatch(/Only Ollama can unload/);
    expect(body).toMatch(/PM can only ask Ollama to unload a model/);
  });
});
