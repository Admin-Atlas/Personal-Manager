// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// An approval / confirm overlay built on Modal (DESIGN_TOKENS.md §7 — the design's
// Approval pattern): a title, a preview of what will happen (children), and
// Confirm / Cancel. Gate irreversible actions (rebuild index, disconnect, remove) on
// this so a single click can't quietly throw work away. `danger` tints the confirm
// for destructive actions; while `busy`, the dialog can't be dismissed by Esc/scrim.

import { useId, type ReactNode } from "react";
import { Button } from "./Button";
import { Modal } from "./Modal";

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
  const titleId = useId();
  return (
    <Modal
      open={open}
      onClose={busy ? () => {} : onClose}
      labelledBy={titleId}
      widthClassName="max-w-md"
    >
      <div className="p-5">
        <h2 id={titleId} className="font-head text-base font-semibold text-ink">
          {title}
        </h2>
        {children != null && (
          <div className="mt-2 text-sm leading-relaxed text-ink3">{children}</div>
        )}
        <div className="mt-5 flex justify-end gap-2">
          <Button variant="tertiary" onClick={onClose} disabled={busy}>
            {cancelLabel}
          </Button>
          <Button
            variant="primary"
            onClick={onConfirm}
            disabled={busy}
            style={
              danger && !busy
                ? {
                    background: "color-mix(in oklab, var(--st-due) 15%, transparent)",
                    color: "var(--st-due)",
                  }
                : undefined
            }
          >
            {busy ? "Working…" : confirmLabel}
          </Button>
        </div>
      </div>
    </Modal>
  );
}
