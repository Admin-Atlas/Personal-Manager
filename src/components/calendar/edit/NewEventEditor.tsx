// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The full editor's create mode (#884, C10): a new event on one of the user's Google calendars. A
// sibling of `EventEditor` sharing its fields (`EventFields`), with nothing to load, no session, no
// conflicts and nothing to delete: the same dialog and checks, and a calendar picker where editing
// names the calendar it saves to. Save, Ctrl+S (⌘S on a Mac) and Enter in the title all save.
// Quick create's More options opens it on that draft (`handover`), sending nothing on the way.
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
//
// It can repeat (C11): the Repeat row under When offers Google's ready-made rules for the start
// date and Custom…, whose dialog opens inside this one. The backend says what a rule means at the
// start: its words, the day it really starts on when the rule doesn't allow the one typed (said,
// never written into the date fields; the save moves it), or why it can't be saved. Until it has
// said so for exactly the rule and start on screen, nothing saves, by any way to save. A rule whose
// day came from the start date follows the start when it moves.
//
// It can have guests (C12d): the guest box sits under When, as in the editor of an existing event,
// with suggestions from people already in the user's calendars and the three guest permissions.
// Send or Don't send has no default: a new event with guests asks first, from the editor's own
// count, before anything is sent, and again with the backend's numbers if they differ. The question
// sits inside this dialog, so Escape closes only it, and no way to save or close goes from behind
// it. What was sent keeps its answer: Check again sends it again with the draft (the backend sends
// the answer first sent whatever comes). Quick create has no guest box: More options opens this.
//
// Notifications and colour (C13) sit under the location, as when editing: left as the calendar's,
// nothing is sent for them and Google gives the event the calendar's own. Another calendar keeps a
// list the user made (the defaults shown are the new calendar's). Switching between timed and
// all-day puts a list of the event's own back to the defaults, and says so. A notification row that
// can't be committed stops every way to save, as a date a field couldn't read does. Quick create has
// neither row, as Google's has none (INFERRED).
//
// It can have a Google Meet call (C18b): the video-call row sits after the guests, as when editing,
// where the calendar offers Meet. Until the user touches it, it follows Google: on with the first
// guest when the account's "Automatically add Google Meet video conferences to events I create" is
// on, off again when no guest is left (`meetDefault`), and another calendar decides it afresh.
// Touched, it stays, except on a calendar without Meet, which turns it off and says so.
//
// It can open on a copy of an event (Duplicate, C18c, `copyOf`): the copy's fields are the seed, so
// closing it unchanged asks nothing and sends nothing; a line under the bar says what it copies and
// what it doesn't. Its call starts off and stays so until pressed, whatever the setting, as Google's
// Duplicate makes none; its description is copied unchanged.
//
// The description is formatted with Google Calendar's buttons, as when editing (C19d). A copy's
// description with formatting PM can't write back shows what PM can of it, with "Replace with plain
// text…", and is created exactly as Google holds it unless replaced.

