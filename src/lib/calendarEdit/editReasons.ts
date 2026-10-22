// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the event editor says (#884): why something can't be changed, and what a save came to. Every
// reason and outcome the backend can return has a line here, and the switches are exhaustive, so a
// new backend value is a `tsc` failure until it has words. The backend decides; this only explains.

import type { Tone as CalloutTone } from "../../components/ui/tone";
import { formatDateOnly } from "../format";
import type {
  EditLoad,
  MilestoneMove,
  ReadOnlyReason,
  Rsvp,
  RsvpScope,
  SeriesScope,
  SplitAction,
  WriteOutcome,
  WriteWarning,
} from "../types";
import type { DraftProblem, EditorField } from "./eventDraft";
import { rsvpLabel } from "./rsvp";

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
    case "not_invited":
      return "This calendar isn't on the event's guest list, so there's no reply to send from it.";
    case "became_repeating":
      return "This became a repeating event in Google. Refresh, then open one of its occurrences.";
    case "special_type":
      return "Birthdays, events from Gmail, focus time, out of office and working locations can't be changed here.";
    case "locked":
      return "Google has locked this event's title, time, place and description.";
    case "html_description":
      return "This description has formatting, so PM shows its text here and can't change it yet.";
    case "label_read_only":
      return "This event has a Google Calendar label, which sets its colour. Change it in Google Calendar.";
    case "reminders_unreadable":
      return "PM can't read one of this event's notifications, so it leaves them as they are.";
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
    // Google's word for it.
    case "recurrence":
      return "repeat";
    case "guests":
      return "guests";
    case "guest_perms":
      return "guest permissions";
    case "location":
      return "location";
    // Google's word for them, never "reminders".
    case "reminders":
      return "notifications";
    case "colour":
      return "colour";
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
    recurrence: "recurrence",
    attendees: "guests",
    guestsCanModify: "guest_perms",
    guestsCanInviteOthers: "guest_perms",
    guestsCanSeeOtherGuests: "guest_perms",
    location: "location",
    reminders: "reminders",
    // A colour's conflict check watches the label too: to the user, both are the colour.
    colorId: "colour",
    eventLabelId: "colour",
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

/** "a", "a and b", "a, b and c". */
function listWords(words: readonly string[]): string {
  if (words.length <= 1) return words.join("");
  return `${words.slice(0, -1).join(", ")} and ${words[words.length - 1]}`;
}

/** "title", "title and time", "title, time and location". */
export function listFields(fields: readonly EditorField[]): string {
  return listWords(fields.map(fieldLabel));
}

/** The editor's banner after it took Google's newer copy (`head` says why). The guest list merges,
 *  so it isn't Google's version that's shown: what was dropped is the change to each guest Google
 *  changed too (`lostGuests`), named. */
export function rebasedText(
  head: string,
  overwritten: readonly EditorField[],
  lostGuests: readonly string[],
): string {
  const fields = lostGuests.length > 0 ? overwritten.filter((f) => f !== "guests") : overwritten;
  const said: string[] = [];
  if (fields.length > 0) said.push(`Google's version of the ${listFields(fields)} is shown now`);
  if (lostGuests.length > 0) {
    const one = lostGuests.length === 1;
    said.push(
      `Google changed ${listWords(lostGuests)} on the guest list too, so ${
        one ? "your change to that guest wasn't kept" : "your changes to those guests weren't kept"
      }`,
    );
  }
  if (said.length === 0) return `${head} Your changes are still here; save again to apply them.`;
  return `${head} ${said.join(". ")}; your other changes are still here.`;
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
    // A new event comes back with it, a one-off made to repeat, or the one event "Does not repeat"
    // leaves of a repeating one, for every occurrence or from one on (C11).
    case "outside_mirror":
      return "It starts outside the months PM syncs, so PM's calendar won't show it.";
    // A notice per milestone names each one (`milestoneStayedText`); this is the short form.
    case "milestone_not_offered":
      return "PM couldn't find the new part in Google, so a linked milestone stays on the earlier part.";
    // Google applied a one-event save without keeping all of it (C13 review round 1): never
    // "nothing changed", and never a plain "Saved".
    case "partly_applied":
      return "Google didn't keep all of your changes, so check the event.";
    // Only a reply for every occurrence comes back with it (C14): the days are its `leftBehind`.
    case "kept_own_reply":
      return keptReplyText(leftBehind?.length ?? 0);
    default:
      return unreachable(warning);
  }
}

