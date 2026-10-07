// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// One radio row: the control, its label, and the sentence that says what choosing it means.
//
// Plain radios rather than a segmented control: these are consequential, mutually exclusive choices
// that each need a sentence of explanation, which a compact toggle can't carry. Lifted out of
// DeleteProjectDialog unchanged when the Local AI tab's On battery section needed the same row, so
// the two cannot drift. The `<label htmlFor>` pairing is what designGuards' orphan-label scan reads.
//
// `current` may be null — nothing checked — so a section can render the choice before it knows the
// stored value, rather than presenting a default as though it were the user's.

import type { ReactNode } from "react";

export interface RadioChoiceProps<T extends string> {
  /** The radio group's name; with `value` it also mints the input's id. */
  name: string;
  value: T;
  /** The selected value, or null while it isn't known. */
  current: T | null;
  onSelect: (v: T) => void;
  label: string;
  detail: string;
  /** A further quiet line under `detail` — e.g. what this choice would do with the current setup. */
  note?: ReactNode;
  disabled: boolean;
}

export function RadioChoice<T extends string>({
  name,
  value,
  current,
  onSelect,
  label,
  detail,
  note,
  disabled,
}: RadioChoiceProps<T>) {
  const id = `${name}-${value}`;
  return (
    <label
      htmlFor={id}
      className="flex cursor-pointer items-start gap-2 rounded-[var(--radius-sm)] px-2 py-1.5 hover:bg-surface"
    >
      <input
        id={id}
        type="radio"
        name={name}
        checked={current === value}
        onChange={() => onSelect(value)}
        disabled={disabled}
        className="mt-0.5 shrink-0"
      />
      <span className="text-sm leading-snug">
        <span className="text-ink2">{label}</span>
        <span className="block text-xs text-ink4">{detail}</span>
        {note != null && <span className="block text-xs text-ink4">{note}</span>}
      </span>
    </label>
  );
}
