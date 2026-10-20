// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The full editor's create mode (#884, C10): a new event on one of the user's Google calendars. A
// sibling of `EventEditor` sharing its fields (`EventFields`), with nothing to load, no session, no
// conflicts and nothing to delete: the same dialog and checks, and a calendar picker where editing
// names the calendar it saves to. Save, Ctrl+S (⌘S on a Mac) and Enter in the title all save.
//
// A new event never goes twice by accident. Every way to save passes one guard, set before the
// request goes; while it's out, every field, the picker and closing are off, and a held key's
// repeats or the Enter that ends composing text send nothing. What was sent is kept: an answer that
// never arrived shows exactly that draft, locked, and Check again sends it again under the retry
// token the backend gave (it looks for the event first), never anything newer. Only Google showing
// the event or its deletion, or the backend no longer holding it, ends the token. Closing while one
// is held, or an answer that arrives once the editor has gone, leaves a notice in `useEventWrites`
// that says so and keeps Check again. A token that ended without Google showing the event made is
// never "nothing saved": discarding says what PM knows, and closing leaves it as a notice.
//
// Times are in this computer's zone (plan rule R6): when it can't be read, the times can't be
// changed and nothing can be saved, rather than guessing UTC.

import { useCallback, useEffect, useMemo, useRef, useState, type MutableRefObject } from "react";
import { flushSync } from "react-dom";
import type { CalendarOverview, WriteOutcome } from "../../../lib/types";
import {
  NEW_EVENT_BASE,
  changedFields,
  checkDraft,
  endAfterStart,
  endDateMin,
  endTimeChoices,
  endsAfterStart,
  firstEndOnItsDate,
  halfInstant,
  switchNewKind,
  toNewEvent,
  type EditorFields,
} from "../../../lib/calendarEdit/eventDraft";
import {
  ambiguousText,
  CALLOUT_TONE,
  createText,
  problemText,
  retryAfter,
  type Tone,
} from "../../../lib/calendarEdit/editReasons";
import {
  createdWhere,
  lengthFor,
  outsideMirror,
  targetById,
  type CreateTargets,
} from "../../../lib/calendarEdit/createTargets";
import { isLetterKey } from "../../../lib/letterKey";
import { formatDateLocal } from "../../../lib/format";
import { deviceTimeZoneOrNull } from "../../../theme";
import { Button, Callout, ConfirmDialog, Dialog } from "../../ui";
import { CalendarPicker } from "./CalendarPicker";
import {
  ShowAsControl,
  TextRow,
  VisibilitySelect,
  WhenFields,
  ZonePanel,
  type Timed,
} from "./EventFields";
import { createEvent, noteCreateResult, noteHeldCreate, type CreateSent } from "./useEventWrites";

interface Props {
  /** The fields it starts with (`newEventSeed`): closing unchanged from these asks nothing. */
  seed: EditorFields;
  targets: CreateTargets;
  /** The calendar it starts on (PM's id): the last one created in, or the default. */
  calendarId: string;
  /** The mirror's window and each account's default length. */
  overview: CalendarOverview;
  /** Calendars hidden in the calendar list. */
  hidden: ReadonlySet<string>;
  colorOf: (calendarId: string) => string;
  shapeOf: (calendarId: string) => number | undefined;
  /** Filled in with a way to bring this editor's title into focus: a second "+ New event" focuses
   *  the one open rather than replacing it. */
  focusRef?: MutableRefObject<(() => void) | null>;
  onClose: () => void;
}

/** An answer that never arrived: its retry token, exactly what was sent, and the fields that were. */
interface Held {
  token: string;
  sent: CreateSent;
  fields: EditorFields;
}

/** A held token that ended without Google showing the event made (PM lost it, the draft differed,
 *  or it was made and since deleted): what was sent, and the answer that ended it. */
interface Unproven {
  sent: CreateSent;
  outcome: WriteOutcome;
}

const PRIVATE_OFF = "This calendar's sharing doesn't let you make private events on it.";
// The edit mode's line says times can't change; here nothing can be saved, which it has to say.
const NO_ZONE_CREATE =
  "PM couldn't tell which time zone this computer is in, so it can't save a new event. Set the time zone in your system settings.";

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

