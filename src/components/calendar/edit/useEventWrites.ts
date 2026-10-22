// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The one place the webview changes a Google event (#884). ESLint and
// `scripts/check-calendar-write-fence.mjs` let only this file import the write wrappers, so every
// screen that edits an event comes through here, and nothing else (the chat, the briefing) can.
//
// Deletes are held by the backend for an Undo window (plan Q2) and settle later, possibly after the
// Calendar tab has been switched away and back (the tab router unmounts it). So what is in flight
// lives in this module, not in a component: a store the views subscribe to, fed by
// `calendar://delete-settled`. A webview reload empties it, so the first use asks the backend which
// deletes are still waiting (plan A15).
//
// A repeating event's writes come through here too: the choices the dialog shows, a save or delete
// for the occurrences picked (always with its token), and settling a "this and following" change PM
// couldn't finish. A write that would email guests carries Send or Don't send and the numbers the
// question showed (C12); a held delete whose send finds guests to tell asks the delete dialog again. A change like that keeps a notice with its buttons until it's settled, in this
// store, so it outlives the editor and the tab. A reload loses the notice, not the change: the
// backend still holds it, and the dialog's pending row offers the same buttons on the next change.
//
// Creating an event comes through here too (C10). How a create ended is a notice whenever the editor
// isn't there to say it: Created, or one PM can't tell was made, which keeps a Check again button
// holding exactly what was sent until Google shows how it ended. A reload loses that notice; the
// backend's record of it then lapses, and a copy that landed shows at the next sync.
//
// And two things beside the writes (C9b-2): how a repeating event repeats, for the event popover
// (only a read), and, after a "this and following" change lands, a question per milestone linked to
// it from that day on, whether to move it to the new part (DECISIONS 17-10 #2). Moving one changes
// PM's own milestone, never Google; Move back is offered for a few seconds after.
//
// Replying to an invitation (C14) comes through here too: Yes, No or Maybe, with a note, for one
// occurrence or all of a repeating one. Google tells the organiser, so nothing is asked first.

import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import {
  cancelCalendarDelete,
  createCalendarEvent,
  deleteCalendarEvent,
  getCalendarEventForEdit,
  getSeriesSummary,
  listHeldDeletes,
  onCalendarDeleteSettled,
  respondToCalendarEvent,
  seriesChoices,
  setMilestoneEvent,
  settleSeriesSplit,
  updateCalendarEvent,
} from "../../../lib/ipc";
import type {
  CalendarEvent,
  DeleteSettled,
  EditLoad,
  EventPatchDraft,
  GuestNotice,
  MilestoneMove,
  NewEventDraft,
  Notify,
  Rsvp,
  RsvpScope,
  SeenSummary,
  SeriesChoices,
  SeriesScope,
  SplitAction,
  WriteOutcome,
} from "../../../lib/types";
import {
  createdText,
  createText,
  deleteChangedText,
  deleteText,
  heldCreateText,
  milestoneStayedText,
  movedBackText,
  movedText,
  moveBackFailedText,
  moveFailedText,
  moveOfferText,
  retryAfter,
  savedNotice,
  settleText,
  type CreatedWhere,
  type Tone,
} from "../../../lib/calendarEdit/editReasons";
import type { Picked } from "../../../lib/calendarEdit/saveChoices";
import { writeLastCreate } from "../../../lib/calendarPrefs";
import { isAnyDialogOpen } from "../../../lib/useDialogLayer";
import { deviceTimeZoneOrNull } from "../../../theme";

/** A line the calendar shows about a write: what happened, and an Undo while one is offered. */
export interface WriteNotice {
  id: number;
  tone: Tone;
  text: string;
  /** While a delete waits: the token that undoes it, and the Undo button's accessible name. */
  undoToken?: string;
  undoLabel?: string;
  /** A "this and following" change PM couldn't finish: the token it is settled by, what Google's
   *  state allows now (buttons in SPLIT_ORDER), and the event's title for the result's words. */
  split?: { token: string; offers: SplitAction[]; title: string };
  /** Its settle is on its way: the buttons are disabled. */
  settling?: boolean;
  /** A new event PM may already have made: its retry token and exactly what was sent, which Check
   *  again sends again (the backend looks for it first). */
  create?: { token: string; sent: CreateSent };
  /** Its Check again is on its way: the button is disabled. */
  checking?: boolean;
  /** Asks whether to move a milestone to the new part (DECISIONS 17-10 #2): Move, and Keep it there. */
  move?: MilestoneMove;
  /** A move that just happened: Move back, for {@link MOVE_BACK_MS}. */
  moveBack?: MilestoneMove;
  /** A move or move back is on its way: its button does nothing meanwhile. */
  moving?: boolean;
}

/** What creating an event sent: the calendar (PM's id) and the draft, kept as they were so a Check
 *  again re-sends them exactly (the backend refuses anything else); and, for a new event with
 *  guests, the user's Send or Don't send with the numbers it was given for, sent again with them
 *  (the backend sends the answer first sent whatever comes). */
export interface CreateSent {
  calendarId: string;
  draft: NewEventDraft;
  answer?: CreateAnswer;
}

/** Send or Don't send for a new event's guests, and the numbers the question showed. */
export interface CreateAnswer {
  notify: Notify;
  asked: GuestNotice;
}

