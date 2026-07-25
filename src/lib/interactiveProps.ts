// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Make a non-<button> element behave like a button for keyboard users: role="button", tabIndex, and
// Enter/Space activation. Codifies the pattern already used by the calendar event chips
// (calendar/parts/EventChip.tsx) so a clickable <div>/<li>/<tr> isn't mouse-only (WCAG 2.1.1). Prefer
// a real <button>/<a> where the markup allows; reach for this only where a native control can't be
// used (a whole table row, a list item that itself contains other controls). Pure — unit-testable.

import type { KeyboardEvent } from "react";

export interface InteractiveProps {
  role: "button";
  tabIndex: 0;
  onClick: () => void;
  onKeyDown: (e: KeyboardEvent<HTMLElement>) => void;
}

export function interactiveProps(onActivate: () => void): InteractiveProps {
  return {
    role: "button",
    tabIndex: 0,
    onClick: onActivate,
    onKeyDown: (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        onActivate();
      }
    },
  };
}
