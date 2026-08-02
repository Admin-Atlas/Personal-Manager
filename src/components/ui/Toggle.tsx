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
//
// NAMING is a required union, never two optionals: the switch renders no text of its own, so a
// switch with neither prop is an unnamed control and must not compile. `ariaLabel` is a string the
// author TYPES; the `aria-*` props are the ones a caller SPREADS from `SettingRow`/`useFieldA11y`,
// which is why they keep their DOM names — one spread then serves Toggle, SegmentedControl, Select
// and Input alike, and the label text is written in exactly one place. Prefer the spread inside
// Settings: 7 hand-typed `ariaLabel`s announced words the visible label did not contain, which is a
// WCAG 2.5.3 Label-in-Name failure ("click Models in use" does nothing today).

import { cn } from "./cn";

interface ToggleBaseProps {
  checked: boolean;
  onChange: (next: boolean) => void;
  disabled?: boolean;
  /** Native tooltip — used to explain why a disabled switch is unavailable. */
  title?: string;
  className?: string;
  /** Spread from `SettingRow`/`useFieldA11y`; a labelable wrapper can then point at the switch. */
  id?: string;
  "aria-describedby"?: string;
}

export type ToggleProps = ToggleBaseProps &
  (
    | { ariaLabel: string; "aria-labelledby"?: never }
    /** Named by the visible label a `SettingRow` (or `Field`) already renders. */
    | { "aria-labelledby": string; ariaLabel?: never }
  );

export function Toggle({
  checked,
  onChange,
  ariaLabel,
  disabled,
  title,
  className,
  id,
  "aria-labelledby": ariaLabelledBy,
  "aria-describedby": ariaDescribedBy,
}: ToggleProps) {
  return (
    <button
      type="button"
      role="switch"
      id={id}
      aria-checked={checked}
      aria-label={ariaLabel}
      aria-labelledby={ariaLabelledBy}
      aria-describedby={ariaDescribedBy}
      title={title}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cn(
        "inline-flex min-h-[var(--tap-min,24px)] min-w-[var(--tap-min,24px)] shrink-0 items-center justify-center bg-transparent",
        disabled && "cursor-not-allowed opacity-50",
        className,
      )}
    >
      {/* The track carries an OUTLINE, not just a fill. `--surface` sits one step off `--panel` by
          design — that separation is enough to read as a raised area behind text, and nowhere near
          enough to read as the EDGE of a control on its own, so an OFF switch was a knob floating
          on what looked like empty page. `--border2` is the ramp's strong edge and it is
          mode-relative in exactly the way this needs: `boost()` pushes it toward the contrast
          extreme, so it darkens against a light page and lightens against a dark one, and `high`
          firms it further (+0.18 L light / +0.20 L dark) rather than leaving the switch alone the
          way an un-outlined track did at every level.

          AN INSET SHADOW, NOT A BORDER, and that is not a stylistic choice. A border is part of the
          box, so with `border-box` it eats a pixel off the padding box — which is what `absolute`
          offsets and percentage `top` resolve against. Adding one shrank the knob's inset from 2px
          to 1px on all four sides, making the dot look larger and its surround thinner, and left
          the two states' edges landing on different device-pixel boundaries at a fractional DPR
          (Windows at 150%), so the gap looked wider on one side than the other. A shadow paints
          over the track's own edge and takes part in no layout at all, so the knob geometry is
          exactly what it was before the outline existed and is identical in both states. */}
      <span
        className={cn(
          "relative inline-block h-[var(--tg-track-h,24px)] w-[var(--tg-track-w,44px)] rounded-full transition-colors",
          checked
            ? "bg-accent shadow-[inset_0_0_0_1px_var(--accent)]"
            : "bg-surface shadow-[inset_0_0_0_1px_var(--border2)]",
        )}
      >
        {/* The knob colour follows `checked`, exactly as the track above does. It used to be
            an unconditional `bg-accent-ink` — a token calibrated ONLY against the accent fill
            it sits on when the switch is ON. Off the accent it has no contract at all, and
            under the mono accent `--accent-ink` and `--bg` are the same literal, so an OFF
            knob was drawn in the page background: 1.00:1 against the page, 1.04:1 against its
            own track. Every default install (slate + dark + mono) saw it. `ink4` is the
            lowest neutral role `boost()` floors at 4.5:1 at every Contrast level, which
            contrast.test.ts already pins, so the OFF knob inherits a real floor instead of
            depending on the accent. */}
        <span
          className={cn(
            // 2px, the original inset: the track's outline is a shadow and so consumes none of the
            // padding box these offsets resolve against. At both densities (44/24/20/20 and
            // 52/28/24/24) that is 2px of track on every side, ON and OFF alike.
            "absolute left-[2px] top-1/2 h-[var(--tg-knob,20px)] w-[var(--tg-knob,20px)] -translate-y-1/2 rounded-full transition-transform",
            checked ? "bg-accent-ink translate-x-[var(--tg-on,20px)]" : "bg-ink4 translate-x-0",
          )}
        />
      </span>
    </button>
  );
}
