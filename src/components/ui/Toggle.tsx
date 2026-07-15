// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// An on/off switch (DESIGN_TOKENS.md §7). For a setting that applies the moment you flip it — a
// checkbox is for a choice you go on to confirm, a switch is the choice itself.
//
// The app has several hand-rolled copies of this markup predating the component. They are NOT all
// identical (two different off-track/knob palettes, and one carries its own disabled styling), so
// they are not a mechanical swap — migrating them is its own change, not a drive-by.

import { cn } from "./cn";

export interface ToggleProps {
  checked: boolean;
  onChange: (next: boolean) => void;
  /** Required: the switch renders no text of its own, so this is its only name. */
  ariaLabel: string;
  disabled?: boolean;
  className?: string;
}

export function Toggle({ checked, onChange, ariaLabel, disabled, className }: ToggleProps) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={ariaLabel}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cn(
        "inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors",
        checked ? "bg-accent" : "bg-surface",
        disabled && "cursor-not-allowed opacity-50",
        className,
      )}
    >
      <span
        className={cn(
          "inline-block h-4 w-4 transform rounded-full bg-accent-ink transition-transform",
          checked ? "translate-x-4" : "translate-x-0.5",
        )}
      />
    </button>
  );
}
