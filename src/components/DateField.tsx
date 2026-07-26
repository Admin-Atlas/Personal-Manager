// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// PM's date input — a forgiving DD-MM-YYYY text field plus a calendar popover. Replaces every
// `<input type="date">` in the app.
//
// WHY NOT THE NATIVE ONE: on Linux/WebKitGTK there is no real date widget. The popup that does open
// swallows outside clicks until Escape (which is what made the milestone and pinboard date fields
// feel stuck), and `type="date"` renders in the OS locale, so those fields never obeyed the app's
// DD-MM-YYYY rule. Owning the control fixes the engine split and the design-system violation at once.
// The parse layer lives in `lib/dateField.ts` and is deliberately lenient — on Linux the text half is
// now the primary input path, not a fallback, so a mid-typing value must never be thrown away.
//
// COMMIT MODEL: `onCommit` receives the new ISO value as an ARGUMENT rather than the caller reading
// its own state afterwards. Callers here persist on blur from a closure over that state, and handing
// them a value they'd have to `setState`-then-read is exactly the stale-closure trap that made the
// Documents photo-copy checkbox silently drop its setting (#555).

import { useRef, useState } from "react";
import { MonthPicker } from "./calendar/parts/MonthPicker";
import { dateToIso, isoToDate, isoToDisplay, parseDisplay, todayIso } from "../lib/dateField";
import { Popover, cn } from "./ui";

interface Props {
  /** Stored value: `YYYY-MM-DD`, or "" for no date. */
  value: string;
  /** Fired with the new ISO value ("" when cleared). Only fires when the value actually changes. */
  onCommit: (iso: string) => void;
  disabled?: boolean;
  /** Applied to the text input (height, padding, font). */
  className?: string;
  /** Applied to the field's flex root — this is where a caller puts its LAYOUT (`flex-1`, a fixed
   *  width), since the root, not the input, is what the parent row lays out. */
  wrapperClassName?: string;
  placeholder?: string;
  ariaLabel?: string;
  title?: string;
  id?: string;
  /** Show a Clear shortcut in the popover. Off where the date is mandatory. */
  clearable?: boolean;
}

export function DateField({
  value,
  onCommit,
  disabled,
  className,
  wrapperClassName,
  placeholder = "dd-mm-yyyy",
  ariaLabel = "Date",
  title,
  id,
  clearable = true,
}: Props) {
  const [draft, setDraft] = useState(() => isoToDisplay(value));
  const [invalid, setInvalid] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  // Adjust the draft when the stored value changes underneath us (a refetch, or the popover writing
  // a pick). Render-phase sync rather than an effect: an effect would paint one frame of the stale
  // text first, and this field sits in lists that refetch after every mutation.
  const lastValue = useRef(value);
  if (lastValue.current !== value) {
    lastValue.current = value;
    setDraft(isoToDisplay(value));
    setInvalid(false);
  }

  const selected = isoToDate(value);

  /** Parse the draft and push it up. Unparseable text reverts to the stored value rather than
   *  committing junk — but only on blur, never mid-typing. */
  function commitDraft() {
    const parsed = parseDisplay(draft);
    if (parsed === null) {
      setDraft(isoToDisplay(value));
      setInvalid(false);
      return;
    }
    setInvalid(false);
    // Normalise what's shown ("4/8" → "04-08-2026") so the field always reads back in one format.
    setDraft(isoToDisplay(parsed));
    if (parsed !== value) onCommit(parsed);
  }

  function pick(iso: string) {
    setDraft(isoToDisplay(iso));
    setInvalid(false);
    if (iso !== value) onCommit(iso);
  }

  return (
    <div
      className={cn("flex min-w-0 items-center gap-1", disabled && "opacity-60", wrapperClassName)}
    >
      <input
        ref={inputRef}
        id={id}
        type="text"
        inputMode="numeric"
        autoComplete="off"
        value={draft}
        disabled={disabled}
        placeholder={placeholder}
        aria-label={ariaLabel}
        aria-invalid={invalid || undefined}
        title={title ?? "Type a date as dd-mm-yyyy, or pick one from the calendar"}
        onChange={(e) => {
          setDraft(e.currentTarget.value);
          // Live-flag nonsense so the border reacts as you type, but never rewrite the text — the
          // whole point is that a half-typed date survives.
          setInvalid(parseDisplay(e.currentTarget.value) === null);
        }}
        onBlur={commitDraft}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            commitDraft();
            inputRef.current?.blur();
          }
        }}
        className={cn(
          "min-w-0 flex-1 rounded-[var(--radius-sm)] border bg-surface outline-none transition placeholder:text-ink4 focus:border-accent",
          invalid ? "border-st-due" : "border-border2",
          className,
        )}
      />
      <Popover
        align="right"
        ariaLabel="Pick a date"
        panelClassName="w-auto min-w-0"
        trigger={({ open, toggle }) => (
          <button
            type="button"
            onClick={toggle}
            disabled={disabled}
            aria-expanded={open}
            aria-label="Open the date picker"
            title="Pick a date"
            className="shrink-0 rounded-[var(--radius-sm)] px-1 text-ink4 transition hover:bg-surface hover:text-ink disabled:opacity-40"
          >
            <span aria-hidden="true">📅</span>
          </button>
        )}
      >
        {({ close }) => (
          <MonthPicker
            selected={selected}
            onPick={(d) => {
              pick(dateToIso(d));
              close();
            }}
            footer={
              <div className="mt-1 flex items-center justify-between border-t border-border px-1 pt-1">
                <button
                  type="button"
                  onClick={() => {
                    pick(todayIso());
                    close();
                  }}
                  className="rounded-[var(--radius-sm)] px-1.5 py-0.5 text-xs text-accent-text hover:bg-surface"
                >
                  Today
                </button>
                {clearable && (
                  <button
                    type="button"
                    onClick={() => {
                      pick("");
                      close();
                    }}
                    className="rounded-[var(--radius-sm)] px-1.5 py-0.5 text-xs text-ink4 hover:bg-surface hover:text-ink"
                  >
                    Clear
                  </button>
                )}
              </div>
            }
          />
        )}
      </Popover>
    </div>
  );
}
