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
// couldn't finish. A change like that keeps a notice with its buttons until it's settled, in this
// store, so it outlives the editor and the tab. A reload loses the notice, not the change: the
// backend still holds it, and the dialog's pending row offers the same buttons on the next change.

import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import {
  cancelCalendarDelete,
  deleteCalendarEvent,
  getCalendarEventForEdit,
  listHeldDeletes,
  onCalendarDeleteSettled,
  seriesChoices,
  settleSeriesSplit,
  updateCalendarEvent,
} from "../../../lib/ipc";
import type {
  CalendarEvent,
  DeleteSettled,
  EditLoad,
  EventPatchDraft,
  SeenSummary,
  SeriesChoices,
  SeriesScope,
  SplitAction,
  WriteOutcome,
} from "../../../lib/types";
import {
  deleteChangedText,
  deleteText,
  savedNotice,
  settleText,
  type Tone,
} from "../../../lib/calendarEdit/editReasons";
import type { Picked } from "../../../lib/calendarEdit/saveChoices";
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
}

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
 *  keeps its buttons until it's settled, whatever its tone. */
function put(notice: WriteNotice) {
  const shown = state.notices.some((n) => n.id === notice.id);
  update({
    notices: shown
      ? state.notices.map((n) => (n.id === notice.id ? notice : n))
      : [...state.notices, notice],
  });
  const inFlight = state.held.some((h) => h.noticeId === notice.id);
  if (notice.tone === "ok" && !notice.undoToken && !notice.split && !inFlight) {
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

/** Ask to delete `event` (for `picked`, when it repeats). The backend holds it for an Undo window;
 *  hidden meanwhile are the row and every row a delete of several occurrences reaches. "ask": the
 *  backend wants the occurrences chosen first, or chosen again (R9), and nothing was held or said;
 *  the caller opens the choices. */
export async function startDelete(
  event: CalendarEvent,
  picked: Picked | null = null,
): Promise<"held" | "ask" | "refused"> {
  watchHeldDeletes();
  const title = quoted(event.summary);
  try {
    // A scope never goes without its token: `Picked` carries both.
    const start = await deleteCalendarEvent(
      event.id,
      seenOf(event),
      picked?.scope ?? null,
      picked?.token ?? null,
    );
    if (start.outcome === "refused") {
      const result = start.result;
      if (result.outcome === "needs_choice") return "ask";
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

/** A save's outcome. `newPartial`, on a `partial_split` only: whether its notice is new. False
 *  means one for that change was already up, so an earlier change blocked this save. */
export type SaveAnswer = WriteOutcome & { newPartial?: boolean };

/** Save an editor's changes; `picked` names the occurrences and the plan chosen (a scope never goes
 *  without its token), and `title` is the event's for a notice. The outcome comes back as data; the
 *  editor explains it. A change PM couldn't finish keeps its notice and buttons here, in the store,
 *  so it outlives the editor and the Calendar tab. */
export async function saveEdit(
  session: string,
  changes: EventPatchDraft,
  picked: Picked | null,
  title: string,
): Promise<SaveAnswer> {
  const outcome = await updateCalendarEvent(
    session,
    changes,
    picked?.scope ?? null,
    picked?.token ?? null,
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
 *  where a "this and following" save starts. */
export function noteSaved(
  summary: string,
  outcome: WriteOutcome,
  scope: SeriesScope | null = null,
  from?: string,
) {
  if (outcome.outcome !== "saved") return;
  put({ id: nextNotice++, ...savedNotice(summary, outcome, scope, from) });
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
 *  backend wanted the occurrences chosen (`again`: a choice was made, but the plan changed since). */
export interface DeleteAsk {
  event: CalendarEvent;
  reask: { again: boolean } | null;
}

/** The delete dialog's state and its delete. A delete's answer can come seconds later (a delete of
 *  chosen occurrences reads Google first), and one that wants the occurrences chosen reopens the
 *  dialog once, asking whatever the row says. It never replaces a dialog opened meanwhile, nor
 *  opens over the editor (`editorOpen`): a notice says to delete again instead, as it does when the
 *  plan keeps changing under the user. */
export function useDeleteDialog(editorOpen: boolean) {
  const [asked, setAskedState] = useState<DeleteAsk | null>(null);
  // What's open now, readable when a delete's answer lands.
  const open = useRef<DeleteAsk | null>(null);
  const editor = useRef(editorOpen);
  useEffect(() => {
    editor.current = editorOpen;
  }, [editorOpen]);
  const setAsked = useCallback((next: DeleteAsk | null) => {
    open.current = next;
    setAskedState(next);
  }, []);
  const ask = useCallback((event: CalendarEvent) => setAsked({ event, reask: null }), [setAsked]);
  const close = useCallback(() => setAsked(null), [setAsked]);
  const confirm = useCallback(
    (event: CalendarEvent, picked: Picked | null) => {
      const reasked = open.current?.reask != null;
      setAsked(null);
      void startDelete(event, picked).then((r) => {
        if (r !== "ask") return;
        if (reasked || open.current !== null || editor.current) {
          // Chosen, and the plan changed since; or never asked, and something else is open now.
          noteSeriesNotice(
            picked
              ? deleteChangedText(event.summary).text
              : deleteText({ outcome: "needs_choice" }, event.summary).text,
          );
          return;
        }
        setAsked({ event, reask: { again: picked !== null } });
      });
    },
    [setAsked],
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
    undoDelete,
    undoLatest,
    settleSplit,
    noteSeriesNotice,
    dismissNotice,
    pruneRemoved,
  };
}
