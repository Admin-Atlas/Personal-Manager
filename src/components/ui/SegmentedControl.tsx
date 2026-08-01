// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A small segmented toggle. Generalises ReviewView's importance picker and backs the Settings
// System / Mode / Depth pickers. Active segment = accent fill; the group is one token-bordered
// strip.
//
// The group had NO way to be named until now, so 13 Settings groups shipped anonymous: a screen
// reader on Appearance heard "Editorial, not pressed / Slate, not pressed / Terminal, not pressed"
// with no hint the group is "System". `ariaLabel` is a string the author types; `aria-labelledby` /
// `aria-describedby` / `id` keep their DOM names because they are what a caller SPREADS from
// `SettingRow`, so one object names Toggle, SegmentedControl, Select and Input alike.
//
// The name is optional only because 26 call sites predate it. Once each has one, make it a required
// union the way `Toggle` already does — the pattern is there to copy.

import type { ReactNode } from "react";
import { cn } from "./cn";

export interface SegOption<T extends string> {
  value: T;
  label: ReactNode;
  title?: string;
}

export interface SegmentedControlProps<T extends string> {
  options: ReadonlyArray<SegOption<T>>;
  value: T;
  onChange: (value: T) => void;
  className?: string;
  /** The group's accessible name, typed at the call site. */
  ariaLabel?: string;
  id?: string;
  /** Spread from `SettingRow`/`useFieldA11y` — names the group from the visible row label. */
  "aria-labelledby"?: string;
  "aria-describedby"?: string;
}

export function SegmentedControl<T extends string>({
  options,
  value,
  onChange,
  className,
  ariaLabel,
  id,
  "aria-labelledby": ariaLabelledBy,
  "aria-describedby": ariaDescribedBy,
}: SegmentedControlProps<T>) {
  return (
    <div
      className={cn(
        "inline-flex items-center gap-0.5 rounded-[var(--radius-sm)] border border-border2 p-0.5",
        className,
      )}
      role="group"
      id={id}
      aria-label={ariaLabel}
      aria-labelledby={ariaLabelledBy}
      aria-describedby={ariaDescribedBy}
    >
      {options.map((opt) => {
        const active = opt.value === value;
        return (
          <button
            key={opt.value}
            type="button"
            title={opt.title}
            aria-pressed={active}
            onClick={() => onChange(opt.value)}
            className={cn(
              "inline-flex min-h-[var(--tap-min,24px)] items-center justify-center rounded-[var(--radius-sm)] px-2.5 py-1 text-xs transition",
              active ? "bg-accent text-accent-ink font-medium" : "text-ink3 hover:text-ink",
            )}
          >
            {opt.label}
          </button>
        );
      })}
    </div>
  );
}
