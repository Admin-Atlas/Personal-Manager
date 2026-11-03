// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// PM's right-click menu (#1038). Bobby: "Add right click menu to pinboard / the whole app."
//
// One menu at a time, app-wide, so this is a tiny store rather than per-surface state: a surface
// calls `openContextMenu` from its own `onContextMenu`, and the one `ContextMenuHost` (mounted in
// main.tsx, so every window has it) draws it. Where no surface offers a menu, the host's fallback
// does: Copy for selected text, Open and Copy for a link, and nothing at all otherwise — never the
// browser's page menu, whose Back and Reload would throw away the app's state.
//
// Text you type into keeps the system menu (spelling suggestions, Paste), and Shift+right-click
// always gives it, anywhere: the way back to the system menu, and to Inspect in a dev build.

import { useSyncExternalStore } from "react";
import { MOD } from "./setupGuide";

export type MenuItem =
  | {
      kind?: "item";
      label: string;
      onSelect: () => void;
      disabled?: boolean;
      /** Drawn in the warning colour: a delete. */
      danger?: boolean;
      /** A shortcut shown beside the label, such as "Ctrl+Z". */
      hint?: string;
    }
  | { kind: "separator" }
  /** A row of colour choices (a note's tint), each a radio. */
  | {
      kind: "swatches";
      label: string;
      options: { name: string; color: string; active: boolean; onSelect: () => void }[];
    };

export interface OpenMenu {
  x: number;
  y: number;
  /** The menu's accessible name, such as "Note" or "Table". */
  label: string;
  items: MenuItem[];
  /** Focus before the menu opened, given back when it closes without a choice. */
  returnFocus: HTMLElement | null;
}

let current: OpenMenu | null = null;
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((l) => l());

/** Open `items` at the pointer of the `contextmenu` event `e`, in place of the system menu. */
export function openContextMenu(
  e: { clientX: number; clientY: number; preventDefault: () => void },
  label: string,
  items: MenuItem[],
): void {
  e.preventDefault();
  if (!items.some((i) => i.kind !== "separator")) return closeContextMenu();
  const active = document.activeElement;
  current = {
    x: e.clientX,
    y: e.clientY,
    label,
    items: tidy(items),
    returnFocus: active instanceof HTMLElement ? active : null,
  };
  emit();
}

export function closeContextMenu(): void {
  if (!current) return;
  current = null;
  emit();
}

export function useContextMenu(): OpenMenu | null {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => current,
  );
}

/** Separators only between groups: none at either end, never two in a row. Lets a surface build its
 *  list from optional groups without minding the joins. */
export function tidy(items: MenuItem[]): MenuItem[] {
  const out: MenuItem[] = [];
  const last = () => out[out.length - 1];
  for (const item of items) {
    if (item.kind === "separator" && (out.length === 0 || last().kind === "separator")) continue;
    out.push(item);
  }
  while (out.length && last().kind === "separator") out.pop();
  return out;
}

/** Whether right-clicking `target` should leave the system menu alone: it's in text you can type
 *  into, where the system menu has spelling suggestions and Paste. */
export function inEditable(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) return false;
  const field = target.closest("input, textarea, select, [contenteditable]");
  if (!field) return false;
  if (field instanceof HTMLInputElement) {
    // A checkbox, a button or a range is not text.
    return ["text", "search", "url", "email", "tel", "password", "number", ""].includes(field.type);
  }
  if (field.hasAttribute("contenteditable"))
    return field.getAttribute("contenteditable") !== "false";
  return true;
}

/** Whether a surface's own `onContextMenu` should open PM's menu for `e`: not when another surface
 *  already took it, not with Shift held (the system menu, on purpose), and not in editable text. */
export function wantsAppMenu(e: {
  defaultPrevented: boolean;
  shiftKey: boolean;
  target: EventTarget | null;
}): boolean {
  return !e.defaultPrevented && !e.shiftKey && !inEditable(e.target);
}

/** Where a `w`×`h` menu opened at the pointer (`x`, `y`) goes so it stays inside a `vw`×`vh`
 *  window, with `pad` px clear of each edge: flipped to the pointer's left or upward when it would
 *  run off, then pulled in. */
export function placeMenu(
  x: number,
  y: number,
  w: number,
  h: number,
  vw: number,
  vh: number,
  pad = 4,
): { left: number; top: number } {
  let left = x + w + pad > vw ? x - w : x;
  let top = y + h + pad > vh ? y - h : y;
  left = Math.max(pad, Math.min(left, vw - w - pad));
  top = Math.max(pad, Math.min(top, vh - h - pad));
  return { left, top };
}

/** The fallback menu for a right-click no surface claimed: a link's Open and Copy, and Copy for
 *  selected text. Empty means no menu at all. */
export function fallbackItems(
  target: EventTarget | null,
  selection: string,
  actions: { openUrl: (url: string) => void; copy: (text: string) => void },
): MenuItem[] {
  const items: MenuItem[] = [];
  const link =
    target instanceof Element ? target.closest<HTMLAnchorElement>("a[href]")?.href : undefined;
  if (link && /^https?:\/\//i.test(link)) {
    items.push(
      { label: "Open link", onSelect: () => actions.openUrl(link) },
      { label: "Copy link", onSelect: () => actions.copy(link) },
      { kind: "separator" },
    );
  }
  if (selection.trim())
    items.push({ label: "Copy", hint: `${MOD}C`, onSelect: () => actions.copy(selection) });
  return tidy(items);
}
