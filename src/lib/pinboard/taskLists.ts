// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

/** Pure decisions for the pinboard's Task list cards (PA-1): where a note's conversion puts its card,
 *  what deleting a card costs, and which keys inside a card keep their own native undo. No React — the
 *  view calls these, the tests read them directly. */

import type { Widget } from "./types";

/** What "Make this a task list" does on the board once its tasks are in:
 *  - `reuse` — a top-level card with the same filter is already there, so that card is shown (scrolled
 *    to and flashed) and the board is not changed at all. Converting the same note twice used to add
 *    an identical card every time.
 *  - `add` — a new card goes beside the note. It is a history `barrier` only when tasks were actually
 *    written (see `addTaskList`): a conversion that found every item "already there" wrote nothing,
 *    so it is an ordinary `push` step and leaves the board's undo history alone. */
export type TaskListPlacement = { reuse: string } | { add: "barrier" | "push" };

/** The card a conversion into `project` (`null` → No project) would show: a task list filtered to that
 *  project, or the All tasks list (no filter) for No project. A card whose project was deleted
 *  (`project === null` on the widget) never matches — it shows nothing until the user re-points it.
 *  Top level only: a task list is never filed into a folder (`isFileable`). */
export function matchingTaskList(widgets: Widget[], project: string | null): Widget | undefined {
  return widgets.find(
    (w) =>
      w.kind === "tasklist" && (project === null ? w.project === undefined : w.project === project),
  );
}

export function placeConvertedTaskList(
  widgets: Widget[],
  project: string | null,
  created: number,
): TaskListPlacement {
  const existing = matchingTaskList(widgets, project);
  if (existing) return { reuse: existing.id };
  return { add: created > 0 ? "barrier" : "push" };
}

/** The delete confirmation's line for a task list card: only the card goes, and where the tasks are
 *  still to be found. A project's list names that project's page. The All tasks list (`undefined`)
 *  spans many projects and tasks with none, so it points at the Runway; and a list whose project was
 *  deleted (`null`) has no project page left to point at — its tasks lost their project with it. */
export function taskListDeleteCost(project: string | null | undefined): string {
  if (typeof project === "string") {
    return `Only the card goes — your tasks stay, on ${project}'s project page and the calendar's Runway.`;
  }
  if (project === null) {
    return "Only the card goes. Its project was deleted, so those tasks have no project now — they stay on the calendar's Runway.";
  }
  return "Only the card goes — your tasks stay, on the calendar's Runway.";
}

/** Input types that hold typed text, and so have a native undo of their own. A checkbox, a button or a
 *  range has none: Ctrl+Z there should still undo the board. */
const TEXT_INPUT_TYPES: ReadonlySet<string> = new Set([
  "text",
  "search",
  "url",
  "tel",
  "email",
  "password",
  "number",
]);

/** Whether Ctrl+Z (or a `historyUndo` beforeinput) on `target` belongs to the field rather than to the
 *  board: it is a text field — an `<input>` that takes text, a `<textarea>` or a contenteditable —
 *  inside a `[data-native-undo]` region, such as a task list's add row and inline edit fields, whose
 *  typing reaches the board's history nowhere (tasks are backend rows). Anything else in that region —
 *  a tick, a row button, a fold toggle — has no undo of its own, so the board keeps the shortcut. */
export function keepsNativeUndo(target: EventTarget | null): boolean {
  if (!(target instanceof Element) || !target.closest("[data-native-undo]")) return false;
  if (target instanceof HTMLTextAreaElement) return true;
  if (target instanceof HTMLInputElement) return TEXT_INPUT_TYPES.has(target.type);
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  // jsdom has no `isContentEditable`, so the nearest attribute decides there.
  const ce = target.closest("[contenteditable]")?.getAttribute("contenteditable");
  return ce != null && ce !== "false";
}
