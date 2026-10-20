// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Repeat menu (#884, C11), in Google Calendar's order: "Does not repeat", the ready-made rules
// for the event's start date, the current rule when it is none of them (in the backend's words), and
// "Custom…". A native select, so it reads and works as one everywhere.
//
// "Custom…" opens a dialog (or, in quick create, the full editor), so it acts only on a pick the user
// commits: a click, or Enter once it shows. Arrowing through a closed select changes its value at
// every step on Linux and Windows, and a dialog springing open on the way past would take the
// keyboard somewhere the user never asked to go (C11 critique 2 M9). Arrowed onto, it only shows
// "Custom…" until Enter (or Space) opens it, or the user moves on.

import { useRef, useState } from "react";
import { ONCE, sameRule, type RepeatValue } from "../../../lib/calendarEdit/recurrenceDraft";
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
  // "Custom…" arrowed onto: shown, waiting for Enter, nothing opened.
  const [armed, setArmed] = useState(false);
  // A key that steps the select is down, so a change now is the keyboard passing through.
  const stepping = useRef(false);

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
    setArmed(false);
    if (next === "once") {
      onChange(ONCE);
    } else if (next.startsWith("preset:")) {
      const preset = presets?.[Number(next.slice("preset:".length))];
      if (preset) onChange({ kind: "rule", rule: preset.rule });
    }
    // "current" is the value already: picking it again changes nothing.
  }

  return (
    <Select
      aria-label="Repeat"
      compact={compact}
      value={armed ? "custom" : selected}
      disabled={disabled}
      onKeyDown={(e) => {
        if (armed && (e.key === "Enter" || e.key === " ")) {
          e.preventDefault();
          setArmed(false);
          onCustom();
          return;
        }
        const letter = e.key.length === 1 && e.key !== " " && !e.ctrlKey && !e.metaKey;
        // Alt+Down opens the list rather than stepping.
        if (!e.altKey && (STEP_KEYS.has(e.key) || letter)) stepping.current = true;
      }}
      onKeyUp={() => {
        stepping.current = false;
      }}
      onBlur={() => {
        stepping.current = false;
        setArmed(false);
      }}
      onChange={(e) => {
        const next = e.target.value;
        if (next !== "custom") {
          pick(next);
        } else if (stepping.current) {
          setArmed(true);
        } else {
          setArmed(false);
          onCustom();
        }
      }}
    >
      <option value="once">Does not repeat</option>
      {presets?.map((p, i) => (
        <option key={i} value={`preset:${i}`}>
          {p.label}
        </option>
      ))}
      {current !== null && <option value="current">{current}</option>}
      <option value="custom">Custom…</option>
    </Select>
  );
}