import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type MutableRefObject,
} from "react";
import { flushSync } from "react-dom";
import type { CalendarOverview, GuestNotice, Notify, WriteOutcome } from "../../../lib/types";
import {
  COPY_CALL_OFF_NOTE,
  NEW_EVENT_BASE,
  NO_BAD_DATES,
  changedFields,
  checkDraft,
  endAfterStart,
  endDateMin,
  endTimeChoices,
  endsAfterStart,
  firstEndOnItsDate,
  followStart,
  guestNotice,
  halfInstant,
  hasBadDate,
  newEventLength,
  refusedDateId,
  remindersAfterKindSwitch,
  switchNewKind,
  toNewEvent,
  type BadDates,
  type EditorFields,
} from "../../../lib/calendarEdit/eventDraft";
import { kindNoteText, type KindSwitchNote } from "../../../lib/calendarEdit/reminders";
import {
  parseDescription,
  replacementDescription,
} from "../../../lib/calendarEdit/descriptionHtml";
import {
  ambiguousText,
  CALLOUT_TONE,
  createText,
  heldCreateText,
  problemText,
  reasonText,
  retryAfter,
  type Tone,
} from "../../../lib/calendarEdit/editReasons";
import {
  createdWhere,
  createNotes,
  lengthFor,
  meetDefault,
  meetOffered,
  meetOffText,
  meetOnCalendar,
  targetById,
  type CreateTargets,
} from "../../../lib/calendarEdit/createTargets";
import { sameAddress, type GuestSuggestion } from "../../../lib/calendarEdit/guests";
import { repeatStartOf, startDateOf } from "../../../lib/calendarEdit/recurrenceDraft";
import { isLetterKey } from "../../../lib/letterKey";
import { deviceTimeZoneOrNull } from "../../../theme";
import { Button, Callout, ConfirmDialog, Dialog } from "../../ui";
import { CalendarPicker } from "./CalendarPicker";
import { CustomRecurrenceDialog } from "./CustomRecurrenceDialog";
import { DescriptionEditor, DescriptionView, ReplaceQuestion } from "./DescriptionEditor";
import { EventColorPicker } from "./EventColorPicker";
import { GuestField } from "./GuestField";
import { MeetField } from "./MeetField";
import { NotifyPrompt } from "./NotifyPrompt";
import { ReminderList } from "./ReminderList";
import {
  NO_ZONE_CREATE,
  RepeatRow,
  ShowAsControl,
  TextRow,
  VisibilitySelect,
  WhenFields,
  ZonePanel,
  type Timed,
} from "./EventFields";
import { createEvent, noteCreateResult, noteHeldCreate, type CreateSent } from "./useEventWrites";
import { useRepeatView } from "./useRepeat";
import { useMailLink } from "../parts/useMailLink";

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
  /** People already in the user's calendars, for the guest box (worked out by the calendar).
   *  Required, so a caller can't leave the box without them unnoticed. */
  suggestions: readonly GuestSuggestion[];
  colorOf: (calendarId: string) => string;
  shapeOf: (calendarId: string) => number | undefined;
  /** Filled in with a way to bring this editor's title into focus: a second "+ New event" focuses
   *  the one open rather than replacing it. */
  focusRef?: MutableRefObject<(() => void) | null>;
  /** Quick create's "More options": its draft and calendar to carry on with (`seed` and
   *  `calendarId` stay where it started, for "changed?"), an answer that never arrived, which opens
   *  locked on Check again under the same token, the words of a token that ended unproven, and a
   *  date its field couldn't read, still refusing. */
  handover?: CreateHandover;
  /** A copy of an event (Duplicate, C18c): `seed` holds its fields. */
  copyOf?: CopyOf;
  onClose: () => void;
}

/** What the editor says of a copy it opens on (C18c): the original's title and what the copy leaves
 *  out (`EventCopy`, the fields aside). */
export interface CopyOf {
  title: string;
  notes: string[];
  callNote: string | null;
}

/** What quick create hands the full editor (C10c): nothing is sent on the way. */
export interface CreateHandover {
  draft: EditorFields;
  calendarId: string;
  /** A create PM may already have made: its retry token and exactly what was sent. */
  held: { token: string; sent: CreateSent } | null;
  /** A token that ended there without Google showing the event made: its words carry on here. */
  unproven: Unproven | null;
  /** Whether the user picked Show as, or changed the time, or pressed Google Meet, there. */
  showAsTouched: boolean;
  timeTouched: boolean;
  meetTouched: boolean;
  /** Text quick create's date field couldn't read: it stands here as the start's (Start date or
   *  First day), refusing a save until that field is given a date, as it did there. */
  badDate: string | null;
  /** Quick create's Repeat row picked Custom…: the dialog opens here, over the full editor, since a
   *  modal dialog over quick create's box would fight its click-outside rule. */
  openCustom: boolean;
}

/** Where the Custom dialog was opened from: the Repeat row here (focus goes back to it), or quick
 *  create's hand-over (focus goes to the title). */
type CustomFrom = "row" | "handover";

/** An answer that never arrived: its retry token, exactly what was sent, and the fields that were. */
interface Held {
  token: string;
  sent: CreateSent;
  fields: EditorFields;
}

/** A held token that ended without Google showing the event made (PM lost it, the draft differed,
 *  or it was made and since deleted): what was sent, and the answer that ended it. */
export interface Unproven {
  sent: CreateSent;
  outcome: WriteOutcome;
}

