// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A blocking modal over a fixed scrim (DESIGN_TOKENS.md §7). Esc and scrim-click close. The
// scrim tint is the one intentionally-fixed colour in the system (a neutral darkening that reads
// the same under any accent). Consolidates the app's ad-hoc bg-neutral-950/80 overlays and backs
// the design's Approval / Permission patterns.

import { useEffect, type ReactNode } from "react";
import { cn } from "./cn";

export interface ModalProps {
  open: boolean;
  onClose: () => void;
  children: ReactNode;
  /** Width class for the dialog (default max-w-lg). */
  widthClassName?: string;
  className?: string;
  labelledBy?: string;
}

export function Modal({
  open,
  onClose,
  children,
  widthClassName,
  className,
  labelledBy,
}: ModalProps) {
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;

  return (
    // Start below the custom title bar (top-9 = its h-9) so the frameless window's drag region and
    // min/max/close controls stay visible and clickable while a modal is open — same convention as
    // DocumentReader. The scrim never covers the top chrome.
    <div
      className="fixed inset-x-0 bottom-0 top-9 z-50 flex items-center justify-center p-6"
      style={{ background: "rgba(8,6,4,0.5)" }}
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby={labelledBy}
        className={cn(
          "max-h-[85vh] w-full overflow-y-auto rounded-[var(--radius)] border border-border2 bg-surface shadow-2xl",
          widthClassName ?? "max-w-lg",
          className,
        )}
      >
        {children}
      </div>
    </div>
  );
}
