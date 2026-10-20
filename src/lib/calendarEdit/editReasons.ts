// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the event editor says (#884): why something can't be changed, and what a save came to. Every
// reason and outcome the backend can return has a line here, and the switches are exhaustive, so a
// new backend value is a `tsc` failure until it has words. The backend decides; this only explains.

import type { Tone as CalloutTone } from "../../components/ui/tone";
import { formatDateOnly } from "../format";
import type {
  EditLoad,
  ReadOnlyReason,
  SeriesScope,
  SplitAction,
  WriteOutcome,
  WriteWarning,
} from "../types";
import type { BadDates, DraftProblem, EditorField } from "./eventDraft";

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
    case "became_repeating":
      return "This became a repeating event in Google. Refresh, then open one of its occurrences.";
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

/** How each tone shows as a Callout: the editor's banner, the calendar's notices and the
 *  repeating-event dialog read this one map. */
export const CALLOUT_TONE: Record<Tone, CalloutTone> = {
  ok: "info",
  warn: "warning",
  error: "danger",
};

/** Whether words are for a change or a delete. */
export type ChoiceKind = "save" | "delete";

const REAUTH =
  "Google needs you to sign in again. Reconnect the account in Settings › Connectors, then try again.";
// A 404 doesn't say which: the event was deleted, or the calendar stopped being visible to the account.
const GONE =
  "This event is no longer in Google: it was deleted there, or this account can't see it any more.";

/** The days of occurrences a change left behind, as a parenthesis to put in its sentence: up to
 *  five, then how many more. Empty when there are none. */
export function leftBehindText(days?: readonly string[]): string {
  if (!days || days.length === 0) return "";
  const shown = days.slice(0, 5).map(formatDateOnly).join(", ");
  const more = days.length > 5 ? ` and ${days.length - 5} more` : "";
  return ` (${shown}${more})`;
}

/** A warning a save or a delete came back with, as a sentence. `leftBehind`: the days of
 *  `left_on_old_series`. A delete's occurrences "may still show"; a save's may be on the earlier
 *  part. */
export function writeWarningText(
  warning: WriteWarning,
  kind: ChoiceKind,
  leftBehind?: readonly string[],
): string {
  switch (warning) {
    case "mirror_refresh_pending":
      return kind === "save"
        ? "PM's calendar will show it after the next refresh."
        : "PM's calendar will catch up after the next refresh.";
    case "left_on_old_series": {
      const days = leftBehindText(leftBehind);
      return kind === "save"
        ? `Some occurrences you changed on their own may still be on the earlier part, or hidden by Google${days}; check them in Google Calendar.`
        : `Some occurrences changed on their own may still show${days}; check them in Google Calendar.`;
    }
    // Only a new event comes back with it.
    case "outside_mirror":
      return "It starts outside the months PM syncs, so PM's calendar won't show it.";
    default:
      return unreachable(warning);
  }
}

/** A "this and following" change PM couldn't finish, worded by what Google's state allows now
 *  (review H3/M3): never "Saved" while the event may show twice, and never a hint to delete while
 *  the earlier part already ends before the day. Check alone is also what a new part changed in
 *  Google since leaves, so it never says PM lost touch (C9b-1 critique 2 M5). */
export function partialText(offers: readonly SplitAction[], from: string): string {
  const day = formatDateOnly(from);
  const finish = offers.includes("finish");
  const remove = offers.includes("remove");
  if (finish && remove) {
    return `PM couldn't finish this change: it may show twice from ${day}. Finish the change, or remove the new part.`;
  }
  if (finish) {
    return `Saved from ${day}, but PM couldn't finish moving the occurrences you changed on their own. Press Finish.`;
  }
  if (remove) {
    return `PM couldn't finish this change: it may show twice from ${day}. Remove the new part, or check again.`;
  }
  return `PM couldn't finish this change from ${day}. Check again; if it still can't, look at that day in Google Calendar: only if it shows twice, delete the newer one.`;
}