/** Occurrences answered on their own that kept their answer when the user replied for every
 *  occurrence (C14 critiques C1-M3, C2-H1). */
function keptReplyText(count: number): string {
  if (count === 1) return "1 occurrence you answered on its own kept its answer.";
  if (count > 1) return `${count} occurrences you answered on their own kept their answer.`;
  return "Occurrences you answered on their own kept their answer.";
}

/** What settling a change that went with Send would email guests, before a button is pressed
 *  (C12 critiques C1-M4, C2-M2): a line for each offer that would, none for the rest. `emails` is
 *  the backend's (left out when none would). */
function settleEmailsText(
  offers: readonly SplitAction[],
  emails: readonly SplitAction[] = [],
): string {
  const lines: string[] = [];
  if (offers.includes("remove") && emails.includes("remove")) {
    lines.push("If you remove the new part, guests will get a cancellation email.");
  }
  if (offers.includes("finish") && emails.includes("finish")) {
    lines.push("If you finish it, guests will get the rest of the update emails.");
  }
  return lines.join(" ");
}

/** `text`, then what settling would email guests, when anything. */
export function withSettleEmails(
  text: string,
  offers: readonly SplitAction[],
  emails?: readonly SplitAction[],
): string {
  const said = settleEmailsText(offers, emails);
  return said ? `${text} ${said}` : text;
}

/** A "this and following" change PM couldn't finish, worded by what Google's state allows now
 *  (review H3/M3): never "Saved" while the event may show twice, and never a hint to delete while
 *  the earlier part already ends before the day. Check alone is also what a new part changed in
 *  Google since leaves, so it never says PM lost touch (C9b-1 critique 2 M5). `emails`: the offers
 *  that would email guests (C12). */
export function partialText(
  offers: readonly SplitAction[],
  from: string,
  emails?: readonly SplitAction[],
): string {
  const day = formatDateOnly(from);
  const finish = offers.includes("finish");
  const remove = offers.includes("remove");
  const said = (text: string) => withSettleEmails(text, offers, emails);
  if (finish && remove) {
    return said(
      `PM couldn't finish this change: it may show twice from ${day}. Finish the change, or remove the new part.`,
    );
  }
  if (finish) {
    return said(
      `Saved from ${day}, but PM couldn't finish moving the occurrences you changed on their own. Press Finish.`,
    );
  }
  if (remove) {
    return said(
      `PM couldn't finish this change: it may show twice from ${day}. Remove the new part, or check again.`,
    );
  }
  return `PM couldn't finish this change from ${day}. Check again; if it still can't, look at that day in Google Calendar: only if it shows twice, delete the newer one.`;
}

/** An earlier change PM couldn't finish blocks this one: what it is and what to press first,
 *  worded by what Google allows now. Never a hint to delete unless Remove is offered. `emails`:
 *  the offers that would email guests (C12). */
