// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the event editor says (#884): why something can't be changed, and what a save came to. Every
// reason and outcome the backend can return has a line here, and the switches are exhaustive, so a
// new backend value is a `tsc` failure until it has words. The backend decides; this only explains.

import type { EditLoad, ReadOnlyReason, WriteOutcome } from "../types";
import type { DraftProblem, EditorField } from "./eventDraft";

function unreachable(value: never): never {
  throw new Error(`unhandled calendar edit value: ${JSON.stringify(value)}`);
}

/** Why PM won't change an event, or a part of it. */
export function reasonText(reason: ReadOnlyReason): string {
  switch (reason) {
    case "not_google":
      return "This event comes from a calendar PM can only read: a subscription, or an Outlook calendar.";
    case "editing_off":
      return "Editing isn't on for this Google account. Turn it on in Settings › Connectors › Google Calendar.";
    case "calendar_read_only":
      return "This calendar is shared with you to view only, so its events can't be changed here.";
    case "private_event":
      return "This is a private event on a calendar shared with you, and you can't change private events there.";
    case "not_organizer":
      return "Someone else organises this event, so only they can change it.";
    case "recurring":
      return "This is a repeating event, and PM can't change repeating events yet.";
    case "has_guests":
      return "This event has guests, and PM can't change events with guests yet.";
    case "special_type":
      return "Birthdays, events from Gmail, focus time, out of office and working locations can't be changed here.";
    case "locked":
      return "Google has locked this event's title, time, place and description.";
    case "html_description":
      return "This description has formatting, so PM shows its text here and can't change it yet.";
    default:
      return unreachable(reason);
  }
}

/** A field's name in a sentence. */
export function fieldLabel(field: EditorField): string {
  switch (field) {
    case "summary":
      return "title";
    case "time":
      return "time";
    case "location":
      return "location";
    case "description":
      return "description";
    case "show_as":
      return "busy or free";
    case "visibility":
      return "visibility";
    default:
      return unreachable(field);
  }
}

/** Google's field names in a conflict, as the editor's fields (start and end are both "time"). */
export function conflictFields(keys: readonly string[]): EditorField[] {
  const map: Record<string, EditorField> = {
    summary: "summary",
    start: "time",
    end: "time",
    location: "location",
    description: "description",
    transparency: "show_as",
    visibility: "visibility",
  };
  const out: EditorField[] = [];
  for (const key of keys) {
    const field = map[key];
    if (field && !out.includes(field)) out.push(field);
  }
  return out;
}

/** "title", "title and time", "title, time and location". */
export function listFields(fields: readonly EditorField[]): string {
  const words = fields.map(fieldLabel);
  if (words.length <= 1) return words.join("");
  return `${words.slice(0, -1).join(", ")} and ${words[words.length - 1]}`;
}

export type Tone = "ok" | "warn" | "error";

const REAUTH =
  "Google needs you to sign in again. Reconnect the account in Settings › Connectors, then try again.";
// A 404 doesn't say which: the event was deleted, or the calendar stopped being visible to the account.
const GONE =
  "This event is no longer in Google: it was deleted there, or this account can't see it any more.";

/** What a save came to, and how it should look. A conflict names the fields Google changed; it
 *  promises only that nothing was saved over them (the editor, rebasing onto Google's copy, says
 *  which of the user's changes Google's replaced). Deletes have their own words: `deleteText`. */
export function outcomeText(outcome: WriteOutcome): { tone: Tone; text: string } {
  switch (outcome.outcome) {
    case "saved":
      return {
        tone: "ok",
        text: outcome.warnings.includes("mirror_refresh_pending")
          ? "Saved to Google. PM's calendar will show it after the next refresh."
          : "Saved.",
      };
    case "no_change":
      return { tone: "ok", text: "Nothing to save: Google already has this." };
    case "conflict": {
      const fields = conflictFields(outcome.fields);
      const what = fields.length ? ` (the ${listFields(fields)})` : "";
      return {
        tone: "warn",
        text: `This event changed in Google while you were editing${what}. Nothing was saved over it.`,
      };
    }
    case "gone":
      return { tone: "error", text: GONE };
    case "read_only":
      return { tone: "error", text: reasonText(outcome.reason) };
    case "busy":
      return {
        tone: "warn",
        text: "Google is busy right now. Try again in a moment; your changes are still here.",
      };
    case "reauth":
      return { tone: "error", text: REAUTH };
    case "unconfirmed":
      return {
        tone: "warn",
        text: "PM lost touch with Google before it could confirm this save. Refresh the calendar to see whether it went through.",
      };
    case "failed":
      return { tone: "error", text: outcome.message };
    default:
      return unreachable(outcome);
  }
}

/** What a delete of the event called `title` came to, as one sentence naming it. A delete that
 *  lands reports `saved`; one that found the event already gone has what it wanted. */
export function deleteText(outcome: WriteOutcome, title: string): { tone: Tone; text: string } {
  const t = `“${title.trim() || "(no title)"}”`;
  const notDeleted = (why: string, tone: Tone = "error") => ({
    tone,
    text: `${t} wasn't deleted. ${why}`,
  });
  switch (outcome.outcome) {
    case "saved":
      return {
        tone: "ok",
        text: outcome.warnings.includes("mirror_refresh_pending")
          ? `Deleted ${t} from Google. PM's calendar will catch up after the next refresh.`
          : `Deleted ${t}.`,
      };
    case "gone":
      return {
        tone: "ok",
        text: `${t} is no longer in Google: it was deleted there, or this account can't see it any more.`,
      };
    case "no_change":
      return notDeleted("Google had nothing to change.", "warn");
    case "conflict": {
      const fields = conflictFields(outcome.fields);
      const what = fields.length ? ` (the ${listFields(fields)})` : "";
      return notDeleted(`It changed in Google since you opened it${what}.`, "warn");
    }
    case "read_only":
      return notDeleted(reasonText(outcome.reason));
    case "busy":
      return notDeleted("Google is busy right now; try again in a moment.", "warn");
    case "reauth":
      return notDeleted(REAUTH);
    case "unconfirmed":
      return {
        tone: "warn",
        text: `PM lost touch with Google before it could confirm whether ${t} was deleted. Refresh the calendar to see.`,
      };
    case "failed":
      return notDeleted(outcome.message);
    default:
      return unreachable(outcome);
  }
}

/** Why the editor couldn't open. */
export function loadText(load: Exclude<EditLoad, { outcome: "ready" }>): string {
  switch (load.outcome) {
    case "gone":
      return GONE;
    case "reauth":
      return REAUTH;
    case "failed":
      return load.message;
    default:
      return unreachable(load);
  }
}

/** What stops a draft saving, in words. */
export function problemText(problem: DraftProblem): string {
  switch (problem.kind) {
    case "end_before_start":
      return "The event has to end after it starts.";
    case "gap":
      return `${problem.time} doesn't happen in ${problem.zone} that night: the clocks skip it. Pick a time outside that hour.`;
    case "unreadable": {
      const half = problem.half === "start" ? "start" : "end";
      return problem.what === "zone"
        ? `The ${half} time zone isn't one PM knows.`
        : `The ${half} ${problem.what} isn't one PM can read.`;
    }
    default:
      return unreachable(problem);
  }
}

/** The note for a time the clocks pass twice: which of the two the save means. */
export function ambiguousText(half: "start" | "end"): string {
  return `The ${half} time happens twice that night, as the clocks go back. PM saves the first.`;
}
