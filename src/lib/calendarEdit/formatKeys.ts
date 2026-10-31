// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The event description's formatting buttons and their shortcuts (#884, C19 §7.2-§7.3), one table so
// a button's tooltip, its name for assistive technology and the key that runs it can't drift apart
// (as the Pinboard note's `formatForKey` sits beside its buttons). Google Calendar's seven, in the
// order research F12 and the Pinboard give (probe E11-0 records Google's own).

import { IS_MAC, MOD, SHIFT } from "../setupGuide";

export type FormatKeyName =
  "bold" | "italic" | "underline" | "bulleted" | "numbered" | "link" | "clear";

export interface FormatButton {
  readonly key: FormatKeyName;
  readonly label: string;
  /** The tooltip's shortcut, in the platform's glyphs ("Ctrl+B", "⌘B"). */
  readonly hint: string;
  /** The same shortcut as `aria-keyshortcuts` names it ("Control+B", "Meta+B"). */
  readonly keys: string;
  /** It shows pressed while the selection has it. Link says whether its panel is open instead, and
   *  Remove formatting is never on. */
  readonly toggle: boolean;
}

const ARIA_MOD = IS_MAC ? "Meta" : "Control";

export const FORMAT_BUTTONS: readonly FormatButton[] = [
  { key: "bold", label: "Bold", hint: `${MOD}B`, keys: `${ARIA_MOD}+B`, toggle: true },
  { key: "italic", label: "Italic", hint: `${MOD}I`, keys: `${ARIA_MOD}+I`, toggle: true },
  { key: "underline", label: "Underline", hint: `${MOD}U`, keys: `${ARIA_MOD}+U`, toggle: true },
  {
    key: "bulleted",
    label: "Bulleted list",
    hint: `${MOD}${SHIFT}8`,
    keys: `${ARIA_MOD}+Shift+8`,
    toggle: true,
  },
  {
    key: "numbered",
    label: "Numbered list",
    hint: `${MOD}${SHIFT}7`,
    keys: `${ARIA_MOD}+Shift+7`,
    toggle: true,
  },
  { key: "link", label: "Link", hint: `${MOD}K`, keys: `${ARIA_MOD}+K`, toggle: false },
  {
    key: "clear",
    label: "Remove formatting",
    hint: `${MOD}\\`,
    keys: `${ARIA_MOD}+\\`,
    toggle: false,
  },
];

/** A keydown's formatting key, or null. On a Mac only ⌘ formats: Ctrl+B, Ctrl+I and Ctrl+K are the
 *  system's own text keys there (back a character, and so on). Elsewhere only Ctrl. Never with Alt.
 *  `code`, not `key`, so any keyboard layout reaches them. */
export function formatKey(
  e: Pick<KeyboardEvent, "ctrlKey" | "metaKey" | "altKey" | "shiftKey" | "code">,
  mac: boolean = IS_MAC,
): FormatKeyName | null {
  const mod = mac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
  if (!mod || e.altKey) return null;
  if (e.shiftKey) {
    if (e.code === "Digit8") return "bulleted";
    if (e.code === "Digit7") return "numbered";
    return null;
  }
  switch (e.code) {
    case "KeyB":
      return "bold";
    case "KeyI":
      return "italic";
    case "KeyU":
      return "underline";
    case "KeyK":
      return "link";
    case "Backslash":
      return "clear";
    default:
      return null;
  }
}
