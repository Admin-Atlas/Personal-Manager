// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Repeat menu (#884, C11): a button naming the rule the Repeat value holds, opening a list in
// Google Calendar's order: "Does not repeat", the ready-made rules for the event's start date and the
// current rule when it is none of them (in the backend's words), as radios, then "Custom…", a button
// of its own. Google's Repeat control is a list of this kind too. A native select can't be trusted
// here: WebKit and Chromium track its changes differently, so it could show a rule the draft didn't
// hold, or open Custom… on an arrow key passing over it (C11 critique 2 M9).
//
// It works as the calendar list does (CalendarPicker). The list stays inside the editor's dialog,
// where its focus trap and screen readers reach it, and opening puts focus on the rule picked. The
// arrow keys move the choice within the radios, and the button follows; a click on a row, or Enter,
// picks and closes. Tab goes from the rules to Custom… and out of the list to the button, as Escape
// does (closing only the list). Custom… opens its dialog (or, in quick create, the full editor) on a
// click, Enter or Space, never on an arrow key, since it isn't one of the radios. Disabled (a save on
// its way), the list closes.

import { useEffect, useId, useRef, useState } from "react";
import { ONCE, sameRule, type RepeatValue } from "../../../lib/calendarEdit/recurrenceDraft";
import { Button, Popover, cn } from "../../ui";
import { usePresets } from "./useRepeat";

interface Props {
  value: RepeatValue;
  /** The event's start date (YYYY-MM-DD): the ready-made rules follow it. */
  startDate: string | null;
  disabled: boolean;
  compact?: boolean;
  /** The words for a rule that isn't a ready-made one (the backend's), shown as its own row. */
  ruleWords: string | null;
  onChange: (value: RepeatValue) => void;
  /** "Custom…" was picked: the parent opens the dialog. The value stays as it was. */
  onCustom: () => void;
}

/** A row of the list: its words, and the value picking it gives. */
interface Row {
  key: string;
  words: string;
  value: RepeatValue;
}

/** Whether two Repeat values are the same choice. A rule PM can only show is the one the event came
 *  with, never another. */
function same(a: RepeatValue, b: RepeatValue): boolean {
  if (a.kind === "rule" && b.kind === "rule") return sameRule(a.rule, b.rule);
  return a.kind === b.kind;
}

/** Focus on the rule picked (else the first), so the keyboard is in the list. */
function focusPicked(list: HTMLElement | null) {
  const radio =
    list?.querySelector<HTMLInputElement>("input[type=radio]:checked") ??
    list?.querySelector<HTMLInputElement>("input[type=radio]");
  radio?.focus();
}

