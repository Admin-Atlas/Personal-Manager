// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A blocking modal over a fixed scrim (DESIGN_TOKENS.md §7). Esc and scrim-click close. The
// scrim tint is the one intentionally-fixed colour in the system (a neutral darkening that reads
// the same under any accent). Consolidates the app's ad-hoc bg-neutral-950/80 overlays and backs
// the design's Approval / Permission patterns.
//
// This is the dialog SHELL — the semantics, not the layout. It owns `role="dialog"` +
// `aria-modal`, the accessible name, Escape (topmost only), the focus trap and focus restore, and
// the scrim. What goes inside is entirely the caller's: `Dialog` wears it for the two chromes 18
// call sites had retyped, and the command palette and Settings can wear it with their own
// (top-anchored, full-height) layouts. Forcing one shape on all of them would be worse than the
// duplication it removes.
//
// NOT a scroll lock, and deliberately so: `index.css` makes html/body `overflow: hidden` app-wide,
// so the document behind a dialog has nothing to scroll. Do not claim one in a changelog.

import { useEffect, useRef, type CSSProperties, type ReactNode } from "react";
import { cn } from "./cn";
import { useDialogLayer } from "../../lib/useDialogLayer";
import { useFocusTrap } from "../../lib/useFocusTrap";

/** Where the dialog sits in the viewport. A variant rather than a class the caller passes, because
 *  `cn()` is a plain joiner: an `items-start` passed through `className` would leave `items-center`
 *  in the list too and let stylesheet order pick the winner. The whole padding string swaps with it
 *  for the same reason — Tailwind emits `p-*` and `pt-*` in an order no call site should have to
 *  reason about. */
export type ModalPlacement = "center" | "top";

export interface ModalBaseProps {
  open: boolean;
  onClose: () => void;
  children: ReactNode;
  /** Width class for the dialog (default max-w-lg). */
  widthClassName?: string;
  /** Height class for the dialog (default max-h-[85vh]). REPLACES the default rather than competing
   *  with it: `cn` is a plain joiner, not tailwind-merge, so passing a rival max-h-* through
   *  `className` leaves both in the class list and lets stylesheet order decide the winner. */
  heightClassName?: string;
  /** Overflow class for the dialog (default overflow-y-auto) — same replace-not-compete reason. A
   *  dialog that manages its own scrolling inside (e.g. a board) passes `overflow-hidden`. */
  overflowClassName?: string;
  /** Inline styles for the dialog — for a size only known at runtime (e.g. a share of a measured
   *  surface), which no class can express. Beats the class defaults, so pair it with the
   *  `*ClassName` seams above rather than leaving a rival class in play. */
  style?: CSSProperties;
  className?: string;
  /** "center" (default) or "top" — a search palette hangs from the top of the window rather than
   *  sitting in the middle of it. */
  placement?: ModalPlacement;
  /** Registry id for help mode. Lands as `data-help` on the DIALOG element, same as `SettingRow`.
   *  It has to be the dialog itself and not a wrapper inside it: `HelpOverlay` resolves a hovered
   *  element with `closest("[data-help]")`, but `.help-mode [data-help]:hover` also draws the
   *  outline that TELLS you there is help here — and an element with no box of its own paints no
   *  outline. The command palette is the one dialog in the tree with a registry entry. */
  helpId?: string;
}

/** How the dialog gets its accessible name — REQUIRED, as an either/or. A `role="dialog"` with
 *  neither is announced as just "dialog", which is what 12 of PM's 19 dialogs did before this batch,
 *  including every remove / delete / merge confirmation, whose headings were sitting in the DOM one
 *  line below the `<Modal>`, unwired.
 *
 *  Prefer `labelledBy`, pointing at the heading already on screen, so the name cannot drift from
 *  the visible title (WCAG 2.5.3). `label` is the escape hatch for a dialog whose "title" is not a
 *  heading — PinboardView's folder board, whose title is an editable `<input>`, is the only one in
 *  the tree.
 *
 *  This is a union, not two optionals, and that is the whole point of the batch: the NEXT unnamed
 *  dialog is a `tsc` failure rather than an audit finding. Nearly nothing should reach for it
 *  directly — `Dialog` requires a `title`, mints the id and wires `labelledBy` itself, so anything
 *  going through the shell is named by construction.
 *
 *  The `?: never` arms make it exclusive, and are also what lets the implementation destructure
 *  both names off a union type: a property TypeScript can see on every member is a property it will
 *  let you read. Passing both would be meaningless anyway — `aria-labelledby` wins the accessible-
 *  name computation outright, so an `aria-label` alongside it is dead text nobody hears. */
export type ModalNameProps =
  { labelledBy: string; label?: never } | { label: string; labelledBy?: never };

export type ModalProps = ModalBaseProps & ModalNameProps;

export function Modal({
  open,
  onClose,
  children,
  widthClassName,
  heightClassName,
  overflowClassName,
  style,
  className,
  placement = "center",
  helpId,
  labelledBy,
  label,
}: ModalProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  // Registers this dialog while it is open so it can tell whether anything deeper is open on top of
  // it. Escape is a WINDOW listener (so it works from any focus position, which is the right
  // trade), which means every open dialog hears every Escape — without this, one keypress closes
  // the whole stack. See lib/useDialogLayer.ts.
  const isTopmost = useDialogLayer(open, dialogRef);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (!isTopmost()) return;
      onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose, isTopmost]);

  // Trap Tab inside the dialog, focus into it on open, and restore focus to the opener on close —
  // fixes every dialog (ConfirmDialog, MoveConversationDialog, …) at once. The trap stands down
  // while a nested dialog holds focus; see useFocusTrap's header.
  useFocusTrap(open, dialogRef);

  if (!open) return null;

  return (
    // Start below the custom title bar (top-9 = its h-9) so the frameless window's drag region and
    // min/max/close controls stay visible and clickable while a modal is open — same convention as
    // DocumentReader. The scrim never covers the top chrome.
    <div
      className={cn(
        "fixed inset-x-0 bottom-0 top-9 z-50 flex justify-center",
        placement === "top" ? "items-start px-6 pb-6 pt-[12vh]" : "items-center p-6",
      )}
      style={{ background: "var(--scrim)" }}
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={labelledBy}
        aria-label={label}
        data-help={helpId}
        tabIndex={-1}
        style={style}
        className={cn(
          "w-full rounded-[var(--radius)] border border-border2 bg-surface shadow-2xl",
          heightClassName ?? "max-h-[85vh]",
          overflowClassName ?? "overflow-y-auto",
          widthClassName ?? "max-w-lg",
          className,
        )}
      >
        {children}
      </div>
    </div>
  );
}
