// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A small hover/focus tooltip. The app otherwise uses the native `title` attribute, which is fine
// almost everywhere — but WebView2 does NOT reliably show a `title` on a control while a <textarea>
// has focus, which is exactly the pinboard note's formatting toolbar (and tint dots) while editing.
// This JS tooltip guarantees the label + shortcut shows there. It renders through a portal to
// document.body so the note tile's `overflow-hidden` can never clip it, and is pointer-events-none
// so it never gets in the way of the control it describes.

import { useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";

export interface TooltipProps {
  /** The text to show (e.g. `Bullet list · Ctrl+Shift+8`). */
  label: string;
  children: ReactNode;
}

export function Tooltip({ label, children }: TooltipProps) {
  const [pos, setPos] = useState<{ x: number; y: number } | null>(null);
  const ref = useRef<HTMLSpanElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  const show = () => {
    timer.current = setTimeout(() => {
      const r = ref.current?.getBoundingClientRect();
      if (r) setPos({ x: r.left + r.width / 2, y: r.top - 6 });
    }, 120);
  };
  const hide = () => {
    clearTimeout(timer.current);
    setPos(null);
  };

  return (
    <span
      ref={ref}
      className="inline-flex"
      onPointerEnter={show}
      onPointerLeave={hide}
      onPointerDown={hide}
      onFocusCapture={show}
      onBlurCapture={hide}
    >
      {children}
      {pos &&
        createPortal(
          <span
            role="tooltip"
            style={{ left: pos.x, top: pos.y }}
            className="pointer-events-none fixed z-[60] -translate-x-1/2 -translate-y-full whitespace-nowrap rounded-[var(--radius-sm)] border border-border2 bg-panel px-1.5 py-0.5 text-[0.625rem] text-ink2 shadow-md"
          >
            {label}
          </span>,
          document.body,
        )}
    </span>
  );
}
