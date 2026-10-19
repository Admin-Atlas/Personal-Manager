// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The event editor (#884): change one Google event you organise without guests, a repeating one
// included: for an occurrence, the choices dialog asks which occurrences first. It opens on Google's
// fresh copy (never the mirror, which clips text), greys out whatever the backend says can't change
// and says why, and saves only the fields the user changed. If the event changed in Google
// meanwhile on a field this save also sends, nothing is overwritten: the editor reloads Google's
// copy, keeps the user's other changes, and names the fields where Google's version now shows. A
// dirty draft asks before it is thrown away (closing, Escape, or Delete).
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
// stops the save rather than vanishing.

import { useEffect, useMemo, useRef, useState } from "react";
import { flushSync } from "react-dom";
import type {
  Calendar,
  CalendarEvent,
  EventForEdit,
  EventPatchDraft,
  EventVisibility,
  FieldPermissions,
  Milestone,
  SeenSummary,
  SeriesChoices,
  SeriesInfo,
  ShowAs,
  SplitAction,
  TimeDraft,
} from "../../../lib/types";
import {
  DEFAULT_LENGTH_MS,
  canEdit,
  changedFields,
  checkDraft,
  endAfterStart,
  endDateMin,
  endTimeChoices,
  endsAfterStart,
  firstEndOnItsDate,
  halfInstant,
  rebaseDraft,
  seedDraft,
  toChanges,
  type EditorFields,
  type EndChoice,
} from "../../../lib/calendarEdit/eventDraft";
import {
  ambiguousText,
  CALLOUT_TONE,
  earlierChangeText,
  listFields,
  loadText,
  outcomeText,
  problemText,
  reasonText,
  type Tone,
} from "../../../lib/calendarEdit/editReasons";
import { conflictOf, isRepeating, type Picked } from "../../../lib/calendarEdit/saveChoices";
import { descriptionText } from "../../../lib/calendarEdit/descriptionText";
import { deviceTimeZoneOrNull, useDepth } from "../../../theme";
import {
  Button,
  Callout,
  ConfirmDialog,
  Dialog,
  Input,
  SegmentedControl,
  Select,
  Skeleton,
  Textarea,
  Toggle,
  useFieldA11y,
} from "../../ui";
import { DateField } from "../../DateField";
import { TimeField } from "../../TimeField";
import { ZoneSearchList } from "../ZoneSearchList";
import { SaveChoicesDialog, type ChoicesView } from "./SaveChoicesDialog";
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
  /** The account the save goes through. */
  account: string | null;
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

type Timed = Extract<TimeDraft, { kind: "timed" }>;

const SHOW_AS: { value: ShowAs; label: string }[] = [
  { value: "busy", label: "Busy" },
  { value: "free", label: "Free" },
];

const VISIBILITY: { value: EventVisibility; label: string }[] = [
  { value: "default", label: "Calendar default" },
  { value: "public", label: "Public" },
  { value: "private", label: "Private" },
];

const NO_ZONE =
  "PM couldn't tell which time zone this computer is in, so it can't change times. Set the time zone in your system settings.";

/** `date` (`YYYY-MM-DD`) moved by `days`. */
function addDays(date: string, days: number): string {
  const [y, m, d] = date.split("-").map(Number);
  return new Date(Date.UTC(y, m - 1, d + days)).toISOString().slice(0, 10);
}

/** Whole days from `a` to `b` (`YYYY-MM-DD`). */
function daysBetween(a: string, b: string): number {
  return Math.round((Date.parse(`${b}T00:00:00Z`) - Date.parse(`${a}T00:00:00Z`)) / 86_400_000);
}

/** All day ↔ timed. Back to the kind Google holds restores Google's time exactly; otherwise an
 *  all-day span becomes 09:00 on its first day to 10:00 on its last, in `zone`, and a timed event
 *  keeps its days (an end at midnight closes the day before). */
