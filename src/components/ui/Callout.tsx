// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The tinted message container (DESIGN_TOKENS.md §7) — the error banner, the caution strip, the
// "here is what will happen" note. Built on the same pattern `StatusBadge` already proves: take a
// semantic key, derive the token, own the `color-mix` inside the primitive.
//
// It exists for two reasons, and the second one matters more.
//
// 1. The recipe had drifted into five rival danger ratios across ~45 hand-written blocks, with the
//    radius drifting alongside. `tone.ts` is now the only place a ratio is written.
// 2. Of those ~45 tinted banners, 8 announced to a screen reader. The other ~31 were silent, and
//    about 27 of them are dynamic error messages that are the ONLY feedback an action failed —
//    including the whole vault-recovery path. So this primitive OWNS the live-region semantics:
//    `danger` announces by default. The asymmetry is deliberate. A spurious announcement is a
//    mildly chatty dialog; a missed one is a blind user hitting Save, hearing nothing, and
//    believing the app is idle. Static consequence-prose opts out with `live={false}`.
//
// Two things it deliberately does NOT do:
//
// • **It emits no margin and no positioning.** Spacing and placement stay at the call site, through
//   `className`. This is not stylistic tidiness — `cn()` is a plain joiner, so if the primitive
//   emitted `mb-3` and a caller passed `mt-4`, BOTH would survive and CSS source order would pick
//   the winner. Emitting nothing means there is never a conflicting pair.
// • **It does not fork by System.** `StatusBadge` forks editorial/slate/terminal and an argument
//   exists that a tinted box is wrong for editorial's set-like-a-page language — but no callout in
//   the tree forks today, so building the fork in would smuggle an unrequested design decision
//   through a refactor. One treatment; the fork is a separate, deliberate call.
//
// It reads no Depth either: Depth shows and hides features, and an error is not a feature.

import type { HTMLAttributes, ReactNode } from "react";
import { cn } from "./cn";
import { toneSurface, type Tone } from "./tone";

export type CalloutTone = Tone;
/** `box` = the bordered, rounded note that sits inside a column. `strip` = the full-bleed band that
 *  spans a pane and separates itself with a bottom rule (chat errors, the fallback notice). */
export type CalloutVariant = "box" | "strip";
export type CalloutSize = "sm" | "md";
/** `tone` colours the message in the tone's own hue; `ink` leaves the text inheriting, for callouts
 *  that wrap neutral prose or controls rather than a one-line message. */
export type CalloutBody = "tone" | "ink";

// Swap the framing, never layer it — the same discipline `Select`'s `compact` and `Button`'s `size`
// follow, and for the same reason (see the file header on `cn()`).
const VARIANT: Record<CalloutVariant, string> = {
  box: "rounded-[var(--radius-sm)] border px-3 py-2",
  strip: "border-b px-4 py-2",
};

const SIZE: Record<CalloutSize, string> = {
  sm: "text-xs",
  md: "text-sm",
};

export interface CalloutProps extends HTMLAttributes<HTMLElement> {
  /** Defaults to `danger`: it is what the overwhelming majority of these are. */
  tone?: CalloutTone;
  variant?: CalloutVariant;
  size?: CalloutSize;
  body?: CalloutBody;
  /** Announce on appearance. Defaults to `true` for `danger` (→ `role="alert"`) and `false`
   *  otherwise; passing it explicitly on `info`/`warning` gives them `role="status"`. Pass
   *  `live={false}` for prose that is present at mount rather than appearing on failure — assistive
   *  tech is inconsistent about an alert that exists at first render — and for a site that already
   *  carries its own live region (e.g. a spread `useFieldA11y.errorProps`), so the two don't fight. */
  live?: boolean;
  /** ~20 conversion targets are paragraphs; keep their markup so a `<div>` never lands inside a
   *  `<p>` parent (React warns, and browsers reflow around it). */
  as?: "div" | "p";
  children?: ReactNode;
}

export function Callout({
  tone = "danger",
  variant = "box",
  size = "sm",
  body = "tone",
  live,
  as = "div",
  className,
  style,
  children,
  ...rest
}: CalloutProps) {
  const Tag = as;
  const announce = live ?? tone === "danger";
  const role = announce ? (tone === "danger" ? "alert" : "status") : undefined;
  return (
    <Tag
      role={role}
      className={cn(VARIANT[variant], SIZE[size], className)}
      // Caller style wins, so a site with a genuine reason to differ (the reader toast mixes into
      // --bg rather than transparent, because it floats over content and must stay opaque) can say
      // so at the call site instead of forking the primitive.
      style={{ ...toneSurface(tone, body), ...style }}
      {...rest}
    >
      {children}
    </Tag>
  );
}