/** A delete the backend is holding or sending. */
interface HeldDelete {
  token: string;
  eventId: string;
  /** Every row it hides: the row asked about, and every occurrence a delete of several reaches. */
  hidden: readonly string[];
  /** The title the user saw. */
  summary: string;
  noticeId: number;
  /** The row the user asked to delete, so the dialog can ask again with it when Google's copy has
   *  guests to tell (C12 critique C1-H2). Unknown for a delete picked up after a reload. */
  event?: CalendarEvent;
}

/** Ask about a delete's guests again: Google's copy had guests to tell at the send (added during
 *  the Undo window, or numbers that changed since the answer). Whether the delete dialog opened;
 *  set while the calendar shows one ({@link useDeleteDialog}). */
type GuestReask = (event: CalendarEvent, guests: GuestNotice, again: boolean) => boolean;
let guestReask: GuestReask | null = null;

type PartialSplit = Extract<WriteOutcome, { outcome: "partial_split" }>;

interface State {
  held: readonly HeldDelete[];
  /** Rows a delete removed from Google, hidden until a read of the mirror no longer has them (the
   *  read that `calendar://write-outcome` starts can land after the settle). A number is the
   *  settle's mark: a read that started after it is the truth about the row, so a row it still
   *  has comes back (an occurrence a delete of several couldn't cancel stays in Google). Null: the
   *  row is hidden while any read has it (Google deleted it; only the mirror lags). */
  removed: ReadonlyMap<string, number | null>;
  notices: readonly WriteNotice[];
  /** How many deletes have settled. The calendar reads the mirror again on each, since the read
   *  the backend's landing starts can begin before the webview hears the settle. */
  settles: number;
}

/** How long a notice with nothing left to do stays up. Anything else stays until dismissed. */
const NOTICE_MS = 5000;

let state: State = { held: [], removed: new Map(), notices: [], settles: 0 };
const subscribers = new Set<() => void>();
let nextNotice = 1;
let listening = false;

function update(next: Partial<State>) {
  state = { ...state, ...next };
  for (const notify of subscribers) notify();
}

/** Done-and-fine notices put while no view was showing them: a delete that settled while PM was on
 *  another tab, perhaps after a reload. Their {@link NOTICE_MS} starts when a view subscribes, so
 *  the Calendar tab still says how the delete ended, however late it's opened. */
const unseen = new Set<WriteNotice>();

function expire(notice: WriteNotice) {
  setTimeout(() => {
    if (state.notices.includes(notice)) dismissNotice(notice.id);
  }, NOTICE_MS);
}

function subscribe(onChange: () => void) {
  subscribers.add(onChange);
  for (const notice of unseen) expire(notice);
  unseen.clear();
  return () => {
    subscribers.delete(onChange);
  };
}

const quoted = (summary: string) => `“${summary.trim() || "(no title)"}”`;
const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** Show `notice` (replacing the one with its id, if any). One that is done and fine goes by itself,
 *  once a view has shown it; the timer only removes the notice it was set for, never a later one
 *  under the same id. A delete still held or being sent isn't done, though its notice has lost its
 *  Undo: that one stays until the delete settles. Nor is a change PM couldn't finish: its notice
 *  keeps its buttons until it's settled, whatever its tone; nor a new event PM may have made. Nor a
 *  milestone's move offer, which waits for an answer, or its Move back, which has its own window. */
function put(notice: WriteNotice) {
  const shown = state.notices.some((n) => n.id === notice.id);
  update({
    notices: shown
      ? state.notices.map((n) => (n.id === notice.id ? notice : n))
      : [...state.notices, notice],
  });
  const inFlight = state.held.some((h) => h.noticeId === notice.id);
  if (
    notice.tone === "ok" &&
    !notice.undoToken &&
    !notice.split &&
    !notice.create &&
    !notice.move &&
    !notice.moveBack &&
    !inFlight
  ) {
    if (subscribers.size > 0) expire(notice);
    else unseen.add(notice);
  }
}

export function dismissNotice(id: number) {
  update({ notices: state.notices.filter((n) => n.id !== id) });
}

/** The "Deleting …" notice for a held delete, with its Undo until `seconds` have passed. */
function showHeld(held: HeldDelete, seconds: number) {
  const notice: WriteNotice = {
    id: held.noticeId,
    tone: "ok",
    text: `Deleting ${quoted(held.summary)}.`,
    undoToken: held.token,
    undoLabel: `Undo deleting ${quoted(held.summary)}`,
  };
  put(notice);
  // When the window closes the Undo goes; the notice stays until the delete settles.
  setTimeout(() => {
    if (state.notices.includes(notice)) {
      put({ id: notice.id, tone: notice.tone, text: notice.text });
    }
  }, seconds * 1000);
}

/** What the user saw of the row they asked to delete, in the mirror's own terms. */
function seenOf(event: CalendarEvent): SeenSummary {
  return {
    summary: event.summary,
    start: event.start,
    end: event.end ?? null,
    all_day: event.all_day,
    location: event.location ?? null,
  };
}

/** The rows a held delete hides: the one asked about first, then any others it reaches, once each. */
function hiddenOf(eventId: string, more: readonly string[] | undefined): string[] {
  return [...new Set([eventId, ...(more ?? [])])];
}

/** Keep a change PM couldn't finish on screen with its buttons, worded `text`: the notice already
 *  up for its token takes the new offers and words, or a new one goes up (under `id`, replacing
 *  that notice, when given). Whether it was new. */
