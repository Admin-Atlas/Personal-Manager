// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The event fields both editors share (#884): changing an event (`EventEditor`) and creating one
// (`NewEventEditor`) show the same title, When, zone and choice controls, so they live here once.
//
// Time fields never use `<input type="time">` (WebKitGTK has no widget): TimeField and DateField.

import type { EventVisibility, ShowAs, TimeDraft } from "../../../lib/types";
import {
  dateFieldId,
  type BadDates,
  type EditorFields,
  type EndChoice,
} from "../../../lib/calendarEdit/eventDraft";
import { badDateText } from "../../../lib/calendarEdit/editReasons";
import { Button, Input, SegmentedControl, Select, Textarea, Toggle, useFieldA11y } from "../../ui";
import { DateField } from "../../DateField";
import { TimeField } from "../../TimeField";
import { ZoneSearchList } from "../ZoneSearchList";

export type Timed = Extract<TimeDraft, { kind: "timed" }>;

type Half = "start" | "end";

/** Each half's date field under When, by the event's kind: its name, on screen and in a refusal. */
const DATE_FIELD = {
  timed: { start: "Start date", end: "End date" },
  all_day: { start: "First day", end: "Last day" },
} as const;

const SHOW_AS: { value: ShowAs; label: string }[] = [
  { value: "busy", label: "Busy" },
  { value: "free", label: "Free" },
];

const VISIBILITY: { value: EventVisibility; label: string }[] = [
  { value: "default", label: "Calendar default" },
  { value: "public", label: "Public" },
  { value: "private", label: "Private" },
];

export const NO_ZONE =
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

/** Busy or free. */
export function ShowAsControl({
  value,
  disabled,
  onChange,
}: {
  value: ShowAs;
  disabled: boolean;
  onChange: (v: ShowAs) => void;
}) {
  return (
    <SegmentedControl
      ariaLabel="Shows as"
      options={SHOW_AS}
      value={value}
      disabled={disabled}
      onChange={onChange}
    />
  );
}

/** Who sees the event's details. `confidential` offers Google's older choice too (only when the
 *  event has it); `privateOff` greys out Private, for a calendar that can't hold a private event. */
export function VisibilitySelect({
  value,
  disabled,
  onChange,
  confidential = false,
  privateOff = false,
}: {
  value: EventVisibility;
  disabled: boolean;
  onChange: (v: EventVisibility) => void;
  confidential?: boolean;
  privateOff?: boolean;
}) {
  return (
    <label className="flex items-center gap-2 text-sm text-ink2">
      Visibility
      <Select
        compact
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value as EventVisibility)}
      >
        {VISIBILITY.map((v) => (
          <option key={v.value} value={v.value} disabled={v.value === "private" && privateOff}>
            {v.label}
          </option>
        ))}
        {confidential && <option value="confidential">Confidential</option>}
      </Select>
    </label>
  );
}

/** A labelled text field (one line, or several). */
export function TextRow({
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

/** The time-zone search under When, with a way to keep the zone as it is. */
export function ZonePanel({
  onPick,
  onKeep,
}: {
  onPick: (zone: string) => void;
  onKeep: () => void;
}) {
  return (
    <div
      className="rounded-[var(--radius-sm)] border border-border2 p-2"
      onKeyDown={(e) => {
        // Escape closes this panel, not the whole editor.
        if (e.key === "Escape") {
          e.stopPropagation();
          onKeep();
        }
      }}
    >
      <ZoneSearchList autoFocus onPick={onPick} />
      <Button variant="tertiary" size="xs" onClick={onKeep}>
        Keep the zone
      </Button>
    </div>
  );
}

/** When: all day or timed, the dates and times, and the zone(s). */
export function WhenFields({
  id,
  draft,
  badDates,
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
  /** What its date fields' ids start with (`dateFieldId`), so a save can find one. */
  id: string;
  draft: EditorFields;
  /** Text each half's date field couldn't read, said under the fields. */
  badDates: BadDates;
  /** Google's timed start and end, kept on offer in the time lists. */
  held: Timed | null;
  /** The End time list: times after the start, with the event's length. */
  endChoices: readonly EndChoice[];
  /** The first day the End date can be. */
  endMin: string | undefined;
  disabled: boolean;
  onChange: (t: TimeDraft) => void;
  /** A date field gave up on typed text it couldn't read, and showed its date again: `half` is the
   *  part of the event it sets (Start date or First day, End date or Last day). */
  onRejectDate: (half: Half, text: string) => void;
  /** That half's date field was given a date, typed or picked. */
  onAcceptDate: (half: Half) => void;
  moveStart: (t: Timed, date: string, time: string) => Timed;
  moveEndDate: (t: Timed, date: string) => Timed;
  onSwitchKind: () => void;
  canSwitchKind: boolean;
  onPickZone: (which: "both" | "start" | "end") => void;
}) {
  const t = draft.time;
  const names = DATE_FIELD[t.kind];
  const lineId = (half: Half) => `${dateFieldId(id, half)}-refused`;
  // What every date field here shares, for the half of the event it sets: a refusal it holds marks
  // it wrong and points it at the line that names it.
  const dateProps = (half: Half) => ({
    id: dateFieldId(id, half),
    ariaLabel: names[half],
    clearable: false,
    disabled,
    refused: badDates[half] !== null,
    describedBy: badDates[half] !== null ? lineId(half) : undefined,
    onReject: (text: string) => onRejectDate(half, text),
    onAccept: () => onAcceptDate(half),
  });
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
            {...dateProps("start")}
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
            {...dateProps("end")}
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
              {...dateProps("start")}
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
              {...dateProps("end")}
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
      {/* A date a field couldn't read, said as it happens and naming the field it points at: each
          half its own line, the same text in both included. */}
      {(["start", "end"] as const).map((half) => {
        const text = badDates[half];
        return (
          text !== null && (
            <p key={half} id={lineId(half)} role="alert" className="text-xs text-st-due">
              {badDateText(names[half], text)}
            </p>
          )
        );
      })}
    </fieldset>
  );
}
