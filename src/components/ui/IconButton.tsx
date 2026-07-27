// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The icon-only sibling of Button (DESIGN_TOKENS.md §7). For a control whose content is a single icon
// or glyph and no text — a toolbar action, an inline remove/close, a chevron. Use this instead of a
// hand-rolled <button> so the three things icon buttons kept getting wrong stay a one-place concern:
//   • a ≥24px tap target (WCAG 2.5.8) via --tap-min — 24px by default, 44px at Comfortable density —
//     while the *visible* icon keeps its own size (the box grows around it, not the glyph);
//   • an accessible name — `label` is required (there's no text to name it) and doubles as the tooltip;
//   • the shared focus ring / disabled / transition treatment.

import type { ComponentPropsWithRef, ReactNode } from "react";
import { cn } from "./cn";

export type IconButtonVariant = "ghost" | "subtle" | "danger" | "pressed";

// Hover/active gated to :enabled so a disabled icon button never reacts to the pointer. All start at
// --ink4 (the subtle resting colour icon controls across PM already use); they differ in hover.
const VARIANT: Record<IconButtonVariant, string> = {
  // Toolbar / header actions: a hover surface + full-strength ink.
  ghost: "text-ink4 enabled:hover:bg-surface enabled:hover:text-ink",
  // Inline (no hover surface) — dismiss ✕, nav chevrons.
  subtle: "text-ink4 enabled:hover:text-ink",
  // Destructive — remove / delete.
  danger:
    "text-ink4 enabled:hover:bg-[color-mix(in_oklab,var(--st-due)_18%,transparent)] enabled:hover:text-st-due",
  // A toggle currently ON — the same filled treatment SegmentedControl gives its active segment, so
  // "this one is chosen" reads identically wherever it appears. Swap the VARIANT (not the class
  // list) to show pressed state: `cn` is a plain joiner, not tailwind-merge, so a `text-*` passed
  // through `className` does NOT replace the variant's own `text-ink4` — both survive and CSS
  // source order silently decides. That is #469, and it is why an ad-hoc pressed colour looks like
  // a control that does nothing.
  pressed: "bg-accent text-accent-ink enabled:hover:brightness-110",
};

export interface IconButtonProps extends Omit<ComponentPropsWithRef<"button">, "aria-label"> {
  /** Required accessible name — the button renders no text, so this is its only name. Also the
   *  default native tooltip (override with an explicit `title`). */
  label: string;
  variant?: IconButtonVariant;
  /** The icon / glyph. */
  children: ReactNode;
}

export function IconButton({
  label,
  variant = "ghost",
  className,
  children,
  title,
  type,
  ...rest
}: IconButtonProps) {
  return (
    <button
      type={type ?? "button"}
      aria-label={label}
      title={title ?? label}
      className={cn(
        "inline-flex min-h-[var(--tap-min,24px)] min-w-[var(--tap-min,24px)] shrink-0 items-center justify-center rounded-[var(--radius-sm)] transition disabled:cursor-not-allowed disabled:opacity-40",
        VARIANT[variant],
        className,
      )}
      {...rest}
    >
      {children}
    </button>
  );
}