function holdPartial(title: string, outcome: PartialSplit, text: string, id?: number): boolean {
  const split = { token: outcome.token, offers: outcome.offers, title };
  const shown = state.notices.find((n) => n.split?.token === outcome.token);
  if (shown) {
    put({ id: shown.id, tone: "warn", text, split, settling: shown.settling });
    if (id !== undefined && id !== shown.id) dismissNotice(id);
    return false;
  }
  put({ id: id ?? nextNotice++, tone: "warn", text, split });
  return true;
}

/** A held delete went to Google: say how it ended. Rows that left Google stay hidden until the
 *  mirror read catches up; any other outcome leaves them in the mirror, so they come back. One an
 *  earlier change PM couldn't finish blocked gets that change's buttons. */
function settle(settled: DeleteSettled) {
  const held = state.held.find((h) => h.token === settled.undo_token);
  if (!held) return; // undone, or from before a reload the backend no longer listed
  const result = settled.result;
  const gone = result.outcome === "saved" || result.outcome === "gone";
  const mark = state.settles + 1;
  // Only a delete that landed and left occurrences in Google is settled by the next read: those
  // rows are still in the mirror, and would otherwise stay hidden until a reload. Any other stays
  // hidden while the mirror has it, so a deleted event never shows again before a sync catches up.
  const releasable =
    result.outcome === "saved" &&
    result.warnings.includes("left_on_old_series") &&
    !result.warnings.includes("mirror_refresh_pending");
  const removed = new Map(state.removed);
  if (gone) for (const id of held.hidden) removed.set(id, releasable ? mark : null);
  update({ held: state.held.filter((h) => h !== held), removed, settles: mark });
  if (result.outcome === "partial_split") {
    holdPartial(held.summary, result, deleteText(result, held.summary).text, held.noticeId);
    return;
  }
  // Google's copy had guests to tell, and nothing was deleted: the dialog asks again with the
  // backend's numbers, which Google's copy, now in PM's calendar, also gives a delete started again.
  // Where it can't open, the notice says to delete again (C12 critique C1-H2).
  if (
    result.outcome === "needs_choice" &&
    result.guests &&
    !result.scope &&
    held.event &&
    guestReask?.(held.event, result.guests, !!result.again)
  ) {
    dismissNotice(held.noticeId);
    return;
  }
  put({ id: held.noticeId, ...deleteText(result, held.summary) });
}

/** Hear how held deletes end, and pick up any still waiting (a reload emptied this store; the backend
 *  still knows what it is holding). Once per webview. The app calls it as soon as the vault is open,
 *  whatever tab it opens on, so a delete that settles before the Calendar tab is visited is still
 *  heard, and its notice is waiting there; the Calendar tab and a delete call it too. */
export function watchHeldDeletes() {
  if (listening) return;
  listening = true;
  onCalendarDeleteSettled(settle).catch(() => {
    listening = false;
  });
  listHeldDeletes()
    .then((waiting) => {
      for (const w of waiting) {
        if (state.held.some((h) => h.token === w.undo_token)) continue;
        const held = {
          token: w.undo_token,
          eventId: w.event_id,
          hidden: hiddenOf(w.event_id, w.hidden),
          summary: w.summary,
          noticeId: nextNotice++,
        };
        update({ held: [...state.held, held] });
        showHeld(held, w.seconds_left);
      }
    })
    .catch(() => {});
}

/** What a delete the backend wants asked again needs: `guests`, the backend's numbers when it's
 *  whether Google emails the guests (null: which occurrences); `again`, when an answer came for
 *  other numbers. */
export interface DeleteReask {
  ask: GuestNotice | null;
  again: boolean;
}

/** Ask to delete `event` (for `picked`, when it repeats; `notify`, Send or Don't send, for the
 *  numbers `asked` the question showed, when it has guests). The backend holds it for an Undo
 *  window; hidden meanwhile are the row and every row a delete of several occurrences reaches.
 *  `{ ask }`: the backend wants the occurrences chosen, or chosen again (`ask` null), or whether
 *  Google emails the guests (`ask`, its numbers; R9), and nothing was held or said: the caller asks.
 *  Whether Google emails the guests is never asked by reopening the occurrences (C12). */
export async function startDelete(
  event: CalendarEvent,
  picked: Picked | null = null,
  notify: Notify | null = null,
  asked: GuestNotice | null = null,
): Promise<"held" | DeleteReask | "refused"> {
  watchHeldDeletes();
  const title = quoted(event.summary);
  try {
    // A scope never goes without its token: `Picked` carries both.
    const start = await deleteCalendarEvent(
      event.id,
      seenOf(event),
      picked?.scope ?? null,
      picked?.token ?? null,
      notify,
      asked,
    );
    if (start.outcome === "refused") {
      const result = start.result;
      if (result.outcome === "needs_choice" && result.guests && !result.scope) {
        return { ask: result.guests, again: !!result.again };
      }
      if (result.outcome === "needs_choice" && result.scope) return { ask: null, again: false };
      if (result.outcome === "partial_split") {
        holdPartial(event.summary, result, deleteText(result, event.summary).text);
        return "refused";
      }
      // Busy before anything was held is PM's own write to the event, not Google.
      put(
        result.outcome === "busy"
          ? {
              id: nextNotice++,
              tone: "warn",
              text: `PM is already changing ${title}. Wait a moment, then try again.`,
            }
          : { id: nextNotice++, ...deleteText(result, event.summary) },
      );
      return "refused";
    }
    const held = {
      token: start.undo_token,
      eventId: event.id,
      hidden: hiddenOf(event.id, start.hidden),
      summary: event.summary,
      noticeId: nextNotice++,
      event,
    };
    update({ held: [...state.held, held] });
    showHeld(held, start.undo_seconds);
    return "held";
  } catch (e) {
    put({ id: nextNotice++, tone: "error", text: `${title} wasn't deleted. ${message(e)}` });
    return "refused";
  }
}

