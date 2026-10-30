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
import { inCancelZone } from "./calendarDrag";
import { FORMAT_BUTTONS } from "./calendarEdit/formatKeys";
import { CHANGELOG } from "./changelog";
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

  it("explains the chat Thinking button and the fold it fills", () => {
    // Same silent-miss as above: ThinkingToggle and ThinkingBlock point their `data-help` here.
    for (const [id, title] of [
      ["chat-thinking-toggle", "Thinking"],
      ["chat-thinking", "The model's thinking"],
    ] as const) {
      const entry = HELP[id];
      expect(entry, `no help entry for ${id}`).toBeDefined();
      expect(entry?.title).toBe(title);
      expect(entry?.body.trim()).not.toBe("");
    }
    // The never-stored promise is the one a reader most needs to be able to find.
    expect(HELP["chat-thinking-toggle"].body).toContain("The thinking isn't saved");
  });

  it("promises background work skips thinking only where PM can switch it off", () => {
    // LM Studio and llama-server are sent no switch, so background work there thinks if the server
    // is set up to. An unscoped "background work never thinks" told those users something PM can't
    // make true — so the sentence about it has to name them.
    const sentence = HELP["chat-thinking-toggle"].body
      .split(/(?<=\.) /)
      .find((s) => s.includes("background work"));
    expect(sentence, "the toggle's help no longer says what background work does").toBeDefined();
    expect(sentence).toContain("LM Studio and llama-server");
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
    // The speeds are estimates or rough guides now, never "conservative".
    expect(bodies).not.toContain("The numbers are conservative estimates.");
    // Sections are named, never pointed at by direction.
    expect(bodies).not.toMatch(/\b(above|below)\b/);
    // The fold it names is the one Model server shows.
    expect(HELP["settings-localai-endpoint"].body).toContain(`“${TUNING_TITLE}”`);
  });

  it("no Local AI help calls a speed a ceiling", () => {
    // A card's figure is an estimate scaled by PM's own tests, worded "about" wherever it is shown,
    // and it can be under or over the real speed — so a "ceiling" anywhere would tell the reader the
    // opposite of the line beside the number.
    for (const { help } of LOCALAI_SECTIONS) {
      expect(HELP[help].body, help).not.toMatch(/\bceilings?\b/);
      expect(HELP[help].body, help).not.toMatch(/up to \d/);
    }
  });

  it("never names the chat floor's figure", () => {
    // Help is static and the backend owns the floor (better_fit.rs `chat_floor_tps`), so a number
    // here could go on teaching one the pick no longer uses.
    for (const { help } of LOCALAI_SECTIONS) expect(HELP[help].body, help).not.toMatch(/tok\/s/);
  });

  it("says the limit on unloading is PM's, not the servers'", () => {
    // LM Studio can eject a model (in the app, `lms unload`, or its REST API since 0.4.0). What is
    // true is that PM only drives Ollama's unload.
    const body = HELP["settings-localai-lifecycle"].body;
    expect(body).not.toMatch(/Only Ollama can unload/);
    expect(body).toMatch(/PM can only ask Ollama to unload a model/);
  });
});

/** The calendar's help entries (#884, C20b): the tab's, its editing switch's, and every part's. */
const CALENDAR_HELP = Object.keys(HELP).filter(
  (id) => id.startsWith("calendar-") || id === "nav-calendar" || id === "settings-calendar-editing",
);

// The calendar's drag (#884, PR C): help and What's New say what it does, not more.
describe("calendar help agrees with the drag and its Undo", () => {
  const said = [
    ...CALENDAR_HELP.map((id) => HELP[id].body),
    ...CHANGELOG.flatMap((e) => e.highlights),
  ];

  it("puts an event back only where letting go does", () => {
    // Beside or above the calendar puts it back; below scrolls and drops, so "outside" says more.
    const box = { left: 0, top: 0, right: 100, bottom: 100 };
    expect([
      inCancelZone(box, -1, 50),
      inCancelZone(box, 100, 50),
      inCancelZone(box, 50, -1),
    ]).toEqual([true, true, true]);
    expect(inCancelZone(box, 50, 150)).toBe(false);
    for (const text of said) expect(text).not.toMatch(/let(ting)? go outside the calendar/);
    expect(HELP["calendar-view"].body).toMatch(/letting go beside or above the calendar/);
  });

  it("offers Undo after a save only where the backend does", () => {
    // One event or one occurrence, and never after a change to the guests, what they may do (the
    // backend's Undo leaves the permissions out) or the repeat.
    const saves = said.filter((t) => /After a save|works after saving/.test(t));
    expect(saves.length).toBeGreaterThan(0);
    for (const text of saves) {
      expect(text).toMatch(/one event,? or (of )?one occurrence/);
      expect(text).toMatch(/guests, what they may do, or how (it|the event) repeats/);
    }
  });
});

