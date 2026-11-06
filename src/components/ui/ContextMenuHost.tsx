// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Draws PM's right-click menu (lib/contextMenu) and answers every right-click no surface claimed.
// Mounted once per window, in main.tsx.
//
// The fallback listens on the document in the bubble phase, so it runs after React has delivered the
// event to every surface: a surface that opened its own menu has called preventDefault by then, and
// is left alone. Its Open link says in the link notice when the link didn't open (lib/linkFailure).

import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import { createPortal } from "react-dom";
import {
  closeContextMenu,
  fallbackItems,
  inEditable,
  openContextMenu,
  placeMenu,
  useContextMenu,
  type MenuItem,
} from "../../lib/contextMenu";
import { openLink } from "../../lib/linkFailure";

/** A shortcut as the menu shows it ("Ctrl+Z", "⌘⇧Z") in the form `aria-keyshortcuts` reads
 *  ("Control+Z", "Meta+Shift+Z"). */
const keyShortcut = (hint: string) =>
  hint.replace("Ctrl+", "Control+").replace("⌘", "Meta+").replace("⇧", "Shift+");

const copy = (text: string) => void navigator.clipboard?.writeText(text).catch(() => {});

export function ContextMenuHost() {
  const menu = useContextMenu();

  useEffect(() => {
    const onMenu = (e: MouseEvent) => {
      if (e.defaultPrevented || e.shiftKey || inEditable(e.target)) return;
      const items = fallbackItems(e.target, window.getSelection()?.toString() ?? "", {
        openUrl: openLink,
        copy,
      });
      // With nothing to offer, still no system menu: its Back and Reload would reset PM.
      openContextMenu(e, "Menu", items);
    };
    document.addEventListener("contextmenu", onMenu);
    return () => document.removeEventListener("contextmenu", onMenu);
  }, []);

  if (!menu) return null;
  // Keyed on the open, so each opening starts fresh (focus, placement) even at the same spot.
  return createPortal(<Menu key={`${menu.x},${menu.y},${menu.label}`} />, document.body);
}

function Menu() {
  const menu = useContextMenu()!;
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);

  // Measured before paint, so it never shows in the wrong place first.
  useLayoutEffect(() => {
    const el = ref.current!;
    const r = el.getBoundingClientRect();
    setPos(placeMenu(menu.x, menu.y, r.width, r.height, window.innerWidth, window.innerHeight));
    // The menu itself, not its first item: the arrow keys work at once, and a menu opened with the
    // mouse doesn't start with a row lit up that the pointer never touched.
    el.focus();
  }, [menu.x, menu.y]);

  // Anything else ends it: a press outside, the window losing focus or resizing, a scroll.
  useEffect(() => {
    const onDown = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node)) closeContextMenu();
    };
    const close = () => closeContextMenu();
    const onScroll = (e: Event) => {
      if (!ref.current?.contains(e.target as Node)) closeContextMenu();
    };
    document.addEventListener("pointerdown", onDown, true);
    window.addEventListener("blur", close);
    window.addEventListener("resize", close);
    document.addEventListener("scroll", onScroll, true);
    return () => {
      document.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("blur", close);
      window.removeEventListener("resize", close);
      document.removeEventListener("scroll", onScroll, true);
    };
  }, []);

  const choose = (onSelect: () => void) => {
    closeContextMenu();
    onSelect();
  };

  function onKeyDown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      closeContextMenu();
      menu.returnFocus?.focus();
      return;
    }
    if (e.key === "Tab") {
      e.preventDefault();
      closeContextMenu();
      menu.returnFocus?.focus();
      return;
    }
    const step =
      e.key === "ArrowDown" || e.key === "ArrowRight"
        ? 1
        : e.key === "ArrowUp" || e.key === "ArrowLeft"
          ? -1
          : 0;
    const items = [
      ...ref.current!.querySelectorAll<HTMLElement>("[role^=menuitem]:not(:disabled)"),
    ];
    if (!items.length) return;
    const at = items.indexOf(document.activeElement as HTMLElement);
    if (step) {
      e.preventDefault();
      // From the menu itself (nothing chosen yet), Down starts at the top and Up at the bottom.
      const next = at < 0 ? (step > 0 ? 0 : items.length - 1) : at + step;
      items[(next + items.length) % items.length].focus();
    } else if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      items[e.key === "Home" ? 0 : items.length - 1].focus();
    }
  }

  return (
    <div
      ref={ref}
      role="menu"
      tabIndex={-1}
      aria-label={menu.label}
      // A press inside is a press inside whatever opened it: the pinboard note keeps its edit going
      // through a press in any popover panel (NoteBody's outside-press rule).
      data-popover-panel=""
      onKeyDown={onKeyDown}
      onContextMenu={(e) => e.preventDefault()}
      // Keep the selection and the caret where they were, so Copy and the table changes act on them.
      onMouseDown={(e) => e.preventDefault()}
      className="fixed z-[70] min-w-[11rem] outline-none max-w-[18rem] rounded-[var(--radius-sm)] border border-border2 bg-panel p-1 text-[0.8125rem] shadow-lg"
      style={pos ?? { left: menu.x, top: menu.y, visibility: "hidden" }}
    >
      {menu.items.map((item, i) => (
        <Row key={i} item={item} choose={choose} />
      ))}
    </div>
  );
}

function Row({ item, choose }: { item: MenuItem; choose: (f: () => void) => void }) {
  if (item.kind === "separator")
    return <div role="separator" className="my-1 border-t border-[var(--rule)]" />;
  if (item.kind === "swatches")
    return (
      <div role="group" aria-label={item.label} className="flex items-center gap-1.5 px-2 py-1.5">
        <span className="mr-auto text-ink3">{item.label}</span>
        {item.options.map((o) => (
          <button
            key={o.color}
            type="button"
            role="menuitemradio"
            aria-checked={o.active}
            aria-label={o.name}
            title={o.name}
            onClick={() => choose(o.onSelect)}
            className={`h-4 w-4 rounded-full border focus:outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)] ${
              o.active ? "ring-1 ring-ink3 ring-offset-1 ring-offset-[var(--panel)]" : ""
            }`}
            style={{
              background: `var(--${o.color})`,
              borderColor: "color-mix(in oklab, var(--ink) 20%, transparent)",
            }}
          />
        ))}
      </div>
    );
  return (
    <button
      type="button"
      role="menuitem"
      disabled={item.disabled}
      aria-keyshortcuts={item.hint ? keyShortcut(item.hint) : undefined}
      onClick={() => choose(item.onSelect)}
      className={`flex w-full items-center gap-4 rounded-[var(--radius-sm)] px-2 py-1 text-left hover:bg-surface focus:bg-surface focus:outline-none disabled:opacity-50 disabled:hover:bg-transparent ${
        item.danger ? "text-st-due" : "text-ink2"
      }`}
    >
      <span className="min-w-0 flex-1 truncate">{item.label}</span>
      {/* The shortcut is for the eye; its key is named in `aria-keyshortcuts` instead, so the item's
          name stays "Undo" rather than "UndoCtrl+Z". */}
      {item.hint && (
        <span aria-hidden className="shrink-0 font-mono text-[0.6875rem] text-ink4">
          {item.hint}
        </span>
      )}
    </button>
  );
}