/** Tokens with an Undo on its way, so a second click sends nothing. */
const undoing = new Set<string>();

/** Undo a held delete. */
export async function undoDelete(token: string): Promise<void> {
  const before = state.held.find((h) => h.token === token);
  if (!before || undoing.has(token)) return;
  undoing.add(token);
  // The button goes at once too.
  const shown = state.notices.find((n) => n.id === before.noticeId);
  if (shown) put({ id: shown.id, tone: shown.tone, text: shown.text });
  let undone: boolean;
  try {
    undone = await cancelCalendarDelete(token);
  } catch (e) {
    put({ id: nextNotice++, tone: "error", text: `Couldn't undo. ${message(e)}` });
    return;
  } finally {
    undoing.delete(token);
  }
  // Settled meanwhile: its own notice already says how it ended.
  const held = state.held.find((h) => h.token === token);
  if (!held) return;
  if (undone) {
    update({ held: state.held.filter((h) => h !== held) });
    put({ id: held.noticeId, tone: "ok", text: `Kept ${quoted(held.summary)}.` });
  } else {
    put({
      id: held.noticeId,
      tone: "warn",
      text: `Too late to undo: ${quoted(held.summary)} is already being deleted.`,
    });
  }
}

/** The newest Undo on screen: the last notice still offering one. Notices hold the truth, not the
 *  held deletes, so a key never undoes what the user can't see (a notice dismissed with ×, or one
 *  whose window has closed). */
export function latestUndo(notices: readonly WriteNotice[]): string | null {
  for (let i = notices.length - 1; i >= 0; i--) {
    const token = notices[i].undoToken;
    if (token) return token;
  }
  return null;
}

/** Press the newest Undo on screen (Ctrl+Z on the Calendar tab). Whether there was one to press.
 *  `undoDelete` takes its button away at once, so pressing again undoes the next newest. */
export function undoLatest(): boolean {
  const token = latestUndo(state.notices);
  if (!token) return false;
  void undoDelete(token);
  return true;
}

/** Open an event for editing: Google's fresh copy, what may change, and the session a save names.
 *  The device zone goes with it only when the webview knows it (never a guessed UTC). */
export function openForEdit(eventId: string): Promise<EditLoad> {
  return getCalendarEventForEdit(eventId, deviceTimeZoneOrNull());
}

/** The choices for a change in an open editor. Call it only when the dialog opens: a notice in the
 *  answer is said once, and the backend forgets it once returned. The token binds the session and
 *  the draft, so the save must send this very `draft`. */
export function choicesForEdit(session: string, draft: EventPatchDraft): Promise<SeriesChoices> {
  return seriesChoices({ kind: "edit", session, draft });
}

/** Choices asked for and not yet answered, by row and what the user saw: StrictMode mounts a
 *  dialog's effect twice, and a second ask would read Google twice for one answer. */
const asking = new Map<string, Promise<SeriesChoices>>();

/** The choices for deleting a mirror row: what the user saw is taken from the row itself, exactly
 *  as `startDelete` sends it, because the token binds it. */
export function choicesForDelete(event: CalendarEvent): Promise<SeriesChoices> {
  const seen = seenOf(event);
  const key = JSON.stringify([event.id, seen]);
  const waiting = asking.get(key);
  if (waiting) return waiting;
  const answer = seriesChoices({ kind: "delete", event_id: event.id, seen }).finally(() => {
    asking.delete(key);
  });
  asking.set(key, answer);
  return answer;
}

/** Send a reply to an invitation (by its row id): Yes, No or Maybe, with `note` (null keeps the one
 *  Google holds, "" removes it), and for an occurrence of a repeating event which ones. The outcome
 *  comes back as data; the reply control says what it came to. */
export function respond(
  eventId: string,
  reply: Rsvp,
  note: string | null = null,
  scope: RsvpScope | null = null,
): Promise<WriteOutcome> {
  return respondToCalendarEvent(eventId, reply, note, scope);
}

/** A save's outcome. `newPartial`, on a `partial_split` only: whether its notice is new. False
 *  means one for that change was already up, so an earlier change blocked this save. */
export type SaveAnswer = WriteOutcome & { newPartial?: boolean };

/** Save an editor's changes; `picked` names the occurrences and the plan chosen (a scope never goes
 *  without its token), and `title` is the event's for a notice. `notify` is Send or Don't send
 *  when the save would email guests, and `asked` the numbers the question showed (a repeating
 *  event's token binds them instead). The outcome comes back as data; the editor explains it. A
 *  change PM couldn't finish keeps its notice and buttons here, in the store, so it outlives the
 *  editor and the Calendar tab. */
export async function saveEdit(
  session: string,
  changes: EventPatchDraft,
  picked: Picked | null,
  title: string,
  notify: Notify | null = null,
  asked: GuestNotice | null = null,
): Promise<SaveAnswer> {
  const outcome = await updateCalendarEvent(
    session,
    changes,
    picked?.scope ?? null,
    picked?.token ?? null,
    notify,
    asked,
  );
  if (outcome.outcome === "partial_split") {
    return { ...outcome, newPartial: notePartial(title, outcome) };
  }
  return outcome;
}

