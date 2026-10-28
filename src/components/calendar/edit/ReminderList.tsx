// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The editor's Notifications (#884, C13), as Google Calendar's event page has them: one row each,
// how it's sent, how long before, and on an all-day event the time of day; up to five. A timed event
// on its calendar's defaults shows them as ordinary rows, as Google does: left alone they send
// nothing, and the first change makes the list the event's own (C13 critique C2-M7). An all-day
// event's defaults aren't in Google's API, so they're named, with Change to make a list of its own.
// "Use {calendar}'s defaults" puts them back (Google web has no such button: there the default rows
// are put back by hand, which PM can't do for an all-day event).
//
// A row's number counts once it's committed, on blur or Enter (a save blurs the focused field
// first, so Ctrl+S never loses what was typed); a choice from a list commits at once. A row that
// can't be committed (not a whole number, past four weeks, none on an all-day event, or on the list
// already) says why under it and stays as typed, and the editor refuses to save meanwhile
// (`onInvalid`, the date fields' pattern). A notification Google holds that PM wouldn't make (an old
// way of sending it, a time outside Google's range) is one line, kept as it is unless removed.
//
// Focus never falls to the page: Add and Change go to the new row's number, Remove to the row before
// (or Add), and the defaults button to Change or Add. Every control's name says which row it's in.

import { useEffect, useId, useRef, useState } from "react";
import type { Reminder, RemindersDraft } from "../../../lib/types";
import {
  ALL_DAY_UNITS,
  ALREADY_LISTED,
  MAX_REMINDERS,
  METHODS,
  TIMED_UNITS,
  allDayDefaultsText,
  allDayMinutes,
  allDayParts,
  calendarDefaults,
  defaultsButtonLabel,
  defaultsText,
  isEditable,
  listed,
  newRow,
  noDefaultsText,
  parseCount,
  reminderText,
  rowProblem,
  timedMinutes,
  timedParts,
  unitWord,
  type AllDayUnit,
  type TimedUnit,
} from "../../../lib/calendarEdit/reminders";
import { Button, IconButton, Select, Tooltip } from "../../ui";
import { TimeField } from "../../TimeField";

interface Props {
  value: RemindersDraft;
  allDay: boolean;
  /** The calendar it's on: its name, and its (timed) default notifications. */
  calendar: { name: string; default_reminders: Reminder[] } | null;
  disabled: boolean;
  /** Google holds a notification PM can't read: nothing is shown as the event's (the editor's
   *  reason says why), and nothing can change. */
  unreadable?: boolean;
  /** Said under the list: a switch between timed and all-day put the defaults back. */
  note?: string | null;
  onChange: (next: RemindersDraft) => void;
  /** Whether a row holds something it can't commit: the editor refuses a save while it does. */
  onInvalid: (invalid: boolean) => void;
}

const FULL = "Google allows up to 5 notifications.";

/** The number box, as small as the lists beside it (Select's compact size). */
const COUNT =
  "w-14 rounded-[var(--radius-sm)] border border-border2 bg-surface px-2 py-1 text-xs text-ink2 outline-none transition focus:border-accent aria-invalid:border-st-due disabled:cursor-not-allowed disabled:bg-transparent disabled:text-ink3";

/** A row as its fields hold it. `at` is an all-day row's time of day. */
interface Form {
  method: string;
  text: string;
  unit: string;
  at: string;
}

function formOf(r: Reminder, allDay: boolean): Form {
  if (allDay) {
    const p = allDayParts(r.minutes);
    return { method: r.method, text: String(p.n), unit: p.unit, at: p.at };
  }
  const p = timedParts(r.minutes);
  return { method: r.method, text: String(p.n), unit: p.unit, at: "00:00" };
}

function minutesOf(f: Form, allDay: boolean): number | null {
  const n = parseCount(f.text);
  if (n === null) return null;
  return allDay
    ? allDayMinutes({ n, unit: f.unit as AllDayUnit, at: f.at })
    : timedMinutes({ n, unit: f.unit as TimedUnit });
}