export function earlierChangeText(
  offers: readonly SplitAction[],
  from: string,
  emails?: readonly SplitAction[],
): string {
  const head = `PM is still finishing an earlier change to it from ${formatDateOnly(from)}.`;
  const finish = offers.includes("finish");
  const remove = offers.includes("remove");
  const said = (text: string) => withSettleEmails(text, offers, emails);
  if (finish && remove) return said(`${head} Finish it or remove the new part first.`);
  if (finish) return said(`${head} Finish it first.`);
  if (remove) return said(`${head} Remove the new part, or check again.`);
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
      // Google didn't keep all of it (C13 review round 1): saved, and worth a look.
      if (outcome.warnings.includes("partly_applied")) {
        const pending = outcome.warnings.includes("mirror_refresh_pending")
          ? ` ${writeWarningText("mirror_refresh_pending", "save")}`
          : "";
        return {
          tone: "warn",
          text: `Saved to Google. ${writeWarningText("partly_applied", "save")}${pending}`,
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
      // Whether Google emails the guests (C12): never answered here by choosing occurrences.
      if (outcome.guests && !outcome.scope) {
        return {
          tone: "warn",
          text: "Choose whether Google emails the guests. Nothing was saved.",
        };
      }
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
      return { tone: "warn", text: partialText(outcome.offers, outcome.from, outcome.emails) };
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
      // Whether Google emails the guests (C12): never answered here by choosing occurrences. Said
      // when the dialog can't ask again: guests nobody had answered about, or numbers that changed
      // since the user chose (neutral: the backend knows only that, C2-L6). Unanswered, it may be
      // asking from PM's older row after Google dropped the guests, so not "it has guests now"
      // (C12c review round 2).
      if (outcome.guests && !outcome.scope) {
        return notDeleted(
          outcome.again
            ? "Who Google would email has changed since you chose. Delete it again to choose whether Google emails the guests."
            : "PM found guests who would be told. Delete it again to choose whether Google emails them.",
          "warn",
        );
      }
      // Also a delete sent with no choice that the backend wants one for: a held delete can't
      // reopen the dialog seconds later, so the user deletes again and chooses then.
      return notDeleted(
        "PM needs to ask which occurrences first. Delete it again to choose.",
        "warn",
      );
    case "ignored_by_google":
      return notDeleted("Google answered as if it deleted it, but kept it.", "warn");
    case "partial_split":
      return notDeleted(earlierChangeText(outcome.offers, outcome.from, outcome.emails), "warn");
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

/** What a reply to an invitation came to (C14), for the reply control's line: Google's words for
 *  the choice ("Yes", "No", "Maybe"), and "all events" as Google says it, never "every
 *  occurrence", since an occurrence answered on its own may keep its answer (said when it did). */
export function replyText(
  outcome: WriteOutcome,
  reply: Rsvp,
  scope: RsvpScope | null,
): { tone: Tone; text: string } {
  switch (outcome.outcome) {
    case "saved": {
      const all = scope === "all" ? " for all events" : "";
      const words = [`You replied ${rsvpLabel(reply)}${all}.`];
      if (outcome.warnings.includes("mirror_refresh_pending")) {
        words.push(writeWarningText("mirror_refresh_pending", "save"));
      }
      const kept = outcome.warnings.includes("kept_own_reply");
      if (kept) words.push(writeWarningText("kept_own_reply", "save", outcome.left_behind));
      return { tone: kept ? "warn" : "ok", text: words.join(" ") };
    }
    case "no_change":
      return { tone: "ok", text: "That's already your reply." };
    case "conflict":
      return {
        tone: "warn",
        text: "This event kept changing in Google while PM sent your reply. Try again.",
      };
    case "needs_choice":
      return { tone: "warn", text: "Choose this occurrence or all of them, then reply again." };
    // Google answered as if it took the reply, and its copy doesn't hold it (C14 critiques C1-M2,
    // C2-M2).
    case "ignored_by_google":
      return {
        tone: "warn",
        text: "Google didn't take the reply. Try again, or reply in Google Calendar.",
      };
    case "gone":
      return { tone: "error", text: GONE };
    case "read_only":
      return { tone: "error", text: reasonText(outcome.reason) };
    case "busy":
      return { tone: "warn", text: "Google is busy right now. Reply again in a moment." };
    case "reauth":
      return { tone: "error", text: REAUTH };
    case "unconfirmed":
      return {
        tone: "warn",
        text: "PM lost touch with Google before it could confirm your reply. Refresh the calendar to see whether it went through.",
      };
    case "failed":
      return { tone: "error", text: outcome.message };
    // Never a reply's answer; the switch stays exhaustive.
    case "partial_split":
    case "split_removed":
    case "ambiguous":
      return { tone: "error", text: "Your reply wasn't sent." };
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
      const said = savedWarnings(result);
      return {
        tone: said.warn ? "warn" : "ok",
        text: [`Finished the change to ${t}.`, ...said.words].join(" "),
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
        text: `${t}: ${
          result.note
            ? withSettleEmails(result.note, result.offers, result.emails)
            : partialText(result.offers, result.from, result.emails)
        }`,
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
 *  how to change it back, and for "this and following", which makes an event of its own from `from`
 *  (YYYY-MM-DD), that editing again won't join it back (C9b-1 critique 2 H2). `stops`: "Does not
 *  repeat" was saved, so one event is left, never a repeating one nor every occurrence (C11c review,
 *  C11d review 1). `asAll`: the "this and following" row was the first occurrence's, which saves
 *  every occurrence (`RowView.asAll`), so it's worded as All, a stop naming the day of the one event
 *  left, `from` (C11c review); All's own stop names none, since the one it keeps is the first
 *  occurrence still there, which the choices named. One that left the event outside the months PM
 *  syncs says so, and points to Google Calendar, never to editing it again here (C11c review, C11d
 *  review 1). */
export function savedNotice(
  title: string,
  outcome: Extract<WriteOutcome, { outcome: "saved" }>,
  scope: SeriesScope | null,
  from?: string,
  stops = false,
  asAll = false,
): { tone: Tone; text: string } {
  const t = quote(title);
  // A new event or a one-off made to repeat whose occurrences all fall outside the months PM syncs
  // (C11 §10), or the one event "Does not repeat" leaves there.
  const outside = outcome.warnings.includes("outside_mirror");
  const pending = outcome.warnings.includes("mirror_refresh_pending");
  const day = from ? formatDateOnly(from) : null;
  const saved: SeriesScope | null = asAll && scope === "following" ? "all" : scope;
  let first: string;
  let back: string | null;
  switch (saved) {
    case null:
      first = pending || outside ? `Saved ${t} to Google.` : `Saved ${t}.`;
      back = null;
      break;
    case "this":
      first = `Saved ${t} for this occurrence.`;
      back = "To change it back, edit it again.";
      break;
    case "following": {
      // Like the choices' own line: a separate event from the part before it.
      const since = day ?? "this occurrence";
      first = stops
        ? `Saved ${t} from ${since} on, as a separate event that doesn't repeat.`
        : `Saved ${t} from ${since} on, as a repeating event of its own.`;
      back = `There's no Undo: editing it again changes that ${stops ? "event" : "part"}, but won't join it back to the earlier one.`;
      break;
    }
    case "all":
      // A stop keeps one event: from the first occurrence, the one opened, where the draft put it;
      // from any other, the first occurrence still there, which `from` isn't.
      first = stops
        ? `Saved ${t} as one event that doesn't repeat${asAll && day ? `, on ${day}` : ""}.`
        : `Saved ${t} for every occurrence.`;
      back = stops
        ? "There's no Undo: to make it repeat again, edit it and pick a repeat."
        : "To change it back, edit it again.";
      break;
    default:
      return unreachable(saved);
  }
  // PM's calendar won't show it, so it can't be edited again here: Google Calendar can.
  if (outside && saved !== null) {
    back =
      saved === "following"
        ? "There's no Undo: to change it, use Google Calendar."
        : "To change it back, use Google Calendar.";
  }
  const said = savedWarnings(outcome);
  const parts = [first, ...said.words];
  // With occurrences left on the earlier part, changing it back isn't the whole story.
  if (back && !outcome.warnings.includes("left_on_old_series")) parts.push(back);
  return { tone: said.warn ? "warn" : "ok", text: parts.join(" ") };
}

/** A saved outcome's warnings in words, in one order for every notice that says a save landed
 *  (`savedNotice`, and `settleText` for a Finish): outside the months PM syncs; then PM's calendar
 *  catching up, which beside an event outside is in a delete's words, since the saved event won't
 *  show after a refresh but what it replaced still does until then (C11c review); then occurrences
 *  left on the earlier part. Google not keeping all of a save comes first (C13 review round 1). A
 *  milestone that stayed has a notice of its own, naming it. `warn`: the user should look, and
 *  nothing says to edit it again here. */
function savedWarnings(outcome: Extract<WriteOutcome, { outcome: "saved" }>): {
  words: string[];
  warn: boolean;
} {
  const partly = outcome.warnings.includes("partly_applied");
  const outside = outcome.warnings.includes("outside_mirror");
  const pending = outcome.warnings.includes("mirror_refresh_pending");
  const left = outcome.warnings.includes("left_on_old_series");
  const words: string[] = [];
  if (partly) words.push(writeWarningText("partly_applied", "save"));
  if (outside) words.push(writeWarningText("outside_mirror", "save"));
  if (pending) words.push(writeWarningText("mirror_refresh_pending", outside ? "delete" : "save"));
  if (left) words.push(writeWarningText("left_on_old_series", "save", outcome.left_behind));
  return { words, warn: partly || outside || left };
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
    // Whether Google sends the invitations (C12d): said only where the question can't be asked
    // again (a second ask in one save, or an answer after the editor has gone). Nothing was sent.
    case "needs_choice":
      if (outcome.guests && !outcome.scope) {
        return notMade(
          outcome.again
            ? "PM's count of who Google would invite differs from the one you answered. Save it again to choose whether Google sends the invitations."
            : "Choose whether Google sends the invitations, then save it again.",
          "warn",
        );
      }
      return notMade(outcomeText(outcome).text, "warn");
    // Never a create's answer; said as a change's would be.
    case "no_change":
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

// --- moving a milestone after a "this and following" change (C9b-2) ---

/** Where the earlier part ends and what a milestone left on it costs, for the words: a last day
 *  before today has ended, so it shows as overdue now; one today or later, only after that (C9b-2
 *  review round 3, R3-F3). With no last day PM can say, just that it ended. */
const staysThere = (m: MilestoneMove) => {
  const day = m.earlierLastDay;
  if (!day) return "which has ended, and shows as overdue";
  return day < m.today
    ? `which ended on ${formatDateOnly(day)}, and shows as overdue`
    : `whose last day is ${formatDateOnly(day)}, and shows as overdue after that`;
};

/** The question after a "this and following" change landed (DECISIONS 17-10 #2): the new part by
 *  its title and first day, and what saying no costs (C9b-2 critique C2-H1). */
export function moveOfferText(m: MilestoneMove): string {
  const starts = m.toDate ? `, which starts on ${formatDateOnly(m.toDate)}` : "";
  return `Move the “${m.label}” milestone in ${m.project} to the new part of ${quote(m.newTitle)}${starts}? If you don't, it stays on the earlier part, ${staysThere(m)}.`;
}

/** The Move button's accessible name. Each move button's name starts with the words it shows, so
 *  saying "click Move back" or "click Keep it there" finds it (WCAG 2.5.3). */
export function moveLabel(m: MilestoneMove): string {
  return `Move “${m.label}” to the new part of ${quote(m.newTitle)}`;
}

/** The Keep it there button's accessible name. */
export function keepLabel(m: MilestoneMove): string {
  return `Keep it there: “${m.label}” stays on the earlier part of ${quote(m.earlierTitle)}`;
}

/** A move that landed; a new part PM's calendar doesn't show yet says when it will. */
export function movedText(m: MilestoneMove): string {
  const later = m.pending ? " PM's calendar will show its new date after the next refresh." : "";
  return `Moved “${m.label}” to the new part of ${quote(m.newTitle)}.${later}`;
}

/** The Move back button's accessible name. */
export function moveBackLabel(m: MilestoneMove): string {
  return `Move back “${m.label}” to the earlier part of ${quote(m.earlierTitle)}`;
}

export function movedBackText(m: MilestoneMove): string {
  return `“${m.label}” is back on the earlier part of ${quote(m.earlierTitle)}.`;
}

/** `why`: what the move threw ("That milestone no longer exists."). */
export function moveFailedText(m: MilestoneMove, why: string): string {
  return `Couldn't move “${m.label}”. ${why}`;
}

export function moveBackFailedText(m: MilestoneMove, why: string): string {
  return `Couldn't move “${m.label}” back. ${why} Unlink it in ${m.project} if it shouldn't follow the new part.`;
}

/** A milestone PM couldn't offer to move, because it couldn't find the new part (C9b-2 critique
 *  C2-M1): where it stays, what that costs, and the way out. */
export function milestoneStayedText(m: MilestoneMove): string {
  return `PM couldn't find the new part of ${quote(m.newTitle)} in Google, so “${m.label}” in ${m.project} stays on the earlier part, ${staysThere(m)}. Unlink it in ${m.project} to set its date yourself.`;
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
    case "bad_guest":
      return `“${problem.email}” isn't an email address.`;
    default:
      return unreachable(problem);
  }
}

/** The note for a time the clocks pass twice: which of the two the save means. */
export function ambiguousText(half: "start" | "end"): string {
  return `The ${half} time happens twice that night, as the clocks go back. PM saves the first.`;
}

/** The line for typed `text` the date field named `field` ("End date", "Last day") couldn't read.
 *  It names the field, since only a date typed or picked there ends the refusal (`BadDates`), even
 *  the one it shows already. */
export function badDateText(field: string, text: string): string {
  return `${field}: “${text}” isn't a date PM can read. Type the ${field.toLowerCase()} again as dd-mm-yyyy, or pick it from the calendar.`;
}