/** Tokens with a settle on its way. The backend claims the repeating event and takes the token
 *  first too; this only spares a request. */
const settling = new Set<string>();

/** Settle a change PM couldn't finish. One at a time per token: a second call while one runs sends
 *  nothing and answers null. Its notice (made if there's none, as for the dialog's pending row)
 *  then says how it ended, and keeps its buttons, with the offers now, while the change is still
 *  held; only an answer that ends it takes them away. */
export async function settleSplit(
  token: string,
  action: SplitAction,
  title: string,
): Promise<WriteOutcome | null> {
  if (settling.has(token)) return null;
  settling.add(token);
  for (const n of state.notices) {
    if (n.split?.token === token) put({ ...n, settling: true });
  }
  let result: WriteOutcome;
  let answered = true;
  try {
    result = await settleSeriesSplit(token, action);
  } catch (e) {
    result = { outcome: "failed", message: message(e) };
    answered = false;
  } finally {
    settling.delete(token);
  }
  const shown = state.notices.find((n) => n.split?.token === token);
  const id = shown?.id ?? nextNotice++;
  if (result.outcome === "partial_split") {
    put({
      id,
      tone: "warn",
      text: settleText(result, title).text,
      split: { token: result.token, offers: result.offers, title },
    });
  } else if (!answered) {
    // No answer came: nothing says the change ended, so its buttons stay.
    put({
      id,
      ...settleText(result, title),
      split: shown?.split ?? { token, offers: ["check"], title },
    });
  } else {
    put({ id, ...settleText(result, title) });
    // A Finish that lands offers what the first send read, as a save does.
    offerMoves(result);
  }
  return result;
}

/** A save that went partial: a notice that stays, with its actions. Whether it was new: false means
 *  a notice for that change was already up, so this was an earlier change blocking the save. */
export function notePartial(title: string, outcome: PartialSplit): boolean {
  return holdPartial(title, outcome, settleText(outcome, title).text);
}

/** Words about a repeating event kept as a notice until dismissed: what the choices said once (a
 *  change PM was making when it closed) that the user closed without choosing, so they aren't
 *  lost, or a delete that wanted the occurrences chosen when the dialog couldn't reopen. */
export function noteSeriesNotice(text: string) {
  put({ id: nextNotice++, tone: "warn", text });
}

/** A save that landed, said once the editor has closed. `scope`: a repeating event's occurrences
 *  saved, which have no Undo, so the notice says how to change them back; `from` (YYYY-MM-DD) is
 *  where a "this and following" save starts; `stops`: the save made it "Does not repeat", which
 *  leaves one event; `asAll`: the "this and following" row picked saved every occurrence
 *  (`Picked.asAll`). */
export function noteSaved(
  summary: string,
  outcome: WriteOutcome,
  scope: SeriesScope | null = null,
  from?: string,
  stops = false,
  asAll = false,
) {
  if (outcome.outcome !== "saved") return;
  put({ id: nextNotice++, ...savedNotice(summary, outcome, scope, from, stops, asAll) });
  offerMoves(outcome);
}

// --- how a repeating event repeats, and moving its milestones (C9b-2) ---

/** Asks on their way, by row, so the popover's two effect runs in StrictMode (and two quick clicks)
 *  send one request. */
const describing = new Map<string, Promise<string | null>>();

/** How the repeating event a Google row belongs to repeats, in Google Calendar's words, or null when
 *  PM can't say. Only a read; the backend keeps the answer a while. */
export function describeSeries(eventId: string): Promise<string | null> {
  const going = describing.get(eventId);
  if (going) return going;
  const ask = getSeriesSummary(eventId)
    .catch(() => null)
    .finally(() => describing.delete(eventId));
  describing.set(eventId, ask);
  return ask;
}

/** How long Move back is offered after a move, like a delete's Undo window; paused while the
 *  pointer or focus is on its notice. */
export const MOVE_BACK_MS = 8000;

/** How long Move back ignores a press once it shows. It takes Move's place, so a press meant for
 *  Move (a second Enter, a quick double-click) lands on it, and would undo the move just made. */
const MOVE_BACK_ARMS_MS = 500;

/** After a "this and following" change landed: one question per milestone the backend read as dated
 *  on or after its first day, or, when PM couldn't find the new part, one notice per milestone
 *  saying it stayed. Nothing for any other outcome. */
function offerMoves(outcome: WriteOutcome): void {
  if (outcome.outcome !== "saved" || !outcome.milestone_moves) return;
  const moves = outcome.milestone_moves;
  const pending = outcome.warnings.includes("mirror_refresh_pending");
  for (const m of moves.milestones) {
    const move: MilestoneMove = {
      milestoneId: m.id,
      label: m.label,
      project: m.project,
      newTitle: moves.new_title,
      earlierTitle: moves.earlier_title,
      toIcal: moves.new_series_ical ?? "",
      toDate: moves.first_day,
      backIcal: moves.earlier_series_ical,
      backDate: m.date,
      earlierLastDay: moves.earlier_last_day,
      today: moves.today,
      pending,
    };
    put(
      moves.new_series_ical === null
        ? { id: nextNotice++, tone: "warn", text: milestoneStayedText(move) }
        : { id: nextNotice++, tone: "ok", text: moveOfferText(move), move },
    );
  }
}

