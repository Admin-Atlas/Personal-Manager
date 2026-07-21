// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A small anchored popover — opens *under its trigger* and stays put for multi-select, which the
// centered/blocking Modal primitive can't do. Click-outside + Esc close it. Token-driven
// (bg-panel / border-border2); no colours of its own. Used by the calendar chrome, the context
// meter, and the retrieval-explain panel — a generic primitive, hence its home in ui/.

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { cn } from "./cn";

interface Props {
  /** Renders the trigger; `open` reflects state and `toggle` opens/closes. */
  trigger: (args: { open: boolean; toggle: () => void }) => ReactNode;
  /** Panel contents. A function form receives `close` so an action inside can dismiss the popover
   *  (e.g. picking a date); a plain node stays open for multi-select. */
  children: ReactNode | ((args: { close: () => void }) => ReactNode);
  /** Which edge the panel aligns to under the trigger. */
  align?: "left" | "right";
  /** Which side of the trigger the panel opens on. "bottom" (default) drops down; "top" lifts it up —
   *  for triggers pinned to the window's bottom edge, e.g. the chat composer row. */
  side?: "top" | "bottom";
  panelClassName?: string;
  /** Accessible name for the panel — it's a `group` of controls (a checkbox list or a date grid),
   *  not a `menu` of menuitems, so screen readers shouldn't enter menu-navigation mode. */
  ariaLabel?: string;
  /** Controlled open state. Pass with `onOpenChange` to drive the popover from the parent (so it can
   *  gate effects on open or close it from an action inside). Omit for self-managed open state. */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}

export function Popover({
  trigger,
  children,
  align = "left",
  side = "bottom",
  panelClassName,
  ariaLabel,
  open: controlledOpen,
  onOpenChange,
}: Props) {
  const [internalOpen, setInternalOpen] = useState(false);
  const isControlled = controlledOpen !== undefined;
  const open = isControlled ? controlledOpen : internalOpen;
  const rootRef = useRef<HTMLDivElement>(null);
  // The element focused when we opened, so Escape can hand focus back to the trigger (not the body).
  const restoreFocusRef = useRef<HTMLElement | null>(null);

  const setOpen = useCallback(
    (v: boolean) => {
      if (!isControlled) setInternalOpen(v);
      onOpenChange?.(v);
    },
    [isControlled, onOpenChange],
  );

  const close = useCallback(
    (restoreFocus: boolean) => {
      setOpen(false);
      if (restoreFocus && restoreFocusRef.current) restoreFocusRef.current.focus();
    },
    [setOpen],
  );

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
      {trigger({ open, toggle: () => setOpen(!open) })}
      {open && (
        <div
          role="group"
          aria-label={ariaLabel}
          className={cn(
            "absolute z-30 min-w-[15rem] rounded-[var(--radius-sm)] border border-border2 bg-panel p-1 shadow-lg",
            side === "top" ? "bottom-full mb-1" : "mt-1",
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
