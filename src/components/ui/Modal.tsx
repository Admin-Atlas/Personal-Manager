// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A blocking modal over a fixed scrim (DESIGN_TOKENS.md §7). Esc and scrim-click close. The
// scrim tint is the one intentionally-fixed colour in the system (a neutral darkening that reads
// the same under any accent). Consolidates the app's ad-hoc bg-neutral-950/80 overlays and backs
// the design's Approval / Permission patterns.

import { useEffect, useRef, type CSSProperties, type ReactNode } from "react";
import { cn } from "./cn";
import { useFocusTrap } from "../../lib/useFocusTrap";

export interface ModalProps {
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
  labelledBy?: string;
}

export function Modal({
  open,
  onClose,
  children,
  widthClassName,
  heightClassName,
  overflowClassName,
  style,
  className,
  labelledBy,
}: ModalProps) {
  const dialogRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  // Trap Tab inside the dialog, focus into it on open, and restore focus to the opener on close —
  // fixes every dialog (ConfirmDialog, MoveConversationDialog, …) at once.
  useFocusTrap(open, dialogRef);

  if (!open) return null;

  return (
    // Start below the custom title bar (top-9 = its h-9) so the frameless window's drag region and
    // min/max/close controls stay visible and clickable while a modal is open — same convention as
    // DocumentReader. The scrim never covers the top chrome.
    <div
      className="fixed inset-x-0 bottom-0 top-9 z-50 flex items-center justify-center p-6"
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
