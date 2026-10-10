// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The calendar write fence's own rules. Each case builds the smallest tree that breaks one rule, on
// top of a baseline that passes, so the failure a test sees is its own.
//
// Importing the module does not run the gate — entry-point guard at the bottom of it.

import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { scan, WRITE_COMMANDS } from "./check-calendar-write-fence.mjs";

const roots = [];
afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

/** A tree that passes: the write module, its registration, the wrappers, the hook, and enough
 *  model-path modules for the floor. `extra` adds or replaces files. */
function fixture(extra = {}) {
  const commands = WRITE_COMMANDS.map(([c]) => `#[tauri::command]\npub async fn ${c}() {}\n`);
  const wrappers = WRITE_COMMANDS.map(
    ([c, w]) => `export const ${w} = (id: string) =>\n  invoke<WriteOutcome>("${c}", { id });\n`,
  );
  const files = {
    "src-tauri/src/lib.rs": `mod calendar_write;\nmod llm_gateway;\ncommands::update_calendar_event,`,
    "src-tauri/src/commands/calendar_edit.rs":
      `use crate::calendar_write::plan;\n${commands.join("")}` +
      `pub(super) fn edit_blocks() {}\n` +
      `mod google_io { fn send() { crate::google::authorized_send_with(); } }`,
    "src-tauri/src/calendar_write/plan.rs": `const CALENDAR_API: &str = "https://www.googleapis.com/calendar/v3";`,
    "src-tauri/src/commands/calendars.rs": `use crate::calendar_write::dto::ReadOnlyReason;\nuse crate::calendar_write::reconcile::Stamp;`,
    "src-tauri/src/calendar.rs": `const CALENDAR_API: &str = "https://www.googleapis.com/calendar/v3";\nfn list() { google::authorized_get(); }`,
    "src/lib/ipc.ts": `export const syncCalendar = () => invoke<number>("sync_calendar");\n${wrappers.join("")}export const last = 1;\n`,
    "src/components/calendar/edit/useEventWrites.ts": `import { updateCalendarEvent } from "../../../lib/ipc";`,
    "src/components/calendar/CalendarView.tsx": `import { listAllCalendarEvents } from "../../lib/ipc";`,
    "eslint.config.js": `const CALENDAR_WRITES = {\n  regex: "(^|/)ipc$",\n  importNames: [${WRITE_COMMANDS.map(([, w]) => `"${w}"`).join(", ")}],\n};`,
  };
  for (let i = 0; i < 10; i++) {
    files[`src-tauri/src/model_${i}.rs`] =
      `use crate::llm_gateway::complete;\nfn f() { calendar::upcoming(); }`;
  }
  Object.assign(files, extra);
  const root = mkdtempSync(join(tmpdir(), "pm-calendar-fence-"));
  roots.push(root);
  for (const [path, text] of Object.entries(files)) {
    if (text === null) continue;
    mkdirSync(join(root, dirname(path)), { recursive: true });
    writeFileSync(join(root, path), text);
  }
  return root;
}

const problems = (extra) => scan(fixture(extra)).problems.join("\n");