export function RecurrenceSelect({
  value,
  startDate,
  disabled,
  compact = false,
  ruleWords,
  onChange,
  onCustom,
}: Props) {
  const presets = usePresets(startDate);
  const [open, setOpen] = useState(false);
  // The rule held when the list opened, when it's none of the ready-made ones. Its row stays while
  // the list is open, so the arrow keys passing over it can come back to it.
  const [opened, setOpened] = useState<Row | null>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const customRef = useRef<HTMLButtonElement>(null);
  // A pointer went down on a row: the click that follows is a pick, whatever its `detail` (a click
  // on the words reaches the radio forwarded by its label, with `detail` 0, as an arrow key's does).
  // Any key clears it.
  const pointer = useRef(false);
  const name = useId();
  const shown = open && !disabled;
  const held = shown ? opened : null;

  const isPreset = (v: RepeatValue) =>
    v.kind === "rule" && !!presets?.some((p) => sameRule(p.rule, v.rule));
  // A rule that is no ready-made one (or whose ready-made ones are still loading, so it never
  // flickers to "Does not repeat"), or one PM can only show: its own row. Moved off it, the rule held
  // when the list opened keeps its row until the list closes.
  const own: Row | null =
    value.kind === "once" || isPreset(value)
      ? held && !isPreset(held.value)
        ? held
        : null
      : {
          key: "own",
          value,
          words:
            value.kind === "kept"
              ? value.words
              : (ruleWords ?? (held && same(held.value, value) ? held.words : "Repeats")),
        };
  const rows: Row[] = [
    { key: "once", words: "Does not repeat", value: ONCE },
    ...(presets ?? []).map((p, i): Row => ({
      key: `preset:${i}`,
      words: p.label,
      value: { kind: "rule", rule: p.rule },
    })),
    ...(own ? [own] : []),
  ];
  // The button names the rule held, worked out from the value alone, so it can never show another.
  const picked = rows.find((r) => same(r.value, value));
  const label = picked?.words ?? "Repeats";

  // Disabled while open (Ctrl+S with the list up): closed for good, not back by itself after.
  useEffect(() => {
    if (disabled) setOpen(false);
  }, [disabled]);

  // Opened, focus goes to the rule picked, so the keyboard is in the list. After the panel's
  // measuring frame, which hides it, and a hidden radio can't take focus.
  useEffect(() => {
    if (!shown) return;
    const t = setTimeout(() => focusPicked(listRef.current), 0);
    return () => clearTimeout(t);
  }, [shown]);

  /** Open or close the list; opening, keep the rule held if it has a row of its own. */
  const openList = (next: boolean) => {
    if (next) setOpened(own);
    setOpen(next);
  };

  /** Close, with focus back on the button (the popover's own restore is Escape's). */
  const done = () => {
    setOpen(false);
    triggerRef.current?.focus();
  };

  return (
    <Popover
      open={shown}
      onOpenChange={openList}
      escapeClipping
      withinDialog
      ariaLabel="Repeat"
      panelClassName="max-w-sm"
      trigger={({ open: expanded, toggle }) => (
        <Button
          ref={triggerRef}
          variant="secondary"
          size={compact ? "xs" : "sm"}
          aria-label={`Repeat: ${label}`}
          aria-expanded={expanded}
          disabled={disabled}
          // Never wider than its row: a long rule's words wrap.
          className="max-w-full"
          onClick={() => {
            // A click on a Mac leaves focus where it was. The button takes it, so the list's Escape
            // hands it back here.
            triggerRef.current?.focus();
            toggle();
          }}
        >
          <span className="text-left">{label}</span>
          <span aria-hidden>▾</span>
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
          if (e.key !== "Tab") return;
          // Tab goes from the rules to Custom…, and Shift+Tab back. Out of either end it leaves the
          // list for the button it hangs from, rather than for wherever the list sits in the dialog.
          e.preventDefault();
          const onCustomRow = e.target === customRef.current;
          if (!onCustomRow && !e.shiftKey) customRef.current?.focus();
          else if (onCustomRow && e.shiftKey) focusPicked(listRef.current);
          else done();
        }}
      >
        <div
          className="flex flex-col"
          onKeyDown={(e) => {
            // Enter picks what the arrow keys moved to, as a click does.
            if (e.key === "Enter") {
              e.preventDefault();
              done();
            }
          }}
        >
          {rows.map((r) => (
            <label
              key={r.key}
              onPointerDown={() => (pointer.current = true)}
              className="flex cursor-pointer items-center gap-2 rounded-[var(--radius-sm)] px-2 py-1 text-sm text-ink hover:bg-surface"
            >
              <input
                type="radio"
                name={name}
                value={r.key}
                checked={r === picked}
                onChange={() => onChange(r.value)}
                // A pointer's click closes the list; one the arrow keys make (no pointer, so
                // `detail` 0) only moves the choice.
                onClick={(e) => {
                  if (e.detail > 0 || pointer.current) done();
                }}
                className="accent-[var(--accent)]"
              />
              <span>{r.words}</span>
            </label>
          ))}
        </div>
        <div className="border-t border-rule pt-1">
          <button
            ref={customRef}
            type="button"
            // Focus goes back on the button first, so the dialog Custom… opens hands it back there.
            onClick={() => {
              done();
              onCustom();
            }}
            className="w-full rounded-[var(--radius-sm)] px-2 py-1 text-left text-sm text-ink hover:bg-surface"
          >
            Custom…
          </button>
        </div>
      </div>
    </Popover>
  );
}