/** Notices with a move or move back on its way: a second press sends nothing. */
const movingIds = new Set<number>();

/** Where the pointer and focus are, by notice with a move: on one, they hold its Move back open. */
const holding = new Map<number, { pointer: boolean; focus: boolean }>();

const held = (id: number) => {
  const on = holding.get(id);
  return !!on && (on.pointer || on.focus);
};

/** A Move back window open now: the notice it closes, its timer (null while held), when it ends,
 *  the time it had left when it was held, and when Move back first takes a press. */
interface MoveBackWindow {
  notice: WriteNotice;
  timer: ReturnType<typeof setTimeout> | null;
  endsAt: number;
  left: number;
  armsAt: number;
}
const moveBackWindows = new Map<number, MoveBackWindow>();

function runMoveBack(open: MoveBackWindow) {
  open.endsAt = Date.now() + open.left;
  open.timer = setTimeout(() => closeMoveBack(open.notice.id), open.left);
}

/** Offer Move back on `notice` for {@link MOVE_BACK_MS}, waiting first if it's held already. */
function openMoveBack(notice: WriteNotice) {
  const open: MoveBackWindow = {
    notice,
    timer: null,
    endsAt: 0,
    left: MOVE_BACK_MS,
    armsAt: Date.now() + MOVE_BACK_ARMS_MS,
  };
  moveBackWindows.set(notice.id, open);
  if (!held(notice.id)) runMoveBack(open);
}

/** End the Move back window on notice `id`, if one is open: the notice it was opened on. */
function endMoveBack(id: number): WriteNotice | null {
  const open = moveBackWindows.get(id);
  if (!open) return null;
  moveBackWindows.delete(id);
  if (open.timer) clearTimeout(open.timer);
  return open.notice;
}

/** The window over: Move back goes, and the notice then goes by itself like any done one. Only if
 *  that very notice is still up. */
function closeMoveBack(id: number) {
  const notice = endMoveBack(id);
  if (notice && state.notices.includes(notice)) {
    put({ id: notice.id, tone: notice.tone, text: notice.text });
  }
}

/** The pointer or focus (`by`) is on a notice with a move (`on`), or has left it. Move back waits
 *  while either is there (WCAG 2.2.1), then goes on with the time it had left. */
export function holdMoveBack(id: number, by: "pointer" | "focus", on: boolean) {
  const where = { ...(holding.get(id) ?? { pointer: false, focus: false }), [by]: on };
  if (where.pointer || where.focus) holding.set(id, where);
  else holding.delete(id);
  const open = moveBackWindows.get(id);
  if (!open) return;
  if (held(id) && open.timer) {
    clearTimeout(open.timer);
    open.timer = null;
    open.left = Math.max(0, open.endsAt - Date.now());
  } else if (!held(id) && !open.timer) {
    runMoveBack(open);
  }
}

/** A move or move back from the notice `noticeId`: marked on its way, sent, then said. Whether it
 *  changed the milestone. */
async function sendMove(
  noticeId: number,
  pick: (n: WriteNotice) => MilestoneMove | undefined,
  send: (m: MilestoneMove) => Promise<void>,
  done: (m: MilestoneMove) => void,
  failed: (m: MilestoneMove, why: string) => void,
): Promise<boolean> {
  const shown = state.notices.find((n) => n.id === noticeId);
  const move = shown && pick(shown);
  if (!shown || !move || movingIds.has(noticeId)) return false;
  movingIds.add(noticeId);
  put({ ...shown, moving: true });
  try {
    await send(move);
  } catch (e) {
    failed(move, message(e));
    return false;
  } finally {
    movingIds.delete(noticeId);
  }
  done(move);
  return true;
}

/** Move the milestone a notice offers to the new part: only while it's still linked to the earlier
 *  one, so an offer left up never undoes a change made since. Move back is then offered for
 *  {@link MOVE_BACK_MS}. Whether it moved. */
export function moveMilestone(noticeId: number): Promise<boolean> {
  return sendMove(
    noticeId,
    (n) => n.move,
    (m) => setMilestoneEvent(m.milestoneId, m.toIcal, m.toDate, m.backIcal),
    (m) => {
      // The notice's one button stays where it was, now Move back.
      const notice: WriteNotice = { id: noticeId, tone: "ok", text: movedText(m), moveBack: m };
      put(notice);
      openMoveBack(notice);
    },
    // The Move button stays, for another try.
    (m, why) => put({ id: noticeId, tone: "error", text: moveFailedText(m, why), move: m }),
  );
}

/** Move it back to the earlier part, while the moved notice offers that: only while it's still
 *  linked to the new part, and never in its first {@link MOVE_BACK_ARMS_MS}. Whether it moved. */
export function moveMilestoneBack(noticeId: number): Promise<boolean> {
  const open = moveBackWindows.get(noticeId);
  if (open && Date.now() < open.armsAt) return Promise.resolve(false);
  return sendMove(
    noticeId,
    (n) => n.moveBack,
    (m) => setMilestoneEvent(m.milestoneId, m.backIcal, m.backDate, m.toIcal),
    (m) => {
      endMoveBack(noticeId);
      put({ id: noticeId, tone: "ok", text: movedBackText(m) });
    },
    // No button: the window was the one chance, and the words name the way out.
    (m, why) => {
      endMoveBack(noticeId);
      put({ id: noticeId, tone: "error", text: moveBackFailedText(m, why) });
    },
  );
}