describe("calendar write fence", () => {
  it("passes on the real tree", () => {
    const root = new URL("..", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1");
    const result = scan(root);
    expect(result.problems).toEqual([]);
    expect(result.modelPath).toBeGreaterThanOrEqual(10);
  });

  it("passes on its baseline fixture", () => {
    expect(scan(fixture()).problems).toEqual([]);
  });

  it("refuses a write command named outside its module, lib.rs and ipc.ts", () => {
    expect(
      problems({ "src-tauri/src/commands/assistant.rs": `// calls update_calendar_event` }),
    ).toMatch(/assistant\.rs:1 names the calendar write command `update_calendar_event`/);
  });

  it("refuses a write wrapper anywhere but the editor's hook", () => {
    // A namespace import gets past an import rule; the name still shows.
    expect(
      problems({
        "src/components/chat/ChatView.tsx": `import * as ipc from "../../lib/ipc";\nipc.updateCalendarEvent();`,
      }),
    ).toMatch(/ChatView\.tsx:2 names the calendar write wrapper `updateCalendarEvent`/);
  });

  it("lets tests name the wrappers and commands", () => {
    expect(
      problems({
        "src/components/calendar/edit/useEventWrites.test.ts": `vi.mock("updateCalendarEvent"); // update_calendar_event`,
      }),
    ).toBe("");
  });

  it("keeps the sender private to its module", () => {
    expect(problems({ "src-tauri/src/drive.rs": `use super::google_io;` })).toMatch(
      /drive\.rs:1 names `google_io`/,
    );
  });

  it("keeps the write core to the write commands, and the sync to its types and reconcile", () => {
    expect(problems({ "src-tauri/src/flags.rs": `use crate::calendar_write::plan;` })).toMatch(
      /flags\.rs:1 uses the calendar write core/,
    );
    expect(
      problems({
        "src-tauri/src/commands/calendars.rs": `use crate::calendar_write::reconcile::Stamp;\nuse crate::calendar_write::plan::patch_event;`,
      }),
    ).toMatch(/calendars\.rs:2 uses the calendar write core.*may take its `dto` and `reconcile`/);
  });

  it("refuses a model path that names the write path or sends to Google", () => {
    const briefing = (body) =>
      problems({ "src-tauri/src/briefing.rs": `use crate::openrouter::complete;\n${body}` });
    expect(briefing(`use crate::commands::CalendarEditState;`)).toMatch(
      /briefing\.rs:2 is on a model path .* names the calendar editing state/,
    );
    expect(briefing(`google::authorized_send(&c, k, b);`)).toMatch(
      /names an authorised Google request/,
    );
    expect(briefing(`let u = "https://www.googleapis.com/calendar/v3/calendars";`)).toMatch(
      /names the Google Calendar API/,
    );
    expect(briefing(`delete_calendar_event(app)`)).toMatch(/names the `delete_calendar_event`/);
  });

  it("counts a model gateway as a model path even when it names no other gateway", () => {
    expect(
      problems({ "src-tauri/src/openrouter.rs": `fn send() { google::authorized_send(); }` }),
    ).toMatch(/openrouter\.rs:1 is on a model path/);
  });

  it("refuses a model call on the write path", () => {
    expect(
      problems({
        "src-tauri/src/calendar_write/gate.rs": `use crate::llm_gateway::complete;`,
      }),
    ).toMatch(/gate\.rs:1 is on the calendar write path and calls a model/);
  });

  it("refuses to pass when it can no longer see the model paths or the sender", () => {
    const extra = {};
    for (let i = 0; i < 10; i++) extra[`src-tauri/src/model_${i}.rs`] = null;
    expect(problems(extra)).toMatch(/only 0 model-path modules found/);
    expect(
      problems({
        "src-tauri/src/commands/calendar_edit.rs": `pub async fn update_calendar_event() {}`,
      }),
    ).toMatch(/no longer declares `mod google_io`/);
  });

  it("refuses a second Calendar writer outside the write commands", () => {
    // calendar.rs holds the API URL for its reads; a write helper beside them is a second door.
    expect(
      problems({
        "src-tauri/src/calendar.rs": `const CALENDAR_API: &str = "x";\nfn rename() { client.patch(url); }`,
      }),
    ).toMatch(/calendar\.rs:2 holds a Google Calendar API URL and can send more than a read/);
  });

  it("refuses a second wrapper around a write command in ipc.ts", () => {
    expect(
      problems({
        "src/lib/ipc.ts":
          WRITE_COMMANDS.map(
            ([c, w]) => `export const ${w} = () => invoke<WriteOutcome>("${c}");\n`,
          ).join("") +
          `export const dismissEvent = () => invoke<WriteOutcome>("delete_calendar_event");\n`,
      }),
    ).toMatch(/invokes "delete_calendar_event" 2 times/);
  });

  it("checks its own lists against the code", () => {
    // A new command in calendar_edit.rs that the fence doesn't list.
    expect(
      problems({
        "src-tauri/src/commands/calendar_edit.rs":
          WRITE_COMMANDS.map(([c]) => `#[tauri::command]\npub async fn ${c}() {}\n`).join("") +
          `#[tauri::command]\npub async fn respond_to_calendar_event() {}\nmod google_io {}`,
      }),
    ).toMatch(/defines the commands .*respond_to_calendar_event.* but this fence lists/);
    // A plain `pub fn` the commands glob would re-export.
    expect(
      problems({
        "src-tauri/src/commands/calendar_edit.rs":
          WRITE_COMMANDS.map(([c]) => `#[tauri::command]\npub async fn ${c}() {}\n`).join("") +
          `pub fn send_raw() {}\nmod google_io {}`,
      }),
    ).toMatch(/has a plain `pub fn send_raw`/);
    // A wrapper renamed in ipc.ts, and an ESLint list that drifted.
    expect(
      problems({
        "src/lib/ipc.ts": `export const removeEvent = () => invoke("delete_calendar_event");\n`,
      }),
    ).toMatch(/no `export const deleteCalendarEvent` invoking "delete_calendar_event"/);
    expect(
      problems({
        "eslint.config.js": `const CALENDAR_WRITES = { importNames: ["updateCalendarEvent"] };`,
      }),
    ).toMatch(/eslint\.config\.js restricts the imports \[updateCalendarEvent\]/);
  });
});
