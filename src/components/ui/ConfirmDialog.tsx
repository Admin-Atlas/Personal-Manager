// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// An approval / confirm overlay built on Dialog (DESIGN_TOKENS.md §7 — the design's
// Approval pattern): a title, a preview of what will happen (children), and
// Confirm / Cancel. Gate irreversible actions (rebuild index, disconnect, remove) on
// this so a single click can't quietly throw work away. `danger` tints the confirm
// for destructive actions; while `busy`, the dialog can't be dismissed by Esc/scrim.
//
// It is now a PRESET over `Dialog`'s card chrome, not a second copy of it — the heading recipe,
// the `p-5`, the `mt-5` footer row and the heading-id wiring all live in `Dialog` and are written
// once. What is left here is the only thing this component actually decides: that a confirmation
// is a title, a body and exactly two buttons in that order, and that `busy` disables both while
// blocking dismissal. `danger` colours the CONFIRM BUTTON, not the heading — the destructive thing
// is the action, and tinting the title as well would double-count it.

import { type ReactNode } from "react";
import { Button } from "./Button";
import { Dialog } from "./Dialog";

export interface ConfirmDialogProps {
  open: boolean;
  title: string;
  /** What will happen — the preview/explanation shown above the buttons. */
  children?: ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
  /** Style the confirm as destructive (uses the danger role) for irreversible actions. */
  danger?: boolean;
  /** Disable the buttons + block dismissal while the action runs. */
  busy?: boolean;
  onConfirm: () => void;
  onClose: () => void;
}

export function ConfirmDialog({
  open,
  title,
  children,
  confirmLabel = "Confirm",
  cancelLabel = "Cancel",
  danger,
  busy,
  onConfirm,
  onClose,
}: ConfirmDialogProps) {
  return (
    <Dialog
      open={open}
      onClose={busy ? () => {} : onClose}
      title={title}
      widthClassName="max-w-md"
      footer={
        <>
          <Button variant="tertiary" onClick={onClose} disabled={busy}>
            {cancelLabel}
          </Button>
          <Button variant={danger ? "danger" : "primary"} onClick={onConfirm} disabled={busy}>
            {busy ? "Working…" : confirmLabel}
          </Button>
        </>
      }
    >
      {children != null && <div className="mt-2 text-sm leading-relaxed text-ink3">{children}</div>}
    </Dialog>
  );
}