// --- creating an event (C10) ---

/** Create an event on one of PM's Google calendars; `retryToken` (from an earlier `ambiguous`
 *  answer) has the backend look for that attempt before anything is sent again. `answer`: Send or
 *  Don't send for its guests, for the numbers the question showed; one with guests and none is
 *  answered `needs_choice`, nothing sent. The outcome comes back as data; the caller says how it
 *  went. */
export function createEvent(
  calendarId: string,
  draft: NewEventDraft,
  retryToken: string | null,
  answer: CreateAnswer | null = null,
): Promise<WriteOutcome> {
  return createCalendarEvent(
    calendarId,
    draft,
    retryToken,
    answer?.notify ?? null,
    answer?.asked ?? null,
  );
}

/** The notice up for a held create's token, if one is. */
const createNotice = (token: string) => state.notices.find((n) => n.create?.token === token);

/** How a create ended, as a notice that outlives whatever sent it (the editor, closed or gone with
 *  its tab; a notice's own Check again). `sentToken`: the retry token it was sent under, if any.
 *  Saved says Created (and where PM's calendar won't show it), and remembers the calendar for the
 *  next new event. An answer that leaves a token held (`retryAfter`) keeps Check again, holding
 *  exactly what was sent, and never goes by itself; anything else says it wasn't created. `id`: the
 *  notice this replaces (Check again's own); a notice already up for the token is replaced too. */
export function noteCreateResult(
  sent: CreateSent,
  sentToken: string | null,
  outcome: WriteOutcome,
  where: CreatedWhere,
  id?: number,
) {
  const title = sent.draft.summary;
  const token = retryAfter(sentToken, outcome);
  const shown =
    (sentToken !== null ? createNotice(sentToken) : undefined) ??
    (token !== null ? createNotice(token) : undefined);
  const noticeId = id ?? shown?.id ?? nextNotice++;
  if (shown && id !== undefined && shown.id !== id) dismissNotice(shown.id);
  if (outcome.outcome === "saved") {
    writeLastCreate(sent.calendarId);
    put({ id: noticeId, ...createdText(title, outcome, where) });
    return;
  }
  put({
    id: noticeId,
    ...createText(outcome, title, token !== null),
    ...(token !== null ? { create: { token, sent } } : {}),
  });
}

/** A surface closed (or went with its tab) while PM may already have made its new event: a notice
 *  that keeps Check again, never "discarded". One per token. */
export function noteHeldCreate(sent: CreateSent, token: string) {
  put({
    id: createNotice(token)?.id ?? nextNotice++,
    ...heldCreateText(sent.draft.summary),
    create: { token, sent },
  });
}

/** Tokens with a Check again on its way, so a second press sends nothing. */
const checking = new Set<string>();

/** Check again from a notice: send exactly what was sent, under its token (the backend looks for
 *  the event before anything goes again). `whereOf` says where the calendar shows now, for the
 *  words if it was made. With no answer at all, the button stays. */
export async function checkCreate(
  id: number,
  whereOf: (calendarId: string) => CreatedWhere,
): Promise<void> {
  const notice = state.notices.find((n) => n.id === id);
  const held = notice?.create;
  if (!notice || !held || checking.has(held.token)) return;
  checking.add(held.token);
  put({ ...notice, checking: true });
  let outcome: WriteOutcome;
  try {
    outcome = await createEvent(
      held.sent.calendarId,
      held.sent.draft,
      held.token,
      held.sent.answer ?? null,
    );
  } catch (e) {
    put({
      id,
      tone: "warn",
      text: `${heldCreateText(held.sent.draft.summary).text} ${message(e)}`,
      create: held,
    });
    return;
  } finally {
    checking.delete(held.token);
  }
  noteCreateResult(held.sent, held.token, outcome, whereOf(held.sent.calendarId), id);
}

/** The read mark: take it before a read of the mirror starts, and give it to `pruneRemoved` with
 *  what the read found. */
export function readMark(): number {
  return state.settles;
}

/** A fresh read of the mirror, started at `startedAt` (a {@link readMark}): rows it no longer has
 *  needn't be hidden any more, nor a row it has whose delete settled before the read began and
 *  left it in Google. Without a mark, only rows the read no longer has. */
export function pruneRemoved(events: readonly CalendarEvent[], startedAt = -1) {
  if (state.removed.size === 0) return;
  const present = new Set(events.map((e) => e.id));
  const still = [...state.removed].filter(
    ([id, mark]) => present.has(id) && (mark === null || mark > startedAt),
  );
  if (still.length !== state.removed.size) update({ removed: new Map(still) });
}

/** The calendar's reads of the mirror, with `read`: each one marked, and only the newest one started
 *  is applied (`apply`) and shows again what it settles. An older read that lands after a newer one
 *  began is dropped whole, pruning nothing: it may have started before a delete it no longer has
 *  settled, and would show that row again until the newer read lands. `failed`: the newest read
 *  threw. A delete that settles starts one more read, since the read the backend's landing starts
 *  can begin before the settle is heard, and so can't show again a row the delete left in Google.
 *  Keep the three functions stable; the answer is the read to call. */
