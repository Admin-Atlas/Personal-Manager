// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The event editor (#884): change one Google event, a one-off you organise without guests. It opens
// on Google's fresh copy (never the mirror, which clips text), greys out whatever the backend says
// can't change and says why, and saves only the fields the user changed. If the event changed in
// Google meanwhile on a field this save also sends, nothing is overwritten: the editor reloads
// Google's copy, keeps the user's other changes, and names the fields where Google's version now
// shows. A dirty draft asks before it is thrown away (closing, Escape, or Delete).
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
  EventVisibility,
  FieldPermissions,
  Milestone,
  SeenSummary,
  ShowAs,
  TimeDraft,
} from "../../../lib/types";
import {
  canEdit,
  changedFields,
  checkDraft,
  rebaseDraft,
  seedDraft,
  toChanges,
  type EditorFields,
} from "../../../lib/calendarEdit/eventDraft";
import {
  ambiguousText,
  listFields,
  loadText,
  outcomeText,
  problemText,
  reasonText,
  type Tone,
} from "../../../lib/calendarEdit/editReasons";
import { descriptionText } from "../../../lib/calendarEdit/descriptionText";
import { wallInstant, wallTimeOf } from "../../../lib/wallTime";
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
import { noteSaved, openForEdit, saveEdit } from "./useEventWrites";

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
  // The event's length, kept while its start moves (and across a start that can't be read).
  const lengthRef = useRef<number | null>(null);
  const [banner, setBanner] = useState<{ tone: Tone; text: string } | null>(null);
  const [saving, setSaving] = useState(false);
  // An earlier save's answer never came: it may be in Google already.
  const [uncertain, setUncertain] = useState(false);
  const [askDiscard, setAskDiscard] = useState<"close" | "delete" | null>(null);
  const [zonePick, setZonePick] = useState<"both" | "start" | "end" | null>(null);

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

  /** The instant one half of a timed draft names: Google's exact one while the half is untouched
   *  (it may be the second of a repeated hour), else its first occurrence. */
  function instantOf(half: "start" | "end", t: Timed): number | null {
    const was = loaded?.base.time.kind === "timed" ? loaded.base.time : null;
    const same =
      was &&
      (half === "start"
        ? was.start_date === t.start_date &&
          was.start_time === t.start_time &&
          was.start_zone === t.start_zone
        : was.end_date === t.end_date &&
          was.end_time === t.end_time &&
          was.end_zone === t.end_zone);
    const held = half === "start" ? loaded?.event.start_at : loaded?.event.end_at;
    if (same && held) return Date.parse(held);
    const at =
      half === "start"
        ? wallInstant(t.start_date, t.start_time, t.start_zone)
        : wallInstant(t.end_date, t.end_time, t.end_zone);
    return at ? at.getTime() : null;
  }

  /** Move the start, the end following so the event keeps its length. */
  function moveStart(t: Timed, date: string, time: string): Timed {
    const s = instantOf("start", t);
    const e = instantOf("end", t);
    if (s !== null && e !== null) lengthRef.current = e - s;
    const moved = { ...t, start_date: date, start_time: time };
    const start = wallInstant(date, time, t.start_zone);
    if (!start || lengthRef.current === null) return moved;
    const end = wallTimeOf(new Date(start.getTime() + lengthRef.current), t.end_zone);
    return { ...moved, end_date: end.date, end_time: end.time };
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
    const focused = document.activeElement;
    commitFocused();
    const current = draftRef.current;
    if (!loaded || !current || saving) return;
    if (badDateRef.current) {
      setBanner({
        tone: "error",
        text: `“${badDateRef.current}” isn't a date PM can read. Type it as dd-mm-yyyy, or pick it from the calendar.`,
      });
      return;
    }
    const check = checkDraft(loaded.base, current, loaded.event);
    if (check.problems.length > 0) {
      setBanner({ tone: "error", text: check.problems.map(problemText).join(" ") });
      return;
    }
    const changes = toChanges(loaded.base, current);
    if (Object.keys(changes).length === 0) {
      onClose();
      return;
    }
    setZonePick(null);
    setSaving(true);
    setBanner(null);
    let stayOpen = true;
    try {
      const outcome = await saveEdit(loaded.session, changes);
      switch (outcome.outcome) {
        case "saved":
          noteSaved(current.summary, outcome);
          stayOpen = false;
          onClose();
          return;
        case "no_change":
          stayOpen = false;
          onClose();
          return;
        case "conflict":
          await rebaseAfterConflict(outcomeText(outcome).text);
          return;
        case "unconfirmed":
          setUncertain(true);
          setBanner(outcomeText(outcome));
          return;
        default:
          setBanner(outcomeText(outcome));
      }
    } catch (e) {
      setUncertain(true);
      setBanner({ tone: "error", text: message(e) });
    } finally {
      setSaving(false);
      // Staying open: hand focus back to where it was, rather than leaving it on the page.
      if (stayOpen && focused instanceof HTMLElement && focused.isConnected) {
        setTimeout(() => focused.focus(), 0);
      }
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
            <Button variant="primary" onClick={() => void save()} disabled={!loaded || saving}>
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
              <Callout
                tone={
                  banner.tone === "error" ? "danger" : banner.tone === "warn" ? "warning" : "info"
                }
                live
              >
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
              disabled={!timeEditable}
              onChange={(time) => {
                badDateRef.current = null;
                setDraft({ ...draft, time });
              }}
              onRejectDate={(text) => {
                badDateRef.current = text;
              }}
              moveStart={moveStart}
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
                      time: {
                        ...t,
                        start_zone: zonePick === "end" ? t.start_zone : zone,
                        end_zone: zonePick === "start" ? t.end_zone : zone,
                      },
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
  disabled,
  onChange,
  onRejectDate,
  moveStart,
  onSwitchKind,
  canSwitchKind,
  onPickZone,
}: {
  draft: EditorFields;
  /** Google's timed start and end, kept on offer in the time lists. */
  held: Timed | null;
  disabled: boolean;
  onChange: (t: TimeDraft) => void;
  onRejectDate: (text: string) => void;
  moveStart: (t: Timed, date: string, time: string) => Timed;
  onSwitchKind: () => void;
  canSwitchKind: boolean;
  onPickZone: (which: "both" | "start" | "end") => void;
}) {
  const t = draft.time;
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
            ariaLabel="First day"
            value={t.first_day}
            clearable={false}
            disabled={disabled}
            onReject={onRejectDate}
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
            ariaLabel="Last day"
            value={t.last_day}
            clearable={false}
            disabled={disabled}
            onReject={onRejectDate}
            onCommit={(last_day) => onChange({ ...t, last_day })}
          />
        </div>
      ) : (
        <>
          <div className="flex flex-wrap items-center gap-2 text-xs text-ink3">
            <span className="w-10">Starts</span>
            <DateField
              ariaLabel="Start date"
              value={t.start_date}
              clearable={false}
              disabled={disabled}
              onReject={onRejectDate}
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
              ariaLabel="End date"
              value={t.end_date}
              clearable={false}
              disabled={disabled}
              onReject={onRejectDate}
              onCommit={(end_date) => onChange({ ...t, end_date })}
            />
            <TimeField
              ariaLabel="End time"
              compact
              value={t.end_time}
              held={held?.end_time}
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
