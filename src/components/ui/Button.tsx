// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Token-driven button (DESIGN_TOKENS.md §7). Four variants × four sizes; the terminal System wraps
// the label in brackets and forces mono. Never style a button instance directly — use this.

import type { ComponentPropsWithRef, ReactNode } from "react";
import { useTheme } from "../../theme";
import { cn } from "./cn";

export type ButtonVariant = "primary" | "secondary" | "tertiary" | "danger";
export type ButtonSize = "xs" | "sm" | "md" | "lg";

// Hover/active are gated to :enabled so a disabled button never reacts to the pointer — combined
// with the base disabled:opacity-40 it reads as unmistakably inert, not merely dimmed.
const VARIANT: Record<ButtonVariant, string> = {
  primary:
    "bg-accent text-accent-ink font-semibold enabled:hover:brightness-105 enabled:active:brightness-95 disabled:bg-surface disabled:text-faint",
  secondary:
    "bg-transparent text-ink2 border border-border2 enabled:hover:bg-surface disabled:text-faint",
  tertiary: "bg-transparent text-ink4 enabled:hover:text-ink2 disabled:text-faint",
  // Destructive — the confirm on a delete/reset/wipe. Five call sites had hand-tinted a
  // `variant="primary"` with an inline `--st-due` mix, which is the same drift `Callout` exists to
  // end; this is where that recipe lives now. The percentages ARE `tone.ts`'s `TONE_MIX.fill` /
  // `.fillHover` and `Button.test.tsx` asserts they still match — they are spelled literally here
  // because Tailwind extracts class names by scanning source text, so an interpolated arbitrary
  // value would compile to no CSS at all. Brightness is the wrong hover step for a translucent
  // tint (it barely moves), so the mix deepens instead.
  danger:
    "bg-[color-mix(in_oklab,var(--st-due)_15%,transparent)] text-st-due font-semibold enabled:hover:bg-[color-mix(in_oklab,var(--st-due)_24%,transparent)] enabled:active:brightness-95 disabled:bg-surface disabled:text-faint",
};

// SWAP the size classes, never layer them. `cn()` is a plain joiner, so emitting the base `px-3`
// AND a call site's `px-2` leaves the winner to stylesheet order — and Tailwind emits spacing
// ASCENDING, so the base wins and the call site's smaller value is dead. That is not a theory: 49
// of the 50 sites that passed sizing utilities were rendering at default padding, with only their
// `text-*` taking effect (font sizes are emitted alphabetically, so `text-xs` beats `text-sm`).
// Every compact button in PM was therefore a full-size box with shrunken type. Same hazard, same
// fix as `Select`'s `compact`.
const SIZE: Record<ButtonSize, string> = {
  // Dense steppers and micro toolbar chips. --tap-min still floors the box to 24px, so xs and sm
  // render the same HEIGHT at Standard density and differ only in horizontal padding.
  xs: "px-1.5 py-0.5 text-xs",
  // Byte-identical to Select's `compact`, so a Button lines up beside one. Button.test.tsx pins it.
  sm: "px-2 py-1 text-xs",
  // Today's base, verbatim — every unannotated call site is untouched by the seam landing.
  md: "px-3 py-1.5 text-sm",
  // Matches Input's own px-4 py-2, for a control paired with a full-size field.
  lg: "px-4 py-2 text-sm",
};

export interface ButtonProps extends ComponentPropsWithRef<"button"> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  children?: ReactNode;
}

export function Button({
  variant = "secondary",
  // `size` MUST stay destructured: <button> has no `size` attribute (only input/select do), so
  // TypeScript would not catch it landing in `...rest` and React would emit it to the DOM.
  size = "md",
  className,
  children,
  ...rest
}: ButtonProps) {
  const { system } = useTheme();
  const terminal = system === "terminal";
  return (
    <button
      className={cn(
        "inline-flex min-h-[var(--tap-min,24px)] min-w-[var(--tap-min,24px)] items-center justify-center gap-1.5 rounded-[var(--radius-sm)] transition disabled:cursor-not-allowed disabled:opacity-40",
        SIZE[size],
        terminal && "font-mono",
        VARIANT[variant],
        className,
      )}
      {...rest}
    >
      {terminal ? <>[&nbsp;{children}&nbsp;]</> : children}
    </button>
  );
}
