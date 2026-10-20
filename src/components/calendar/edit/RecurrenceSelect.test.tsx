// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The Repeat menu (#884, C11 §6): Google's order, a rule that is none of the ready-made ones shown as
// its own row in the backend's words, Custom… acting only on a pick the user commits (never on the
// keyboard passing through it) and leaving the value as it was, and a rule PM can only show kept
// until it's replaced.

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RepeatPreset, RepeatRule } from "../../../lib/types";
import type { RepeatValue } from "../../../lib/calendarEdit/recurrenceDraft";

const recurrencePresets = vi.fn<(day: string) => Promise<RepeatPreset[]>>();

vi.mock("../../../lib/ipc", () => ({
  recurrencePresets: (day: string) => recurrencePresets(day),
}));

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard" }),
}));

import { RecurrenceSelect } from "./RecurrenceSelect";

const rule = (pattern: RepeatRule["pattern"]): RepeatRule => ({
  pattern,
  interval: 1,
  ends: { kind: "never" },
});

/** The five ready-made rules for Tue 13-10-2026, as the backend gives them. */
const PRESETS: RepeatPreset[] = [
  { label: "Daily", rule: rule({ kind: "daily" }) },
  { label: "Weekly on Tuesday", rule: rule({ kind: "weekly", days: ["tue"] }) },
  {
    label: "Monthly on the second Tuesday",
    rule: rule({ kind: "monthly_weekday", nth: "second", weekday: "tue" }),
  },
  { label: "Annually on 13 October", rule: rule({ kind: "yearly_date" }) },
  {
    label: "Every weekday (Monday to Friday)",
    rule: rule({ kind: "weekly", days: ["mon", "tue", "wed", "thu", "fri"] }),
  },
];

const monWed: RepeatValue = { kind: "rule", rule: rule({ kind: "weekly", days: ["mon", "wed"] }) };

beforeEach(() => {
  recurrencePresets.mockResolvedValue(PRESETS);
});

afterEach(() => {
  cleanup();
  vi.resetAllMocks();
});

function open(value: RepeatValue, ruleWords: string | null = null) {
  const onChange = vi.fn();
  const onCustom = vi.fn();
  const view = render(
    <RecurrenceSelect
      value={value}
      startDate="2026-10-13"
      disabled={false}
      ruleWords={ruleWords}
      onChange={onChange}
      onCustom={onCustom}
    />,
  );
  const rerender = (next: RepeatValue) =>
    view.rerender(
      <RecurrenceSelect
        value={next}
        startDate="2026-10-13"
        disabled={false}
        ruleWords={ruleWords}
        onChange={onChange}
        onCustom={onCustom}
      />,
    );
  return { onChange, onCustom, rerender };
}

const select = () => screen.getByRole<HTMLSelectElement>("combobox", { name: "Repeat" });
const rows = () => Array.from(select().options).map((o) => o.text);
const shown = () => select().selectedOptions[0]?.text;