export function ReminderList({
  value,
  allDay,
  calendar,
  disabled,
  unreadable = false,
  note = null,
  onChange,
  onInvalid,
}: Props) {
  const name = calendar?.name ?? null;
  const defaults = calendar?.default_reminders ?? [];
  const fullId = useId();
  const box = useRef<HTMLFieldSetElement>(null);
  // What's on screen as rows: the event's own list, or a timed event's defaults.
  const rows: readonly Reminder[] = value.use_default ? (allDay ? [] : defaults) : value.overrides;
  // A key per row, so a row's typing stays with it when one above goes. Kept here, since only this
  // list adds and removes rows; a list that changed from outside (a rebase, the kind switch) starts
  // afresh.
  const keys = useRef<number[]>([]);
  const nextKey = useRef(0);
  if (keys.current.length !== rows.length) keys.current = rows.map(() => nextKey.current++);
  // Rows holding something they can't commit, and whether the editor was last told any do.
  const bad = useRef(new Set<number>());
  const said = useRef(false);
  // Where focus goes once the change it follows has rendered.
  const focusNext = useRef<string | null>(null);
  const full = rows.length >= MAX_REMINDERS;

  useEffect(() => {
    const target = focusNext.current;
    if (target === null) return;
    focusNext.current = null;
    box.current?.querySelector<HTMLElement>(target)?.focus();
  });

  function report(key: number, isBad: boolean) {
    if (isBad) bad.current.add(key);
    else bad.current.delete(key);
    const any = bad.current.size > 0;
    if (any !== said.current) {
      said.current = any;
      onInvalid(any);
    }
  }

  const own = (overrides: Reminder[]) => onChange({ use_default: false, overrides });
  const copies = () => rows.map((r) => ({ ...r }));

  function commit(i: number, r: Reminder) {
    own(rows.map((x, j) => (j === i ? r : { ...x })));
  }

  function remove(i: number) {
    const at = keys.current;
    focusNext.current = i > 0 ? `[data-remove="${at[i - 1]}"]` : "[data-add]";
    keys.current = at.filter((_, j) => j !== i);
    own(copies().filter((_, j) => j !== i));
  }

  function add() {
    if (full) return;
    const key = nextKey.current++;
    keys.current = [...keys.current, key];
    focusNext.current = `[data-count="${key}"]`;
    own([...copies(), newRow(rows, allDay)]);
  }

  function backToDefaults() {
    keys.current = [];
    focusNext.current = allDay ? "[data-change]" : "[data-add]";
    onChange(calendarDefaults());
  }

  return (
    <fieldset ref={box} className="flex min-w-0 flex-col gap-2" disabled={disabled}>
      <legend className="text-sm text-ink2">Notifications</legend>
      {unreadable ? null : value.use_default && allDay ? (
        <div className="flex flex-wrap items-center gap-2 text-xs text-ink3">
          <span>{allDayDefaultsText(name)}</span>
          <Button variant="tertiary" size="xs" data-change="" disabled={disabled} onClick={add}>
            Change
          </Button>
        </div>
      ) : (
        <>
          {value.use_default && (
            <p className="text-xs text-ink3">
              {defaults.length > 0 ? defaultsText(name) : noDefaultsText(name)}
            </p>
          )}
          {!value.use_default && rows.length === 0 && (
            <p className="text-xs text-ink3">No notifications.</p>
          )}
          {rows.length > 0 && (
            <ul aria-label="Notifications" className="flex flex-col gap-1.5">
              {rows.map((r, i) => {
                const key = keys.current[i];
                const others = rows.filter((_, j) => j !== i);
                return isEditable(r, allDay) ? (
                  <ReminderRow
                    key={key}
                    rowKey={key}
                    value={r}
                    allDay={allDay}
                    others={others}
                    disabled={disabled}
                    onCommit={(next) => commit(i, next)}
                    onRemove={() => remove(i)}
                    onBad={(isBad) => report(key, isBad)}
                  />
                ) : (
                  <li key={key} className="flex min-w-0 items-center gap-2 text-xs text-ink2">
                    <span>{reminderText(r, allDay)}</span>
                    <IconButton
                      label={`Remove notification: ${reminderText(r, allDay)}`}
                      variant="subtle"
                      disabled={disabled}
                      data-remove={key}
                      onClick={() => remove(i)}
                    >
                      ×
                    </IconButton>
                  </li>
                );
              })}
            </ul>
          )}
          <div className="flex flex-wrap items-center gap-2">
            {/* Always in a tooltip, so reaching five doesn't remount the button under focus. */}
            <Tooltip label={full ? FULL : "Add notification · up to 5"}>
              <Button
                variant="tertiary"
                size="xs"
                data-add=""
                aria-disabled={full || undefined}
                aria-describedby={full ? fullId : undefined}
                className="aria-disabled:cursor-not-allowed"
                disabled={disabled}
                onClick={add}
              >
                Add notification
              </Button>
            </Tooltip>
            {!value.use_default && (
              <Button variant="tertiary" size="xs" disabled={disabled} onClick={backToDefaults}>
                {defaultsButtonLabel(name)}
              </Button>
            )}
          </div>
          {full && (
            <p id={fullId} className="text-xs text-ink4">
              {FULL}
            </p>
          )}
        </>
      )}
      {note && <p className="text-xs text-ink4">{note}</p>}
      {/* A consequence, not an explanation: never folded away. */}
      <p className="text-xs text-ink4">Google Calendar sends these, not PM.</p>
    </fieldset>
  );
}

