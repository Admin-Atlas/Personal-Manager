// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Google Calendar's "Custom recurrence" dialog, in PM's words (#884, C11): repeat every N days, weeks,
// months or years; on the weekdays chosen, or the month's day or numbered weekday; ending never, on a
// date, or after a number of times. Done hands the rule back and changes nothing else; Cancel, ✕
// and Escape hand back nothing.
//
// It is rendered inside the editor's own dialog, so it alone is topmost: one Escape closes only it,
// and Tab stays in it. Where it differs from Google's: the weekdays start on Monday, as PM's weeks
// do; and the end date has no lower limit, so a date before the start is said rather than silently
// moved to it (C11 critiques C1-M5, C2-M3). What the fields can't make is said in one line, with
// the backend's own words, and Done waits until it's fixed.

import { useId, useState } from "react";
import type { DayCode, RepeatRule } from "../../../lib/types";
import {
  formFromRule,
  monthlyChoices,
  monthlyLabel,
  repeatProblemText,
  ruleFromForm,
  WEEKDAYS,
  yearlyLabel,
  type RepeatForm,
  type RepeatUnit,
} from "../../../lib/calendarEdit/recurrenceDraft";
import { badDateText } from "../../../lib/calendarEdit/editReasons";
import { Button, Dialog, Select, cn } from "../../ui";
import { DateField } from "../../DateField";

interface Props {
  /** The rule to start from (the Repeat row's), or null for Google's defaults. */
  rule: RepeatRule | null;
  /** The event's start date (YYYY-MM-DD): the defaults, the monthly choices and the yearly date
   *  come from it. */
  startDate: string;
  onDone: (rule: RepeatRule) => void;
  onClose: () => void;
}

const UNITS: { value: RepeatUnit; one: string; many: string }[] = [
  { value: "day", one: "day", many: "days" },
  { value: "week", one: "week", many: "weeks" },
  { value: "month", one: "month", many: "months" },
  { value: "year", one: "year", many: "years" },
];

/** A small number field, in the editor's field tokens. */
const NUMBER =
  "w-16 rounded-[var(--radius-sm)] border border-border2 bg-surface px-2 py-1 text-sm text-ink2 outline-none transition focus:border-accent disabled:cursor-not-allowed disabled:bg-transparent disabled:text-ink3";

