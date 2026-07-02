// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A small anchored popover — the calendar chrome needs a dropdown that opens *under its trigger* and
// stays put for multi-select, which the centered/blocking Modal primitive can't do. Click-outside +
// Esc close it. Token-driven (bg-panel / border-border2); no colours of its own.

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { cn } from "../ui";

interface Props {
  /** Renders the trigger; `open` reflects state and `toggle` opens/closes. */
  trigger: (args: { open: boolean; toggle: () => void }) => ReactNode;
  /** Panel contents. A function form receives `close` so an action inside can dismiss the popover
   *  (e.g. picking a date); a plain node stays open for multi-select. */
  children: ReactNode | ((args: { close: () => void }) => ReactNode);
  /** Which edge the panel aligns to under the trigger. */
  align?: "left" | "right";
  panelClassName?: string;
  /** Accessible name for the panel — it's a `group` of controls (a checkbox list or a date grid),
   *  not a `menu` of menuitems, so screen readers shouldn't enter menu-navigation mode. */
  ariaLabel?: string;
}

export function Popover({ trigger, children, align = "left", panelClassName, ariaLabel }: Props) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  // The element focused when we opened, so Escape can hand focus back to the trigger (not the body).
  const restoreFocusRef = useRef<HTMLElement | null>(null);

  const close = useCallback((restoreFocus: boolean) => {
    setOpen(false);
    if (restoreFocus && restoreFocusRef.current) restoreFocusRef.current.focus();
  }, []);

  useEffect(() => {
    if (!open) return;
    restoreFocusRef.current = document.activeElement as HTMLElement | null;
    const onDown = (e: MouseEvent) => {
      // Outside click already moves focus, so don't yank it back.
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) close(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close(true);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open, close]);

  return (
    <div ref={rootRef} className="relative">
      {trigger({ open, toggle: () => setOpen((v) => !v) })}
      {open && (
        <div
          role="group"
          aria-label={ariaLabel}
          className={cn(
            "absolute z-30 mt-1 min-w-[15rem] rounded-[var(--radius-sm)] border border-border2 bg-panel p-1 shadow-lg",
            align === "right" ? "right-0" : "left-0",
            panelClassName,
          )}
        >
          {typeof children === "function" ? children({ close: () => close(false) }) : children}
        </div>
      )}
    </div>
  );
}
