// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Quick create (#884, C10c): the small box Shift+C opens under "+ New event", as Google Calendar's
// quick box. Type a title and press Enter; More options opens the full editor on the same draft,
// sending nothing on the way. Times are this computer's; an end on another day and other time
// zones are the full editor's, shown here as words.
//
// A non-modal dialog, like an event's details: no focus trap and no scrim, so the grid behind stays
// in view, with the new event drawn on it as it's typed (the ghost, fed by `onDraft`, which also
// says how far it has got). It registers as a dialog layer all the same, so the calendar's keys and
// swipe and the app's Ctrl+K stand down while it's open: the palette would switch tabs.
//
// A draft is never lost by accident. A click outside, or Escape, closes the box only while the title
// is empty; with a title it stays, and ✕ is how to throw it away. A click in a picker the box opened
// (the date's, portalled, so outside its frame) isn't outside, nor one on the ghost; Escape counts
// only with focus in the box, so Escape in another popover is that popover's. A titled draft that
// goes with the Calendar tab, nothing sent (a click in the sidebar), is handed back (`onLeave`), and
// the box opens on it again when the calendar does (`resume`). The calendar list stays inside the
// box, as it does in the full editor's dialog.
//
// Saving is the full editor's: one guard for every way to save, never on a held key's repeats or the
// Enter that ends composing text; nothing editable, closable or handed over while a save is out; and
// an answer that never arrived kept locked as it was sent, with Check again under the same retry
// token. An answer once the box has gone is said as a notice instead. A token that ended without
// Google showing the event made is never "nothing saved": its words stay as a notice when the box
// goes, or go on to the full editor with More options, until a save that's made. A date the field
// couldn't read refuses every save until that field is given a date: its line names the field, a
// save it stops puts focus there, and it goes on refusing in the full editor with More options.
//
// It can repeat (C11): a compact Repeat row offers Google's ready-made rules in place, and says where
// the event really starts or why its rule can't be saved, which stops every save as in the full
// editor. Custom… carries on in the full editor with its dialog open, sending nothing: a modal
// dialog over this box would fight its click-outside rule.

import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type MutableRefObject,
  type RefObject,
} from "react";
import { flushSync } from "react-dom";
import type { CalendarOverview, TimeDraft, WriteOutcome } from "../../../lib/types";
import type { DraftState } from "../../../lib/calendar-layout";
import {
  NEW_EVENT_BASE,
  checkDraft,
  dateFieldId,
  endAfterStart,
  endTimeChoices,
  followStart,
  halfInstant,
  switchNewKind,
  toNewEvent,
  type EditorFields,
} from "../../../lib/calendarEdit/eventDraft";
import { repeatStartOf } from "../../../lib/calendarEdit/recurrenceDraft";
import {
  ambiguousText,
  badDateText,
  CALLOUT_TONE,
  createText,
  problemText,
  retryAfter,
  type Tone,
} from "../../../lib/calendarEdit/editReasons";
import {
  createdWhere,
  createNotes,
  lengthFor,
  targetById,
  type CreateTargets,
} from "../../../lib/calendarEdit/createTargets";
import { formatDateOnly } from "../../../lib/format";
import { useDialogLayer } from "../../../lib/useDialogLayer";
import { deviceTimeZoneOrNull } from "../../../theme";
import { Button, Callout, IconButton, Input, Toggle } from "../../ui";
import { DateField } from "../../DateField";
import { TimeField } from "../../TimeField";
import { CalendarPicker } from "./CalendarPicker";
import { NO_ZONE_CREATE, RepeatRow, type Timed } from "./EventFields";
import type { CreateHandover, Unproven } from "./NewEventEditor";
import { createEvent, noteCreateResult, noteHeldCreate, type CreateSent } from "./useEventWrites";
import { useRepeatView } from "./useRepeat";