/** An earlier change PM couldn't finish blocks this one: what it is and what to press first,
 *  worded by what Google allows now. Never a hint to delete unless Remove is offered. */
export function earlierChangeText(offers: readonly SplitAction[], from: string): string {
  const head = `PM is still finishing an earlier change to it from ${formatDateOnly(from)}.`;
  const finish = offers.includes("finish");
  const remove = offers.includes("remove");
  if (finish && remove) return `${head} Finish it or remove the new part first.`;
  if (finish) return `${head} Finish it first.`;
  if (remove) return `${head} Remove the new part, or check again.`;
  return `${head} Check again; if it still can't, look at that day in Google Calendar.`;
}

/** What a save came to, and how it should look. A conflict names the fields Google changed; it
 *  promises only that nothing was saved over them (the editor, rebasing onto Google's copy, says
 *  which of the user's changes Google's replaced). Deletes have their own words: `deleteText`. */
export function outcomeText(outcome: WriteOutcome): { tone: Tone; text: string } {
  switch (outcome.outcome) {
    case "saved":
      // A "this and following" change that couldn't move every occurrence changed on its own.
      if (outcome.warnings.includes("left_on_old_series")) {
        return {
          tone: "warn",
          text: `Saved from that day on. ${writeWarningText("left_on_old_series", "save", outcome.left_behind)}`,
        };
      }
      return {
        tone: "ok",
        text: outcome.warnings.includes("mirror_refresh_pending")
          ? `Saved to Google. ${writeWarningText("mirror_refresh_pending", "save")}`
          : "Saved.",
      };
    case "no_change":
      return { tone: "ok", text: "Nothing to save: Google already has this." };
    case "needs_choice":
      return {
        tone: "warn",
        text: "This repeating event changed since you chose which occurrences to change. Choose again; nothing was saved.",
      };
    case "ignored_by_google":
      return {
        tone: "warn",
        text: outcome.split
          ? "Google answered as if it saved this, but kept the event as it was, so PM removed the new part it had made. Nothing changed."
          : "Google answered as if it saved this, but kept the event as it was. Nothing changed.",
      };
    case "partial_split":
      return { tone: "warn", text: partialText(outcome.offers, outcome.from) };
    case "split_removed":
      return {
        tone: "ok",
        text: "PM removed the new part it had made. The repeating event is as it was.",
      };
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
    // Only creating an event answers this; its token is kept so Check again can look first.
    case "ambiguous":
      return {
        tone: "warn",
        text: "PM lost touch with Google while creating this event, so it may already be in your calendar. Check again: PM looks for it first and sends it again only if Google still doesn't show it.",
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
      // Occurrences changed on their own that a "this and following" delete couldn't cancel.
      if (outcome.warnings.includes("left_on_old_series")) {
        return {
          tone: "warn",
          text: `Deleted ${t} from that day on. ${writeWarningText("left_on_old_series", "delete", outcome.left_behind)}`,
        };
      }
      return {
        tone: "ok",
        text: outcome.warnings.includes("mirror_refresh_pending")
          ? `Deleted ${t} from Google. ${writeWarningText("mirror_refresh_pending", "delete")}`
          : `Deleted ${t}.`,
      };
    case "gone":
      return {
        tone: "ok",
        text: `${t} is no longer in Google: it was deleted there, or this account can't see it any more.`,
      };
    case "no_change":
      return notDeleted("Google had nothing to change.", "warn");
    case "needs_choice":
      // Also a delete sent with no choice that the backend wants one for: a held delete can't
      // reopen the dialog seconds later, so the user deletes again and chooses then.
      return notDeleted(
        "PM needs to ask which occurrences first. Delete it again to choose.",
        "warn",
      );
    case "ignored_by_google":
      return notDeleted("Google answered as if it deleted it, but kept it.", "warn");
    case "partial_split":
      return notDeleted(earlierChangeText(outcome.offers, outcome.from), "warn");
    case "split_removed":
      return notDeleted("PM removed the new part of an earlier change instead.", "warn");
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
    // Only creating an event answers this; a delete never does, so it says only what's known.
    case "ambiguous":
      return {
        tone: "warn",
        text: `PM lost touch with Google, so it can't tell whether ${t} was deleted. Refresh the calendar to see.`,
      };
    case "failed":
      return notDeleted(outcome.message);
    default:
      return unreachable(outcome);
  }
}

/** A delete of chosen occurrences that the plan changed under again, after the dialog reopened
 *  once (or with something else open by then): the user deletes again and chooses then. */
export function deleteChangedText(title: string): { tone: Tone; text: string } {
  return {
    tone: "warn",
    text: `${quote(title)} wasn't deleted. This repeating event changed since you chose which occurrences to delete. Delete it again to choose.`,
  };
}

const quote = (title: string) => `“${title.trim() || "(no title)"}”`;

/** The buttons for a change PM couldn't finish, always in this order, filtered by what's offered. */
export const SPLIT_ORDER: readonly SplitAction[] = ["finish", "remove", "check"];

/** A settle button's label. */
export function splitActionLabel(action: SplitAction): string {
  switch (action) {
    case "finish":
      return "Finish";
    case "remove":
      return "Remove the new part";
    case "check":
      return "Check again";
    default:
      return unreachable(action);
  }
}

/** What settling a change PM couldn't finish came to, as a notice naming the event `title`. One
 *  still held carries its own words (`note`), or says what Google allows now. */
export function settleText(result: WriteOutcome, title: string): { tone: Tone; text: string } {
  const t = quote(title);
  switch (result.outcome) {
    case "saved": {
      const warnings = result.warnings.map((w) => writeWarningText(w, "save", result.left_behind));
      return {
        tone: result.warnings.includes("left_on_old_series") ? "warn" : "ok",
        text: [`Finished the change to ${t}.`, ...warnings].join(" "),
      };
    }
    case "split_removed":
      return {
        tone: "ok",
        text: `Removed the new part of ${t}. The repeating event is as it was.`,
      };
    case "partial_split":
      return {
        tone: "warn",
        text: `${t}: ${result.note ?? partialText(result.offers, result.from)}`,
      };
    // PM's own claim on the repeating event comes back as `partial_split` with a note, the change still
    // held. A plain busy means Google refused Finish's next step and PM took its new part back, the
    // change no longer held (C9b-1a review round 2).
    case "busy":
      return {
        tone: "warn",
        text: `${t}: Google is busy right now, so PM put the repeating event back as it was. Make the change again in a moment.`,
      };
    // `ambiguous` is listed for completeness: only creating an event answers it; a settle never does.
    case "no_change":
    case "needs_choice":
    case "ignored_by_google":
    case "conflict":
    case "gone":
    case "read_only":
    case "reauth":
    case "unconfirmed":
    case "ambiguous":
    case "failed": {
      const said = outcomeText(result);
      return { tone: said.tone, text: `${t}: ${said.text}` };
    }
    default:
      return unreachable(result);
  }
}

/** A save that landed, said once the editor has closed. `scope` names the occurrences of a
 *  repeating event it was for: there's no Undo after one (DECISIONS 17-10 #4), so the notice says
 *  how to change it back, and for "this and following", which makes a repeating event of its own
 *  from `from` (YYYY-MM-DD), that editing again won't join it back (C9b-1 critique 2 H2). */
export function savedNotice(
  title: string,
  outcome: Extract<WriteOutcome, { outcome: "saved" }>,
  scope: SeriesScope | null,
  from?: string,
): { tone: Tone; text: string } {
  const t = quote(title);
  const pending = outcome.warnings.includes("mirror_refresh_pending");
  const left = outcome.warnings.includes("left_on_old_series");
  let first: string;
  let back: string | null;
  switch (scope) {
    case null:
      first = pending ? `Saved ${t} to Google.` : `Saved ${t}.`;
      back = null;
      break;
    case "this":
      first = `Saved ${t} for this occurrence.`;
      back = "To change it back, edit it again.";
      break;
    case "following":
      first = `Saved ${t} from ${from ? formatDateOnly(from) : "this occurrence"} on, as a repeating event of its own.`;
      back =
        "There's no Undo: editing it again changes that part, but won't join it back to the earlier one.";
      break;
    case "all":
      first = `Saved ${t} for every occurrence.`;
      back = "To change it back, edit it again.";
      break;
    default:
      return unreachable(scope);
  }
  const parts = [first];
  if (pending) parts.push(writeWarningText("mirror_refresh_pending", "save"));
  // Occurrences left on the earlier part come first: changing it back isn't the whole story.
  if (left) parts.push(writeWarningText("left_on_old_series", "save", outcome.left_behind));
  else if (back) parts.push(back);
  return { tone: left ? "warn" : "ok", text: parts.join(" ") };
}

// --- creating an event (C10) ---

/** The backend's words for a new event's retry token it no longer holds (PM restarted, or eight
 *  hours passed), and for a retry naming a draft other than the one it was creating. Copies of the
 *  backend's own, which a test there pins to these: the webview tells these two from every other
 *  failure only by their words, and both end the token. */
export const CREATE_LOST =
  "PM lost touch with Google and can't tell whether this new event was made. Look for it in Google Calendar before creating it again, so it isn't made twice.";
export const CREATE_CHANGED =
  "This differs from the new event PM was creating when it lost touch with Google, so PM didn't send it. PM may already have made the earlier one: look for it in Google Calendar before creating it again, so it isn't made twice.";

/** Whether a create's answer ends its retry token: only Google showing the event made (`saved`),
 *  made and then deleted (`gone`), or the backend no longer holding it or refusing a changed draft.
 *  Anything else may still leave the first copy in Google, so Check again stays (C10 critiques). */
export function endsRetry(outcome: WriteOutcome): boolean {
  if (outcome.outcome === "saved" || outcome.outcome === "gone") return true;
  return (
    outcome.outcome === "failed" &&
    (outcome.message === CREATE_LOST || outcome.message === CREATE_CHANGED)
  );
}

/** The retry token a create's answer leaves: the one an `ambiguous` answer gives, else the one that
 *  was sent while nothing ended it, else none. */
export function retryAfter(sent: string | null, outcome: WriteOutcome): string | null {
  if (outcome.outcome === "ambiguous") return outcome.retry_token;
  return sent !== null && !endsRetry(outcome) ? sent : null;
}

/** Where a new event went: its calendar's name, and whether PM's calendar keeps it out of sight
 *  there (`createdWhere`). */
export interface CreatedWhere {
  name: string;
  hidden: boolean;
}

/** Why a calendar can't take a new event, in a create's terms (the rest read as for a change). */
export function createReasonText(reason: ReadOnlyReason): string {
  switch (reason) {
    case "private_event":
      return "This calendar's sharing doesn't let you make private events on it.";
    case "calendar_read_only":
      return "This calendar doesn't let you add events. Choose another.";
    default:
      return reasonText(reason);
  }
}

/** A new event Google holds, once the editor has closed (C10 §15): created, and whether PM's
 *  calendar will show it. Ok when it will, a warning (which stays) when it won't. */
export function createdText(
  title: string,
  outcome: Extract<WriteOutcome, { outcome: "saved" }>,
  where: CreatedWhere,
): { tone: Tone; text: string } {
  const t = quote(title);
  if (outcome.warnings.includes("outside_mirror")) {
    return {
      tone: "warn",
      text: `Created ${t} in Google. It starts outside the months PM syncs, so it won't show here.`,
    };
  }
  if (where.hidden) {
    return { tone: "warn", text: `Created ${t} in ${where.name}, which is hidden here.` };
  }
  if (outcome.warnings.includes("mirror_refresh_pending")) {
    return {
      tone: "ok",
      text: `Created ${t} in Google. ${writeWarningText("mirror_refresh_pending", "save")}`,
    };
  }
  return { tone: "ok", text: `Created ${t}.` };
}

/** What creating `title` came to when it wasn't saved, in the editor's banner or a notice that
 *  outlives it. `held`: a retry token is still held after it (`retryAfter`), so the first copy may
 *  be in Google: it never says the event wasn't made, and Check again stays. */
export function createText(
  outcome: WriteOutcome,
  title: string,
  held: boolean,
): { tone: Tone; text: string } {
  const t = quote(title);
  const notMade = (why: string, tone: Tone = "error") =>
    held
      ? { tone: "warn" as const, text: `${t}: ${why}` }
      : { tone, text: `${t} wasn't created: ${why}` };
  switch (outcome.outcome) {
    case "saved":
      return { tone: "ok", text: `Created ${t}.` };
    case "ambiguous":
      return {
        tone: "warn",
        text: `PM lost touch with Google while creating ${t}, so it may already be in your calendar. Check again: PM looks for it first and sends it again only if Google still doesn't show it.`,
      };
    case "gone":
      return { tone: "warn", text: `PM made ${t}, but it has since been deleted in Google.` };
    case "failed":
      if (outcome.message === CREATE_LOST) {
        return {
          tone: "warn",
          text: `PM lost touch with Google and can't tell whether ${t} was made. Look for it in Google Calendar before creating it again, so it isn't made twice.`,
        };
      }
      if (outcome.message === CREATE_CHANGED) {
        return {
          tone: "warn",
          text: `${t} wasn't sent again: it differs from the event PM was creating when it lost touch with Google. Look for it in Google Calendar before creating it again, so it isn't made twice.`,
        };
      }
      return notMade(outcome.message);
    // Held, the draft is locked to what was sent, so there's no other calendar to choose: the
    // change's own words say why, and that the first send may have landed.
    case "read_only":
      return held
        ? {
            tone: "warn",
            text: `${t} wasn't sent again: ${reasonText(outcome.reason)} PM may already have made it: look for it in Google Calendar before creating it again, so it isn't made twice.`,
          }
        : notMade(createReasonText(outcome.reason));
    case "reauth":
      return held
        ? {
            tone: "warn",
            text: `${t}: Google needs you to sign in again. Reconnect the account in Settings › Connectors, then check again.`,
          }
        : notMade(REAUTH);
    case "busy":
      return held
        ? { tone: "warn", text: `${t}: Google is busy right now. Check again in a moment.` }
        : notMade("Google is busy right now. Try again in a moment.", "warn");
    // Never a create's answer; said as a change's would be.
    case "no_change":
    case "needs_choice":
    case "ignored_by_google":
    case "partial_split":
    case "split_removed":
    case "conflict":
    case "unconfirmed": {
      const said = outcomeText(outcome);
      return notMade(said.text, said.tone);
    }
    default:
      return unreachable(outcome);
  }
}

/** A surface closed while PM may already have made its new event: what the notice it leaves says. */
export function heldCreateText(title: string): { tone: Tone; text: string } {
  return {
    tone: "warn",
    text: `PM may already have made ${quote(title)}. Check again: PM looks for it in Google first and sends it again only if it isn't there.`,
  };
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

/** The line for each typed date a field couldn't read (`BadDates`), the start's first. */
export function badDateLines(bad: BadDates): string[] {
  return [bad.start, bad.end]
    .filter((text) => text !== null)
    .map(
      (text) =>
        `“${text}” isn't a date PM can read. Type it as dd-mm-yyyy, or pick it from the calendar.`,
    );
}