interface RowProps {
  rowKey: number;
  value: Reminder;
  allDay: boolean;
  /** The rest of the list: the same notification twice is refused. */
  others: readonly Reminder[];
  disabled: boolean;
  onCommit: (r: Reminder) => void;
  onRemove: () => void;
  onBad: (bad: boolean) => void;
}

/** One notification as fields. What it shows is what it last committed (`value`) until the user
 *  changes it; a change that can't be committed stays on screen, refused, until one can. */
function ReminderRow({
  rowKey,
  value,
  allDay,
  others,
  disabled,
  onCommit,
  onRemove,
  onBad,
}: RowProps) {
  const id = useId();
  const problemId = `${id}-problem`;
  const [edit, setEdit] = useState<(Form & { problem: string | null }) | null>(null);
  const shown = edit ?? formOf(value, allDay);
  const words = reminderText(value, allDay);
  const count = parseCount(shown.text) ?? 2;
  // Gone, it holds nothing the editor should wait for.
  const onBadRef = useRef(onBad);
  useEffect(() => {
    onBadRef.current = onBad;
  });
  useEffect(() => () => onBadRef.current(false), []);

  /** Commit `form`, or keep it on screen saying why it can't be. */
  function attempt(form: Form) {
    const minutes = minutesOf(form, allDay);
    const problem =
      rowProblem(minutes, allDay) ??
      (minutes !== null && listed(others, { method: form.method, minutes })
        ? ALREADY_LISTED
        : null);
    if (minutes === null || problem !== null) {
      setEdit({ ...form, problem });
      onBad(true);
      return;
    }
    setEdit(null);
    onBad(false);
    if (form.method !== value.method || minutes !== value.minutes) {
      onCommit({ method: form.method, minutes });
    }
  }

  const units = allDay ? ALL_DAY_UNITS : TIMED_UNITS;
  return (
    <li className="flex min-w-0 flex-wrap items-center gap-2 text-xs text-ink3">
      <Select
        compact
        aria-label={`How: ${words}`}
        value={shown.method}
        disabled={disabled}
        onChange={(e) => attempt({ ...shown, method: e.target.value })}
      >
        {METHODS.map((m) => (
          <option key={m.value} value={m.value}>
            {m.label}
          </option>
        ))}
      </Select>
      <input
        className={COUNT}
        inputMode="numeric"
        aria-label={`How many: ${words}`}
        aria-invalid={edit?.problem ? true : undefined}
        aria-describedby={edit?.problem ? problemId : undefined}
        data-count={rowKey}
        value={shown.text}
        disabled={disabled}
        onChange={(e) =>
          setEdit({ ...shown, text: e.target.value, problem: edit?.problem ?? null })
        }
        onBlur={() => edit && attempt(edit)}
        onKeyDown={(e) => {
          // An input method's Enter ends composing, nothing more (WebKit sends it as key code 229).
          if (e.key !== "Enter" || e.nativeEvent.isComposing || e.nativeEvent.keyCode === 229) {
            return;
          }
          e.preventDefault();
          if (edit) attempt(edit);
        }}
      />
      <Select
        compact
        aria-label={`Unit: ${words}`}
        value={shown.unit}
        disabled={disabled}
        onChange={(e) => attempt({ ...shown, unit: e.target.value })}
      >
        {units.map((u) => (
          <option key={u} value={u}>
            {unitWord(count, u)}
          </option>
        ))}
      </Select>
      {allDay ? (
        <>
          <span>before at</span>
          <TimeField
            compact
            ariaLabel={`At: ${words}`}
            value={shown.at}
            held={formOf(value, true).at}
            disabled={disabled}
            onChange={(at) => attempt({ ...shown, at })}
          />
        </>
      ) : (
        <span>before</span>
      )}
      <IconButton
        label={`Remove notification: ${words}`}
        variant="subtle"
        disabled={disabled}
        data-remove={rowKey}
        onClick={onRemove}
      >
        ×
      </IconButton>
      {edit?.problem && (
        <p id={problemId} className="w-full text-xs text-st-due">
          {edit.problem}
        </p>
      )}
    </li>
  );
}
