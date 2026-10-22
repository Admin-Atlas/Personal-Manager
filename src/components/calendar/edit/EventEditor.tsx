// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The event editor (#884): change one Google event you organise, a repeating one or one with guests
// included: for an occurrence the choices dialog asks which occurrences first, and a change Google
// would email guests about asks whether it should. It opens on Google's fresh copy (never the
// mirror, which clips text), greys out whatever the backend says can't change and says why, and
// saves only the fields the user changed. If the event changed in Google meanwhile on a field this
// save also sends, nothing is overwritten: the editor reloads Google's copy, keeps the user's other
// changes, and names the fields where Google's version now shows. A dirty draft asks before it is
// thrown away (closing, Escape, or Delete).
//
// Guests (C12): the list, Optional, take off, and the three guest permissions, as Google Calendar
// has them, with suggestions from people already in the user's calendars. Send or Don't send has no
// default: a one-off's question opens from the editor's own count before anything is sent, and
// again with the backend's numbers if Google's copy reaches someone else; a repeating event's rows
// ask it inside the choices dialog. A change to only Busy/free emails nobody, so it never asks.
// Like the choices dialog, the question sits inside the editor's dialog, so Escape closes only it.
//
// The choices dialog sits inside the editor's, so it alone hears Escape and holds Tab. Its answer
// binds this session and these very changes, so the save sends the object the choices were asked
// for, and an answer to an earlier ask (Back, then Save again) is never applied. Words the choices
// say once are kept as a notice whenever the dialog closes without a choice.
//
// Time fields never use `<input type="time">` (WebKitGTK has no widget): TimeField and DateField.
// Every time carries its own zone; when this computer's zone can't be read, times can't be changed
// at all rather than guessing UTC (plan rule R6). A typed date only counts once its field commits
// it (on blur), so Save and Close blur the focused field first, and a date the field couldn't read
// stops the save rather than vanishing, until that field is given a date: its line names it, and a
// save it stops puts focus there. The fields themselves live in `EventFields`, shared with creating
// an event (`NewEventEditor`).
//
// How it repeats can change here too (C11): the Repeat row starts as the repeating event's rule (or
// its words, for one the menu can't make), and Custom… opens inside this dialog. A one-off given a
// rule saves alone, once the backend has said where that rule really starts, or why it can't be
// saved, for exactly the rule and start on screen. For an occurrence, the rule is the whole
// repeating event's: the row only names it, the choices then say what each would do (never "This
// event" alone), and a rule left as it was never stops a save. Moving the date carries along only a
// rule picked here, never the repeating event's own, whose move is the choices' question.

import { useEffect, useId, useMemo, useRef, useState } from "react";
import { flushSync } from "react-dom";
import type {
  Calendar,
  CalendarEvent,
  EditLoad,
  EventForEdit,
  EventPatchDraft,
  FieldPermissions,
  GuestNotice,
  Milestone,
  Notify,
  SeenSummary,
  SeriesChoices,
  SeriesInfo,
  SplitAction,
  TimeDraft,
} from "../../../lib/types";
import {
  DEFAULT_LENGTH_MS,
  NO_BAD_DATES,
  allDayOf,
  canEdit,
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
  rebaseDraft,
  refusedDateId,
  repeatSeed,
  seedDraft,
  toChanges,
  type BadDates,
  type EditorFields,
} from "../../../lib/calendarEdit/eventDraft";
import {
  ONCE,
  repeatingStartOf,
  repeatStartOf,
  sameRepeat,
  startDateOf,
} from "../../../lib/calendarEdit/recurrenceDraft";
import {
  ambiguousText,
  CALLOUT_TONE,
  earlierChangeText,
  listFields,
  loadText,
  outcomeText,
  problemText,
  reasonText,
  rebasedText,
  type Tone,
} from "../../../lib/calendarEdit/editReasons";
import { conflictOf, isRepeating, type Picked } from "../../../lib/calendarEdit/saveChoices";
import { descriptionText } from "../../../lib/calendarEdit/descriptionText";
import type { GuestSuggestion } from "../../../lib/calendarEdit/guests";
import { deviceTimeZoneOrNull, useDepth } from "../../../theme";
import { Button, Callout, ConfirmDialog, Dialog, Skeleton } from "../../ui";
import { CustomRecurrenceDialog } from "./CustomRecurrenceDialog";
import { GuestField } from "./GuestField";
import { NotifyPrompt } from "./NotifyPrompt";
import {
  NO_ZONE,
  RepeatRow,
  ShowAsControl,
  TextRow,
  VisibilitySelect,
  WhenFields,
  ZonePanel,
  type Timed,
} from "./EventFields";
import { SaveChoicesDialog, type ChoicesView } from "./SaveChoicesDialog";
import { useRepeatView, type RepeatState } from "./useRepeat";
import {
  choicesForEdit,
  noteSaved,
  noteSeriesNotice,
  openForEdit,
  saveEdit,
  settleSplit,
} from "./useEventWrites";

