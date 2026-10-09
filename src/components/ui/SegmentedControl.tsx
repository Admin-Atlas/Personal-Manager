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
// NAMING is now a required union, the way `Toggle`'s is: all 26 call sites carry a name (11 through
// a `SettingRow` spread, 15 typed), so a group with neither prop no longer compiles. That is the
// point at which the rule stops depending on anyone remembering it.

import type { ReactNode } from "react";
import { cn } from "./cn";

export interface SegOption<T extends string> {
  value: T;
  label: ReactNode;
  title?: string;
}

interface SegmentedControlBaseProps<T extends string> {
  options: ReadonlyArray<SegOption<T>>;
  value: T;
  onChange: (value: T) => void;
  className?: string;
  id?: string;
  "aria-describedby"?: string;
  /** Shown but not changeable (a value the user may see and not set). */
  disabled?: boolean;
}

export type SegmentedControlProps<T extends string> = SegmentedControlBaseProps<T> &
  (
    | { ariaLabel: string; "aria-labelledby"?: never }
    /** Spread from `SettingRow`/`useFieldA11y` — names the group from the visible row label. */
    | { "aria-labelledby": string; ariaLabel?: never }
  );

export function SegmentedControl<T extends string>({
  options,
  value,
  onChange,
  className,
  ariaLabel,
  id,
  "aria-labelledby": ariaLabelledBy,
  "aria-describedby": ariaDescribedBy,
  disabled,
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
            disabled={disabled}
            onClick={() => onChange(opt.value)}
            className={cn(
              "inline-flex min-h-[var(--tap-min,24px)] items-center justify-center rounded-[var(--radius-sm)] px-2.5 py-1 text-xs transition disabled:cursor-not-allowed",
              // Disabled is a colour change, no alpha (as Button): the chosen value stays legible
              // on a neutral fill, the others step down a tier. Never `faint`: the options are
              // information (what it could be), not decoration (designGuards).
              active
                ? disabled
                  ? "bg-surface text-ink2 font-medium"
                  : "bg-accent text-accent-ink font-medium"
                : "text-ink3 enabled:hover:text-ink disabled:text-ink4",
            )}
          >
            {opt.label}
          </button>
        );
      })}
    </div>
  );
}