interface Props {
  /** The "+ New event" button's rect: the box opens under it. */
  anchor: DOMRect;
  /** The fields it starts with (`newEventSeed`). */
  seed: EditorFields;
  targets: CreateTargets;
  /** The calendar it starts on (PM's id). */
  calendarId: string;
  /** The mirror's window and each account's default length. */
  overview: CalendarOverview;
  /** Calendars hidden in the calendar list. */
  hidden: ReadonlySet<string>;
  colorOf: (calendarId: string) => string;
  shapeOf: (calendarId: string) => number | undefined;
  /** Where focus goes when the box closes from inside (saved, ✕, Escape): the header button, which
   *  Shift+C never focused, so remembering the focus at open would hand it to the page. */
  returnFocusTo: RefObject<HTMLElement | null>;
  /** Filled in with a way to bring the keyboard back into the box: a click on the ghost, or on "+ New
   *  event", comes back here rather than starting another. */
  focusRef?: MutableRefObject<(() => void) | null>;
  /** A draft that went with the Calendar tab (`onLeave`), to carry on with: its fields, whether its
   *  time was the user's, and a date it couldn't read. Its calendar is `calendarId`. */
  resume?: LeftDraft;
  /** The draft and calendar as they change, and how far the event has got, for the ghost. */
  onDraft: (draft: EditorFields, calendarId: string, state: DraftState) => void;
  /** More options: carry on in the full editor. */
  onMore: (handover: CreateHandover) => void;
  /** Gone: `saved` when the event was created, so its ghost can stay until the calendar shows it. */
  onClose: (saved: boolean) => void;
  /** Gone with the Calendar tab (never closed from inside), titled, with nothing sent: the draft, to
   *  open the box on again. Called once the box has really gone, and never for a draft that a notice
   *  or the full editor speaks for. */
  onLeave?: (left: LeftDraft) => void;
}

/** A titled draft whose box went with the Calendar tab, nothing sent: what the box opens on again. */
export interface LeftDraft {
  /** The fields the box started with, for All day's switch back. */
  seed: EditorFields;
  draft: EditorFields;
  calendarId: string;
  timeTouched: boolean;
  /** Text the date field couldn't read, still refusing a save. */
  badDate: string | null;
}

/** An answer that never arrived: its retry token, exactly what was sent, and the fields that were. */
interface Held {
  token: string;
  sent: CreateSent;
  fields: EditorFields;
}

type AllDay = Extract<TimeDraft, { kind: "all_day" }>;

/** Room kept between the box and the window's edge. */
const MARGIN = 8;

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** `date` (`YYYY-MM-DD`) as days since 1970, for moving an all-day span whole. */
const dayNumber = (date: string) => Date.parse(`${date}T00:00:00Z`) / 86_400_000;

/** An all-day span moved to start on `first_day`, keeping how many days it covers. */
function moveAllDay(t: AllDay, first_day: string): AllDay {
  const length = dayNumber(t.last_day) - dayNumber(t.first_day);
  const last = new Date((dayNumber(first_day) + length) * 86_400_000).toISOString().slice(0, 10);
  return { ...t, first_day, last_day: last };
}

