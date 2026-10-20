// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Repeat menu (#884, C11), in Google Calendar's order: "Does not repeat", the ready-made rules
// for the event's start date, the current rule when it is none of them (in the backend's words), and
// "Custom…". A native select, so it reads and works as one everywhere.
//
// It only ever shows the rule the Repeat value holds, so a save sends what it shows. "Custom…" opens
// a dialog (or, in quick create, the full editor), so it acts only on a pick from the list: a click,
// or Enter in the list once the keyboard has opened it (Space or Alt+Down; Up and Down on a Mac).
// Arrowing through a closed select changes its value at every step on Linux and Windows, and a dialog
// springing open on the way past would take the keyboard somewhere the user never asked to go (C11
// critique 2 M9), so while a key steps the select, "Custom…" is off and the step passes over it.

import { useRef } from "react";
import { ONCE, sameRule, type RepeatValue } from "../../../lib/calendarEdit/recurrenceDraft";
import { IS_MAC } from "../../../lib/setupGuide";
import { Select } from "../../ui";
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

/** Keys that step a closed select through its options (type-ahead letters do too). */
const STEP_KEYS = new Set([
  "ArrowUp",
  "ArrowDown",
  "ArrowLeft",
  "ArrowRight",
  "Home",
  "End",
  "PageUp",
  "PageDown",
]);

export function RecurrenceSelect({
  value,
  startDate,
  disabled,
  compact,
  ruleWords,
  onChange,
  onCustom,
}: Props) {
  const presets = usePresets(startDate);
  // The "Custom…" row, off while a key steps the select (`holdCustom`).
  const customRow = useRef<HTMLOptionElement>(null);

  const match =
    value.kind === "rule" && presets ? presets.findIndex((p) => sameRule(p.rule, value.rule)) : -1;
  const selected = value.kind === "once" ? "once" : match >= 0 ? `preset:${match}` : "current";
  // A rule that is no ready-made one (or whose ready-made rules are still loading, so the row never
  // flickers to "Does not repeat"), or one PM can only show: its own row, selected.
  const current =
    value.kind === "kept"
      ? value.words
      : value.kind === "rule" && match < 0
        ? (ruleWords ?? "Repeats")
        : null;

  function pick(next: string) {
    if (next === "once") {
      onChange(ONCE);
    } else if (next.startsWith("preset:")) {
      const preset = presets?.[Number(next.slice("preset:".length))];
      if (preset) onChange({ kind: "rule", rule: preset.rule });
    }
    // "current" is the value already: picking it again changes nothing.
  }

  /** Turn "Custom…" off for the key being pressed: a step passes over a row that's off (Chromium and
   *  WebKit alike), so the keyboard going through the list never lands on it. It comes back on by a
   *  timer, not the key coming up: the step is made inside the key's own handling, before any timer
   *  runs, and a key that opened a menu instead may never send its key-up. */
  function holdCustom() {
    const row = customRow.current;
    if (!row) return;
    row.disabled = true;
    setTimeout(() => {
      row.disabled = false;
    }, 0);
  }

  return (
    <Select
      aria-label="Repeat"
      compact={compact}
      // Never wider than its row: a long rule's words are cut short on screen, and read out whole.
      className="max-w-full"
      value={selected}
      disabled={disabled}
      onKeyDown={(e) => {
        const steps = STEP_KEYS.has(e.key) || (e.key.length === 1 && e.key !== " ");
        // Alt+Down opens the list rather than stepping, and on a Mac Up and Down alone do.
        const opens = e.altKey || (IS_MAC && (e.key === "ArrowUp" || e.key === "ArrowDown"));
        if (steps && !opens) holdCustom();
      }}
      onChange={(e) => {
        const next = e.target.value;
        // Only a pick from the list reaches it; the select goes back to the value it holds.
        if (next === "custom") onCustom();
        else pick(next);
      }}
    >
      <option value="once">Does not repeat</option>
      {presets?.map((p, i) => (
        <option key={i} value={`preset:${i}`}>
          {p.label}
        </option>
      ))}
      {current !== null && <option value="current">{current}</option>}
      <option ref={customRow} value="custom">
        Custom…
      </option>
    </Select>
  );
}
