// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The words and rules of the "which occurrences?" dialog for a repeating event (#884), as data:
// the three rows and what each says, what is picked when it opens, the earlier change PM couldn't
// finish that has to be settled first, and every warning a row carries. Pure, so it's tested
// without a screen, and it sends nothing: the backend planned every row, and decides again at Save.

import { formatDateOnly } from "../format";
import type {
  CalendarEvent,
  GuestNotice,
  ScopeRow,
  SeriesChoices,
  SeriesScope,
  SeriesWarning,
  SplitAction,
} from "../types";
import { conflictFields, listFields, outcomeText, type ChoiceKind } from "./editReasons";

export type { ChoiceKind } from "./editReasons";
export type ReadyChoices = Extract<SeriesChoices, { outcome: "ready" }>;
/** A row the user picked, with the token the write must name (release builds require it). */
export interface Picked {
  scope: SeriesScope;
  token: string;
  /** The row was "this and following" from the first occurrence (`RowView.asAll`): what saves is
   *  every occurrence's plan, so the notice after it speaks as All. Never sent: the write names
   *  `scope` and `token` only. */
  asAll?: boolean;
}

function unreachable(value: never): never {
  throw new Error(`unhandled series choice value: ${JSON.stringify(value)}`);
}

const quote = (title: string) => `“${title.trim() || "(no title)"}”`;

/** Whether a mirror row is an occurrence of a repeating event (the trigger for asking first). Google
 *  rows are recurring by their series id; `recurring` covers a row read before `series_id` existed. */
export function isRepeating(row: Pick<CalendarEvent, "series_id" | "recurring">): boolean {
  return Boolean(row.series_id) || Boolean(row.recurring);
}

export interface RowView {
  scope: SeriesScope;
  /** "This event" | "This and following events" | "All events", as Google Calendar labels them. */
  label: string;
  /** One sentence: what it does, or why not. */
  detail: string;
  /** Offered, or "this and following" from the first occurrence (every occurrence). */
  selectable: boolean;
  /** "This and following" from the first occurrence: its warnings are every occurrence's plan's,
   *  and are worded as All's. */
  asAll: boolean;
  /** The row's token when selectable. */
  token: string | null;
  warnings: SeriesWarning[];
  /** Who Google would email with Send, when anyone (C12): choosing the row asks it. */
  guests: GuestNotice | null;
}

const SCOPES: readonly SeriesScope[] = ["this", "following", "all"];

const LABEL: Record<SeriesScope, string> = {
  this: "This event",
  following: "This and following events",
  all: "All events",
};

/** What an offered row does. `day`: the occurrence's day as the user sees it (DD-MM), for a
 *  delete's "This event". Only a "this and following" delete is said to skip Google's Trash: that
 *  one is documented, the others aren't proved for deletes PM sends. */
function offeredDetail(scope: SeriesScope, asAll: boolean, kind: ChoiceKind, day?: string): string {
  switch (scope) {
    case "this":
      return kind === "delete" && day ? `Only the one on ${day}.` : "Only this occurrence.";
    case "following":
      if (asAll) {
        return kind === "save"
          ? "This is the first occurrence, so this changes every occurrence, like All events."
          : "This is the first occurrence, so this deletes every occurrence, like All events.";
      }
      return kind === "save"
        ? "This occurrence and every one after it. Earlier ones stay as they are."
        : "This occurrence and every one after it. Google Calendar can't restore these from its Trash.";
    case "all":
      return "Every occurrence, earlier ones too.";
    default:
      return unreachable(scope);
  }
}

function rowView(scope: SeriesScope, row: ScopeRow, kind: ChoiceKind, day?: string): RowView {
  const base = { scope, label: LABEL[scope], asAll: false, guests: null };
  switch (row.row) {
    case "offered":
    case "as_all":
      return {
        ...base,
        detail: offeredDetail(scope, row.row === "as_all", kind, day),
        selectable: true,
        asAll: row.row === "as_all",
        token: row.token,
        warnings: row.warnings,
        guests: row.guests ?? null,
      };
    case "refused":
      return { ...base, detail: row.message, selectable: false, token: null, warnings: [] };
    case "conflict": {
      const fields = conflictFields(row.fields);
      const what = fields.length ? ` (the ${listFields(fields)})` : "";
      return {
        ...base,
        detail: `Changed in Google since you opened it${what}.`,
        selectable: false,
        token: null,
        warnings: [],
      };
    }
    case "pending":
      return {
        ...base,
        detail: "Not until the earlier change below is settled.",
        selectable: false,
        token: null,
        warnings: [],
      };
    default:
      return unreachable(row);
  }
}

/** The three rows in order (this, following, all), as the dialog draws them. `day`: the
 *  occurrence's day (DD-MM), which a delete's "This event" names. */