// The description's toolbar (#884, C19d): its help names every button and shortcut the toolbar has,
// in the toolbar's order, so the two can't drift apart.
describe("the description's formatting help agrees with its toolbar", () => {
  /** Whether `body` says each of `words`, in that order. */
  const inOrder = (body: string, words: readonly string[]) => {
    let at = 0;
    for (const w of words) {
      const next = body.indexOf(w, at);
      if (next < 0) return false;
      at = next + w.length;
    }
    return true;
  };
  const body = HELP["calendar-description-format"].body;

  it("names each button and its shortcut, in order", () => {
    // "Bulleted list" is said "bulleted and numbered lists".
    const named = FORMAT_BUTTONS.map((b) => b.label.toLowerCase().replace(/ list$/, ""));
    expect(inOrder(body, named), named.join(", ")).toBe(true);
    const keys = FORMAT_BUTTONS.map((b) => b.hint);
    expect(inOrder(body, keys), keys.join(", ")).toBe(true);
    expect(body).toContain("⌘ instead of Ctrl on a Mac");
  });

  it("doesn't claim the toolbar is Google Calendar's own (probe E11-0 hasn't said)", () => {
    expect(body).not.toMatch(/same (buttons )?as Google/i);
  });
});

// The calendar's help (#884, C20b §11): each part of the calendar explains itself on its own element,
// in a panel that can't scroll (HelpOverlay's is fixed with no height limit), and says nothing the
// editing stack made untrue.
describe("the calendar's help", () => {
  /** Every component's source, tests left out: where the help ids are put on elements. */
  const SHIPPED = Object.entries(
    import.meta.glob<string>("../components/**/*.tsx", {
      query: "?raw",
      import: "default",
      eager: true,
    }),
  )
    .filter(([path]) => !/\.test\.tsx$/.test(path))
    .map(([, text]) => text)
    .join("\n");
  /** The parts of the calendar, each with an element of its own. */
  const PARTS = [
    "calendar-view",
    "calendar-header",
    "calendar-event",
    "calendar-editor",
    "calendar-new-event",
    "calendar-choices",
    "calendar-delete",
    "calendar-description-format",
  ];

  it("has an entry for every part of the calendar, each put on an element", () => {
    for (const id of PARTS) expect(HELP[id], id).toBeDefined();
    // Both ways: no calendar entry is left with nothing to explain.
    for (const id of CALENDAR_HELP) expect(SHIPPED, id).toContain(`"${id}"`);
  });

  it("names only ids that exist", () => {
    // `data-help="x"`, `helpId="x"`, and `data-help={… ? "x" : …}`: every one in the calendar's
    // components, and every calendar id anywhere.
    const ids = (text: string) =>
      [...text.matchAll(/\b(?:data-help|helpId)=(?:"([^"]+)"|\{[^}]*?"([^"]+)")/g)].map(
        (m) => m[1] ?? m[2],
      );
    const calendar = Object.entries(
      import.meta.glob<string>("../components/calendar/**/*.tsx", {
        query: "?raw",
        import: "default",
        eager: true,
      }),
    )
      .filter(([path]) => !/\.test\.tsx$/.test(path))
      .flatMap(([, text]) => ids(text));
    expect(calendar.length).toBeGreaterThanOrEqual(PARTS.length);
    const named = [...calendar, ...ids(SHIPPED).filter((id) => id.startsWith("calendar-"))];
    for (const id of named) expect(HELP[id], id).toBeDefined();
  });

  it("fits on the screen", () => {
    for (const id of CALENDAR_HELP) {
      expect(HELP[id].body.length, id).toBeLessThanOrEqual(1000);
    }
  });

  it("says nothing the stack made untrue", () => {
    for (const id of CALENDAR_HELP) {
      const body = HELP[id].body;
      for (const stale of [
        "one-off",
        "without guests",
        "for now",
        "or label",
        "Settings → Google Calendar",
        "read-only",
      ]) {
        expect(body, `${id}: ${stale}`).not.toContain(stale);
      }
      // Parts are named, never pointed at by direction. Where letting go of a drag puts it back is a
      // place on the screen, pinned by the drag's own test above.
      expect(body.replace("beside or above the calendar", ""), id).not.toMatch(/\b(above|below)\b/);
    }
  });

  it("promises nothing that isn't built", () => {
    for (const id of CALENDAR_HELP) {
      const body = HELP[id].body;
      // The help never says it is complete.
      expect(body, id).not.toMatch(/\bfinal\b|\bcomplete\b/i);
    }
  });

  it("says where each thing is done", () => {
    // The grid: drags and clicks. The details: the ask. The editor: its buttons. A new event: both
    // ways of making one. The which-occurrences question: a delete's too.
    expect(HELP["calendar-view"].body).toMatch(/drag an event to move it/);
    expect(HELP["calendar-event"].body).toMatch(/ask the owner for edit access/);
    expect(HELP["calendar-event"].body).toMatch(/when PM knows who owns it/);
    // Only what the copy's calendar list offers (C18 critique C2-L7).
    expect(HELP["calendar-event"].body).toMatch(/on any calendar you can create events on/);
    // Remove, where the button is and where its question is, each saying who hears of it.
    for (const id of ["calendar-event", "calendar-delete"]) {
      expect(HELP[id].body, id).toMatch(/off your calendar \(a repeating one, all of it\)/);
      expect(HELP[id].body, id).toMatch(/Google tells the organiser you declined/);
    }
    // Its buttons have their own help (`calendar-description-format`, on the toolbar).
    expect(HELP["calendar-editor"].body).toMatch(/format the description with its buttons \(hover/);
    expect(HELP["calendar-editor"].body).toMatch(/Duplicate makes a copy to change/);
    expect(HELP["calendar-editor"].body).toMatch(/organiser's unless they let guests change it/);
    expect(HELP["calendar-new-event"].body).toMatch(/full editor.*quick box/);
    expect(HELP["calendar-choices"].body).toMatch(/a delete offers the same choices/);
    expect(HELP["focus-agenda"].body).toContain("Settings → Connectors");
  });
});