export function NewEventEditor({
  seed,
  targets,
  calendarId: firstCalendar,
  overview,
  hidden,
  colorOf,
  shapeOf,
  focusRef,
  onClose,
}: Props) {
  const deviceZone = useMemo(() => deviceTimeZoneOrNull(), []);
  const [draft, setDraftState] = useState<EditorFields>(seed);
  // The latest draft and calendar, readable synchronously: Save blurs the focused field first, and a
  // date typed but not yet committed commits in that blur, before React re-renders.
  const draftRef = useRef<EditorFields>(seed);
  const [calendarId, setCalendarIdState] = useState(firstCalendar);
  const calendarRef = useRef(firstCalendar);
  const badDateRef = useRef<string | null>(null);
  const [badDate, setBadDateState] = useState<string | null>(null);
  const setBadDate = (text: string | null) => {
    badDateRef.current = text;
    setBadDateState(text);
  };
  // The event's length, kept while its start moves: the account's default to begin with.
  const lengthRef = useRef(lengthFor(overview, targetById(targets, firstCalendar)));
  // Whether the user has changed the time: until then, another calendar brings its own length.
  const timeTouched = useRef(false);
  // Whether the user has picked Show as: until then, it follows the kind (all day free).
  const showAsTouched = useRef(false);
  const [banner, setBanner] = useState<{ tone: Tone; text: string } | null>(null);
  // The one guard every way to save passes, set before the request goes (state alone would let a
  // second Enter in the same tick through).
  const savingRef = useRef(false);
  const [saving, setSaving] = useState(false);
  const heldRef = useRef<Held | null>(null);
  const [held, setHeldState] = useState<Held | null>(null);
  // PM may have made an event this editor sent, though no token is held for it any more.
  const unprovenRef = useRef<Unproven | null>(null);
  const [unproven, setUnprovenState] = useState<Unproven | null>(null);
  const [askDiscard, setAskDiscardState] = useState(false);
  const askDiscardRef = useRef(false);
  const [zonePick, setZonePick] = useState<"both" | "start" | "end" | null>(null);
  const saveButton = useRef<HTMLButtonElement>(null);
  const titleBox = useRef<HTMLDivElement>(null);
  // The latest overview and hidden set, for the words an answer gets after the props moved on.
  const overviewRef = useRef(overview);
  const hiddenRef = useRef(hidden);
  useEffect(() => {
    overviewRef.current = overview;
    hiddenRef.current = hidden;
  });
  // Whether this editor is still showing: an answer after it has gone is said as a notice instead.
  const alive = useRef(true);

  const setDraft = (next: EditorFields) => {
    draftRef.current = next;
    setDraftState(next);
  };
  const setCalendarId = (id: string) => {
    calendarRef.current = id;
    setCalendarIdState(id);
  };
  const setHeld = (next: Held | null) => {
    heldRef.current = next;
    setHeldState(next);
  };
  const setUnproven = (next: Unproven | null) => {
    unprovenRef.current = next;
    setUnprovenState(next);
  };
  const setAskDiscard = (open: boolean) => {
    askDiscardRef.current = open;
    setAskDiscardState(open);
  };
  const focusTitle = useCallback(
    () => titleBox.current?.querySelector<HTMLInputElement>("input")?.focus(),
    [],
  );
  const refocusSave = () => setTimeout(() => saveButton.current?.focus(), 0);
  const whereOf = (id: string) => createdWhere(overviewRef.current, hiddenRef.current, id);

  // The title takes focus first (after the dialog's own first focus, which this runs after).
  useEffect(() => {
    focusTitle();
  }, [focusTitle]);

  useEffect(() => {
    if (!focusRef) return;
    focusRef.current = focusTitle;
    return () => {
      focusRef.current = null;
    };
  }, [focusRef, focusTitle]);

  // Going (closed, or with the Calendar tab) while PM may already have made the event leaves a
  // notice with Check again. A save still out says how it ended itself, once it does. One whose
  // token ended unproven leaves its words (look for it before creating it again).
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      const u = unprovenRef.current;
      if (u) {
        const where = createdWhere(overviewRef.current, hiddenRef.current, u.sent.calendarId);
        noteCreateResult(u.sent, null, u.outcome, where);
      }
      const h = heldRef.current;
      if (h && !savingRef.current) noteHeldCreate(h.sent, h.token);
    };
  }, []);

  /** Commit whatever the focused field holds, so the draft read next is the one on screen. */
  function commitFocused() {
    const active = document.activeElement;
    if (active instanceof HTMLElement && active.closest('[role="dialog"]')) {
      active.blur();
      flushSync(() => {});
    }
  }

  /** Focus back on what `commitFocused` blurred (`el`), when the save it began went no further:
   *  never left on the page behind the dialog. */
  function refocus(el: Element | null) {
    if (!(el instanceof HTMLElement) || !el.closest('[role="dialog"]')) return;
    if (el.isConnected && !el.matches(":disabled")) el.focus();
    else focusTitle();
  }

  const isDirty = () =>
    changedFields(seed, draftRef.current).length > 0 || calendarRef.current !== firstCalendar;

  const requestClose = () => {
    if (savingRef.current) return;
    commitFocused();
    // Nothing to discard: what may be in Google stays in hand, as a notice.
    if (heldRef.current) onClose();
    else if (isDirty() || badDateRef.current) setAskDiscard(true);
    else onClose();
  };

  /** The event's length now, kept for the next start move, or the last it had (never under a
   *  minute, so a moved start always gets an end after it). */
  function lengthOf(t: Timed): number {
    const s = halfInstant(NEW_EVENT_BASE, t, "start");
    const e = halfInstant(NEW_EVENT_BASE, t, "end");
    if (s !== null && e !== null && e - s >= 60_000) lengthRef.current = e - s;
    return lengthRef.current;
  }

  /** Move the start, the end following so the event keeps its length. */
  const moveStart = (t: Timed, date: string, time: string): Timed =>
    endAfterStart(NEW_EVENT_BASE, { ...t, start_date: date, start_time: time }, lengthOf(t));

  /** A new end date: the end time stays if it still ends the event after its start, else becomes the
   *  first time on that date that does (or the length's worth after the start). */
  function moveEndDate(t: Timed, end_date: string): Timed {
    const moved = { ...t, end_date };
    if (endsAfterStart(NEW_EVENT_BASE, moved)) return moved;
    return (
      firstEndOnItsDate(NEW_EVENT_BASE, moved) ?? endAfterStart(NEW_EVENT_BASE, moved, lengthOf(t))
    );
  }

  /** New zones keep the times on screen; an end then at or before the start follows the start. */
  function changeZones(t: Timed, start_zone: string, end_zone: string): Timed {
    const length = lengthOf(t);
    const moved = { ...t, start_zone, end_zone };
    if (endsAfterStart(NEW_EVENT_BASE, moved)) return moved;
    return endAfterStart(NEW_EVENT_BASE, moved, length);
  }

  /** A time change by the user: from now on the time is theirs, whatever the calendar. */
  function changeTime(next: EditorFields) {
    timeTouched.current = true;
    setDraft(next);
  }

  /** Another calendar. While the time is still the seed's, the end follows that account's length
   *  (and back again), as Google's own length would. */
  function pickCalendar(id: string) {
    setCalendarId(id);
    if (timeTouched.current) return;
    const length = lengthFor(overviewRef.current, targetById(targets, id));
    lengthRef.current = length;
    const t = draftRef.current.time;
    if (t.kind === "timed") {
      setDraft({ ...draftRef.current, time: endAfterStart(NEW_EVENT_BASE, t, length) });
    }
  }

  async function save() {
    // Refused before anything is blurred: focus stays put (in the discard question too).
    if (savingRef.current || askDiscardRef.current) return;
    const h = heldRef.current;
    // Without a zone a new draft can't go (that's said under the fields); a held one goes as sent.
    if (!h && deviceZone === null) return;
    const focused = document.activeElement;
    commitFocused();
    let sent: CreateSent;
    let fields: EditorFields;
    if (h) {
      // Check again: exactly what was sent, under its token.
      sent = h.sent;
      fields = h.fields;
    } else {
      const current = draftRef.current;
      // What stops it is already said under When, as it happened.
      if (badDateRef.current || checkDraft(NEW_EVENT_BASE, current).problems.length > 0) {
        setBanner(null);
        refocus(focused);
        return;
      }
      sent = { calendarId: calendarRef.current, draft: toNewEvent(current) };
      fields = current;
    }
    const token = h?.token ?? null;
    savingRef.current = true;
    setSaving(true);
    setZonePick(null);
    setBanner(null);
    let outcome: WriteOutcome;
    try {
      outcome = await createEvent(sent.calendarId, sent.draft, token);
    } catch (e) {
      // No answer at all (the app's own channel failed): nothing says how it ended, so a held token
      // stays held.
      savingRef.current = false;
      if (!alive.current) {
        if (token) noteHeldCreate(sent, token);
        else
          noteCreateResult(
            sent,
            null,
            { outcome: "failed", message: message(e) },
            whereOf(sent.calendarId),
          );
        return;
      }
      setSaving(false);
      setBanner({ tone: "error", text: message(e) });
      refocusSave();
      return;
    }
    savingRef.current = false;
    if (!alive.current) {
      noteCreateResult(sent, token, outcome, whereOf(sent.calendarId));
      return;
    }
    setSaving(false);
    if (outcome.outcome === "saved") {
      setHeld(null);
      // Made again after the look-first words: they've done their job.
      setUnproven(null);
      noteCreateResult(sent, token, outcome, whereOf(sent.calendarId));
      onClose();
      return;
    }
    const next = retryAfter(token, outcome);
    if (next !== null) {
      // It may be in Google: show exactly what was sent, locked, for Check again.
      setHeld({ token: next, sent, fields });
      setDraft(fields);
      setCalendarId(sent.calendarId);
    } else {
      setHeld(null);
      // A held token ended without Google showing it made: it may still be there.
      if (h) setUnproven({ sent, outcome });
    }
    setBanner(createText(outcome, fields.summary, next !== null));
    refocusSave();
  }

  // Ctrl+S (⌘S) saves, as Google's event page does, through the same guard; the page's own save
  // never opens over the editor. A held key's repeats save nothing more (the first answer may have
  // just unlocked Save), nor does a key that's part of composing text.
  const saveNow = useRef(save);
  useEffect(() => {
    saveNow.current = save;
  });
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.altKey || e.shiftKey || !isLetterKey(e, "s")) return;
      e.preventDefault();
      if (e.repeat || e.isComposing || e.keyCode === 229) return;
      void saveNow.current();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const locked = saving || held !== null;
  const timeOff = deviceZone === null;
  const timeEditable = !locked && !timeOff;
  const target = targetById(targets, calendarId);
  const noPrivate = target?.noPrivate ?? false;
  const check = checkDraft(NEW_EVENT_BASE, draft);
  const timed = draft.time.kind === "timed" ? draft.time : null;
  // The End list resolves every quarter hour, so it's worked out once per time change.
  const endChoices = useMemo(() => (timed ? endTimeChoices(NEW_EVENT_BASE, timed) : []), [timed]);
  const endMin = useMemo(() => (timed ? endDateMin(NEW_EVENT_BASE, timed) : undefined), [timed]);
  const where = createdWhere(overview, hidden, calendarId);
  const outside = outsideMirror(draft.time, overview);
  const range = `${formatDateLocal(new Date(overview.mirror_start))} – ${formatDateLocal(
    new Date(overview.mirror_end),
  )}`;
  const lines = [
    ...new Set([
      ...(badDate
        ? [
            `“${badDate}” isn't a date PM can read. Type it as dd-mm-yyyy, or pick it from the calendar.`,
          ]
        : []),
      ...check.problems.map(problemText),
    ]),
  ];
  const notes = [
    ...(timeOff ? [NO_ZONE_CREATE] : []),
    ...(outside
      ? [
          `This starts outside the months PM syncs (${range}), so it'll be saved in Google but won't show here.`,
        ]
      : []),
    ...(hidden.has(calendarId) && where.hidden
      ? [
          `${where.name} is hidden in your calendar list, so the new event won't show until you show that calendar again.`,
        ]
      : []),
    ...(noPrivate ? [PRIVATE_OFF] : []),
  ];

  return (
    <>
      <Dialog
        open
        onClose={requestClose}
        chrome="bar"
        title="New event"
        closeLabel="Close"
        closeDisabled={saving}
        widthClassName="max-w-lg"
        heightClassName="max-h-[85vh]"
        footer={
          <>
            <Button variant="tertiary" onClick={requestClose} disabled={saving}>
              {held ? "Close" : "Cancel"}
            </Button>
            <Button
              ref={saveButton}
              variant="primary"
              onClick={() => void save()}
              // A held Enter here, once an answer put focus on it, clicks it again on every repeat.
              onKeyDown={(e) => {
                if (e.key === "Enter" && e.repeat) e.preventDefault();
              }}
              disabled={saving || timeOff}
            >
              {saving ? (held ? "Checking…" : "Saving…") : held ? "Check again" : "Save"}
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-4">
          {banner && (
            <Callout tone={CALLOUT_TONE[banner.tone]} live>
              {banner.text}
            </Callout>
          )}

          <div className="flex flex-col items-start gap-1">
            <span className="text-sm text-ink2">Calendar</span>
            <CalendarPicker
              targets={targets}
              value={calendarId}
              onChange={pickCalendar}
              privateDraft={draft.visibility === "private"}
              hidden={hidden}
              colorOf={colorOf}
              shapeOf={shapeOf}
              disabled={locked}
            />
          </div>

          <div
            ref={titleBox}
            onKeyDown={(e) => {
              // Enter in the title saves, as in Google's quick box; not while composing text (WebKit
              // sends the Enter that ends a composition after it, as key code 229), nor again on a
              // held key's repeats.
              if (e.key !== "Enter" || e.shiftKey) return;
              if (e.nativeEvent.isComposing || e.nativeEvent.keyCode === 229) return;
              if (!(e.target instanceof HTMLInputElement)) return;
              e.preventDefault();
              if (e.repeat) return;
              void save();
            }}
          >
            <TextRow
              label="Title"
              value={draft.summary}
              disabled={locked}
              onChange={(summary) => setDraft({ ...draft, summary })}
            />
          </div>

          <WhenFields
            draft={draft}
            held={null}
            endChoices={endChoices}
            endMin={endMin}
            disabled={!timeEditable}
            onChange={(time) => {
              setBadDate(null);
              changeTime({ ...draft, time });
            }}
            onRejectDate={setBadDate}
            onAcceptDate={() => setBadDate(null)}
            moveStart={moveStart}
            moveEndDate={moveEndDate}
            onSwitchKind={() =>
              deviceZone &&
              changeTime(
                switchNewKind(draft, seed, deviceZone, lengthRef.current, showAsTouched.current),
              )
            }
            canSwitchKind={!timeOff}
            onPickZone={setZonePick}
          />
          {zonePick && timeEditable && draft.time.kind === "timed" && (
            <ZonePanel
              onPick={(zone) => {
                if (draft.time.kind !== "timed") return;
                const t = draft.time;
                changeTime({
                  ...draft,
                  time: changeZones(
                    t,
                    zonePick === "end" ? t.start_zone : zone,
                    zonePick === "start" ? t.end_zone : zone,
                  ),
                });
                setZonePick(null);
              }}
              onKeep={() => setZonePick(null)}
            />
          )}
          {check.ambiguous.map((half) => (
            <p key={half} className="text-xs text-ink4">
              {ambiguousText(half)}
            </p>
          ))}
          {/* What would stop the save, shown as it happens rather than only on Save. */}
          {lines.map((text) => (
            <p key={text} className="text-xs text-st-due">
              {text}
            </p>
          ))}

          <TextRow
            label="Location"
            value={draft.location}
            disabled={locked}
            onChange={(location) => setDraft({ ...draft, location })}
          />
          <TextRow
            label="Description"
            multiline
            value={draft.description}
            disabled={locked}
            onChange={(description) => setDraft({ ...draft, description })}
          />

          <div className="flex flex-wrap items-center gap-4">
            <ShowAsControl
              value={draft.show_as}
              disabled={locked}
              onChange={(show_as) => {
                showAsTouched.current = true;
                setDraft({ ...draft, show_as });
              }}
            />
            <VisibilitySelect
              value={draft.visibility}
              disabled={locked}
              onChange={(visibility) => setDraft({ ...draft, visibility })}
              privateOff={noPrivate}
            />
          </div>

          {notes.map((n) => (
            <p key={n} className="text-xs leading-relaxed text-ink4">
              {n}
            </p>
          ))}
        </div>
      </Dialog>

      <ConfirmDialog
        open={askDiscard}
        title="Discard this new event?"
        confirmLabel="Discard"
        cancelLabel="Keep editing"
        danger
        onClose={() => setAskDiscard(false)}
        onConfirm={() => {
          setAskDiscard(false);
          onClose();
        }}
      >
        {unproven
          ? createText(unproven.outcome, unproven.sent.draft.summary, false).text
          : "Nothing has been saved to Google yet."}
      </ConfirmDialog>
    </>
  );
}