export function rowsOf(choices: ReadyChoices, kind: ChoiceKind, day?: string): RowView[] {
  return SCOPES.map((scope) => rowView(scope, choices[scope], kind, day));
}

/** "This event" when it's offered (Q5); otherwise nothing (Bobby 17-10 #1: a change Google applies
 *  series-wide, e.g. visibility, pre-selects nothing and Save waits for a pick). */
export function defaultScope(choices: ReadyChoices): SeriesScope | null {
  return choices.this.row === "offered" ? "this" : null;
}

/** A pending row's change to settle first, once per dialog: every pending row shares one token. */
export function pendingOf(
  choices: ReadyChoices,
): { token: string; offers: SplitAction[]; from: string } | null {
  for (const scope of SCOPES) {
    const row = choices[scope];
    if (row.row === "pending") return { token: row.token, offers: row.offers, from: row.from };
  }
  return null;
}

/** The settle block's words, by what Google allows now: never a hint to delete unless Remove is
 *  offered, and never "lost touch" for Check alone, which a new part changed in Google also leaves. */
export function pendingText(offers: readonly SplitAction[], from: string): string {
  const day = formatDateOnly(from);
  const finish = offers.includes("finish");
  const remove = offers.includes("remove");
  if (finish && remove) {
    return `PM couldn't finish an earlier change to this repeating event: it may show twice from ${day}. Finish it or remove the new part first.`;
  }
  if (finish) {
    return `An earlier change from ${day} is saved, but PM couldn't finish moving the occurrences you changed on their own. Finish it first.`;
  }
  if (remove) {
    return `PM couldn't finish an earlier change to this repeating event: it may show twice from ${day}. Remove the new part, or check again.`;
  }
  return `PM couldn't finish an earlier change to this repeating event from ${day}. Check again; if it still can't, look at that day in Google Calendar.`;
}

/** Any row Google changed since the editor opened (the editor rebases instead of asking): the
 *  fields of every conflict row, or null when there are none. An empty list still counts. */
export function conflictOf(choices: ReadyChoices): string[] | null {
  let fields: string[] | null = null;
  for (const scope of SCOPES) {
    const row = choices[scope];
    if (row.row !== "conflict") continue;
    const union: string[] = fields ?? [];
    for (const f of row.fields) if (!union.includes(f)) union.push(f);
    fields = union;
  }
  return fields;
}

/** The re-ask's line: the plan changed between the choice and the write, so nothing was sent. */
export function againText(kind: ChoiceKind): string {
  return kind === "save"
    ? outcomeText({ outcome: "needs_choice", scope: true }).text
    : "This repeating event changed since you chose which occurrences to delete. Choose again; nothing was deleted.";
}

const WARNING_ORDER: readonly SeriesWarning["warning"][] = [
  "repeat_day_changes",
  "becomes_one_event",
  "start_moved",
  "changes_lost",
  "length_changes",
  "exceptions_may_reset",
  "not_carried",
  "new_identity",
  "split_sibling",
];

/** The selected row's lines, the question first; equal kinds keep the backend's order. A warning
 *  with nothing to say (a "not carried" naming nothing) has no line. Each is worded by the plan the
 *  row was made from: a "this and following" row that changes every occurrence speaks as All. */
export function warningLines(row: RowView, kind: ChoiceKind, title: string): string[] {
  const rank = (w: SeriesWarning) => WARNING_ORDER.indexOf(w.warning);
  const scope: SeriesScope = row.asAll ? "all" : row.scope;
  return [...row.warnings]
    .map((w, i) => ({ w, i }))
    .sort((a, b) => rank(a.w) - rank(b.w) || a.i - b.i)
    .map(({ w }) => seriesWarningText(w, kind, scope, title))
    .filter((line) => line !== "");
}