interface Props {
  /** The row that was clicked: its title while Google's copy loads, and its id. */
  row: CalendarEvent;
  calendar: Calendar | null;
  /** The account the save goes through: who Google sends guests' emails from, and never suggested
   *  as a guest. */
  account: string | null;
  /** People already in the user's calendars, for the guest box (worked out by the calendar). */
  suggestions?: readonly GuestSuggestion[];
  /** A milestone linked to this event, which moves with it. */
  milestone: Milestone | null;
  /** Delete from the editor: it closes, and the calendar asks as the popover's Delete does. The row
   *  passed carries what the editor showed (Google's fresh copy), which the delete compares. */
  onDelete?: (row: CalendarEvent) => void;
  onClose: () => void;
}

interface Loaded {
  session: string;
  event: EventForEdit;
  base: EditorFields;
  permissions: FieldPermissions;
  seen: SeenSummary | null;
  /** Its repeating event, when it's an occurrence of one Google let PM read. */
  series: SeriesInfo | null;
}

/** The email question, open: the save it is for (the very changes the save sends, and the
 *  occurrences picked, if any), the numbers, and `again` when the backend asked after an answer. */
interface Prompt {
  what: Omit<Asking, "view">;
  picked: Picked | null;
  notice: GuestNotice;
  again: boolean;
}

/** The choices dialog, open: what it was asked for, and what it shows. */
interface Asking {
  /** The session and the very changes the save sends: the token binds both. */
  session: string;
  changes: EventPatchDraft;
  /** The draft's title, for the dialog and the notices. */
  title: string;
  /** The draft's first day (YYYY-MM-DD), where a "this and following" save starts. */
  from: string;
  view: ChoicesView;
}

/** All day ↔ timed. Back to the kind Google holds restores Google's time exactly; otherwise an
 *  all-day span becomes 09:00 on its first day to 10:00 on its last, in `zone`, and a timed event
 *  keeps its days (an end at midnight closes the day before: `allDayOf`). */
function switchKind(t: TimeDraft, base: TimeDraft, zone: string): TimeDraft {
  const to = t.kind === "timed" ? "all_day" : "timed";
  if (base.kind === to) return { ...base };
  if (t.kind === "timed") return allDayOf(t);
  return {
    kind: "timed",
    start_date: t.first_day,
    start_time: "09:00",
    start_zone: zone,
    end_date: t.last_day,
    end_time: "10:00",
    end_zone: zone,
  };
}

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

function unreachable(value: never): never {
  throw new Error(`unhandled save answer: ${JSON.stringify(value)}`);
}

/** The first day a draft's time covers. */
const firstDay = (t: TimeDraft) => (t.kind === "timed" ? t.start_date : t.first_day);

/** Said under the Repeat row of an occurrence whose repeating event PM couldn't read. */
export const UNREADABLE_REPEAT = "PM couldn't read how this repeats, so it can't change it here.";

/** No suggestions: a stable empty list. */
const NO_SUGGESTIONS: readonly GuestSuggestion[] = [];