export function useMirrorReads<T extends readonly CalendarEvent[]>(
  read: () => Promise<T>,
  apply: (events: T) => void,
  failed: () => void,
): () => Promise<void> {
  const latest = useRef(0);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  const load = useCallback(async () => {
    const mine = ++latest.current;
    const mark = readMark();
    try {
      const events = await read();
      if (!alive.current || mine !== latest.current) return;
      pruneRemoved(events, mark);
      apply(events);
    } catch {
      if (alive.current && mine === latest.current) failed();
    }
  }, [read, apply, failed]);
  const { settles } = useSyncExternalStore(subscribe, () => state);
  const settlesSeen = useRef(settles);
  useEffect(() => {
    if (settles === settlesSeen.current) return;
    settlesSeen.current = settles;
    void load();
  }, [settles, load]);
  return load;
}

/** What the delete dialog is asking about: the event, and `reask` when it reopened because the
 *  backend wanted the occurrences chosen (`again`: a choice was made, but the plan changed since).
 *  `guests`: it reopened to ask whether Google emails the guests, with the backend's numbers
 *  (`again`: an answer came, for other numbers); the event is asked about as a one-off. */
export interface DeleteAsk {
  event: CalendarEvent;
  reask: { again: boolean } | null;
  guests?: { notice: GuestNotice; again: boolean } | null;
}

/** The delete dialog's state and its delete. A delete's answer can come seconds later (a delete of
 *  chosen occurrences reads Google first), and one that wants the occurrences chosen, or whether
 *  Google emails the guests, reopens the dialog once, asking that. It never replaces a dialog
 *  opened meanwhile, nor opens over any other dialog (the editor, the command palette) or over what
 *  `openElsewhere` says is open that isn't one (the event popover), nor once the Calendar tab has
 *  gone: a notice says to delete again instead, as it does when the plan keeps changing under the
 *  user. A held delete whose send found guests to tell asks the same way (C12 critique C1-H2). */
export function useDeleteDialog(openElsewhere: () => boolean) {
  const [asked, setAskedState] = useState<DeleteAsk | null>(null);
  // What's open now, readable when a delete's answer lands.
  const open = useRef<DeleteAsk | null>(null);
  const elsewhere = useRef(openElsewhere);
  useEffect(() => {
    elsewhere.current = openElsewhere;
  }, [openElsewhere]);
  // Whether the calendar still shows: an answer after the tab unmounted can't open the dialog.
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  const setAsked = useCallback((next: DeleteAsk | null) => {
    open.current = next;
    setAskedState(next);
  }, []);
  const ask = useCallback((event: CalendarEvent) => setAsked({ event, reask: null }), [setAsked]);
  const close = useCallback(() => setAsked(null), [setAsked]);
  /** Whether the dialog may open by itself now: the calendar shows, and nothing else is open. */
  const free = useCallback(
    () => alive.current && open.current === null && !isAnyDialogOpen() && !elsewhere.current(),
    [],
  );
  // A held delete's send that found guests to tell asks here, when it can.
  useEffect(() => {
    const reask: GuestReask = (event, notice, again) => {
      if (!free()) return false;
      setAsked({ event, reask: null, guests: { notice, again } });
      return true;
    };
    guestReask = reask;
    return () => {
      if (guestReask === reask) guestReask = null;
    };
  }, [free, setAsked]);
  const confirm = useCallback(
    (
      event: CalendarEvent,
      picked: Picked | null,
      notify: Notify | null = null,
      asked: GuestNotice | null = null,
    ) => {
      const was = open.current;
      setAsked(null);
      void startDelete(event, picked, notify, asked).then((r) => {
        if (r === "held" || r === "refused") return;
        // Asked once already (a reopened dialog), or nowhere to ask: said instead, so nothing
        // loops. Occurrences chosen: their token binds who the delete tells, so a guests ask
        // there is the plan changing under the user too.
        const reasked = (r.ask ? was?.guests : was?.reask) != null || (!!r.ask && !!picked);
        if (reasked || !free()) {
          noteSeriesNotice(
            picked
              ? deleteChangedText(event.summary).text
              : r.ask
                ? deleteText(
                    { outcome: "needs_choice", scope: false, guests: r.ask, again: r.again },
                    event.summary,
                  ).text
                : deleteText({ outcome: "needs_choice", scope: true }, event.summary).text,
          );
          return;
        }
        setAsked(
          r.ask
            ? { event, reask: null, guests: { notice: r.ask, again: r.again } }
            : { event, reask: { again: picked !== null } },
        );
      });
    },
    [free, setAsked],
  );
  return { asked, ask, close, confirm };
}

/** The calendar's view of the writes in flight: notices, the rows to hide (held or removed), and
 *  how many deletes have settled (read the mirror again when it changes). */
export function useEventWrites() {
  // On first use (and after a reload): hear how held deletes end, and pick up any still waiting.
  useEffect(watchHeldDeletes, []);
  const snap = useSyncExternalStore(subscribe, () => state);
  const hiddenIds = useMemo(
    () => new Set([...snap.held.flatMap((h) => h.hidden), ...snap.removed.keys()]),
    [snap.held, snap.removed],
  );
  return {
    notices: snap.notices,
    hiddenIds,
    settles: snap.settles,
    startDelete,
    respond,
    undoDelete,
    undoLatest,
    settleSplit,
    checkCreate,
    noteSeriesNotice,
    dismissNotice,
    pruneRemoved,
    describeSeries,
    moveMilestone,
    moveMilestoneBack,
    holdMoveBack,
  };
}