/** Rendered only while open (the parent mounts it), so its fields start afresh each time. */
export function CustomRecurrenceDialog({ rule, startDate, onDone, onClose }: Props) {
  const [first] = useState(() => formFromRule(rule, startDate));
  const [form, setForm] = useState<RepeatForm>(first);
  // Text the end date field couldn't read: it shows its date again, so Done waits until it's given
  // one, typed or picked, rather than ending on a date the user was changing.
  const [badEnd, setBadEnd] = useState<string | null>(null);
  const id = useId();
  const set = (patch: Partial<RepeatForm>) => setForm((f) => ({ ...f, ...patch }));

  const made = ruleFromForm(form, startDate);
  const problem =
    form.ends === "on_date" && badEnd !== null
      ? badDateText("End date", badEnd)
      : made.ok
        ? null
        : repeatProblemText(made.problem);
  const one = form.interval.trim() === "1";
  // The rule's own monthly choice stays on offer when the start date's are others (a rule from
  // another app, or Google's own "last" on a day outside the month's last week).
  const monthly = monthlyChoices(startDate, first.monthly);
  const monthlyIndex = monthly.findIndex((c) =>
    c.by === "day"
      ? form.monthly.by === "day" && form.monthly.day === c.day
      : form.monthly.by === "weekday" &&
        form.monthly.nth === c.nth &&
        form.monthly.weekday === c.weekday,
  );
  const problemId = `${id}-problem`;
  const ends = `${id}-ends`;

  function toggle(day: DayCode) {
    set({
      days: form.days.includes(day) ? form.days.filter((d) => d !== day) : [...form.days, day],
    });
  }

  function done() {
    if (made.ok && problem === null) onDone(made.rule);
  }

  return (
    <Dialog
      open
      onClose={onClose}
      title="Custom recurrence"
      widthClassName="max-w-md"
      footer={
        <>
          <Button variant="tertiary" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={problem !== null} onClick={done}>
            Done
          </Button>
        </>
      }
    >
      <div className="mt-3 flex flex-col gap-4 text-sm text-ink2">
        <div className="flex flex-wrap items-center gap-2">
          <label htmlFor={`${id}-interval`}>Repeat every</label>
          <input
            id={`${id}-interval`}
            type="number"
            min={1}
            max={99}
            data-autofocus
            className={NUMBER}
            value={form.interval}
            onChange={(e) => set({ interval: e.currentTarget.value })}
          />
          <Select
            aria-label="Unit"
            value={form.unit}
            onChange={(e) => set({ unit: e.target.value as RepeatUnit })}
          >
            {UNITS.map((u) => (
              <option key={u.value} value={u.value}>
                {one ? u.one : u.many}
              </option>
            ))}
          </Select>
        </div>

        {form.unit === "week" && (
          <div className="flex flex-col gap-1.5">
            <span id={`${id}-on`}>Repeat on</span>
            <div role="group" aria-labelledby={`${id}-on`} className="flex gap-1.5">
              {WEEKDAYS.map((d) => {
                const on = form.days.includes(d.code);
                return (
                  <button
                    key={d.code}
                    type="button"
                    aria-pressed={on}
                    aria-label={d.name}
                    title={d.name}
                    onClick={() => toggle(d.code)}
                    className={cn(
                      "inline-flex h-8 w-8 items-center justify-center rounded-full text-xs transition",
                      // Off, the edge is `--ink4`: the contrast floor a control's own edge needs.
                      on
                        ? "bg-accent font-medium text-accent-ink"
                        : "text-ink2 shadow-[inset_0_0_0_1px_var(--ink4)] hover:text-ink",
                    )}
                  >
                    {d.name[0]}
                  </button>
                );
              })}
            </div>
          </div>
        )}

        {form.unit === "month" && (
          <Select
            aria-label="Monthly on"
            className="self-start"
            value={String(monthlyIndex)}
            onChange={(e) => {
              const choice = monthly[Number(e.target.value)];
              if (choice) set({ monthly: choice });
            }}
          >
            {monthly.map((c, i) => (
              <option key={monthlyLabel(c)} value={String(i)}>
                {monthlyLabel(c)}
              </option>
            ))}
          </Select>
        )}

        {/* Google's dialog offers no yearly choices: the date comes from the start. */}
        {form.unit === "year" && <p className="text-ink3">{yearlyLabel(form.yearly, startDate)}</p>}

        <fieldset className="flex flex-col gap-2">
          <legend className="mb-1">Ends</legend>
          <div className="flex items-center gap-2">
            <input
              id={`${ends}-never`}
              type="radio"
              name={ends}
              checked={form.ends === "never"}
              onChange={() => set({ ends: "never" })}
            />
            <label htmlFor={`${ends}-never`}>Never</label>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <input
              id={`${ends}-on`}
              type="radio"
              name={ends}
              checked={form.ends === "on_date"}
              onChange={() => set({ ends: "on_date" })}
            />
            <label htmlFor={`${ends}-on`} className="w-10">
              On
            </label>
            <DateField
              ariaLabel="End date"
              value={form.date}
              clearable={false}
              disabled={form.ends !== "on_date"}
              refused={form.ends === "on_date" && badEnd !== null}
              describedBy={form.ends === "on_date" && badEnd !== null ? problemId : undefined}
              wrapperClassName="w-36"
              onReject={setBadEnd}
              onAccept={() => setBadEnd(null)}
              onCommit={(date) => set({ date })}
            />
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <input
              id={`${ends}-after`}
              type="radio"
              name={ends}
              checked={form.ends === "after"}
              onChange={() => set({ ends: "after" })}
            />
            <label htmlFor={`${ends}-after`} className="w-10">
              After
            </label>
            <input
              type="number"
              min={1}
              max={730}
              aria-label="Occurrences"
              className={NUMBER}
              disabled={form.ends !== "after"}
              value={form.count}
              onChange={(e) => set({ count: e.currentTarget.value })}
            />
            <span className="text-ink3">
              {form.count.trim() === "1" ? "occurrence" : "occurrences"}
            </span>
          </div>
        </fieldset>

        {/* Always mounted, so a screen reader hears the line as it comes and goes. */}
        <div aria-live="polite">
          {problem !== null && (
            <p id={problemId} className="text-xs text-st-due">
              {problem}
            </p>
          )}
        </div>
      </div>
    </Dialog>
  );
}
