// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// An on/off switch (DESIGN_TOKENS.md §7). For a setting that applies the moment you flip it — a
// checkbox is for a choice you go on to confirm, a switch is the choice itself.
//
// Density-aware: the outer button is a transparent ≥--tap-min hit area (WCAG 2.5.8), and the
// coloured track inside is sized by the --tg-* vars applyTheme stamps per density. So `compact`
// keeps today's 20px track while still offering a 24px target; `comfortable` grows both. The var
// fallbacks match the `standard` (compliant) default, so the switch is correct even before the
// theme effect first runs.

import { cn } from "./cn";

export interface ToggleProps {
  checked: boolean;
  onChange: (next: boolean) => void;
  /** Required: the switch renders no text of its own, so this is its only name. */
  ariaLabel: string;
  disabled?: boolean;
  /** Native tooltip — used to explain why a disabled switch is unavailable. */
  title?: string;
  className?: string;
}

export function Toggle({ checked, onChange, ariaLabel, disabled, title, className }: ToggleProps) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={ariaLabel}
      title={title}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cn(
        "inline-flex min-h-[var(--tap-min,24px)] min-w-[var(--tap-min,24px)] shrink-0 items-center justify-center bg-transparent",
        disabled && "cursor-not-allowed opacity-50",
        className,
      )}
    >
      <span
        className={cn(
          "relative inline-block h-[var(--tg-track-h,24px)] w-[var(--tg-track-w,44px)] rounded-full transition-colors",
          checked ? "bg-accent" : "bg-surface",
        )}
      >
        <span
          className={cn(
            "absolute left-[2px] top-1/2 h-[var(--tg-knob,20px)] w-[var(--tg-knob,20px)] -translate-y-1/2 rounded-full bg-accent-ink transition-transform",
            checked ? "translate-x-[var(--tg-on,20px)]" : "translate-x-0",
          )}
        />
      </span>
    </button>
  );
}
