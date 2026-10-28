// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the event editor says (#884): why something can't be changed, and what a save came to. Every
// reason and outcome the backend can return has a line here, and the switches are exhaustive, so a
// new backend value is a `tsc` failure until it has words. The backend decides; this only explains.

import type { Tone as CalloutTone } from "../../components/ui/tone";
import { formatDateOnly } from "../format";
import type {
  CalendarEvent,
  CalendarOverview,
  EditLoad,
  MilestoneMove,
  Notify,
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

const NOT_ORGANISER = "Someone else organises this event.";

/** What stays the organiser's on an event they haven't let guests change (C14c). */
const THEIRS =
  "Someone else organises this event, so only they can change its title, time, place, description, visibility and guests.";

/** What the user may change on someone else's event: their reply only where its calendar is on the
 *  guest list (C14 critique C2-L1). */
const ownSettings = (canReply: boolean) =>
  canReply
    ? "Your reply, notifications, colour and busy or free are yours to change."
    : "Your notifications, colour and busy or free are yours to change.";

/** Under an event someone else organises who lets guests change it (A11): a paraphrase of Google's
 *  developer guide ("only reflected on their own copy and might be lost if the organizer makes a
 *  change"). Live test L-I3 records whether Google keeps a guest's change on their copy alone, as
 *  the guide says, or shows it to everyone, as Workspace help reads; the wording follows it (C14
 *  critiques C1-L10, C2-M5). */
export const A11_NOTE =
  "The organiser lets guests change this event. Your changes show on your copy, and may be lost if the organiser changes the event.";

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
    // Never promises a reply: the calendar may not be on the guest list (C14 critiques C1-L10,
    // C2-L1). Where PM knows it is, `notOrganizerText` says so.
    case "not_organizer":
      return `${THEIRS} ${ownSettings(false)}`;
    // Group invitations list the group, not the user, so there's no entry of theirs to answer from
    // (C14 critique C2-M8).
    case "not_invited":
      return "PM can only reply where this calendar is on the guest list itself (not through a group).";
    case "organizer_only_guests":
      return "In PM, only the organiser can change who's invited.";
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

/** The line under an event someone else organises, in the popover and the editor (C14 critiques
 *  C1-L10, C2-L1, C2-L10): what the user may still do from this copy. `canReply`: its calendar is on
 *  the guest list; `editingOn`: its account has editing on, without which nothing is sent, a reply
 *  included. Where the organiser lets guests change the event, `A11_NOTE` says it instead. */
export function notOrganizerText(canReply: boolean, editingOn: boolean): string {
  if (!editingOn) {
    const what = canReply ? "reply and change your own settings" : "change your own settings";
    return `${NOT_ORGANISER} Turn on editing for this account to ${what} from PM.`;
  }
  return `${THEIRS} ${ownSettings(canReply)}`;
}

/** What PM may do to a synced event from the calendar view (#884): Edit, Delete and a drag (C16a),
 *  and the line that says what it can't, or why it is someone else's. */
export interface EventRights {
  canEdit: boolean;
  canDelete: boolean;
  /** Its start and end may change (a drag, a resize). */
  canRetime: boolean;
  reason: string | null;
}

/** An event's rights in the calendar view, from the row's own (the list's `edit_block`,
 *  `can_delete` and `can_retime`, each the gate's answer assuming editing is on) and its account's
 *  editing status in `overview`. One predicate for the popover's Edit and Delete, the dedup and a
 *  drag (C16a), so none offers what the others refuse; the backend checks again on Google's copy at
 *  every write. */
export function eventRights(ev: CalendarEvent, overview: CalendarOverview | null): EventRights {
  const none = (reason: string | null): EventRights => ({
    canEdit: false,
    canDelete: false,
    canRetime: false,
    reason,
  });
  const cal = overview?.calendars.find((c) => c.id === ev.calendar_id);
  const on = cal?.provider === "google" && overview?.editing[cal.source_id] === "on";
  // Someone else's event says what the user may still do from this copy: reply when it's on the
  // guest list, which the strip above the line offers, their own settings, and how when editing is
  // off (C14b, C14c).
  const theirs = ev.organizer_self === false;
  if (ev.edit_block === "not_organizer") {
    return none(notOrganizerText(ev.can_respond === true, on));
  }
  if (ev.edit_block) return none(reasonText(ev.edit_block));
  if (cal?.provider !== "google") return none(null);
  if (!on) {
    return none(
      theirs ? notOrganizerText(ev.can_respond === true, false) : reasonText("editing_off"),
    );
  }
  // The gate decides Delete (C14) and a drag (C16 critique C2-M4): a locked copy, whose delete Google
  // doesn't document and whose times it refuses, and an invitation can't be deleted, nor dragged
  // unless the organiser lets guests change it. A row the list didn't mark gets neither. On an
  // invitation the line says what stays the organiser's, or Google's warning where they let guests
  // change it.
  const reason = ev.locked
    ? reasonText("locked")
    : theirs
      ? ev.guests_can_modify
        ? A11_NOTE
        : notOrganizerText(ev.can_respond === true, true)
      : null;
  return {
    canEdit: true,
    canDelete: ev.can_delete === true,
    canRetime: ev.can_retime === true,
    reason,
  };
}

/** The calendar's dedup rank for one copy of an occurrence two accounts both show (C14 §9.5): the
 *  copy the user organises and can change 2, any other they can change 1, else 0. */
export function copyRank(rights: EventRights, ev: CalendarEvent): number {
  if (!rights.canEdit) return 0;
  return ev.organizer_self === true ? 2 : 1;
}

/** Under the Repeat row of an event someone else organises: how it repeats stays theirs, whatever
 *  they let guests change (C14 critiques C1-H2, C2-H2; the backend's `InvitationRule`). */
export function repeatTheirsText(repeating: boolean): string {
  return repeating
    ? "Someone else organises this repeating event, so only they can change how it repeats."
    : "Someone else organises this event, so only they can make it repeat.";
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
      return savedReplyText(outcome, `You replied ${rsvpLabel(reply)}${all}.`);
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

/** A saved reply's words: `head`, then what PM's calendar still owes and the occurrences that kept
 *  their own answer. */
function savedReplyText(
  outcome: Extract<WriteOutcome, { outcome: "saved" }>,
  head: string,
): { tone: Tone; text: string } {
  const words = [head];
  if (outcome.warnings.includes("mirror_refresh_pending")) {
    words.push(writeWarningText("mirror_refresh_pending", "save"));
  }
  const kept = outcome.warnings.includes("kept_own_reply");
  if (kept) words.push(writeWarningText("kept_own_reply", "save", outcome.left_behind));
  return { tone: kept ? "warn" : "ok", text: words.join(" ") };
}

/** What a reply came to, as one of the calendar's notices, naming the event (C14 critique C2-M1):
 *  for a reply whose control went before its answer came (the popover closed), and for any answer
 *  that isn't a success, which mustn't go with the popover. */
export function replyNoticeText(
  outcome: WriteOutcome,
  reply: Rsvp,
  scope: RsvpScope | null,
  title: string,
): { tone: Tone; text: string } {
  if (outcome.outcome === "saved") {
    const all = scope === "all" ? "all events of " : "";
    return savedReplyText(outcome, `You replied ${rsvpLabel(reply)} to ${all}${quote(title)}.`);
  }
  if (outcome.outcome === "no_change") {
    return { tone: "ok", text: `That's already your reply to ${quote(title)}.` };
  }
  const said = replyText(outcome, reply, scope);
  return { tone: said.tone, text: `Your reply to ${quote(title)}: ${said.text}` };
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
 *  repeating event it was for. A save of one event or of "This event" that the backend offers an
 *  Undo for (C16d) says so through `undo`, the Send / Don't send it went with: the Undo button says
 *  how to change it back, and where the save emailed guests the words say an Undo emails them again.
 *  Without one (a save that changed guests or how it repeats, or one of several occurrences, which
 *  never has one, DECISIONS 17-10 #4), the notice says how to change it back, and for "this and
 *  following", which makes an event of its own from `from`
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
  undo: { sent: Notify | null } | null = null,
): { tone: Tone; text: string } {
  const t = quote(title);
  // A new event or a one-off made to repeat whose occurrences all fall outside the months PM syncs
  // (C11 §10), or the one event "Does not repeat" leaves there.
  const outside = outcome.warnings.includes("outside_mirror");
  const pending = outcome.warnings.includes("mirror_refresh_pending");
  const day = from ? formatDateOnly(from) : null;
  const saved: SeriesScope | null = asAll && scope === "following" ? "all" : scope;
  // The Undo on offer says how to change it back (C16d); only one or "This event" has one.
  const undoing = undo !== null && (saved === null || saved === "this");
  let first: string;
  let back: string | null;
  switch (saved) {
    case null:
      first = pending || outside ? `Saved ${t} to Google.` : `Saved ${t}.`;
      back = null;
      break;
    case "this":
      first = `Saved ${t} for this occurrence.`;
      back = undoing ? null : "To change it back, edit it again.";
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
  if (undoing && emailsGuests(undo?.sent ?? null)) {
    parts.push("Guests will get a second email if you undo.");
  }
  return { tone: said.warn ? "warn" : "ok", text: parts.join(" ") };
}

/** The Undo button's name after an editor's save (C16d): what it undoes, and that the guests get a
 *  second email where the save emailed them. */
export function undoSaveLabel(title: string, sent: Notify | null): string {
  return `Undo saving ${quote(title)}${emailsGuests(sent) ? " (guests get a second email)" : ""}`;
}

/** What undoing an editor's save of the event called `title` came to (C16d): `moveBackText`'s
 *  answers, in a save's words. */
export function saveBackText(outcome: WriteOutcome, title: string): { tone: Tone; text: string } {
  const t = quote(title);
  const notBack = (why: string, tone: Tone = "error") => ({
    tone,
    text: `Couldn't undo the save of ${t}. ${why}`,
  });
  switch (outcome.outcome) {
    case "saved":
      return {
        tone: "ok",
        text: outcome.warnings.includes("mirror_refresh_pending")
          ? `Put ${t} back in Google. ${writeWarningText("mirror_refresh_pending", "save")}`
          : `Put ${t} back as it was.`,
      };
    case "no_change":
      return { tone: "ok", text: `${t} is already as it was.` };
    case "conflict":
      return { tone: "warn", text: `${t} changed in Google since, so PM didn't undo the save.` };
    case "gone":
      return { tone: "warn", text: `${t} isn't in Google any more.` };
    case "busy":
      return notBack("PM is still changing it. Wait a moment, then press Undo again.", "warn");
    case "read_only":
      return notBack(reasonText(outcome.reason));
    case "reauth":
      return notBack(REAUTH);
    case "unconfirmed":
    case "ambiguous":
      return {
        tone: "warn",
        text: `PM lost touch with Google and can't tell whether ${t} was put back. It will show Google's version after the next refresh.`,
      };
    case "ignored_by_google":
      return notBack("Google answered as if it did, but kept it as it was.", "warn");
    // Never an Undo's answer: it names the very event it changed, and refuses a change of guests.
    case "needs_choice":
    case "partial_split":
    case "split_removed":
      return notBack("Edit it again instead.", "warn");
    case "failed":
      return notBack(outcome.message);
    default:
      return unreachable(outcome);
  }
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

// --- moving an event (C16a) ---

/** Whether a write's Send / Don't send emails guests, so an Undo of it would email them again: Send
 *  to everyone, or to those outside Google Calendar (C16 critique C2-L18). */
export function emailsGuests(sent: Notify | null): boolean {
  return sent === "all" || sent === "external_only";
}

/** `whenText`'s line mid-sentence: "all day" in lower case (C16 critique C2-L18). */
const midSentence = (when: string) => when.replace(/^All day/, "all day");

/** What a move of the event called `title` came to, as a notice: `when` is where it is now
 *  (`whenText` of the moved row) and `sent` the Send / Don't send it went with, so the words say
 *  when an Undo would email the guests again. A move of several occurrences that lands has
 *  `savedNotice`'s words instead; `needs_choice` is asked, not said, and `no_change` says nothing
 *  (the event is already there). */
export function moveText(
  outcome: WriteOutcome,
  title: string,
  when: string,
  sent: Notify | null,
): { tone: Tone; text: string } {
  const t = quote(title);
  const notMoved = (why: string, tone: Tone = "error") => ({
    tone,
    text: `${t} wasn't moved. ${why}`,
  });
  const lost = {
    tone: "warn" as Tone,
    text: `PM lost touch with Google and can't tell whether ${t} moved. It will show Google's version after the next refresh.`,
  };
  switch (outcome.outcome) {
    case "saved": {
      // Google didn't keep all of it (a start without its end): saved, and worth a look.
      if (outcome.warnings.includes("partly_applied")) {
        return {
          tone: "warn",
          text: `Moved ${t} in Google. ${writeWarningText("partly_applied", "save")}`,
        };
      }
      if (outcome.warnings.includes("mirror_refresh_pending")) {
        return {
          tone: "ok",
          text: `Moved ${t} in Google. ${writeWarningText("mirror_refresh_pending", "save")}`,
        };
      }
      const again =
        outcome.undo && emailsGuests(sent) ? " Guests will get a second email if you undo." : "";
      return { tone: "ok", text: `${t} is now ${midSentence(when)}.${again}` };
    }
    case "no_change":
      return { tone: "ok", text: `${t} is already there.` };
    case "needs_choice":
      return notMoved("PM needs to ask about it first. Drag it again.", "warn");
    case "conflict":
      return {
        tone: "warn",
        text: `${t} changed in Google since PM last showed it, so it wasn't moved. PM now shows Google's version.`,
      };
    case "gone":
      return { tone: "warn", text: `${t} isn't in Google any more.` };
    // PM's own write to it on its way, or Google rate-limiting: either way, a moment.
    case "busy":
      return {
        tone: "warn",
        text: `PM is already changing ${t}, or Google is busy. Wait a moment, then try again.`,
      };
    case "read_only":
      return notMoved(reasonText(outcome.reason));
    case "reauth":
      return notMoved(REAUTH);
    case "unconfirmed":
    case "ambiguous":
      return lost;
    case "ignored_by_google":
      return notMoved("Google answered as if it moved it, but kept it where it was.", "warn");
    case "partial_split":
      return { tone: "warn", text: partialText(outcome.offers, outcome.from, outcome.emails) };
    case "split_removed":
      return notMoved("PM removed the new part of an earlier change instead.", "warn");
    case "failed":
      return notMoved(outcome.message);
    default:
      return unreachable(outcome);
  }
}

/** The Undo button's name after a move: what it undoes, and that the guests get a second email
 *  where the move emailed them. */
export function undoMoveLabel(title: string, sent: Notify | null): string {
  return `Undo moving ${quote(title)}${emailsGuests(sent) ? " (guests get a second email)" : ""}`;
}

/** What putting a move of the event called `title` back came to. */
export function moveBackText(outcome: WriteOutcome, title: string): { tone: Tone; text: string } {
  const t = quote(title);
  const notBack = (why: string, tone: Tone = "error") => ({
    tone,
    text: `Couldn't move ${t} back. ${why}`,
  });
  switch (outcome.outcome) {
    case "saved":
      return {
        tone: "ok",
        text: outcome.warnings.includes("mirror_refresh_pending")
          ? `Moved ${t} back in Google. ${writeWarningText("mirror_refresh_pending", "save")}`
          : `Moved ${t} back.`,
      };
    case "no_change":
      return { tone: "ok", text: `${t} is already back where it was.` };
    case "conflict":
      return { tone: "warn", text: `${t} changed in Google since, so it wasn't moved back.` };
    case "gone":
      return { tone: "warn", text: `${t} isn't in Google any more.` };
    case "busy":
      return notBack("PM is still changing it. Wait a moment, then press Undo again.", "warn");
    case "read_only":
      return notBack(reasonText(outcome.reason));
    case "reauth":
      return notBack(REAUTH);
    case "unconfirmed":
    case "ambiguous":
      return {
        tone: "warn",
        text: `PM lost touch with Google and can't tell whether ${t} moved back. It will show Google's version after the next refresh.`,
      };
    case "ignored_by_google":
      return notBack("Google answered as if it did, but kept it where it was.", "warn");
    // Never an Undo's answer: it names the very event it moved, and refuses a change of guests.
    case "needs_choice":
    case "partial_split":
    case "split_removed":
      return notBack("Drag it back, or change it in Edit.", "warn");
    case "failed":
      return notBack(outcome.message);
    default:
      return unreachable(outcome);
  }
}

/** A drop asked about again and still not settled (the plan or the guests changed in Google while
 *  the user answered), or one whose question can't be asked now: put back, and the user drags it
 *  again then (C16b, as a delete says "delete it again"). */
export function moveChangedText(title: string): { tone: Tone; text: string } {
  return {
    tone: "warn",
    text: `${quote(title)} changed in Google while you were choosing, so it wasn't moved. Drag it again.`,
  };
}

/** Above the email question after a drop, when Google's copy has guests PM's calendar didn't show:
 *  the drag asked nothing, since the event looked as if it had none (C16b). */
export const MOVE_NEW_GUESTS =
  "This event has guests in Google that PM's calendar didn't show yet.";

/** Above the email question after a drop, asked again because Google's numbers differ from those
 *  the user answered: the editor's line, for a move. */
export const MOVE_CHECKED =
  "PM checked with Google before moving it, and these are the numbers it found.";

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
