// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// PM's time-of-day input: a list of quarter hours, never `<input type="time">`. WebKitGTK (Linux) has
// no native time widget and degrades it to a plain text box in which a strict controlled value can't
// be typed at all (see RangeControl and ui/Select). A select only ever holds a whole valid `HH:MM`, so
// it behaves the same on every engine.
//
// The time Google holds (`held`) is always offered, even off the quarter-hour grid: an event at 09:10
// shows 09:10, and if an arrow key slips it to 09:15 it can be picked back, so the field is untouched
// again and nothing is sent. Picking fires `onChange` with the new `HH:MM`; like DateField, the caller
// receives the value rather than reading its own state back.
//
// It has no name of its own: label it with a wrapping or `htmlFor` `<label>`, `ariaLabelledBy`, or
// `ariaLabel` (an `aria-label` would override a real label, so none is set by default).

import { timeChoices } from "../lib/calendarEdit/timeSlots";
import { Select, cn } from "./ui";

interface Props {
  /** `HH:MM`, 24h. */
  value: string;
  onChange: (hm: string) => void;
  /** The time Google holds for this field, kept on offer whatever `value` is now. */
  held?: string;
  disabled?: boolean;
  /** The smaller size the calendar's popovers use. */
  compact?: boolean;
  className?: string;
  ariaLabel?: string;
  ariaLabelledBy?: string;
  id?: string;
}

export function TimeField({
  value,
  onChange,
  held,
  disabled,
  compact,
  className,
  ariaLabel,
  ariaLabelledBy,
  id,
}: Props) {
  return (
    <Select
      id={id}
      compact={compact}
      value={value}
      disabled={disabled}
      aria-label={ariaLabel}
      aria-labelledby={ariaLabelledBy}
      onChange={(e) => onChange(e.target.value)}
      className={cn("font-mono", className)}
    >
      {timeChoices(value, held).map((hm) => (
        <option key={hm} value={hm}>
          {hm}
        </option>
      ))}
    </Select>
  );
}
