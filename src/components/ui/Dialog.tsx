// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The chrome 18 of PM's 19 dialogs had retyped, and the reason none of them could ship unnamed
// again. `Modal` owns the SEMANTICS (role, aria-modal, Escape, focus trap, scrim); this owns the
// LOOK — heading, optional eyebrow/subtitle, footer — and, critically, the wiring between them:
// it mints the heading id with `useId` and hands it to `Modal` itself, so the accessible name is a
// consequence of passing a `title`, not a second thing to remember.
//
// `title` is REQUIRED. That is the fix for `role="dialog"` with no accessible name, made at the
// type level rather than in a lint rule nobody runs: a screen reader announced 12 of PM's dialogs
// as bare "dialog", including "Remove this data?", "Final confirmation", "Delete <project>" and
// "Remove this tag?". The heading was sitting in the DOM one line below the `<Modal>` at every one
// of them, unwired.
//
// TWO CHROMES, because the tree really has two and neither is a variant of the other:
//   card — a p-5 block with an h2 and a right-aligned footer. Confirmations and short prompts.
//   bar  — a bordered header with an h1 and a Close button, a scrolling body, an optional bordered
//          footer. Long documents: What's New, the engine guide, the share wizard.
// A card carries NO Close affordance of its own. One of the remove-my-data steps is deliberately
// undismissable (`onClose={() => {}}`), and a shell that painted a Close button on it would hand
// back the exit that dialog exists to withhold.
//
// Props-driven rather than an exported Header/Body/Footer trio: 18 of the 18 bodies are a single
// block, and a trio adds an ordering a call site can get wrong and `cn()` cannot rescue.
//
// The children are rendered with NO wrapper of their own. Every card site already carries its own
// leading `mt-*` on its first child, and a primitive that emitted spacing here would be a utility
// the call site immediately has to fight — which, `cn()` being a plain joiner, it cannot win.

import { useId, type ReactNode } from "react";
import { Button } from "./Button";
import { cn } from "./cn";
import { Modal, type ModalBaseProps } from "./Modal";
import { TONE_TEXT_TOKEN } from "./tone";

export type DialogChrome = "card" | "bar";

/** A dialog's tone colours its HEADING only — the shell is chrome, not a message, so it never
 *  tints its own surface. `danger` reads its colour from the one tone→recipe map (`tone.ts`), the
 *  same map `Callout` and `Button variant="danger"` read, so the recipe exists once. */
export type DialogTone = "default" | "danger";

export interface DialogProps extends Omit<ModalBaseProps, "children"> {
  /** The dialog's visible title AND its accessible name — required, and wired to `aria-labelledby`
   *  automatically. There is deliberately no way to render a Dialog without one. */
  title: ReactNode;
  /** The `mt-1 text-xs text-ink4` line under the title (ContextMeter and RebuildProgress both
   *  retype it). */
  subtitle?: ReactNode;
  /** A small mono line ABOVE the title — the share wizard's "Step 2 of 4 · …". */
  eyebrow?: ReactNode;
  tone?: DialogTone;
  chrome?: DialogChrome;
  /** Label for the bar chrome's Close button. Ignored by the card chrome, which has none. */
  closeLabel?: string;
  /** The action row. Card: right-aligned under the body. Bar: a bordered row at the foot, rendered
   *  only when given. */
  footer?: ReactNode;
  children: ReactNode;
}

export function Dialog({
  title,
  subtitle,
  eyebrow,
  tone = "default",
  chrome = "card",
  closeLabel = "Close",
  footer,
  children,
  className,
  onClose,
  ...modal
}: DialogProps) {
  const titleId = useId();
  // Swap the colour, never layer it: `text-ink` and a tone colour would both survive `cn()`.
  const toneStyle = tone === "danger" ? { color: `var(${TONE_TEXT_TOKEN.danger})` } : undefined;
  const headingTint = tone === "danger" ? undefined : "text-ink";

  if (chrome === "bar") {
    return (
      <Modal
        {...modal}
        onClose={onClose}
        labelledBy={titleId}
        className={cn("flex flex-col", className)}
      >
        <div className="flex items-center justify-between border-b border-border px-6 py-4">
          <div className="min-w-0">
            {eyebrow != null && (
              <p className="font-mono text-xs uppercase tracking-wide text-ink4">{eyebrow}</p>
            )}
            <h1
              id={titleId}
              style={toneStyle}
              className={cn("font-head text-lg font-semibold", headingTint)}
            >
              {title}
            </h1>
            {subtitle != null && <p className="mt-1 text-xs text-ink4">{subtitle}</p>}
          </div>
          <Button variant="tertiary" onClick={onClose}>
            {closeLabel}
          </Button>
        </div>

        <div className="flex-1 overflow-y-auto px-6 py-4">{children}</div>

        {footer != null && (
          <div className="flex items-center justify-end gap-2 border-t border-border px-6 py-4">
            {footer}
          </div>
        )}
      </Modal>
    );
  }

  return (
    <Modal {...modal} onClose={onClose} labelledBy={titleId} className={className}>
      <div className="p-5">
        {eyebrow != null && (
          <p className="mb-1 font-mono text-xs uppercase tracking-wide text-ink4">{eyebrow}</p>
        )}
        <h2
          id={titleId}
          style={toneStyle}
          className={cn("font-head text-base font-semibold", headingTint)}
        >
          {title}
        </h2>
        {subtitle != null && <p className="mt-1 text-xs text-ink4">{subtitle}</p>}
        {children}
        {footer != null && <div className="mt-5 flex justify-end gap-2">{footer}</div>}
      </div>
    </Modal>
  );
}