export function EventEditor({
  row,
  calendar,
  account,
  suggestions = NO_SUGGESTIONS,
  milestone,
  onDelete,
  onClose,
}: Props) {
  const { showPower } = useDepth();
  const deviceZone = useMemo(() => deviceTimeZoneOrNull(), []);
  // The row is the safer sign of a repeating event: the series is null when Google's copy of it
  // couldn't be read.
  const rowRepeats = isRepeating(row);
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [draft, setDraftState] = useState<EditorFields | null>(null);
  // The latest draft, readable synchronously: Save and Close blur the focused field first, and a
  // date typed but not yet committed commits in that blur, before React re-renders.
  const draftRef = useRef<EditorFields | null>(null);
  // Text a date field couldn't read, by the half it sets: no save until that field is given a date.
  const badDatesRef = useRef<BadDates>(NO_BAD_DATES);
  const [badDates, setBadDatesState] = useState<BadDates>(NO_BAD_DATES);
  const setBadDates = (next: BadDates) => {
    badDatesRef.current = next;
    setBadDatesState(next);
  };
  // The When fields' own, so a save a refusal stops can find the date field it names.
  const whenId = useId();
  // The event's length, kept while its start moves (and across a start that can't be read).
  const lengthRef = useRef<number | null>(null);
  const [banner, setBanner] = useState<{ tone: Tone; text: string } | null>(null);
  const [saving, setSaving] = useState(false);
  // An earlier save's answer never came: it may be in Google already.
  const [uncertain, setUncertain] = useState(false);
  const [askDiscard, setAskDiscard] = useState<"close" | "delete" | null>(null);
  const [zonePick, setZonePick] = useState<"both" | "start" | "end" | null>(null);
  // The choices dialog for a repeating event (null = closed).
  const [asking, setAsking] = useState<Asking | null>(null);
  // Each ask's number: an answer applies only while its ask is the latest. Back, Escape and a
  // choice end an ask too (C9b-1 critique 1 H1).
  const askSeq = useRef(0);
  // Words the choices said once, until the dialog closes without a choice (then a notice) or the
  // save is over; kept across a choice for the re-ask, which no longer has them.
  const heldNotice = useRef<string | null>(null);
  // Times this Save was asked again because the plan changed: once, then it says so.
  const againRef = useRef(0);
  const [settlingSplit, setSettlingSplit] = useState(false);
  // The email question (null = closed), readable at once: no save or close goes from behind it.
  const [prompt, setPromptState] = useState<Prompt | null>(null);
  const promptRef = useRef<Prompt | null>(null);
  const setPrompt = (next: Prompt | null) => {
    promptRef.current = next;
    setPromptState(next);
  };
  // Text left in the guest box that isn't an address: no save until it's cleared or fixed.
  const badGuestRef = useRef<string | null>(null);
  const [badGuest, setBadGuestState] = useState<string | null>(null);
  const setBadGuest = (text: string | null) => {
    badGuestRef.current = text;
    setBadGuestState(text);
  };
  // The Custom dialog, while open: no save goes from behind it.
  const customRef = useRef(false);
  const [customOpen, setCustomOpenState] = useState(false);
  // Focus returns here whenever the choices close or a save stays open: the button was blurred
  // before the dialog opened, so the dialog's own restore has nowhere to go back to.
  const saveRef = useRef<HTMLButtonElement>(null);
  const refocusSave = () => setTimeout(() => saveRef.current?.focus(), 0);

  const setDraft = (next: EditorFields) => {
    draftRef.current = next;
    setDraftState(next);
  };
  const setCustomOpen = (open: boolean) => {
    customRef.current = open;
    setCustomOpenState(open);
  };

  // A repeating event: its row says so, or Google sent its series.
  const repeating = rowRepeats || !!loaded?.series;
  // What the backend is asked of the Repeat row's rule. A one-off's, at its own start: where it
  // really starts, or why it can't be saved, and nothing saves until it has said so. A repeating
  // event's own rule isn't asked (its words came with it); another one is, for its words only, from
  // where the repeating event starts (C11 critique 2 M6).
  const value = draft?.recurrence ?? ONCE;
  const ownRule = repeating && !!loaded && sameRepeat(value, loaded.base.recurrence);
  const askedStart =
    !draft || ownRule
      ? null
      : !repeating
        ? repeatStartOf(draft.time)
        : value.kind === "rule" && loaded?.series
          ? repeatingStartOf(value.rule, loaded.series.starts, draft.time)
          : null;
  const repeat = useRepeatView(value, askedStart);

  useEffect(() => {
    let alive = true;
    openForEdit(row.id)
      .then((load) => {
        if (!alive) return;
        if (load.outcome !== "ready") {
          setLoadError(loadText(load));
          return;
        }
        const base = seedDraft(load.event, repeatSeed(load.series, rowRepeats || !!load.series));
        setLoaded({
          session: load.session,
          event: load.event,
          base,
          permissions: load.permissions,
          seen: load.seen,
          series: load.series,
        });
        draftRef.current = base;
        setDraftState(base);
      })
      .catch((e) => {
        if (alive) setLoadError(message(e));
      });
    return () => {
      alive = false;
    };
  }, [row.id, rowRepeats]);

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
  }

  const isDirty = () =>
    !!loaded && !!draftRef.current && changedFields(loaded.base, draftRef.current).length > 0;

  const requestClose = () => {
    if (saving || promptRef.current) return;
    commitFocused();
    if (isDirty() || hasBadDate(badDatesRef.current)) setAskDiscard("close");
    else onClose();
  };

  /** The row a delete from here compares: what the editor showed, not a possibly older mirror row,
   *  with the guest list Google's copy has, so the delete asks about those (C12 critique C1-H2). */
  const deleteRow = (): CalendarEvent => {
    if (!loaded) return row;
    const attendees = loaded.event.guests.map((g) => ({
      name: g.name,
      email: g.email,
      response: g.response,
      optional: g.optional,
      organizer: g.organizer,
      self: g.self,
    }));
    const seen = loaded.seen ? { ...loaded.seen, location: loaded.seen.location } : {};
    return { ...row, ...seen, attendees };
  };

  const requestDelete = () => {
    if (!onDelete || saving || promptRef.current) return;
    commitFocused();
    if (isDirty()) setAskDiscard("delete");
    else onDelete(deleteRow());
  };

  /** The event's length now, kept for the next start move, or the last one it had: a length under a
   *  minute (an end picked before the start, a 0-minute event Google holds, seconds an app wrote) is
   *  never carried, so a moved start always gets an end after it. */
  function lengthOf(t: Timed): number {
    if (loaded) {
      const s = halfInstant(loaded.base, t, "start", loaded.event);
      const e = halfInstant(loaded.base, t, "end", loaded.event);
      if (s !== null && e !== null && e - s >= 60_000) lengthRef.current = e - s;
    }
    return lengthRef.current ?? DEFAULT_LENGTH_MS;
  }

  /** Move the start, the end following so the event keeps its length. */
  function moveStart(t: Timed, date: string, time: string): Timed {
    const length = lengthOf(t);
    const moved = { ...t, start_date: date, start_time: time };
    return loaded ? endAfterStart(loaded.base, moved, length, loaded.event) : moved;
  }

  /** A new end date: the end time stays if it still ends the event after its start, else becomes the
   *  first time on that date that does (or the length's worth after the start, when that whole date
   *  is over before the start begins). */
  function moveEndDate(t: Timed, end_date: string): Timed {
    const moved = { ...t, end_date };
    if (!loaded || endsAfterStart(loaded.base, moved, loaded.event)) return moved;
    return (
      firstEndOnItsDate(loaded.base, moved, loaded.event) ??
      endAfterStart(loaded.base, moved, lengthOf(t), loaded.event)
    );
  }

  /** New zones keep the times on screen as they are; if that puts the end at or before the start,
   *  the end follows the start by the event's length instead. */
  function changeZones(t: Timed, start_zone: string, end_zone: string): Timed {
    const length = lengthOf(t);
    const moved = { ...t, start_zone, end_zone };
    if (!loaded || endsAfterStart(loaded.base, moved, loaded.event)) return moved;
    return endAfterStart(loaded.base, moved, length, loaded.event);
  }

  /** Google changed the event on a field this save sends: take its copy, keep the rest. */
  async function rebaseAfterConflict(head: string) {
    if (!loaded || !draftRef.current) return;
    let fresh: EditLoad;
    try {
      fresh = await openForEdit(row.id);
    } catch (e) {
      // A conflict means nothing was written, so this save isn't in doubt.
      setBanner({ tone: "error", text: message(e) });
      return;
    }
    if (fresh.outcome !== "ready") {
      setBanner({ tone: "error", text: loadText(fresh) });
      return;
    }
    const seed = repeatSeed(fresh.series, rowRepeats || !!fresh.series);
    const r = rebaseDraft(loaded.base, seedDraft(fresh.event, seed), draftRef.current);
    setLoaded({
      session: fresh.session,
      event: fresh.event,
      base: r.base,
      permissions: fresh.permissions,
      seen: fresh.seen,
      series: fresh.series,
    });
    setDraft(r.draft);
    setBanner({ tone: "warn", text: rebasedText(head, r.overwritten, r.lost_guests) });
  }

  /** A time change by the user. A rule picked in this editor whose day came from the start date
   *  follows it; the repeating event's own rule stays as it is, since its move is the choices'
   *  question (`RepeatDayChanges`). */
  function changeTime(next: EditorFields) {
    const was = draftRef.current;
    if (!loaded || !was || sameRepeat(next.recurrence, loaded.base.recurrence)) setDraft(next);
    else setDraft(followStart(was, next));
  }

  /** Whether a one-off's repeat may be saved: no repeat, or a rule the backend called ready at
   *  exactly the start of `fields` (a date the blur before a save just committed is a new start,
   *  not yet asked about). A repeating event's never stops a save: each choice says what it does. */
  const repeatReady = (fields: EditorFields) =>
    repeating || repeat.saveable(fields.recurrence, repeatStartOf(fields.time));

  async function save() {
    // Refused before anything is blurred, so focus stays put (in the Custom dialog, or the email
    // question, too).
    if (!loaded || saving || asking || customRef.current || promptRef.current) return;
    const focused = document.activeElement;
    commitFocused();
    const current = draftRef.current;
    if (!current) return;
    // A new Save: no earlier ask's count or words carry into it (C9b-1 critique 1 L8).
    againRef.current = 0;
    heldNotice.current = null;
    // What stops this save is already said under When, as it happened; a banner repeating it would
    // outlive the fix. A date a field couldn't read puts focus on that field, which the line names
    // and only a date typed or picked there clears (a blur gives it nothing, so the refusal stands);
    // anything else puts focus back where it was.
    if (hasBadDate(badDatesRef.current)) {
      setBanner(null);
      refocus(document.getElementById(refusedDateId(whenId, badDatesRef.current)));
      return;
    }
    // A repeat the backend refused, or hasn't answered for yet, is said under the Repeat row; text in
    // the guest box that isn't an address, under the box and in the problem lines.
    if (
      checkDraft(loaded.base, current, loaded.event).problems.length > 0 ||
      !repeatReady(current) ||
      badGuestRef.current !== null
    ) {
      setBanner(null);
      refocus(focused);
      return;
    }
    const changes = toChanges(loaded.base, current);
    if (Object.keys(changes).length === 0) {
      onClose();
      return;
    }
    setZonePick(null);
    setBanner(null);
    const what = {
      session: loaded.session,
      changes,
      title: current.summary,
      from: firstDay(current.time),
    };
    if (repeating) {
      // Each row says who it would email, and asks there.
      void ask(what, false);
      return;
    }
    // Who this would email, counted as the backend counts: asked before anything is sent, as
    // Google's own question is. Nobody, and it goes (the backend has the last word either way).
    const notice = guestNotice(loaded.base, current, loaded.event.guests_omitted, account);
    if (notice) setPrompt({ what, picked: null, notice, again: false });
    else await send(what, null, null, null);
  }

  /** The email question answered: it closes and the save goes with the answer, for its numbers. */
  function onNotify(n: Notify) {
    const p = promptRef.current;
    if (!p) return;
    setPrompt(null);
    void send(p.what, p.picked, n, p.notice);
  }

  /** Back (or Escape) from the email question: nothing was sent, and the draft stays. */
  function onNotifyBack() {
    setPrompt(null);
    refocusSave();
  }

  /** The choices found Google changed the event: rebase, with Save held until it's done (a Save
   *  meanwhile would ask with the session and draft the rebase is replacing), then focus on it. */
  async function rebaseFromChoices(head: string) {
    setSaving(true);
    try {
      await rebaseAfterConflict(head);
    } finally {
      setSaving(false);
      refocusSave();
    }
  }

  /** Close the choices without a choice (Back, Escape, a rebase, a settle): any answer still on its
   *  way is dropped, and words said once become a notice so they aren't lost (critique 2 M1). */
  function closeChoices() {
    askSeq.current++;
    if (heldNotice.current) noteSeriesNotice(heldNotice.current);
    heldNotice.current = null;
    setAsking(null);
  }

  /** Open the choices for `what` and show Google's answer. `again`: a write answered that the plan
   *  changed since the choice, so this is the re-ask. */
  async function ask(what: Omit<Asking, "view">, again: boolean) {
    const seq = ++askSeq.current;
    setAsking({ ...what, view: { state: "loading" } });
    let c: SeriesChoices;
    try {
      c = await choicesForEdit(what.session, what.changes);
    } catch (e) {
      if (seq !== askSeq.current) return;
      closeChoices();
      setBanner({ tone: "error", text: message(e) });
      refocusSave();
      return;
    }
    if (seq !== askSeq.current) {
      // Closed, or asked again, meanwhile: the answer is dropped, its words aren't.
      if (c.outcome === "ready" && c.notice) noteSeriesNotice(c.notice);
      return;
    }
    switch (c.outcome) {
      case "not_repeating":
        // Google's copy doesn't repeat (any more): one event. Counted, so a backend that says
        // "choose" to the plain save and "nothing to choose" here can't loop.
        closeChoices();
        if (++againRef.current > 1) {
          setBanner(outcomeText({ outcome: "needs_choice", scope: true }));
          refocusSave();
          return;
        }
        await send(what, null, null, null);
        return;
      case "refused":
        closeChoices();
        if (c.result.outcome === "conflict") {
          await rebaseFromChoices(outcomeText(c.result).text);
          return;
        }
        setBanner(outcomeText(c.result));
        refocusSave();
        return;
      case "ready": {
        if (c.notice) heldNotice.current = c.notice;
        // Google changed the occurrence or its event since the editor opened: take Google's copy,
        // keep the draft, and the next Save asks again (spec §0.4.5).
        const fields = conflictOf(c);
        if (fields) {
          closeChoices();
          await rebaseFromChoices(outcomeText({ outcome: "conflict", fields }).text);
          return;
        }
        setAsking({
          ...what,
          view: { state: "ready", choices: c, again, notice: heldNotice.current },
        });
        return;
      }
      default:
        return unreachable(c);
    }
  }

  /** The user picked which occurrences (and Send or Don't send, when that row would email guests):
   *  the dialog closes and the save goes, with the very changes the choices were asked for. The
   *  token binds who it emails, so no numbers go with the answer. */
  function onChoose(picked: Picked, notify: Notify | null) {
    if (!asking) return;
    const { session, changes, title, from } = asking;
    askSeq.current++;
    setAsking(null);
    void send({ session, changes, title, from }, picked, notify, null);
  }

  /** Settle the earlier change the dialog's pending row names. Its notice says how it went; the
   *  choices were planned before it, so they close, and the next Save asks again. */
  async function onSettle(token: string, action: SplitAction) {
    if (!asking) return;
    setSettlingSplit(true);
    await settleSplit(token, action, asking.title);
    setSettlingSplit(false);
    closeChoices();
    refocusSave();
  }

  /** Send the save, for `picked` when the choices were asked, with Send or Don't send (`notify`)
   *  for the numbers `asked` when the save would email guests, and act on what came back. */
  async function send(
    what: Omit<Asking, "view">,
    picked: Picked | null,
    notify: Notify | null,
    asked: GuestNotice | null,
  ) {
    setSaving(true);
    let stayOpen = true;
    // The choices or the email question opened again from here: focus is theirs.
    let reasked = false;
    try {
      const outcome = await saveEdit(what.session, what.changes, picked, what.title, notify, asked);
      if (outcome.outcome !== "needs_choice") heldNotice.current = null;
      switch (outcome.outcome) {
        case "saved":
          // "Does not repeat" leaves one event, never a repeating one.
          noteSaved(
            what.title,
            outcome,
            picked?.scope ?? null,
            what.from,
            what.changes.recurrence?.kind === "once",
            picked?.asAll ?? false,
          );
          stayOpen = false;
          onClose();
          return;
        case "no_change":
          stayOpen = false;
          onClose();
          return;
        case "needs_choice":
          // Whether Google emails the guests (C12): asked with the backend's numbers, never by
          // choosing occurrences, whose dialog would say "doesn't repeat" and send it again
          // unanswered, round and round. Once per Save: a second ask is said instead.
          if (outcome.guests && !outcome.scope) {
            if (++againRef.current > 1) {
              setBanner(outcomeText(outcome));
              return;
            }
            reasked = true;
            // "Again" only when an answer was given: the numbers changed since the user chose.
            setPrompt({ what, picked, notice: outcome.guests, again: notify !== null });
            return;
          }
          // No choice sent, and the event repeats after all (R9): ask, saying nothing.
          if (picked === null) {
            reasked = true;
            void ask(what, false);
            return;
          }
          // The plan changed between the choice and the save: ask again once. If it keeps
          // changing under the user, say so and stop asking.
          if (++againRef.current <= 1) {
            reasked = true;
            void ask(what, true);
            return;
          }
          setBanner(outcomeText(outcome));
          return;
        case "partial_split":
          // New: Google holds part of this save, and its notice (with the buttons) outlives the
          // editor, whose session has ended. Not new: an earlier change blocked this save; nothing
          // was sent, so the draft stays, and that change's notice has the buttons.
          if (outcome.newPartial) {
            stayOpen = false;
            onClose();
            return;
          }
          setBanner({
            tone: "warn",
            text: earlierChangeText(outcome.offers, outcome.from, outcome.emails),
          });
          return;
        case "conflict":
          await rebaseAfterConflict(outcomeText(outcome).text);
          return;
        // `ambiguous`: only creating an event answers it, never a save; read as a save it would be
        // unconfirmed.
        case "unconfirmed":
        case "ambiguous":
          setUncertain(true);
          setBanner(outcomeText(outcome));
          return;
        // After a save of several occurrences that Google put back, the session has ended; those
        // words already say to open the event again.
        case "ignored_by_google":
        case "split_removed":
        case "failed":
        case "busy":
        case "read_only":
        case "gone":
        case "reauth":
          setBanner(outcomeText(outcome));
          return;
        default:
          return unreachable(outcome);
      }
    } catch (e) {
      setUncertain(true);
      setBanner({ tone: "error", text: message(e) });
    } finally {
      setSaving(false);
      // Staying open: focus back on Save, rather than left on the page (C9b-1 critiques 1 M2, 2 M2).
      if (stayOpen && !reasked) refocusSave();
    }
  }

  const title = (draft?.summary ?? row.summary).trim() || "(no title)";
  const perms = loaded?.permissions;
  const may = (f: Parameters<typeof canEdit>[1]) => !!perms && canEdit(perms, f) && !saving;
  const timeOff = deviceZone === null;
  const check = loaded && draft ? checkDraft(loaded.base, draft, loaded.event) : null;
  const reasons = [
    ...(perms?.reasons.map(reasonText) ?? []),
    ...(timeOff && perms?.time ? [NO_ZONE] : []),
  ];
  const heldTimed = loaded?.base.time.kind === "timed" ? loaded.base.time : null;
  const zoneFallback = heldTimed?.start_zone ?? calendar?.time_zone ?? deviceZone;
  const timeEditable = may("time") && !timeOff;
  const dirty = loaded && draft ? changedFields(loaded.base, draft).length > 0 : false;
  const timed = draft?.time.kind === "timed" ? draft.time : null;
  // The End list resolves every quarter hour, so it's worked out once per time change.
  const endChoices = useMemo(
    () =>
      loaded && timed ? endTimeChoices(loaded.base, timed, loaded.event, heldTimed?.end_time) : [],
    [loaded, timed, heldTimed],
  );
  const endMin = useMemo(
    () => (loaded && timed ? endDateMin(loaded.base, timed, loaded.event) : undefined),
    [loaded, timed],
  );
  // The Repeat row's words: a repeating event's own rule in the words it came with, any other rule
  // in the backend's. Only a one-off's repeat holds Save (`repeatReady`).
  const series = loaded?.series ?? null;
  const repeatView: RepeatState =
    ownRule && series
      ? { state: "ready", summary: series.repeats, starts: series.starts, moved: false }
      : repeat.state;
  // Its own rule keeps a row in the list whatever was picked since, so picking it again changes
  // nothing (C11d review 1).
  const standing =
    loaded && series ? { value: loaded.base.recurrence, words: series.repeats } : null;
  const repeatHolds =
    !repeating && (repeat.state.state === "loading" || repeat.state.state === "refused");

  return (
    <>
      <Dialog
        open
        onClose={requestClose}
        chrome="bar"
        title="Edit event"
        subtitle={
          calendar ? `Saving to ${calendar.name}${account ? ` · ${account}` : ""}` : undefined
        }
        closeLabel="Close"
        closeDisabled={saving}
        widthClassName="max-w-lg"
        heightClassName="max-h-[85vh]"
        footer={
          <>
            {perms?.delete && onDelete && (
              <span className="mr-auto">
                <Button variant="danger" onClick={requestDelete} disabled={saving}>
                  Delete
                </Button>
              </span>
            )}
            <Button variant="tertiary" onClick={requestClose} disabled={saving}>
              Cancel
            </Button>
            <Button
              ref={saveRef}
              variant="primary"
              onClick={() => void save()}
              disabled={!loaded || saving || repeatHolds}
            >
              {saving ? "Saving…" : "Save"}
            </Button>
          </>
        }
      >
        {loadError ? (
          <Callout tone="danger" live>
            {loadError}
          </Callout>
        ) : !loaded || !draft ? (
          <div className="space-y-3" aria-busy="true">
            <p className="text-sm text-ink3">Opening “{title}” from Google…</p>
            <Skeleton className="h-8 w-full" />
            <Skeleton className="h-20 w-full" />
          </div>
        ) : (
          <div className="flex flex-col gap-4">
            {banner && (
              <Callout tone={CALLOUT_TONE[banner.tone]} live>
                {banner.text}
              </Callout>
            )}

            <TextRow
              label="Title"
              value={draft.summary}
              disabled={!may("summary")}
              onChange={(summary) => setDraft({ ...draft, summary })}
            />

            <WhenFields
              id={whenId}
              draft={draft}
              badDates={badDates}
              held={heldTimed}
              endChoices={endChoices}
              endMin={endMin}
              disabled={!timeEditable}
              onChange={(time) => changeTime({ ...draft, time })}
              onRejectDate={(half, text) => setBadDates({ ...badDatesRef.current, [half]: text })}
              onAcceptDate={(half) => setBadDates({ ...badDatesRef.current, [half]: null })}
              moveStart={moveStart}
              moveEndDate={moveEndDate}
              // A time change like any other: a rule picked here follows the date it lands on.
              onSwitchKind={() =>
                zoneFallback &&
                changeTime({
                  ...draft,
                  time: switchKind(draft.time, loaded.base.time, zoneFallback),
                })
              }
              canSwitchKind={!!zoneFallback}
              onPickZone={setZonePick}
              repeat={
                <RepeatRow
                  value={draft.recurrence}
                  view={repeatView}
                  time={draft.time}
                  // A repeating event repeats in its own zone, whichever this occurrence shows.
                  zone={repeating ? (series?.zone ?? null) : undefined}
                  deviceZone={deviceZone}
                  skippedTime={!!check?.problems.some((p) => p.kind === "gap")}
                  judge={!repeating}
                  offReason={repeating && !series ? UNREADABLE_REPEAT : undefined}
                  standing={standing}
                  disabled={!timeEditable}
                  onChange={(recurrence) =>
                    draftRef.current && setDraft({ ...draftRef.current, recurrence })
                  }
                  onCustom={() => setCustomOpen(true)}
                />
              }
            />
            {zonePick && timeEditable && draft.time.kind === "timed" && (
              <ZonePanel
                onPick={(zone) => {
                  if (draft.time.kind !== "timed") return;
                  const t = draft.time;
                  setDraft({
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
            {check?.ambiguous.map((half) => (
              <p key={half} className="text-xs text-ink4">
                {ambiguousText(half)}
              </p>
            ))}
            {/* What else would stop the save (a date a field couldn't read is said under When),
                shown as it happens rather than only on Save (each sentence once: a start and an end
                in the same skipped hour say the same thing). Text left in the guest box that isn't
                an address is one too, beside the box's own line under it. */}
            {[
              ...new Set([
                ...(check?.problems.map(problemText) ?? []),
                ...(badGuest !== null ? [problemText({ kind: "bad_guest", email: badGuest })] : []),
              ]),
            ].map((text) => (
              <p key={text} className="text-xs text-st-due">
                {text}
              </p>
            ))}

            <GuestField
              guests={draft.guests}
              base={loaded.base.guests}
              perms={draft.guest_perms}
              suggestions={suggestions}
              me={account}
              organizer={loaded.event.organizer_email}
              disabled={!may("guests")}
              omitted={loaded.event.guests_omitted}
              onChange={(guests) => draftRef.current && setDraft({ ...draftRef.current, guests })}
              onPerms={(guest_perms) =>
                draftRef.current && setDraft({ ...draftRef.current, guest_perms })
              }
              onReject={setBadGuest}
            />

            <TextRow
              label="Location"
              value={draft.location}
              disabled={!may("location")}
              onChange={(location) => setDraft({ ...draft, location })}
            />

            {loaded.event.description_html ? (
              <div className="flex flex-col gap-1">
                <span className="text-sm text-ink2">Description</span>
                <p className="whitespace-pre-wrap rounded-[var(--radius-sm)] border border-border2 px-3 py-2 text-sm text-ink3">
                  {descriptionText(draft.description)}
                </p>
              </div>
            ) : (
              <TextRow
                label="Description"
                multiline
                value={draft.description}
                disabled={!may("description")}
                onChange={(description) => setDraft({ ...draft, description })}
              />
            )}

            <div className="flex flex-wrap items-center gap-4">
              <ShowAsControl
                value={draft.show_as}
                disabled={!may("show_as")}
                onChange={(show_as) => setDraft({ ...draft, show_as })}
              />
              <VisibilitySelect
                value={draft.visibility}
                disabled={!may("visibility")}
                onChange={(visibility) => setDraft({ ...draft, visibility })}
                confidential={
                  loaded.base.visibility === "confidential" || draft.visibility === "confidential"
                }
              />
            </div>

            {loaded.event.attachments.length > 0 && (
              <div className="text-xs text-ink3">
                <span className="text-ink4">Attachments (kept as they are): </span>
                {loaded.event.attachments.join(", ")}
              </div>
            )}

            {milestone && (
              <p className="text-xs text-ink3">
                {/* A repeating event's milestone follows its next occurrence, so moving this one
                    moves it only when it's that one: the one-off's words would overclaim. */}
                {isRepeating(row)
                  ? `Linked to the “${milestone.label}” milestone in ${milestone.project_name}, which takes its date from the next occurrence.`
                  : `Linked to the “${milestone.label}” milestone in ${milestone.project_name}: moving this event moves the milestone too.`}
              </p>
            )}

            {reasons.map((r) => (
              <p key={r} className="text-xs leading-relaxed text-ink4">
                {r}
              </p>
            ))}

            {showPower && (
              <div className="flex flex-col gap-1 border-t border-border pt-2 text-[0.6875rem] text-ink4">
                {row.uid && <div className="break-all">UID: {row.uid}</div>}
                {row.updated && <div>Updated: {row.updated}</div>}
                <div>
                  {dirty
                    ? `Changed: ${listFields(changedFields(loaded.base, draft))}`
                    : "No changes yet"}
                </div>
              </div>
            )}
          </div>
        )}
        {/* Inside the editor's dialog, not beside it: nested, it alone is topmost, so one Escape
            closes only it and Tab stays in it (C9b-1 critiques 1 M3, 2 L9). Its focus goes back to
            the Repeat row when it closes. */}
        {customOpen && draft && (
          <CustomRecurrenceDialog
            rule={draft.recurrence.kind === "rule" ? draft.recurrence.rule : null}
            startDate={startDateOf(draft.time)}
            earliestEnd={repeating && loaded?.series ? loaded.series.starts : undefined}
            onDone={(rule) => {
              if (draftRef.current) {
                setDraft({ ...draftRef.current, recurrence: { kind: "rule", rule } });
              }
              setCustomOpen(false);
            }}
            onClose={() => setCustomOpen(false)}
          />
        )}
        {/* The email question, nested the same way: Escape and Back close only it. */}
        {prompt && (
          <NotifyPrompt
            kind="save"
            notice={prompt.notice}
            from={account}
            again={prompt.again}
            onSend={onNotify}
            onBack={onNotifyBack}
          />
        )}
        {asking && (
          <SaveChoicesDialog
            kind="save"
            title={asking.title}
            view={asking.view}
            settling={settlingSplit}
            from={account}
            onChoose={onChoose}
            onSettle={(token, action) => void onSettle(token, action)}
            onClose={() => {
              closeChoices();
              refocusSave();
            }}
          />
        )}
      </Dialog>

      <ConfirmDialog
        open={askDiscard !== null}
        title={
          askDiscard === "delete" ? "Discard your changes and delete?" : "Discard your changes?"
        }
        confirmLabel={askDiscard === "delete" ? "Discard and delete" : "Discard"}
        cancelLabel="Keep editing"
        danger
        onClose={() => setAskDiscard(null)}
        onConfirm={() => {
          const next = askDiscard;
          setAskDiscard(null);
          if (next === "delete" && onDelete) onDelete(deleteRow());
          else onClose();
        }}
      >
        {uncertain
          ? "PM couldn't confirm whether your last save reached Google. Refresh the calendar to check."
          : "Nothing has been saved to Google yet."}
      </ConfirmDialog>
    </>
  );
}
