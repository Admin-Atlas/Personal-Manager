// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Which calendar a new event goes to (#884, C10): a button naming the calendar, opening every
// calendar that can take one, grouped by account as radios (the browser gives the arrow keys within
// a group), with the Google accounts whose editing is off named underneath. The list escapes
// clipping, so the editor's scrolling body never cuts it off, and stays inside the editor's dialog,
// where its focus trap and screen readers reach it. Opening puts focus on the calendar picked. A
// click on a row, or Enter, picks and closes; the arrow keys move the choice without closing; Tab
// and Escape close it, focus back on the button. Disabled (a save on its way), it closes.

import { useEffect, useId, useRef, useState } from "react";
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
  /** Quick create's size: a smaller button with the calendar's name only, and a shorter list. */
  compact?: boolean;
  /** The list's name for a screen reader: what its calendars are for (C18 critique C2-L5). */
  listName?: string;
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
  compact = false,
  listName = "Calendars that can take a new event",
}: Props) {
  const { showMeta } = useDepth();
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  // A pointer went down on a row: the click that follows is a pick, whatever its `detail` (a click
  // on the name or dot reaches the radio forwarded by its label, with `detail` 0, as an arrow key's
  // does). Any key clears it.
  const pointer = useRef(false);
  const name = useId();
  const picked = targetById(targets, value);
  const label = picked?.calendar.name ?? "Choose a calendar";
  const account = picked ? picked.account.email || picked.account.label : null;
  const shown = open && !disabled;

  // Disabled while open (Ctrl+S with the list up): closed for good, not back by itself after.
  useEffect(() => {
    if (disabled) setOpen(false);
  }, [disabled]);

  // Opened, focus goes to the calendar picked (else the first that can be), so the keyboard is in
  // the list. After the panel's measuring frame, which hides it, and a hidden radio can't take focus.
  useEffect(() => {
    if (!shown) return;
    const t = setTimeout(() => {
      const list = listRef.current;
      const radio =
        list?.querySelector<HTMLInputElement>("input[type=radio]:checked:not(:disabled)") ??
        list?.querySelector<HTMLInputElement>("input[type=radio]:not(:disabled)");
      radio?.focus();
    }, 0);
    return () => clearTimeout(t);
  }, [shown]);

  /** Close, with focus back on the button (the popover's own restore is Escape's). */
  const done = () => {
    setOpen(false);
    triggerRef.current?.focus();
  };

  return (
    <Popover
      open={shown}
      onOpenChange={setOpen}
      escapeClipping
      withinDialog
      ariaLabel={listName}
      panelClassName="max-w-sm"
      trigger={({ open: expanded, toggle }) => (
        <Button
          ref={triggerRef}
          variant="secondary"
          size={compact ? "xs" : "sm"}
          aria-label={`Calendar: ${label}`}
          aria-expanded={expanded}
          disabled={disabled}
          onClick={() => {
            // A click on a Mac leaves focus where it was. The button takes it, so the list's Escape
            // hands it back here.
            triggerRef.current?.focus();
            toggle();
          }}
        >
          {picked && (
            <SourceDot
              color={colorOf(picked.calendar.id)}
              shapeIndex={shapeOf(picked.calendar.id)}
            />
          )}
          <span className="truncate">
            {label}
            {showMeta && !compact && account ? ` · ${account}` : ""}
          </span>
        </Button>
      )}
    >
      <div
        ref={listRef}
        className={cn(
          "flex flex-col gap-1 overflow-y-auto",
          compact ? "max-h-[45vh]" : "max-h-[60vh]",
        )}
        onKeyDown={(e) => {
          pointer.current = false;
          // Enter picks what the arrow keys moved to, as a click does; Tab leaves the list for the
          // button it hangs from, rather than for wherever the list sits in the dialog.
          if (e.key === "Enter" || e.key === "Tab") {
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
                  onPointerDown={() => (pointer.current = true)}
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
                      const byPointer = e.detail > 0 || pointer.current;
                      pointer.current = false;
                      if (byPointer) done();
                    }}
                    className="accent-[var(--accent)]"
                  />
                  <SourceDot color={colorOf(id)} shapeIndex={shapeOf(id)} />
                  <span className="truncate">{t.calendar.name}</span>
                  {/* The marks are part of the radio's name: the comma, heard and not seen, keeps
                      them apart from the calendar's own. */}
                  {hidden.has(id) && (
                    <span className="ml-auto text-xs text-ink4">
                      <span className="sr-only">,</span> hidden
                    </span>
                  )}
                  {noPrivate && (
                    <span className="ml-auto shrink-0 text-xs text-ink4">
                      <span className="sr-only">,</span> Can't hold private events
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