export function QuickCreatePopover({
  anchor,
  seed,
  targets,
  calendarId: firstCalendar,
  overview,
  hidden,
  colorOf,
  shapeOf,
  returnFocusTo,
  focusRef,
  resume,
  onDraft,
  onMore,
  onClose,
  onLeave,
}: Props) {
  const deviceZone = useMemo(() => deviceTimeZoneOrNull(), []);
  const panelRef = useRef<HTMLDivElement>(null);
  const titleRef = useRef<HTMLInputElement>(null);
  const saveButton = useRef<HTMLButtonElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const startDraft = resume?.draft ?? seed;
  const [draft, setDraftState] = useState<EditorFields>(startDraft);
  // The latest draft and calendar, readable synchronously: Save blurs the focused field first, and a
  // date typed but not yet committed commits in that blur, before React re-renders.
  const draftRef = useRef<EditorFields>(startDraft);
  const [calendarId, setCalendarIdState] = useState(firstCalendar);
  const calendarRef = useRef(firstCalendar);
  // Text the date field couldn't read: no save until that field is given a date, typed or picked (a
  // blur, a time or All day never ends it), and More options carries it on. The one date field sets
  // the start (the whole event moves with it), so in the full editor it's the start's refusal.
  const badDateRef = useRef<string | null>(resume?.badDate ?? null);
  const [badDate, setBadDateState] = useState<string | null>(resume?.badDate ?? null);
  // The date field's id, so a save its refusal stops can find it, and the id of the line that says
  // what's wrong with it.
  const dateId = dateFieldId(useId(), "start");
  const refusedLineId = `${dateId}-refused`;
  // The event's length, kept while its start moves: the account's default to begin with.
  const lengthRef = useRef(lengthFor(overview, targetById(targets, firstCalendar)));
  // Whether the user has changed the time: until then, another calendar brings its own length.
  const timeTouched = useRef(resume?.timeTouched ?? false);
  const [banner, setBanner] = useState<{ tone: Tone; text: string } | null>(null);
  // The one guard every way to save passes, set before the request goes.
  const savingRef = useRef(false);
  const [saving, setSaving] = useState(false);
  const heldRef = useRef<Held | null>(null);
  const [held, setHeldState] = useState<Held | null>(null);
  // PM may have made an event this box sent, though no token is held for it any more.
  const unprovenRef = useRef<Unproven | null>(null);
  const [unproven, setUnprovenState] = useState<Unproven | null>(null);
  // The latest props, for what's said or handed back after they moved on.
  const overviewRef = useRef(overview);
  const hiddenRef = useRef(hidden);
  const seedRef = useRef(seed);
  const onLeaveRef = useRef(onLeave);
  useEffect(() => {
    overviewRef.current = overview;
    hiddenRef.current = hidden;
    seedRef.current = seed;
    onLeaveRef.current = onLeave;
  });
  // Whether the box is still showing: an answer after it has gone is said as a notice instead.
  const alive = useRef(true);
  // More options took the draft (and any retry token) to the full editor, which speaks for it now.
  const handedOver = useRef(false);
  // Closed from inside (saved, ✕, a click outside or Escape): nothing to hand back.
  const closed = useRef(false);
  // What the backend says of the repeat at this start; until it says it can be saved, nothing saves.
  const repeat = useRepeatView(draft.recurrence, repeatStartOf(draft.time));

  // A dialog layer while open: Ctrl+K, the calendar's keys and its swipe stand down.
  useDialogLayer(true, panelRef);

  const setDraft = (next: EditorFields) => {
    draftRef.current = next;
    setDraftState(next);
  };
  const setCalendarId = (id: string) => {
    calendarRef.current = id;
    setCalendarIdState(id);
  };
  const setBadDate = (text: string | null) => {
    badDateRef.current = text;
    setBadDateState(text);
  };
  const setHeld = (next: Held | null) => {
    heldRef.current = next;
    setHeldState(next);
  };
  const setUnproven = (next: Unproven | null) => {
    unprovenRef.current = next;
    setUnprovenState(next);
  };
  // Into the box: the title, or while it's locked (a save out, or Check again) Check again, else the
  // box itself, so a click that comes back here never leaves the keyboard outside it.
  const focusBox = useCallback(() => {
    const title = titleRef.current;
    const save = saveButton.current;
    if (title && !title.disabled) title.focus();
    else if (save && !save.disabled) save.focus();
    else panelRef.current?.focus();
  }, []);
  const refocusSave = () => setTimeout(() => saveButton.current?.focus(), 0);
  const whereOf = (id: string) => createdWhere(overviewRef.current, hiddenRef.current, id);

  // Under the button, clamped to the window, above it only when there's no room below; hidden until
  // placed so it never flashes at the corner. Placed again as it grows (a problem line, the banner).
  useLayoutEffect(() => {
    const el = panelRef.current;
    if (!el) return;
    const place = () => {
      const w = el.offsetWidth;
      const h = el.offsetHeight;
      const left = Math.max(MARGIN, Math.min(anchor.left, window.innerWidth - w - MARGIN));
      let top = anchor.bottom + 6;
      if (top + h + MARGIN > window.innerHeight) {
        const above = anchor.top - 6 - h;
        top = above >= MARGIN ? above : Math.max(MARGIN, window.innerHeight - h - MARGIN);
      }
      setPos((p) => (p && p.left === left && p.top === top ? p : { left, top }));
    };
    place();
    const ro = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(place);
    ro?.observe(el);
    window.addEventListener("resize", place);
    return () => {
      ro?.disconnect();
      window.removeEventListener("resize", place);
    };
  }, [anchor]);

  // The title takes focus once the box is placed (a hidden element can't), and only then: placing
  // it again as it grows must not pull focus back from wherever the user went.
  const focusedOnce = useRef(false);
  useEffect(() => {
    if (!pos || focusedOnce.current) return;
    focusedOnce.current = true;
    if (!panelRef.current?.contains(document.activeElement)) focusBox();
  }, [pos, focusBox]);

  useEffect(() => {
    if (!focusRef) return;
    focusRef.current = focusBox;
    return () => {
      focusRef.current = null;
    };
  }, [focusRef, focusBox]);

  // The ghost follows the draft and the calendar, and says how far the event has got: once anything
  // was sent, PM may have made it.
  const ghostState: DraftState = saving ? "saving" : held || unproven ? "unsure" : "unsaved";
  useEffect(() => {
    onDraft(draft, calendarId, ghostState);
  }, [draft, calendarId, ghostState, onDraft]);

  // Going (closed, or with the Calendar tab) while PM may already have made the event leaves a
  // notice with Check again, unless the full editor took it over. A save still out says how it
  // ended itself, once it does. One whose token ended unproven leaves its words (look for it before
  // creating it again), whichever way the box went: ✕, a click outside, Escape or the tab. A titled
  // draft that went with the tab, nothing sent, is handed back to open on again.
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      // What the box held as it went, acted on only once it has really gone: StrictMode rehearses an
      // unmount in development (a cleanup straight followed by a setup, which sets `alive` again
      // before this runs), and the notices and the hand-back must happen once, for a real one.
      const u = unprovenRef.current;
      const h = savingRef.current ? null : heldRef.current;
      const d = draftRef.current;
      const left: LeftDraft | null =
        closed.current || handedOver.current || savingRef.current || h || u || !d.summary.trim()
          ? null
          : {
              seed: seedRef.current,
              draft: d,
              calendarId: calendarRef.current,
              timeTouched: timeTouched.current,
              badDate: badDateRef.current,
            };
      queueMicrotask(() => {
        if (alive.current || handedOver.current) return;
        if (u) {
          const where = createdWhere(overviewRef.current, hiddenRef.current, u.sent.calendarId);
          noteCreateResult(u.sent, null, u.outcome, where);
        }
        if (h) noteHeldCreate(h.sent, h.token);
        if (left) onLeaveRef.current?.(left);
      });
    };
  }, []);

  /** Commit whatever the focused field holds, so the draft read next is the one on screen. */
  function commitFocused() {
    const active = document.activeElement;
    if (active instanceof HTMLElement && panelRef.current?.contains(active)) {
      active.blur();
      flushSync(() => {});
    }
  }

  /** Focus on `el` (what `commitFocused` blurred, or the date field a refusal names) when the save
   *  it began went no further: never left on the page behind the box. */
  function refocus(el: Element | null) {
    if (!(el instanceof HTMLElement) || !panelRef.current?.contains(el)) return;
    if (el.isConnected && !el.matches(":disabled")) el.focus();
    else focusBox();
  }

  /** Closed from inside: focus goes back to "+ New event". */
  function closeBack(saved: boolean) {
    closed.current = true;
    returnFocusTo.current?.focus();
    onClose(saved);
  }

  /** ✕: throw the draft away (a retry token held stays as a notice). Never while saving. */
  function discard() {
    if (savingRef.current) return;
    closeBack(false);
  }

  /** A click outside, or Escape: the box goes only while there's no title to lose, and never
   *  while saving. Escape hands focus back; a click has already put it where it was aimed. */
  function dismiss(by: "click" | "escape") {
    if (savingRef.current || draftRef.current.summary.trim() !== "") return;
    if (by === "escape") {
      closeBack(false);
    } else {
      closed.current = true;
      onClose(false);
    }
  }

  const dismissNow = useRef(dismiss);
  useEffect(() => {
    dismissNow.current = dismiss;
  });
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      const t = e.target;
      if (!(t instanceof Element) || panelRef.current?.contains(t)) return;
      // A picker this box opened is portalled outside it, and the ghost on the grid is this event.
      if (t.closest("[data-popover-panel], [data-draft-event]")) return;
      dismissNow.current("click");
    };
    // Escape only with focus in the box itself. A picker it opened closes itself first (a popover
    // stops the key before it reaches the window), so focus in any popover's panel means a popover
    // that isn't the box's, and the key is that one's.
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (e.target instanceof Node && panelRef.current?.contains(e.target)) {
        dismissNow.current("escape");
      }
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, []);

  /** The event's length now, kept for the next start move, or the last it had. */
  function lengthOf(t: Timed): number {
    const s = halfInstant(NEW_EVENT_BASE, t, "start");
    const e = halfInstant(NEW_EVENT_BASE, t, "end");
    if (s !== null && e !== null && e - s >= 60_000) lengthRef.current = e - s;
    return lengthRef.current;
  }

  /** Move the start, the end following so the event keeps its length. */
  const moveStart = (t: Timed, date: string, time: string): Timed =>
    endAfterStart(NEW_EVENT_BASE, { ...t, start_date: date, start_time: time }, lengthOf(t));

  /** A time change by the user: from now on the time is theirs, whatever the calendar. A rule whose
   *  day came from the start date follows it. */
  function changeTime(time: TimeDraft) {
    timeTouched.current = true;
    setDraft(followStart(draftRef.current, { ...draftRef.current, time }));
  }

  /** Another calendar. While the time is still the seed's, the end follows that account's length. */
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

  /** All day ↔ timed, as the full editor switches a new event. */
  function switchKind() {
    if (deviceZone === null) return;
    timeTouched.current = true;
    setDraft(switchNewKind(draftRef.current, seed, deviceZone, lengthRef.current, false));
  }

  /** More options: the full editor carries on with this draft (with the Custom dialog open, for the
   *  Repeat row's Custom…). Never while a save is out: the editor would start a second event under
   *  a new id while the first may still land. */
  function more(openCustom = false) {
    if (savingRef.current) return;
    commitFocused();
    handedOver.current = true;
    const h = heldRef.current;
    onMore({
      draft: draftRef.current,
      calendarId: calendarRef.current,
      held: h ? { token: h.token, sent: h.sent } : null,
      unproven: unprovenRef.current,
      // Quick create has no Show as: it still follows the kind.
      showAsTouched: false,
      timeTouched: timeTouched.current,
      // Read after the blur above, so a date typed and not yet read is refused there too: the field
      // put back the date it had, which the full editor must not save in its place.
      badDate: badDateRef.current,
      openCustom,
    });
  }

  /** Whether the backend has said `fields`' repeat can be saved, for exactly its rule and start (a
   *  date the blur before a save just committed is a new start, not yet asked about). */
  const repeatReady = (fields: EditorFields) =>
    repeat.saveable(fields.recurrence, repeatStartOf(fields.time));

  async function save() {
    // Refused before anything is blurred: focus stays put.
    if (savingRef.current) return;
    const h = heldRef.current;
    // Without a zone a new draft can't go (that's said under the times); a held one goes as sent.
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
      // What stops it is already said under the times, as it happened. A date the field couldn't
      // read puts focus on that field, which the line names and only a date typed or picked there
      // clears (a blur gives it nothing, so the refusal stands); anything else puts focus back where
      // it was.
      if (badDateRef.current !== null) {
        setBanner(null);
        refocus(document.getElementById(dateId));
        return;
      }
      // A repeat the backend refused, or hasn't answered for yet, is said under the Repeat row too.
      if (checkDraft(NEW_EVENT_BASE, current).problems.length > 0 || !repeatReady(current)) {
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
    setBanner(null);
    let outcome: WriteOutcome;
    try {
      outcome = await createEvent(sent.calendarId, sent.draft, token);
    } catch (e) {
      // No answer at all (the app's own channel failed): a held token stays held.
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
      closeBack(true);
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

  const locked = saving || held !== null;
  const timeOff = deviceZone === null;
  const timeEditable = !locked && !timeOff;
  const check = checkDraft(NEW_EVENT_BASE, draft);
  const t = draft.time;
  const timed = t.kind === "timed" ? t : null;
  // The End list resolves every quarter hour, so it's worked out once per time change.
  const endChoices = useMemo(() => (timed ? endTimeChoices(NEW_EVENT_BASE, timed) : []), [timed]);
  // The one date field's name, on screen and in its refusal.
  const dateName = timed ? "Date" : "Day";
  const repeatHolds = repeat.state.state === "loading" || repeat.state.state === "refused";
  // A date the field couldn't read is said under the times, naming the field.
  const lines = [...new Set(check.problems.map(problemText))];
  const notes = [
    ...(timeOff ? [NO_ZONE_CREATE] : []),
    ...createNotes(t, overview, hidden, calendarId),
  ];

  return (
    <div
      ref={panelRef}
      role="dialog"
      // No `aria-modal`: the calendar behind stays live, and Tab can leave. Over the calendar's write
      // notices (z-[48]), as a modal's scrim is: a notice that never goes would cover Save.
      aria-label="New event"
      tabIndex={-1}
      className="fixed z-50 flex w-[22rem] max-w-[calc(100vw-1rem)] flex-col gap-3 rounded-[var(--radius)] border border-border2 bg-panel p-3 shadow-2xl focus:outline-none"
      style={{
        left: pos?.left ?? anchor.left,
        top: pos?.top ?? anchor.bottom + 6,
        visibility: pos ? "visible" : "hidden",
      }}
    >
      <div className="flex items-center gap-2">
        <Input
          ref={titleRef}
          aria-label="Title"
          placeholder="Add title"
          value={draft.summary}
          disabled={locked}
          onChange={(e) => setDraft({ ...draftRef.current, summary: e.target.value })}
          onKeyDown={(e) => {
            // Enter saves, as in Google's quick box; not with a modifier, nor while composing text
            // (WebKit sends the Enter that ends a composition after it, as key code 229), nor again
            // on a held key's repeats.
            if (e.key !== "Enter" || e.shiftKey || e.ctrlKey || e.metaKey || e.altKey) return;
            if (e.nativeEvent.isComposing || e.nativeEvent.keyCode === 229) return;
            e.preventDefault();
            if (e.repeat) return;
            void save();
          }}
        />
        <IconButton
          label={held ? "Close" : "Discard"}
          onClick={discard}
          disabled={saving}
          className="shrink-0"
        >
          ×
        </IconButton>
      </div>

      {banner && (
        <Callout tone={CALLOUT_TONE[banner.tone]} live>
          {banner.text}
        </Callout>
      )}

      <fieldset className="flex flex-col gap-2" disabled={!timeEditable}>
        <legend className="sr-only">When</legend>
        <div className="flex flex-wrap items-center gap-1.5 text-xs text-ink3">
          <DateField
            id={dateId}
            ariaLabel={dateName}
            value={timed ? timed.start_date : t.kind === "all_day" ? t.first_day : ""}
            clearable={false}
            disabled={!timeEditable}
            // A refusal it holds marks it wrong and points it at the line that names it.
            refused={badDate !== null}
            describedBy={badDate !== null ? refusedLineId : undefined}
            className="px-2 py-1 text-xs"
            wrapperClassName="w-28"
            onReject={setBadDate}
            onAccept={() => setBadDate(null)}
            onCommit={(date) => {
              // The whole event moves, keeping its length.
              const was = draftRef.current.time;
              changeTime(
                was.kind === "timed" ? moveStart(was, date, was.start_time) : moveAllDay(was, date),
              );
            }}
          />
          {timed ? (
            <>
              <TimeField
                ariaLabel="Start time"
                compact
                value={timed.start_time}
                disabled={!timeEditable}
                onChange={(hm) => changeTime(moveStart(timed, timed.start_date, hm))}
              />
              {timed.end_date === timed.start_date ? (
                <>
                  <span aria-hidden>–</span>
                  <TimeField
                    ariaLabel="End time"
                    compact
                    value={timed.end_time}
                    choices={endChoices}
                    disabled={!timeEditable}
                    onChange={(end_time) => changeTime({ ...timed, end_time })}
                  />
                </>
              ) : (
                // Another day for the end is the full editor's to change.
                <span>
                  ends {formatDateOnly(timed.end_date)} · {timed.end_time}
                </span>
              )}
            </>
          ) : (
            t.kind === "all_day" &&
            t.last_day !== t.first_day && <span>to {formatDateOnly(t.last_day)}</span>
          )}
        </div>
        <label className="flex items-center gap-2 text-xs text-ink3">
          <Toggle
            ariaLabel="All day"
            checked={t.kind === "all_day"}
            disabled={!timeEditable}
            onChange={switchKind}
          />
          All day
        </label>
        <RepeatRow
          compact
          value={draft.recurrence}
          view={repeat.state}
          time={t}
          deviceZone={deviceZone}
          disabled={!timeEditable}
          onChange={(recurrence) => setDraft({ ...draftRef.current, recurrence })}
          onCustom={() => more(true)}
        />
        {/* A date the field couldn't read, said as it happens and naming the field it points at. */}
        {badDate !== null && (
          <p id={refusedLineId} role="alert" className="text-xs text-st-due">
            {badDateText(dateName, badDate)}
          </p>
        )}
      </fieldset>

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
      {notes.map((n) => (
        <p key={n} className="font-mono text-[0.6875rem] leading-relaxed text-ink4">
          {n}
        </p>
      ))}

      <div className="flex flex-wrap items-center gap-2">
        <CalendarPicker
          compact
          targets={targets}
          value={calendarId}
          onChange={pickCalendar}
          privateDraft={false}
          hidden={hidden}
          colorOf={colorOf}
          shapeOf={shapeOf}
          disabled={locked}
        />
        <div className="ml-auto flex items-center gap-2">
          <Button variant="tertiary" size="sm" onClick={() => more()} disabled={saving}>
            More options
          </Button>
          <Button
            ref={saveButton}
            variant="primary"
            size="sm"
            onClick={() => void save()}
            // A held Enter here, once an answer put focus on it, clicks it again on every repeat.
            onKeyDown={(e) => {
              if (e.key === "Enter" && e.repeat) e.preventDefault();
            }}
            disabled={saving || timeOff || repeatHolds}
          >
            {saving ? (held ? "Checking…" : "Saving…") : held ? "Check again" : "Save"}
          </Button>
        </div>
      </div>
    </div>
  );
}
