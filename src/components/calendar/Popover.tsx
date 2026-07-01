// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A small anchored popover — the calendar chrome needs a dropdown that opens *under its trigger* and
// stays put for multi-select, which the centered/blocking Modal primitive can't do. Click-outside +
// Esc close it. Token-driven (bg-panel / border-border2); no colours of its own.

import { useEffect, useRef, useState, type ReactNode } from "react";
import { cn } from "../ui";

interface Props {
  /** Renders the trigger; `open` reflects state and `toggle` opens/closes. */
  trigger: (args: { open: boolean; toggle: () => void }) => ReactNode;
  /** Panel contents. */
  children: ReactNode;
  /** Which edge the panel aligns to under the trigger. */
  align?: "left" | "right";
  panelClassName?: string;
}

export function Popover({ trigger, children, align = "left", panelClassName }: Props) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div ref={rootRef} className="relative">
      {trigger({ open, toggle: () => setOpen((v) => !v) })}
      {open && (
        <div
          role="menu"
          className={cn(
            "absolute z-30 mt-1 min-w-[15rem] rounded-[var(--radius-sm)] border border-border2 bg-panel p-1 shadow-lg",
            align === "right" ? "right-0" : "left-0",
            panelClassName,
          )}
        >
          {children}
        </div>
      )}
    </div>
  );
}
