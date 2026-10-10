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

import { useEffect, useMemo, useSyncExternalStore } from "react";
import {
  cancelCalendarDelete,
  deleteCalendarEvent,
  getCalendarEventForEdit,
  listHeldDeletes,
  onCalendarDeleteSettled,
  updateCalendarEvent,
} from "../../../lib/ipc";
import type {
  CalendarEvent,
  DeleteSettled,
  EditLoad,
  EventPatchDraft,
  SeenSummary,
  WriteOutcome,
} from "../../../lib/types";
import { deleteText, type Tone } from "../../../lib/calendarEdit/editReasons";
import { deviceTimeZoneOrNull } from "../../../theme";

/** A line the calendar shows about a write: what happened, and an Undo while one is offered. */
export interface WriteNotice {
  id: number;
  tone: Tone;
  text: string;
  /** While a delete waits: the token that undoes it, and the Undo button's accessible name. */
  undoToken?: string;
  undoLabel?: string;
}

/** A delete the backend is holding or sending. */
interface HeldDelete {
  token: string;
  eventId: string;
  /** The title the user saw. */
  summary: string;
  noticeId: number;
}

interface State {
  held: readonly HeldDelete[];
  /** Rows a delete removed from Google, hidden until a read of the mirror no longer has them (the
   *  read that `calendar://write-outcome` starts can land after the settle). */
  removed: ReadonlySet<string>;
  notices: readonly WriteNotice[];
}

/** How long a notice with nothing left to do stays up. Anything else stays until dismissed. */
const NOTICE_MS = 5000;

let state: State = { held: [], removed: new Set(), notices: [] };
const subscribers = new Set<() => void>();
let nextNotice = 1;
let listening = false;

function update(next: Partial<State>) {
  state = { ...state, ...next };
  for (const notify of subscribers) notify();
}

function subscribe(onChange: () => void) {
  subscribers.add(onChange);
  return () => {
    subscribers.delete(onChange);
  };
}

const quoted = (summary: string) => `“${summary.trim() || "(no title)"}”`;
const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** Show `notice` (replacing the one with its id, if any). One that is done and fine goes by itself;
 *  the timer only removes the notice it was set for, never a later one under the same id. */
function put(notice: WriteNotice) {
  const shown = state.notices.some((n) => n.id === notice.id);
  update({
    notices: shown
      ? state.notices.map((n) => (n.id === notice.id ? notice : n))
      : [...state.notices, notice],
  });
  if (notice.tone === "ok" && !notice.undoToken) {
    setTimeout(() => {
      if (state.notices.includes(notice)) dismissNotice(notice.id);
    }, NOTICE_MS);
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

/** A held delete went to Google: say how it ended. A row that left Google stays hidden until the
 *  mirror read catches up; any other outcome leaves the row in the mirror, so it comes back. */
function settle(settled: DeleteSettled) {
  const held = state.held.find((h) => h.token === settled.undo_token);
  if (!held) return; // undone, or from before a reload the backend no longer listed
  const gone = settled.result.outcome === "saved" || settled.result.outcome === "gone";
  update({
    held: state.held.filter((h) => h !== held),
    removed: gone ? new Set([...state.removed, held.eventId]) : state.removed,
  });
  put({ id: held.noticeId, ...deleteText(settled.result, held.summary) });
}

function ensureListening() {
  if (listening) return;
  listening = true;
  onCalendarDeleteSettled(settle).catch(() => {
    listening = false;
  });
  // A reload emptied this store; the backend still knows what it is holding.
  listHeldDeletes()
    .then((waiting) => {
      for (const w of waiting) {
        if (state.held.some((h) => h.token === w.undo_token)) continue;
        const held = {
          token: w.undo_token,
          eventId: w.event_id,
          summary: w.summary,
          noticeId: nextNotice++,
        };
        update({ held: [...state.held, held] });
        showHeld(held, w.seconds_left);
      }
    })
    .catch(() => {});
}

/** Ask to delete `event`. The backend holds it for an Undo window; the row is hidden meanwhile. */
export async function startDelete(event: CalendarEvent): Promise<void> {
  ensureListening();
  const title = quoted(event.summary);
  try {
    const start = await deleteCalendarEvent(event.id, seenOf(event));
    if (start.outcome === "refused") {
      // Busy before anything was held is PM's own write to the event, not Google.
      put(
        start.result.outcome === "busy"
          ? {
              id: nextNotice++,
              tone: "warn",
              text: `PM is already changing ${title}. Wait a moment, then try again.`,
            }
          : { id: nextNotice++, ...deleteText(start.result, event.summary) },
      );
      return;
    }
    const held = {
      token: start.undo_token,
      eventId: event.id,
      summary: event.summary,
      noticeId: nextNotice++,
    };
    update({ held: [...state.held, held] });
    showHeld(held, start.undo_seconds);
  } catch (e) {
    put({ id: nextNotice++, tone: "error", text: `${title} wasn't deleted. ${message(e)}` });
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

/** Open an event for editing: Google's fresh copy, what may change, and the session a save names.
 *  The device zone goes with it only when the webview knows it (never a guessed UTC). */
export function openForEdit(eventId: string): Promise<EditLoad> {
  return getCalendarEventForEdit(eventId, deviceTimeZoneOrNull());
}

/** Save an editor's changes. The outcome comes back as data; the editor explains it. */
export function saveEdit(session: string, changes: EventPatchDraft): Promise<WriteOutcome> {
  return updateCalendarEvent(session, changes);
}

/** A save that landed, said once the editor has closed. */
export function noteSaved(summary: string, outcome: WriteOutcome) {
  if (outcome.outcome !== "saved") return;
  put({
    id: nextNotice++,
    tone: "ok",
    text: outcome.warnings.includes("mirror_refresh_pending")
      ? `Saved ${quoted(summary)} to Google. PM's calendar will show it after the next refresh.`
      : `Saved ${quoted(summary)}.`,
  });
}

/** A fresh read of the mirror: rows it no longer has needn't be hidden any more. */
export function pruneRemoved(events: readonly CalendarEvent[]) {
  if (state.removed.size === 0) return;
  const present = new Set(events.map((e) => e.id));
  const still = [...state.removed].filter((id) => present.has(id));
  if (still.length !== state.removed.size) update({ removed: new Set(still) });
}

/** The calendar's view of the writes in flight: notices, and the rows to hide (held or removed). */
export function useEventWrites() {
  // On first use (and after a reload): hear how held deletes end, and pick up any still waiting.
  useEffect(ensureListening, []);
  const snap = useSyncExternalStore(subscribe, () => state);
  const hiddenIds = useMemo(
    () => new Set([...snap.held.map((h) => h.eventId), ...snap.removed]),
    [snap.held, snap.removed],
  );
  return {
    notices: snap.notices,
    hiddenIds,
    startDelete,
    undoDelete,
    dismissNotice,
    pruneRemoved,
  };
}