function switchKind(t: TimeDraft, base: TimeDraft, zone: string): TimeDraft {
  const to = t.kind === "timed" ? "all_day" : "timed";
  if (base.kind === to) return { ...base };
  if (t.kind === "timed") {
    const last =
      t.end_time === "00:00" && t.end_date > t.start_date ? addDays(t.end_date, -1) : t.end_date;
    return {
      kind: "all_day",
      first_day: t.start_date,
      last_day: last < t.start_date ? t.start_date : last,
    };
  }
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

export function EventEditor({ row, calendar, account, milestone, onDelete, onClose }: Props) {
  const { showPower } = useDepth();
  const deviceZone = useMemo(() => deviceTimeZoneOrNull(), []);
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [draft, setDraftState] = useState<EditorFields | null>(null);
  // The latest draft, readable synchronously: Save and Close blur the focused field first, and a
  // date typed but not yet committed commits in that blur, before React re-renders.
  const draftRef = useRef<EditorFields | null>(null);
  // Text a date field couldn't read; a save is refused while it stands.
  const badDateRef = useRef<string | null>(null);
  const [badDate, setBadDateState] = useState<string | null>(null);
  const setBadDate = (text: string | null) => {
    badDateRef.current = text;
    setBadDateState(text);
  };
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
  // Focus returns here whenever the choices close or a save stays open: the button was blurred
  // before the dialog opened, so the dialog's own restore has nowhere to go back to.
  const saveRef = useRef<HTMLButtonElement>(null);
  const refocusSave = () => setTimeout(() => saveRef.current?.focus(), 0);

  const setDraft = (next: EditorFields) => {
    draftRef.current = next;
    setDraftState(next);
  };

  useEffect(() => {
    let alive = true;
    openForEdit(row.id)
      .then((load) => {
        if (!alive) return;
        if (load.outcome !== "ready") {
          setLoadError(loadText(load));
          return;
        }
        const base = seedDraft(load.event);
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
  }, [row.id]);

  /** Commit whatever the focused field holds, so the draft read next is the one on screen. */
  function commitFocused() {
    const active = document.activeElement;
    if (active instanceof HTMLElement && active.closest('[role="dialog"]')) {
      active.blur();
      flushSync(() => {});
    }
  }

  const isDirty = () =>
    !!loaded && !!draftRef.current && changedFields(loaded.base, draftRef.current).length > 0;

  const requestClose = () => {
    if (saving) return;
    commitFocused();
    if (isDirty() || badDateRef.current) setAskDiscard("close");
    else onClose();
  };

  /** The row a delete from here compares: what the editor showed, not a possibly older mirror row. */
  const deleteRow = (): CalendarEvent =>
    loaded?.seen ? { ...row, ...loaded.seen, location: loaded.seen.location } : row;

  const requestDelete = () => {
    if (!onDelete || saving) return;
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
    const fresh = await openForEdit(row.id);
    if (fresh.outcome !== "ready") {
      setBanner({ tone: "error", text: loadText(fresh) });
      return;
    }
    const r = rebaseDraft(loaded.base, seedDraft(fresh.event), draftRef.current);
    setLoaded({
      session: fresh.session,
      event: fresh.event,
      base: r.base,
      permissions: fresh.permissions,
      seen: fresh.seen,
      series: fresh.series,
    });
    setDraft(r.draft);
    setBanner({
      tone: "warn",
      text: r.overwritten.length
        ? `${head} Google's version of the ${listFields(r.overwritten)} is shown now; your other changes are still here.`
        : `${head} Your changes are still here; save again to apply them.`,
    });
  }

  async function save() {
    commitFocused();
    const current = draftRef.current;
    if (!loaded || !current || saving || asking) return;
    // A new Save: no earlier ask's count or words carry into it (C9b-1 critique 1 L8).
    againRef.current = 0;
    heldNotice.current = null;
    // What stops this save is already said under When, as it happened; a banner repeating it would
    // outlive the fix.
    if (badDateRef.current || checkDraft(loaded.base, current, loaded.event).problems.length > 0) {
      setBanner(null);
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
    // The row is the safer trigger: the series is null when Google's copy of it couldn't be read.
    if (isRepeating(row) || loaded.series !== null) void ask(what, false);
    else await send(what, null);
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
          setBanner(outcomeText({ outcome: "needs_choice" }));
          refocusSave();
          return;
        }
        await send(what, null);
        return;
      case "refused":
        closeChoices();
        if (c.result.outcome === "conflict") await rebaseAfterConflict(outcomeText(c.result).text);
        else setBanner(outcomeText(c.result));
        refocusSave();
        return;
      case "ready": {
        if (c.notice) heldNotice.current = c.notice;
        // Google changed the occurrence or its event since the editor opened: take Google's copy,
        // keep the draft, and the next Save asks again (spec §0.4.5).
        const fields = conflictOf(c);
        if (fields) {
          closeChoices();
          await rebaseAfterConflict(outcomeText({ outcome: "conflict", fields }).text);
          refocusSave();
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

  /** The user picked which occurrences: the dialog closes and the save goes, with the very changes
   *  the choices were asked for. */
  function onChoose(picked: Picked) {
    if (!asking) return;
    const { session, changes, title, from } = asking;
    askSeq.current++;
    setAsking(null);
    void send({ session, changes, title, from }, picked);
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

  /** Send the save, for `picked` when the choices were asked, and act on what came back. */
  async function send(what: Omit<Asking, "view">, picked: Picked | null) {
    setSaving(true);
    let stayOpen = true;
    // The choices opened again from here: focus is theirs.
    let reasked = false;
    try {
      const outcome = await saveEdit(what.session, what.changes, picked, what.title);
      if (outcome.outcome !== "needs_choice") heldNotice.current = null;
      switch (outcome.outcome) {
        case "saved":
          noteSaved(what.title, outcome, picked?.scope ?? null, what.from);
          stayOpen = false;
          onClose();
          return;
        case "no_change":
          stayOpen = false;
          onClose();
          return;
        case "needs_choice":
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
          setBanner({ tone: "warn", text: earlierChangeText(outcome.offers, outcome.from) });
          return;
        case "conflict":
          await rebaseAfterConflict(outcomeText(outcome).text);
          return;
        case "unconfirmed":
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
              disabled={!loaded || saving}
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
              draft={draft}
              held={heldTimed}
              endChoices={endChoices}
              endMin={endMin}
              disabled={!timeEditable}
              onChange={(time) => {
                setBadDate(null);
                setDraft({ ...draft, time });
              }}
              onRejectDate={setBadDate}
              onAcceptDate={() => setBadDate(null)}
              moveStart={moveStart}
              moveEndDate={moveEndDate}
              onSwitchKind={() =>
                zoneFallback &&
                setDraft({ ...draft, time: switchKind(draft.time, loaded.base.time, zoneFallback) })
              }
              canSwitchKind={!!zoneFallback}
              onPickZone={setZonePick}
            />
            {zonePick && timeEditable && draft.time.kind === "timed" && (
              <div
                className="rounded-[var(--radius-sm)] border border-border2 p-2"
                onKeyDown={(e) => {
                  // Escape closes this panel, not the whole editor.
                  if (e.key === "Escape") {
                    e.stopPropagation();
                    setZonePick(null);
                  }
                }}
              >
                <ZoneSearchList
                  autoFocus
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
                />
                <Button variant="tertiary" size="xs" onClick={() => setZonePick(null)}>
                  Keep the zone
                </Button>
              </div>
            )}
            {check?.ambiguous.map((half) => (
              <p key={half} className="text-xs text-ink4">
                {ambiguousText(half)}
              </p>
            ))}
            {/* What would stop the save, shown as it happens rather than only on Save (each sentence
                once: a start and an end in the same skipped hour say the same thing). */}
            {[
              ...new Set([
                ...(badDate
                  ? [
                      `“${badDate}” isn't a date PM can read. Type it as dd-mm-yyyy, or pick it from the calendar.`,
                    ]
                  : []),
                ...(check?.problems.map(problemText) ?? []),
              ]),
            ].map((text) => (
              <p key={text} className="text-xs text-st-due">
                {text}
              </p>
            ))}

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
              <SegmentedControl
                ariaLabel="Shows as"
                options={SHOW_AS}
                value={draft.show_as}
                disabled={!may("show_as")}
                onChange={(show_as) => setDraft({ ...draft, show_as })}
              />
              <label className="flex items-center gap-2 text-sm text-ink2">
                Visibility
                <Select
                  compact
                  value={draft.visibility}
                  disabled={!may("visibility")}
                  onChange={(e) =>
                    setDraft({ ...draft, visibility: e.target.value as EventVisibility })
                  }
                >
                  {VISIBILITY.map((v) => (
                    <option key={v.value} value={v.value}>
                      {v.label}
                    </option>
                  ))}
                  {(loaded.base.visibility === "confidential" ||
                    draft.visibility === "confidential") && (
                    <option value="confidential">Confidential</option>
                  )}
                </Select>
              </label>
            </div>

            {loaded.event.attachments.length > 0 && (
              <div className="text-xs text-ink3">
                <span className="text-ink4">Attachments (kept as they are): </span>
                {loaded.event.attachments.join(", ")}
              </div>
            )}

            {milestone && (
              <p className="text-xs text-ink3">
                Linked to the “{milestone.label}” milestone in {milestone.project_name}: moving this
                event moves the milestone too.
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
            closes only it and Tab stays in it (C9b-1 critiques 1 M3, 2 L9). */}
        {asking && (
          <SaveChoicesDialog
            kind="save"
            title={asking.title}
            view={asking.view}
            settling={settlingSplit}
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

/** A labelled text field (one line, or several). */
function TextRow({
  label,
  value,
  onChange,
  disabled,
  multiline,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  disabled: boolean;
  multiline?: boolean;
}) {
  const a11y = useFieldA11y();
  return (
    <div className="flex flex-col gap-1">
      <label {...a11y.labelProps} className="text-sm text-ink2">
        {label}
      </label>
      {multiline ? (
        <Textarea
          {...a11y.controlProps}
          rows={4}
          value={value}
          disabled={disabled}
          onChange={(e) => onChange(e.target.value)}
        />
      ) : (
        <Input
          {...a11y.controlProps}
          value={value}
          disabled={disabled}
          onChange={(e) => onChange(e.target.value)}
        />
      )}
    </div>
  );
}

/** When: all day or timed, the dates and times, and the zone(s). */
function WhenFields({
  draft,
  held,
  endChoices,
  endMin,
  disabled,
  onChange,
  onRejectDate,
  onAcceptDate,
  moveStart,
  moveEndDate,
  onSwitchKind,
  canSwitchKind,
  onPickZone,
}: {
  draft: EditorFields;
  /** Google's timed start and end, kept on offer in the time lists. */
  held: Timed | null;
  /** The End time list: times after the start, with the event's length. */
  endChoices: readonly EndChoice[];
  /** The first day the End date can be. */
  endMin: string | undefined;
  disabled: boolean;
  onChange: (t: TimeDraft) => void;
  /** A date field gave up on typed text it couldn't read, and showed its date again. */
  onRejectDate: (text: string) => void;
  /** A date field holds a date it read (typed, picked, or left as it was). */
  onAcceptDate: () => void;
  moveStart: (t: Timed, date: string, time: string) => Timed;
  moveEndDate: (t: Timed, date: string) => Timed;
  onSwitchKind: () => void;
  canSwitchKind: boolean;
  onPickZone: (which: "both" | "start" | "end") => void;
}) {
  const t = draft.time;
  // What every date field here shares.
  const dateProps = {
    clearable: false,
    disabled,
    onReject: onRejectDate,
    onAccept: onAcceptDate,
  };
  return (
    <fieldset className="flex flex-col gap-2" disabled={disabled}>
      <legend className="text-sm text-ink2">When</legend>
      <label className="flex items-center gap-2 text-xs text-ink3">
        <Toggle
          ariaLabel="All day"
          checked={t.kind === "all_day"}
          disabled={disabled || !canSwitchKind}
          onChange={onSwitchKind}
        />
        All day
      </label>
      {t.kind === "all_day" ? (
        <div className="flex flex-wrap items-center gap-2 text-xs text-ink3">
          <DateField
            {...dateProps}
            ariaLabel="First day"
            value={t.first_day}
            // Moving the first day moves the whole event; the last day alone changes its length.
            onCommit={(first_day) =>
              onChange({
                ...t,
                first_day,
                last_day: addDays(t.last_day, daysBetween(t.first_day, first_day)),
              })
            }
          />
          <span>to</span>
          <DateField
            {...dateProps}
            ariaLabel="Last day"
            value={t.last_day}
            min={t.first_day}
            onCommit={(last_day) => onChange({ ...t, last_day })}
          />
        </div>
      ) : (
        <>
          <div className="flex flex-wrap items-center gap-2 text-xs text-ink3">
            <span className="w-10">Starts</span>
            <DateField
              {...dateProps}
              ariaLabel="Start date"
              value={t.start_date}
              onCommit={(d) => onChange(moveStart(t, d, t.start_time))}
            />
            <TimeField
              ariaLabel="Start time"
              compact
              value={t.start_time}
              held={held?.start_time}
              disabled={disabled}
              onChange={(hm) => onChange(moveStart(t, t.start_date, hm))}
            />
          </div>
          <div className="flex flex-wrap items-center gap-2 text-xs text-ink3">
            <span className="w-10">Ends</span>
            <DateField
              {...dateProps}
              ariaLabel="End date"
              value={t.end_date}
              min={endMin}
              onCommit={(end_date) => onChange(moveEndDate(t, end_date))}
            />
            <TimeField
              ariaLabel="End time"
              compact
              value={t.end_time}
              choices={endChoices}
              disabled={disabled}
              onChange={(end_time) => onChange({ ...t, end_time })}
            />
          </div>
          <div className="flex flex-wrap items-center gap-2 text-xs text-ink4">
            {t.start_zone === t.end_zone ? (
              <>
                <span>Times in {t.start_zone}</span>
                <Button variant="tertiary" size="xs" onClick={() => onPickZone("both")}>
                  Change time zone
                </Button>
                <Button variant="tertiary" size="xs" onClick={() => onPickZone("end")}>
                  End in another zone
                </Button>
              </>
            ) : (
              <>
                <span>
                  Starts in {t.start_zone}, ends in {t.end_zone}
                </span>
                <Button variant="tertiary" size="xs" onClick={() => onPickZone("start")}>
                  Start zone
                </Button>
                <Button variant="tertiary" size="xs" onClick={() => onPickZone("end")}>
                  End zone
                </Button>
              </>
            )}
          </div>
        </>
      )}
    </fieldset>
  );
}
