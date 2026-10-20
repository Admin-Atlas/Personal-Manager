// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Which calendar a new event goes to (#884, C10): a button naming the calendar, opening every
// calendar that can take one, grouped by account as radios (the browser gives the arrow keys within
// a group), with the Google accounts whose editing is off named underneath. The list escapes
// clipping, so the editor's scrolling body never cuts it off. A click, or Enter, picks and closes;
// the arrow keys move the choice without closing.

import { useId, useRef, useState } from "react";
import type { CalendarAccount, EditingStatus } from "../../../lib/types";
import { targetById, type CreateTargets } from "../../../lib/calendarEdit/createTargets";
import { useDepth } from "../../../theme";
import { Button, Callout, Popover, cn } from "../../ui";
import { SourceDot } from "../parts/SourceDot";

interface Props {
  targets: CreateTargets;
  /** The calendar picked (PM's id). */
  value: string;
  onChange: (calendarId: string) => void;
  /** The draft is private: calendars that can't hold a private event can't be picked, and say so. */
  privateDraft: boolean;
  /** Calendars hidden in the calendar list: still on offer, and marked. */
  hidden: ReadonlySet<string>;
  colorOf: (calendarId: string) => string;
  shapeOf: (calendarId: string) => number | undefined;
  disabled?: boolean;
}

/** Why an account's calendars aren't on offer, as one line. */
function offText(account: CalendarAccount, status: EditingStatus): string {
  const who = account.email || account.label;
  switch (status) {
    case "paused":
      return `Editing is paused for ${who}. Turn it back on in Settings › Connectors.`;
    case "needs_consent":
      return `Editing for ${who} needs Google's permission again. Ask for it in Settings › Connectors.`;
    default:
      return `Editing is off for ${who}. Turn it on in Settings › Connectors.`;
  }
}

export function CalendarPicker({
  targets,
  value,
  onChange,
  privateDraft,
  hidden,
  colorOf,
  shapeOf,
  disabled = false,
}: Props) {
  const { showMeta } = useDepth();
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const name = useId();
  const picked = targetById(targets, value);
  const label = picked?.calendar.name ?? "Choose a calendar";
  const account = picked ? picked.account.email || picked.account.label : null;

  /** Close, with focus back on the button (the popover's own restore is Escape's). */
  const done = () => {
    setOpen(false);
    triggerRef.current?.focus();
  };

  return (
    <Popover
      open={open && !disabled}
      onOpenChange={setOpen}
      escapeClipping
      ariaLabel="Calendars that can take a new event"
      panelClassName="max-w-sm"
      trigger={({ open: shown, toggle }) => (
        <Button
          ref={triggerRef}
          variant="secondary"
          size="sm"
          aria-label={`Calendar: ${label}`}
          aria-expanded={shown}
          disabled={disabled}
          onClick={toggle}
        >
          {picked && (
            <SourceDot
              color={colorOf(picked.calendar.id)}
              shapeIndex={shapeOf(picked.calendar.id)}
            />
          )}
          <span className="truncate">
            {label}
            {showMeta && account ? ` · ${account}` : ""}
          </span>
        </Button>
      )}
    >
      <div
        className="flex max-h-[60vh] flex-col gap-1 overflow-y-auto"
        onKeyDown={(e) => {
          // Enter picks what the arrow keys moved to, as a click does.
          if (e.key === "Enter") {
            e.preventDefault();
            done();
          }
        }}
      >
        {targets.groups.map((g) => (
          <fieldset key={g.account.id} className="flex min-w-0 flex-col">
            <legend className="truncate px-2 pb-0.5 pt-1 font-mono text-[0.625rem] uppercase tracking-wide text-ink4">
              {g.account.email || g.account.label}
            </legend>
            {g.targets.map((t) => {
              const id = t.calendar.id;
              const noPrivate = privateDraft && t.noPrivate;
              return (
                <label
                  key={id}
                  className={cn(
                    "flex items-center gap-2 rounded-[var(--radius-sm)] px-2 py-1 text-sm",
                    noPrivate
                      ? "cursor-not-allowed text-ink4"
                      : "cursor-pointer text-ink hover:bg-surface",
                  )}
                >
                  <input
                    type="radio"
                    name={name}
                    value={id}
                    checked={id === value}
                    disabled={noPrivate}
                    onChange={() => onChange(id)}
                    // A pointer's click closes the list; one the arrow keys make (no pointer, so
                    // `detail` 0) only moves the choice.
                    onClick={(e) => {
                      if (e.detail > 0) done();
                    }}
                    className="accent-[var(--accent)]"
                  />
                  <SourceDot color={colorOf(id)} shapeIndex={shapeOf(id)} />
                  <span className="truncate">{t.calendar.name}</span>
                  {hidden.has(id) && <span className="ml-auto text-xs text-ink4">hidden</span>}
                  {noPrivate && (
                    <span className="ml-auto shrink-0 text-xs text-ink4">
                      Can't hold private events
                    </span>
                  )}
                </label>
              );
            })}
          </fieldset>
        ))}
        {targets.off.length > 0 && (
          <div className="mt-1 flex flex-col">
            {targets.off.map(({ account: a, status }) => (
              <Callout key={a.id} tone="info" variant="strip" body="ink" className="text-ink3">
                {offText(a, status)}
              </Callout>
            ))}
          </div>
        )}
      </div>
    </Popover>
  );
}
