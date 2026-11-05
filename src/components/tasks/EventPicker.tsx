// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Link a task to one calendar event: a type-ahead over the events in the synced mirror, one per iCal
// UID at its next occurrence (or its latest within the last 30 days), each listed "DD-MM · title".
// It only ever hands back a UID; the editor pins the picked choice's date as the occurrence an
// "Event's date" deadline follows (spec R1), and passes it back as `pinnedDate` to show. A read of
// the mirror and no more: this file never imports a calendar write wrapper (the calendar write fence).
//
// The choices come in as a prop (`eventChoices` over `listAllCalendarEvents`, loaded by the
// editor), because the editor needs the chosen event's date too, to show what an "Event's date"
// deadline resolves to.
//
// The list renders in the flow under the field rather than floating over it: it lives inside a
// scrolling dialog, which would clip a floating list.

import { useId, useState, type KeyboardEvent } from "react";
import { formatDateOnly } from "../../lib/format";
import { filterEventChoices, type EventChoice } from "../../lib/tasks";
import { Button, Input, cn } from "../ui";

/** How many matches the list shows at once. */
const MAX_SHOWN = 8;

export function EventPicker({
  labelledBy,
  value,
  choices,
  loading = false,
  linkedTitle,
  pinnedDate,
  onChange,
}: {
  /** The id of the visible words naming this field. */
  labelledBy?: string;
  /** The linked event's UID, or null. */
  value: string | null;
  choices: readonly EventChoice[];
  /** The mirror is still being read. */
  loading?: boolean;
  /** The linked event's title as the task last knew it, for a link that isn't among `choices`
   *  (an older event, or one that left the calendars). */
  linkedTitle?: string | null;
  /** The occurrence the task's deadline is pinned to (`YYYY-MM-DD`), shown in place of the next
   *  one: a recurring event's chosen Friday still reads as that Friday once it has passed. */
  pinnedDate?: string | null;
  onChange: (uid: string | null) => void;
}) {
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const listId = useId();
  const optionId = (i: number) => `${listId}-opt-${i}`;

  if (value !== null) {
    const chosen = choices.find((c) => c.uid === value);
    const pinned = pinnedDate ? `${formatDateOnly(pinnedDate)} · ` : "";
    const text = chosen
      ? pinnedDate
        ? `${pinned}${chosen.title}`
        : chosen.label
      : loading
        ? "Linked event"
        : linkedTitle
          ? `${pinned}${linkedTitle} (not in your calendars right now)`
          : "An event that isn't in your calendars right now";
    return (
      <div className="flex min-w-0 items-center gap-2">
        <span
          className="min-w-0 flex-1 truncate rounded-[var(--radius-sm)] border border-border bg-surface px-3 py-1.5 text-sm text-ink2"
          title={text}
        >
          📅 {text}
        </span>
        <Button
          variant="tertiary"
          size="xs"
          onClick={() => onChange(null)}
          className="shrink-0"
          aria-label="Clear the event link"
        >
          Clear
        </Button>
      </div>
    );
  }

  const matches = filterEventChoices(choices, query).slice(0, MAX_SHOWN);
  const shown = open && !loading;
  const pick = (c: EventChoice) => {
    onChange(c.uid);
    setQuery("");
    setOpen(false);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setOpen(true);
      setActive((a) => Math.min(a + 1, Math.max(matches.length - 1, 0)));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((a) => Math.max(a - 1, 0));
    } else if (e.key === "Enter") {
      if (shown && matches[active]) {
        e.preventDefault();
        pick(matches[active]);
      }
    } else if (e.key === "Escape" && shown) {
      // Close the list, not the dialog around it.
      e.stopPropagation();
      setOpen(false);
    }
  };

  return (
    <div className="flex min-w-0 flex-col gap-1">
      <Input
        aria-labelledby={labelledBy}
        role="combobox"
        aria-autocomplete="list"
        aria-expanded={shown}
        aria-controls={listId}
        aria-activedescendant={shown && matches[active] ? optionId(active) : undefined}
        value={query}
        placeholder={loading ? "Reading your calendars…" : "Type to find an event"}
        disabled={loading}
        onChange={(e) => {
          setQuery(e.target.value);
          setActive(0);
          setOpen(true);
        }}
        onFocus={() => setOpen(true)}
        onBlur={() => setOpen(false)}
        onKeyDown={onKeyDown}
        className="h-8 text-xs"
      />
      {shown && (
        <ul
          id={listId}
          role="listbox"
          aria-label="Calendar events"
          className="max-h-48 overflow-y-auto rounded-[var(--radius-sm)] border border-border bg-surface py-1"
        >
          {matches.length === 0 ? (
            <li className="px-3 py-1 text-xs text-ink4">
              {choices.length === 0
                ? "No calendar events to link yet. Connect a calendar on the Calendar tab."
                : "No event matches that."}
            </li>
          ) : (
            matches.map((c, i) => (
              <li
                key={c.uid}
                id={optionId(i)}
                role="option"
                aria-selected={i === active}
                // mousedown, not click: the input's blur would close the list before a click lands.
                onMouseDown={(e) => {
                  e.preventDefault();
                  pick(c);
                }}
                onMouseEnter={() => setActive(i)}
                className={cn(
                  "cursor-pointer truncate px-3 py-1 text-xs",
                  // One colour class per state: `cn` is a plain joiner, so two would both survive.
                  i === active ? "bg-accent-soft text-ink" : c.past ? "text-ink3" : "text-ink2",
                )}
              >
                {c.label}
                {c.past ? " (past)" : ""}
              </li>
            ))
          )}
        </ul>
      )}
    </div>
  );
}
