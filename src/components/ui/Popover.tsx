// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A small anchored popover — opens *under its trigger* and stays put for multi-select, which the
// centered/blocking Modal primitive can't do. Click-outside + Esc close it. Token-driven
// (bg-panel / border-border2); no colours of its own. Used by the calendar chrome, the context
// meter, and the retrieval-explain panel — a generic primitive, hence its home in ui/.

import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { cn } from "./cn";
import { useRestoreFocus } from "../../lib/useRestoreFocus";

/** Breathing room kept between a clipping-escaped panel and the window edge. */
const MARGIN = 8;

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
  /** Applied to the positioning root — where a caller puts its LAYOUT, since the root (not the
   *  trigger) is what the parent row lays out. */
  rootClassName?: string;
  /** Escape clipping ancestors: portal the panel to `document.body` and position it `fixed` at
   *  viewport coordinates clamped to the window, instead of `absolute` inside the trigger's box.
   *
   *  WHY THIS EXISTS: an absolute panel is clipped by any `overflow-hidden`/`overflow-auto`
   *  ancestor. A pinboard card is `overflow-hidden` and its timeline scrollers are `overflow-auto`,
   *  so the date picker inside one was cut off entirely — the panel is ~15rem wide and the column
   *  it hangs off is 5.5rem, so it rendered outside the card and vanished. `side` becomes a
   *  PREFERENCE here: the panel flips to the other side when the preferred one has no room, and
   *  clamps to the window rather than overflowing it.
   *
   *  Portal (not just `position: fixed`) because a `fixed` element is positioned against the nearest
   *  transformed/filtered ancestor rather than the viewport, so an ancestor gaining a `transform`
   *  later would silently break placement. This follows Tooltip, which portals for the same reason. */
  escapeClipping?: boolean;
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
  rootClassName,
  escapeClipping = false,
  ariaLabel,
  open: controlledOpen,
  onOpenChange,
}: Props) {
  const [internalOpen, setInternalOpen] = useState(false);
  const isControlled = controlledOpen !== undefined;
  const open = isControlled ? controlledOpen : internalOpen;
  const rootRef = useRef<HTMLDivElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  // Viewport coords for the escaped panel. Null until measured, which also hides it for that first
  // frame so it never flashes at 0,0.
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  // The element focused when we opened, so Escape can hand focus back to the trigger (not the body).
  // Shared with the calendar's own event panel, which cannot use this component (it is a singleton
  // anchored to a DOMRect, not a trigger render prop) but needs exactly this behaviour.
  const restoreFocus = useRestoreFocus(open);

  const setOpen = useCallback(
    (v: boolean) => {
      if (!isControlled) setInternalOpen(v);
      onOpenChange?.(v);
    },
    [isControlled, onOpenChange],
  );

  const close = useCallback(
    (shouldRestoreFocus: boolean) => {
      setOpen(false);
      if (shouldRestoreFocus) restoreFocus();
    },
    [setOpen, restoreFocus],
  );

  // Place the escaped panel against the trigger: clamp horizontally, honour `side`, and flip to the
  // other side when the preferred one has no room (falling back to a clamp when neither fits).
  // Layout effect, so the measure-then-place happens before the browser paints.
  //
  // RE-MEASURED ON RESIZE, not just on open. A panel's content can change height while it is open —
  // the month grid inside DateField is 4, 5 or 6 rows depending on the month, so paging months
  // changes it. Placed once from the top, a panel opened ABOVE its trigger keeps that top and grows
  // DOWNWARD over the very field it belongs to. Window resize matters for the same reason: the
  // clamp is computed against a viewport that can change under an open panel.
  useLayoutEffect(() => {
    if (!open || !escapeClipping) {
      setPos(null);
      return;
    }
    const panel = panelRef.current;
    if (!panel) return;

    const place = () => {
      const anchor = rootRef.current?.getBoundingClientRect();
      if (!anchor) return;
      const w = panel.offsetWidth;
      const h = panel.offsetHeight;
      const wanted = align === "right" ? anchor.right - w : anchor.left;
      const left = Math.max(MARGIN, Math.min(wanted, window.innerWidth - w - MARGIN));
      const above = anchor.top - 6 - h;
      const below = anchor.bottom + 6;
      const fitsAbove = above >= MARGIN;
      const fitsBelow = below + h + MARGIN <= window.innerHeight;
      const clamped = Math.max(MARGIN, window.innerHeight - h - MARGIN);
      const top =
        side === "top"
          ? fitsAbove
            ? above
            : fitsBelow
              ? below
              : clamped
          : fitsBelow
            ? below
            : fitsAbove
              ? above
              : clamped;
      // Same coords → same object-equal state, so this can't loop with the ResizeObserver.
      setPos((p) => (p && p.left === left && p.top === top ? p : { left, top }));
    };

    place();
    const ro = new ResizeObserver(place);
    ro.observe(panel);
    window.addEventListener("resize", place);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", place);
    };
  }, [open, escapeClipping, align, side]);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      // Outside click already moves focus, so don't yank it back. A portalled panel is NOT a DOM
      // descendant of the root, so it has to be tested separately or every click inside the panel
      // would read as "outside" and dismiss it.
      const t = e.target as Node;
      const inRoot = rootRef.current?.contains(t) ?? false;
      const inPanel = panelRef.current?.contains(t) ?? false;
      if (!inRoot && !inPanel) close(false);
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

  const panel = open && (
    <div
      ref={panelRef}
      role="group"
      aria-label={ariaLabel}
      className={cn(
        "min-w-[15rem] rounded-[var(--radius-sm)] border border-border2 bg-panel p-1 shadow-lg",
        // The z-index is part of the same conditional as `fixed`/`absolute`, NOT a constant above
        // it, and that is deliberate on two counts.
        //
        // Correctness: an escape-clipping panel leaves its parent's stacking context to sit in the
        // viewport, so inside a Modal (`z-50`) a `z-30` panel painted BEHIND the dialog surface —
        // the date picker in a pinboard folder set to "Overlay" opened invisibly, and the next
        // click read as an outside-dismissal. Anchored panels must stay at `z-30`: they sit within
        // their own parent and lifting them would let a popover in the page punch through overlays
        // that are meant to cover it.
        //
        // Mechanics: `cn` is a plain joiner, not tailwind-merge. Appending `z-[60]` to a constant
        // `z-30` would emit BOTH, and which one wins is decided by their order in the generated
        // stylesheet rather than in this list. Exactly one z-utility must ever be produced.
        escapeClipping
          ? // Placed from measured coords, so no directional utilities — and invisible rather than
            // unmounted for the measuring frame, since an unmounted panel has nothing to measure.
            "fixed z-[60]"
          : cn(
              "absolute z-30",
              side === "top" ? "bottom-full mb-1" : "mt-1",
              align === "right" ? "right-0" : "left-0",
            ),
        escapeClipping && !pos && "invisible",
        panelClassName,
      )}
      style={escapeClipping ? { left: pos?.left ?? 0, top: pos?.top ?? 0 } : undefined}
    >
      {typeof children === "function" ? children({ close: () => close(false) }) : children}
    </div>
  );

  return (
    <div ref={rootRef} className={cn("relative", rootClassName)}>
      {trigger({ open, toggle: () => setOpen(!open) })}
      {escapeClipping ? panel && createPortal(panel, document.body) : panel}
    </div>
  );
}