/** The email question, open: the new event it is for (as it will be sent, with no answer yet, and
 *  the fields it came from), the numbers, and `again` when the backend asked after an answer. */
interface Prompt {
  sent: CreateSent;
  fields: EditorFields;
  notice: GuestNotice;
  again: boolean;
}

const PRIVATE_OFF = "This calendar's sharing doesn't let you make private events on it.";

/** Said under the guest box when the calendar picked was on the list as a guest: it organises the
 *  event now, as Google lists it (C13 review round 1). */
const organiserLeftText = (address: string) =>
  `${address} organises this event now, so it came off the guest list.`;

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

export function NewEventEditor({
  seed,
  targets,
  calendarId: firstCalendar,
  overview,
  hidden,
  suggestions,
  colorOf,
  shapeOf,
  focusRef,
  handover,
  copyOf,
  onClose,
}: Props) {
  const deviceZone = useMemo(() => deviceTimeZoneOrNull(), []);
  const startDraft = handover?.draft ?? seed;
  const startCalendar = handover?.calendarId ?? firstCalendar;
  const [draft, setDraftState] = useState<EditorFields>(startDraft);
  // The latest draft and calendar, readable synchronously: Save blurs the focused field first, and a
  // date typed but not yet committed commits in that blur, before React re-renders.
  const draftRef = useRef<EditorFields>(startDraft);
  const [calendarId, setCalendarIdState] = useState(startCalendar);
  const calendarRef = useRef(startCalendar);
  // Text a date field couldn't read, by the half it sets: no save until that field is given a date.
  // Quick create's one date field sets the start.
  const startBad: BadDates = { ...NO_BAD_DATES, start: handover?.badDate ?? null };
  const badDatesRef = useRef<BadDates>(startBad);
  const [badDates, setBadDatesState] = useState<BadDates>(startBad);
  const setBadDates = (next: BadDates) => {
    badDatesRef.current = next;
    setBadDatesState(next);
  };
  // The When fields' own, so a save a refusal stops can find the date field it names.
  const whenId = useId();
  // The event's length, kept while its start moves or it goes all day and back: its own to begin
  // with (a range dragged on the grid, or quick create's, is the user's, C17), else the account's
  // default.
  const lengthRef = useRef(
    newEventLength(startDraft) ??
      newEventLength(seed) ??
      lengthFor(overview, targetById(targets, startCalendar)),
  );
  // Whether the user has changed the time: until then, another calendar brings its own length. A
  // copy's time is the original's, as Google holds it (C18 §7.1), so it starts as changed: another
  // calendar never makes it the account's default length.
  const timeTouched = useRef(handover?.timeTouched ?? copyOf !== undefined);
  // Whether the user has picked Show as: until then, it follows the kind (all day free). A copy's is
  // the original's, so it starts as picked, and all day and back keeps it, as editing the original
  // would.
  const showAsTouched = useRef(handover?.showAsTouched ?? copyOf !== undefined);
  // The draft's defaults are the ones a kind switch put there, untouched since: switching back puts
  // the seed's list back (`remindersAfterKindSwitch`). Only a copy's seed has a list of its own.
  const kindResetRef = useRef(false);
  // Whether the user has pressed the video-call row (here or in quick create): until then, it
  // follows Google's setting and the guest list (`meetDefault`). A copy starts as pressed: Google's
  // Duplicate ignores the setting (Workspace admin help 9898950).
  const meetTouched = useRef(handover?.meetTouched ?? copyOf !== undefined);
  // Another calendar turned the call off, offering no Meet: said under the row until it next changes.
  const [meetNote, setMeetNote] = useState<string | null>(null);
  // A copy's word that the original's call isn't copied: under the row until it's pressed.
  const [copyCallNote, setCopyCallNote] = useState<string | null>(copyOf?.callNote ?? null);
  // A copy's description as Google holds it (the copy's seed, never the draft), C19d: one with
  // formatting PM can't write back shows what PM can of it, with "Replace with plain text…", and
  // goes to Google unchanged unless replaced. A new event sends whatever the draft holds, so the
  // replacement is the draft's alone: nothing else needs telling.
  const copiedDescription = useMemo(
    () => (copyOf ? parseDescription(seed.description) : null),
    [copyOf, seed.description],
  );
  const [askReplace, setAskReplace] = useState(false);
  const [replaced, setReplaced] = useState(false);
  const descriptionId = useId();
  const descriptionBoxId = useId();
  const formattingReasonId = useId();
  const mail = useMailLink("new event");
  // Handed over while PM may already have made it: locked on what was sent, saying so; or after a
  // token ended unproven there, still saying that.
  const handedHeld: Held | null = handover?.held
    ? { ...handover.held, fields: handover.draft }
    : null;
  const [banner, setBanner] = useState<{ tone: Tone; text: string } | null>(() => {
    if (handedHeld) return heldCreateText(handedHeld.sent.draft.summary);
    const u = handover?.unproven;
    return u ? createText(u.outcome, u.sent.draft.summary, false) : null;
  });
  // The one guard every way to save passes, set before the request goes (state alone would let a
  // second Enter in the same tick through).
  const savingRef = useRef(false);
  const [saving, setSaving] = useState(false);
  const heldRef = useRef<Held | null>(handedHeld);
  const [held, setHeldState] = useState<Held | null>(handedHeld);
  // PM may have made an event this editor (or quick create, before it) sent, though no token is held
  // for it any more.
  const unprovenRef = useRef<Unproven | null>(handover?.unproven ?? null);
  const [unproven, setUnprovenState] = useState<Unproven | null>(handover?.unproven ?? null);
  const [askDiscard, setAskDiscardState] = useState(false);
  const askDiscardRef = useRef(false);
  const [zonePick, setZonePick] = useState<"both" | "start" | "end" | null>(null);
  // The Custom dialog, while open: no save goes from behind it (Ctrl+S included).
  const startCustom: CustomFrom | null = handover?.openCustom ? "handover" : null;
  const customRef = useRef<CustomFrom | null>(startCustom);
  const [custom, setCustomState] = useState<CustomFrom | null>(startCustom);
  // The email question (null = closed), readable at once: no save or close goes from behind it.
  const promptRef = useRef<Prompt | null>(null);
  const [prompt, setPromptState] = useState<Prompt | null>(null);
  // Times the backend asked this Save whether Google sends the invitations: once, then it says so.
  const guestAskRef = useRef(0);
  // Text left in the guest box that isn't an address: no save until it's cleared or fixed.
  const badGuestRef = useRef<string | null>(null);
  const [badGuest, setBadGuestState] = useState<string | null>(null);
  // A notification row holding something it can't commit (said under it): no save meanwhile.
  const badRemindersRef = useRef(false);
  // The kind switch put the defaults back: said under the list until it next changes, naming the
  // calendar picked when it's said (the defaults are that calendar's).
  const [remindersNote, setRemindersNote] = useState<KindSwitchNote | null>(null);
  // A guest another calendar's address took off the list when that calendar was picked, since it
  // organises the event now: said under the guest box until the list next changes.
  const [guestNote, setGuestNote] = useState<string | null>(null);
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
  // What the backend says of the repeat at this start; until it says it can be saved, nothing saves.
  const repeat = useRepeatView(draft.recurrence, repeatStartOf(draft.time));

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
  const setCustom = (from: CustomFrom | null) => {
    customRef.current = from;
    setCustomState(from);
  };
  const setPrompt = (next: Prompt | null) => {
    promptRef.current = next;
    setPromptState(next);
  };
  const setBadGuest = (text: string | null) => {
    badGuestRef.current = text;
    setBadGuestState(text);
  };
  /** The account a new event on `id` is saved through: who Google sends its invitations from, and
   *  never suggested as a guest. */
  const accountOf = (id: string) => targetById(targets, id)?.account.email ?? null;
  const focusTitle = useCallback(
    () => titleBox.current?.querySelector<HTMLInputElement>("input")?.focus(),
    [],
  );
  const refocusSave = () => setTimeout(() => saveButton.current?.focus(), 0);
  const whereOf = (id: string) => createdWhere(overviewRef.current, hiddenRef.current, id);

  // The title takes focus first (after the dialog's own first focus, which this runs after), unless
  // quick create handed over with the Custom dialog open, which has it.
  useEffect(() => {
    if (!customRef.current) focusTitle();
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
      // What the editor held as it went, said only once it has really gone: StrictMode rehearses an
      // unmount in development (a cleanup straight followed by a setup, which sets `alive` again
      // before this runs), and an editor opened on quick create's token or words holds them from
      // its first render, so a rehearsal would say them while it's still open.
      const u = unprovenRef.current;
      const h = savingRef.current ? null : heldRef.current;
      queueMicrotask(() => {
        if (alive.current) return;
        if (u) {
          const where = createdWhere(overviewRef.current, hiddenRef.current, u.sent.calendarId);
          noteCreateResult(u.sent, null, u.outcome, where);
        }
        if (h) noteHeldCreate(h.sent, h.token);
      });
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

  /** Focus on `el` (what `commitFocused` blurred, or the field a refusal names) when the save it
   *  began went no further: never left on the page behind the dialog. */
  function refocus(el: Element | null) {
    if (!(el instanceof HTMLElement) || !el.closest('[role="dialog"]')) return;
    if (el.isConnected && !el.matches(":disabled")) el.focus();
    else focusTitle();
  }

  const isDirty = () =>
    changedFields(seed, draftRef.current).length > 0 || calendarRef.current !== firstCalendar;

  const requestClose = () => {
    if (savingRef.current || promptRef.current) return;
    commitFocused();
    // Nothing to discard: what may be in Google stays in hand, as a notice. Text in the guest box
    // that isn't an address, or a notification row that can't be committed, never reached the
    // draft: it would be lost too.
    if (heldRef.current) onClose();
    else if (
      isDirty() ||
      hasBadDate(badDatesRef.current) ||
      badGuestRef.current !== null ||
      badRemindersRef.current
    ) {
      setAskDiscard(true);
    } else onClose();
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

  /** The Custom dialog closed (Done, Cancel or Escape). Opened from the Repeat row, its focus trap
   *  hands focus back to that row; opened by quick create's hand-over, there's nothing here it came
   *  from, so the title takes it, as it would have on open. */
  function closeCustom() {
    const from = customRef.current;
    setCustom(null);
    if (from === "handover") setTimeout(focusTitle, 0);
  }

  /** A time change by the user: from now on the time is theirs, whatever the calendar. A rule whose
   *  day came from the start date follows it. */
  function changeTime(next: EditorFields) {
    timeTouched.current = true;
    setDraft(followStart(draftRef.current, next));
  }

  /** All day ↔ timed, as a time change, with the notifications fitted to the new kind. */
  function changeKind(zone: string) {
    const was = draftRef.current;
    const switched = switchNewKind(was, seed, zone, lengthRef.current, showAsTouched.current);
    // A new event's seed is on the calendar's defaults (`newEventSeed`); a copy's has the original's
    // list, which switching back puts back, as the event editor does.
    const r = remindersAfterKindSwitch(seed, switched, true, kindResetRef.current);
    kindResetRef.current = r.reset;
    changeTime(r.draft);
    setRemindersNote(r.note);
  }

  /** Another calendar. While the time is still the seed's, the end follows that account's length
   *  (and back again), as Google's own length would. The calendar organises the event, so its own
   *  address typed as a guest on another one comes off the list, said under the box: GuestField
   *  refuses adding it, and the backend counts it as the event's own, never one Google invites (C13
   *  review round 1). The video call follows the new calendar (`meetOnCalendar`, C18b). */
  function pickCalendar(id: string) {
    setCalendarId(id);
    const target = targetById(targets, id);
    const organiser = target?.calendar.remote_id ?? null;
    const was = draftRef.current;
    const left = organiser
      ? was.guests.filter((g) => !sameAddress(g.email, organiser))
      : was.guests;
    const dropped = left.length !== was.guests.length;
    let next = dropped ? { ...was, guests: left } : was;
    setGuestNote(dropped && organiser ? organiserLeftText(organiser) : null);
    if (!timeTouched.current) {
      const length = lengthFor(overviewRef.current, target);
      lengthRef.current = length;
      const t = next.time;
      if (t.kind === "timed") next = { ...next, time: endAfterStart(NEW_EVENT_BASE, t, length) };
    }
    const call = meetOnCalendar(overviewRef.current, target, next, meetTouched.current);
    if (call.meet !== next.meet) next = { ...next, meet: call.meet };
    setMeetNote(call.off && target ? meetOffText(target.calendar.name) : null);
    if (next !== was) setDraft(next);
  }

  /** The guest list changed: an untouched video call follows it, on with the first guest where the
   *  account's setting adds Meet, off when none is left (C18b). */
  function changeGuests(guests: EditorFields["guests"]) {
    const next = { ...draftRef.current, guests };
    const meet = meetTouched.current
      ? next.meet
      : meetDefault(overviewRef.current, targetById(targets, calendarRef.current), guests);
    setDraft(meet === next.meet ? next : { ...next, meet });
  }

  /** Whether the backend has said `fields`' repeat can be saved, for exactly its rule and start (a
   *  date the blur before a save just committed is a new start, not yet asked about). */
  const repeatReady = (fields: EditorFields) =>
    repeat.saveable(fields.recurrence, repeatStartOf(fields.time));

  async function save() {
    // Refused before anything is blurred: focus stays put (in the discard question, the Custom
    // dialog and the email question, too).
    if (savingRef.current || askDiscardRef.current || customRef.current || promptRef.current) {
      return;
    }
    const h = heldRef.current;
    // Without a zone a new draft can't go (that's said under the fields); a held one goes as sent.
    if (!h && deviceZone === null) return;
    const focused = document.activeElement;
    commitFocused();
    // A new Save: no earlier one's ask of the email question carries into it.
    guestAskRef.current = 0;
    if (h) {
      // Check again: exactly what was sent, its answer included, under its token.
      await send(h.sent, h.fields, h.token);
      return;
    }
    const current = draftRef.current;
    // What stops it is already said under When, as it happened. A date a field couldn't read puts
    // focus on that field, which the line names and only a date typed or picked there clears (a
    // blur gives it nothing, so the refusal stands); anything else puts focus back where it was.
    if (hasBadDate(badDatesRef.current)) {
      setBanner(null);
      refocus(document.getElementById(refusedDateId(whenId, badDatesRef.current)));
      return;
    }
    // A repeat the backend refused, or hasn't answered for yet, is said under the Repeat row too;
    // text in the guest box that isn't an address, under the box and in the problem lines; a
    // notification that can't be committed, under its row.
    if (
      checkDraft(NEW_EVENT_BASE, current).problems.length > 0 ||
      !repeatReady(current) ||
      badGuestRef.current !== null ||
      badRemindersRef.current
    ) {
      setBanner(null);
      refocus(focused);
      return;
    }
    const sent: CreateSent = { calendarId: calendarRef.current, draft: toNewEvent(current) };
    // Who it would invite, counted as the backend counts: asked before anything is sent, as
    // Google's own question is. Nobody, and it goes (the backend has the last word either way).
    const notice = guestNotice(NEW_EVENT_BASE, current, false, accountOf(sent.calendarId));
    if (notice) {
      setZonePick(null);
      setBanner(null);
      setPrompt({ sent, fields: current, notice, again: false });
      return;
    }
    await send(sent, current, null);
  }

  /** The email question answered: it closes and the new event goes with the answer, for its
   *  numbers. */
  function onNotify(n: Notify) {
    const p = promptRef.current;
    if (!p) return;
    setPrompt(null);
    void send({ ...p.sent, answer: { notify: n, asked: p.notice } }, p.fields, null);
  }

  /** Back (or Escape) from the email question: nothing was sent, and the draft stays. */
  function onNotifyBack() {
    setPrompt(null);
    refocusSave();
  }

  /** Send `sent` (`fields` being what it was made from), under `token` when it's Check again, and
   *  act on what came back. */
  async function send(sent: CreateSent, fields: EditorFields, token: string | null) {
    savingRef.current = true;
    setSaving(true);
    setZonePick(null);
    setBanner(null);
    let outcome: WriteOutcome;
    try {
      outcome = await createEvent(sent.calendarId, sent.draft, token, sent.answer ?? null);
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
    // Whether Google sends the invitations (C12d): asked with the backend's numbers when they aren't
    // the ones answered (or the editor counted nobody), once per Save; a second ask is said instead.
    // Nothing was sent or held. Check again never asks: it sends the answer first sent.
    if (
      outcome.outcome === "needs_choice" &&
      outcome.guests &&
      !outcome.scope &&
      token === null &&
      ++guestAskRef.current <= 1
    ) {
      setPrompt({
        sent: { calendarId: sent.calendarId, draft: sent.draft },
        fields,
        notice: outcome.guests,
        // "Again" only when an answer was given: the numbers differ from the ones answered.
        again: sent.answer !== undefined,
      });
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
      if (token !== null) setUnproven({ sent, outcome });
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
  const repeatHolds = repeat.state.state === "loading" || repeat.state.state === "refused";
  // A date a field couldn't read is said under When, naming the field. Text left in the guest box
  // that isn't an address is one too, beside the box's own line under it.
  const lines = [
    ...new Set([
      ...check.problems.map(problemText),
      ...(badGuest !== null ? [problemText({ kind: "bad_guest", email: badGuest })] : []),
    ]),
  ];
  const account = target?.account.email ?? null;
  const notes = [
    ...(timeOff ? [NO_ZONE_CREATE] : []),
    ...createNotes(draft.time, draft.recurrence.kind === "rule", overview, hidden, calendarId),
    ...(noPrivate ? [PRIVATE_OFF] : []),
  ];

  return (
    <>
      <Dialog
        open
        onClose={requestClose}
        chrome="bar"
        title="New event"
        helpId="calendar-new-event"
        subtitle={copyOf ? `A copy of “${copyOf.title.trim() || "(no title)"}”.` : undefined}
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
              // Check again sends exactly what was sent, whatever the Repeat row says now.
              disabled={saving || timeOff || (!held && repeatHolds)}
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

          {/* What a copy leaves out of the original (C18c). */}
          {copyOf?.notes.map((n) => (
            <p key={n} className="text-xs text-ink3">
              {n}
            </p>
          ))}

          <div className="flex flex-col items-start gap-1">
            <span className="text-sm text-ink2">Calendar</span>
            <CalendarPicker
              targets={targets}
              value={calendarId}
              onChange={pickCalendar}
              privateDraft={draft.visibility === "private" || draft.visibility === "confidential"}
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
            id={whenId}
            draft={draft}
            badDates={badDates}
            held={null}
            endChoices={endChoices}
            endMin={endMin}
            disabled={!timeEditable}
            onChange={(time) => changeTime({ ...draft, time })}
            onRejectDate={(half, text) => setBadDates({ ...badDatesRef.current, [half]: text })}
            onAcceptDate={(half) => setBadDates({ ...badDatesRef.current, [half]: null })}
            moveStart={moveStart}
            moveEndDate={moveEndDate}
            onSwitchKind={() => deviceZone && changeKind(deviceZone)}
            canSwitchKind={!timeOff}
            onPickZone={setZonePick}
            repeat={
              <RepeatRow
                value={draft.recurrence}
                view={repeat.state}
                time={draft.time}
                deviceZone={deviceZone}
                skippedTime={check.problems.some((p) => p.kind === "gap")}
                disabled={!timeEditable}
                onChange={(recurrence) => setDraft({ ...draftRef.current, recurrence })}
                onCustom={() => setCustom("row")}
              />
            }
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

          {/* The organiser of a new event is the calendar it goes to: never added as a guest. */}
          <GuestField
            guests={draft.guests}
            base={NEW_EVENT_BASE.guests}
            perms={draft.guest_perms}
            suggestions={suggestions}
            me={account}
            organizer={target?.calendar.remote_id ?? null}
            disabled={locked}
            omitted={false}
            notice={guestNote}
            onChange={(guests) => {
              setGuestNote(null);
              changeGuests(guests);
            }}
            onPerms={(guest_perms) => setDraft({ ...draftRef.current, guest_perms })}
            onReject={setBadGuest}
          />

          {/* A new event has no call yet: the row only asks for one (C18b). */}
          <MeetField
            call={null}
            value={draft.meet}
            offered={meetOffered(target)}
            disabled={locked}
            // A copy's note asks for a new call only where there's one to add.
            note={
              meetNote ?? (copyCallNote && !meetOffered(target) ? COPY_CALL_OFF_NOTE : copyCallNote)
            }
            onChange={(meet) => {
              meetTouched.current = true;
              setMeetNote(null);
              setCopyCallNote(null);
              setDraft({ ...draftRef.current, meet });
            }}
          />

          <TextRow
            label="Location"
            value={draft.location}
            disabled={locked}
            onChange={(location) => setDraft({ ...draft, location })}
          />

          <ReminderList
            value={draft.reminders}
            allDay={draft.time.kind === "all_day"}
            calendar={target?.calendar ?? null}
            disabled={locked}
            note={
              remindersNote &&
              kindNoteText(
                remindersNote,
                target?.calendar.name ?? null,
                draft.time.kind === "all_day",
              )
            }
            onChange={(reminders) => {
              setRemindersNote(null);
              kindResetRef.current = false;
              setDraft({ ...draftRef.current, reminders });
            }}
            onInvalid={(bad) => {
              badRemindersRef.current = bad;
            }}
          />

          <div className="flex flex-col items-start gap-1">
            <span className="text-sm text-ink2">Colour</span>
            <EventColorPicker
              value={draft.colour}
              calendarColour={colorOf(calendarId)}
              labelled={false}
              disabled={locked}
              onChange={(colour) => setDraft({ ...draftRef.current, colour })}
            />
          </div>

          <div className="flex flex-col gap-1">
            {/* Text, not a label element: the box is a div, which a label can't name. Clicking it
                still puts the caret in the box (critique C2-L7). */}
            <span
              id={descriptionId}
              className="text-sm text-ink2"
              onClick={() => document.getElementById(descriptionBoxId)?.focus()}
            >
              Description
            </span>
            {copiedDescription && !copiedDescription.supported && !replaced ? (
              // A copy's description PM can't write back goes as Google holds it, formatting kept
              // (C18 critique C2-L12), showing what PM can of it until the user replaces it.
              <>
                <DescriptionView
                  doc={copiedDescription.doc}
                  labelledBy={descriptionId}
                  describedBy={formattingReasonId}
                  onReplace={locked ? undefined : () => setAskReplace(true)}
                  onMail={mail.open}
                />
                <p id={formattingReasonId} className="text-xs leading-relaxed text-ink4">
                  {reasonText("unsupported_formatting")}
                </p>
              </>
            ) : (
              <DescriptionEditor
                id={descriptionBoxId}
                value={draft.description}
                onChange={(description) => setDraft({ ...draftRef.current, description })}
                disabled={locked}
                labelledBy={descriptionId}
                autoFocus={replaced}
                onMail={mail.open}
              />
            )}
            <p role="status" className="text-xs text-st-due">
              {mail.failure ?? ""}
            </p>
          </div>

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
              // Only a copy of an event that has Google's older choice holds it (C18c).
              confidential={
                seed.visibility === "confidential" || draft.visibility === "confidential"
              }
            />
          </div>

          {notes.map((n) => (
            <p key={n} className="text-xs leading-relaxed text-ink4">
              {n}
            </p>
          ))}
        </div>
        {/* Inside the editor's dialog, not beside it: nested, it alone is topmost, so one Escape
            closes only it and Tab stays in it. */}
        {custom && (
          <CustomRecurrenceDialog
            rule={draft.recurrence.kind === "rule" ? draft.recurrence.rule : null}
            startDate={startDateOf(draft.time)}
            onDone={(rule) => {
              setDraft({ ...draftRef.current, recurrence: { kind: "rule", rule } });
              closeCustom();
            }}
            onClose={closeCustom}
          />
        )}
        {/* "Replace with plain text…", nested the same way. */}
        <ReplaceQuestion
          open={askReplace}
          copy
          onKeep={() => setAskReplace(false)}
          onReplace={() => {
            setAskReplace(false);
            if (!copiedDescription) return;
            setDraft({
              ...draftRef.current,
              description: replacementDescription(seed.description),
            });
            setReplaced(true);
          }}
        />
        {/* The email question, nested the same way: Escape and Back close only it. */}
        {prompt && (
          <NotifyPrompt
            kind="create"
            notice={prompt.notice}
            from={accountOf(prompt.sent.calendarId)}
            again={prompt.again}
            onSend={onNotify}
            onBack={onNotifyBack}
          />
        )}
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