/** "a, b and c". */
function andList(items: readonly string[]): string {
  if (items.length <= 1) return items.join("");
  return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

/** One warning as a sentence the user reads before choosing. The "may keep or lose" hedges never
 *  claim to know what Google does with occurrences changed on their own. */
export function seriesWarningText(
  w: SeriesWarning,
  kind: ChoiceKind,
  scope: SeriesScope,
  title: string,
): string {
  const t = quote(title);
  switch (w.warning) {
    case "repeat_day_changes":
      return repeatDayQuestion(title, w, scope);
    case "start_moved":
      // From one occurrence on, only a new repeat moves where the new part starts (C11).
      return scope === "following"
        ? `From this occurrence on, the new part of ${t} starts on ${formatDateOnly(w.to)}, the first day the new repeat allows.`
        : `The first occurrence of ${t} moves from ${formatDateOnly(w.from)} to ${formatDateOnly(w.to)}.`;
    case "becomes_one_event":
      // "Does not repeat" (C11): never "a new repeating event" (C11 critique 2 M2).
      return scope === "following"
        ? `From this occurrence on, ${t} becomes a separate event that doesn't repeat: this occurrence stays, and every one after it goes.`
        : `Only the occurrence on ${formatDateOnly(w.day)} stays, as an event that doesn't repeat. Every other occurrence of ${t} goes.`;
    case "changes_lost":
      return w.count === 1
        ? `One occurrence of ${t} you changed on its own falls on a day the new repeat skips, so that change goes.`
        : `${w.count} occurrences of ${t} you changed on their own fall on days the new repeat skips, so those changes go.`;
    case "length_changes":
      return scope === "following"
        ? "Every occurrence from this one on will last as long as this one."
        : "Every occurrence will last as long as this one.";
    case "exceptions_may_reset":
      if (!w.complete) {
        return `Some occurrences of ${t} may have been changed on their own (PM couldn't see them all). Google may keep or lose those changes.`;
      }
      return w.count === 1
        ? `One occurrence of ${t} was changed on its own. Google may keep or lose that change.`
        : `${w.count} occurrences of ${t} were changed on their own. Google may keep or lose those changes.`;
    case "not_carried": {
      // PM gives the new part no video call, where Google would make a new one (Bobby, 17-10 #3).
      const missing = [
        ...(w.conference ? ["the video call"] : []),
        ...(w.attachments ? ["the attachments"] : []),
        ...(w.label ? ["the label"] : []),
      ];
      if (missing.length === 0) return "";
      const keeps = missing.length === 1 ? "keeps it" : "keeps them";
      const call = w.conference ? " Add a call to it in Google Calendar if you need one." : "";
      return `The new repeating event this makes won't have ${andList(missing)}. The earlier part ${keeps}.${call}`;
    }
    case "new_identity":
      // The flags' warning (C9 design scope-L9) is the last sentence: they aren't moved, and no
      // question follows the save for them (DECISIONS 17-10 #2).
      return "From this occurrence on, it becomes a new repeating event in Google. If a milestone linked to it falls on or after this day, PM offers to move it to the new part after saving. Reminders to prepare may show again for the new part.";
    case "split_sibling": {
      const head = `Part of ${t} was made into a repeating event of its own by an earlier “this and following” change.`;
      return kind === "save"
        ? `${head} Google may treat that part as this one too.`
        : `${head} Google may delete that part too.`;
    }
    default:
      return unreachable(w);
  }
}

/** The question a move to another day raises (plan:113-122): it names both days, never "every
 *  Monday → every Monday", and says how far every occurrence moves when that isn't plain. `before`
 *  is null for a daily rule and for one the editor can't word, so a null never claims "daily". */
export function repeatDayQuestion(
  title: string,
  w: Extract<SeriesWarning, { warning: "repeat_day_changes" }>,
  scope: SeriesScope,
): string {
  const t = quote(title);
  const following = scope === "following";
  const fromOn = following ? ` from ${formatDateOnly(w.to)}` : "";
  const shift = Math.abs(w.shift_days);
  const dir = w.shift_days > 0 ? "later" : "earlier";
  const by = shift > 0 ? `${distanceText(shift)} ${dir}` : null;
  const { before, after } = w;
  if (before !== null && after !== null && after !== before) {
    const moves = by !== null && shift >= 7 ? ` Every occurrence moves ${by}.` : "";
    return `${t} repeats ${before}. Change it to repeat ${after}${fromOn}?${moves}`;
  }
  if (before !== null) {
    // The same day phrase after (or none PM can name): only the distance says what changes.
    if (by === null) return `${t} repeats ${before}. Change when it repeats${fromOn}?`;
    return following
      ? `${t} repeats ${before}. Move this occurrence and every one after it ${by}?`
      : `${t} repeats ${before}. Move every occurrence ${by}?`;
  }
  if (after !== null) {
    const moves = by !== null ? ` Every occurrence moves ${by}.` : "";
    return `Change ${t} to repeat ${after}${fromOn}?${moves}`;
  }
  if (by === null) return `Change when ${t} repeats${fromOn}?`;
  return following
    ? `Move this occurrence of ${t} and every one after it ${by}?`
    : `Move every occurrence of ${t} ${by}?`;
}

/** "a day", "3 days", "a week", "a week and a day", "2 weeks and 3 days", "30 days" (a move of
 *  four weeks or more is a monthly one, said in days). */
export function distanceText(days: number): string {
  if (days === 1) return "a day";
  if (days < 7 || days >= 28) return `${days} days`;
  const weeks = Math.floor(days / 7);
  const rest = days % 7;
  const head = weeks === 1 ? "a week" : `${weeks} weeks`;
  if (rest === 0) return head;
  return rest === 1 ? `${head} and a day` : `${head} and ${rest} days`;
}