describe("RecurrenceSelect", () => {
  it("lists Does not repeat, the presets and Custom… in Google's order", async () => {
    open({ kind: "once" });
    await act(async () => {});
    expect(rows()).toEqual([
      "Does not repeat",
      "Daily",
      "Weekly on Tuesday",
      "Monthly on the second Tuesday",
      "Annually on 13 October",
      "Every weekday (Monday to Friday)",
      "Custom…",
    ]);
    expect(shown()).toBe("Does not repeat");
    expect(recurrencePresets).toHaveBeenCalledWith("2026-10-13");
  });

  it("is labelled Repeat, and a preset picked is the value", async () => {
    const { onChange } = open({ kind: "once" });
    await act(async () => {});
    expect(select().getAttribute("aria-label")).toBe("Repeat");
    fireEvent.change(select(), { target: { value: "preset:1" } });
    expect(onChange).toHaveBeenCalledWith({ kind: "rule", rule: PRESETS[1].rule });
    fireEvent.change(select(), { target: { value: "once" } });
    expect(onChange).toHaveBeenLastCalledWith({ kind: "once" });
  });

  it("a rule outside the presets shows as its own row, selected", async () => {
    open(monWed, "Weekly on Monday and Wednesday");
    await act(async () => {});
    expect(rows()).toEqual([
      "Does not repeat",
      "Daily",
      "Weekly on Tuesday",
      "Monthly on the second Tuesday",
      "Annually on 13 October",
      "Every weekday (Monday to Friday)",
      "Weekly on Monday and Wednesday",
      "Custom…",
    ]);
    expect(shown()).toBe("Weekly on Monday and Wednesday");
  });

  it("a rule equal to a preset, its days in another order, selects that preset", async () => {
    open({
      kind: "rule",
      rule: rule({ kind: "weekly", days: ["fri", "thu", "wed", "tue", "mon"] }),
    });
    await act(async () => {});
    expect(shown()).toBe("Every weekday (Monday to Friday)");
    expect(rows()).toHaveLength(7);
  });

  it("picking Custom… calls onCustom and keeps the value", async () => {
    const { onChange, onCustom } = open(monWed, "Weekly on Monday and Wednesday");
    await act(async () => {});
    fireEvent.change(select(), { target: { value: "custom" } });
    expect(onCustom).toHaveBeenCalledTimes(1);
    expect(onChange).not.toHaveBeenCalled();
    expect(shown()).toBe("Weekly on Monday and Wednesday");
  });

  // A closed select changes value at every arrow on Linux and Windows: passing "Custom…" on the way
  // must not open a dialog nobody asked for (C11 critique 2 M9). Enter on it does.
  it("arrowing past the presets never opens Custom…", async () => {
    const { onChange, onCustom } = open({ kind: "once" });
    await act(async () => {});
    fireEvent.keyDown(select(), { key: "ArrowDown" });
    fireEvent.change(select(), { target: { value: "preset:4" } });
    fireEvent.keyDown(select(), { key: "ArrowDown" });
    fireEvent.change(select(), { target: { value: "custom" } });
    expect(onCustom).not.toHaveBeenCalled();
    // It shows Custom…, waiting.
    expect(shown()).toBe("Custom…");
    // Arrowing on moves away; back onto it and Enter opens it.
    fireEvent.keyDown(select(), { key: "ArrowUp" });
    fireEvent.change(select(), { target: { value: "preset:4" } });
    expect(onChange).toHaveBeenLastCalledWith({ kind: "rule", rule: PRESETS[4].rule });
    fireEvent.keyDown(select(), { key: "ArrowDown" });
    fireEvent.change(select(), { target: { value: "custom" } });
    fireEvent.keyUp(select(), { key: "ArrowDown" });
    expect(onCustom).not.toHaveBeenCalled();
    fireEvent.keyDown(select(), { key: "Enter" });
    expect(onCustom).toHaveBeenCalledTimes(1);
  });

  it("a Custom… arrowed onto and left goes back to the value", async () => {
    const { onCustom } = open({ kind: "once" });
    await act(async () => {});
    fireEvent.keyDown(select(), { key: "End" });
    fireEvent.change(select(), { target: { value: "custom" } });
    expect(shown()).toBe("Custom…");
    fireEvent.blur(select());
    expect(shown()).toBe("Does not repeat");
    // Enter now is just Enter.
    fireEvent.keyDown(select(), { key: "Enter" });
    expect(onCustom).not.toHaveBeenCalled();
  });

  it("a kept rule shows its words; picking a preset replaces it", async () => {
    const { onChange, rerender } = open({ kind: "kept", words: "Repeats (custom rule)" });
    await act(async () => {});
    expect(shown()).toBe("Repeats (custom rule)");
    // Picked again, it changes nothing.
    fireEvent.change(select(), { target: { value: "current" } });
    expect(onChange).not.toHaveBeenCalled();
    fireEvent.change(select(), { target: { value: "preset:0" } });
    expect(onChange).toHaveBeenCalledWith({ kind: "rule", rule: PRESETS[0].rule });
    rerender({ kind: "rule", rule: PRESETS[0].rule });
    expect(shown()).toBe("Daily");
    expect(rows()).not.toContain("Repeats (custom rule)");
  });

  it("presets still loading never show Does not repeat as selected for a rule", async () => {
    recurrencePresets.mockReturnValue(new Promise<RepeatPreset[]>(() => {}));
    open({ kind: "rule", rule: PRESETS[1].rule });
    await act(async () => {});
    expect(rows()).toEqual(["Does not repeat", "Repeats", "Custom…"]);
    expect(shown()).toBe("Repeats");
  });
});
