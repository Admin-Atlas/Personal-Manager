// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The Repeat menu (#884, C11 §6): Google's order, a rule that is none of the ready-made ones shown as
// its own row in the backend's words, Custom… acting only on a pick from the list (never on the
// keyboard stepping through it, which passes it by) and leaving the value as it was, the select never
// showing a row the value isn't, and a rule PM can only show kept until it's replaced.

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RepeatPreset, RepeatRule } from "../../../lib/types";
import type { RepeatValue } from "../../../lib/calendarEdit/recurrenceDraft";

const recurrencePresets = vi.fn<(day: string) => Promise<RepeatPreset[]>>();
let mac = false;

vi.mock("../../../lib/ipc", () => ({
  recurrencePresets: (day: string) => recurrencePresets(day),
}));

vi.mock("../../../lib/setupGuide", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  get IS_MAC() {
    return mac;
  },
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
  mac = false;
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
const customRow = () => Array.from(select().options).find((o) => o.value === "custom")!;

/** A key on the closed select as Linux and Windows take it: the select's own handler, then a step
 *  to the next row that's on (the last, for End), which changes the value. */
function step(key: "ArrowDown" | "End", init: { altKey?: boolean } = {}) {
  const el = select();
  fireEvent.keyDown(el, { key, ...init });
  const on = Array.from(el.options).filter((o) => !o.disabled);
  const next = key === "End" ? on[on.length - 1] : on.find((o) => o.index > el.selectedIndex);
  if (next && next.index !== el.selectedIndex) {
    fireEvent.change(el, { target: { value: next.value } });
  }
}

/** A row picked from the open list, as a click or Enter there picks it: one that's off can't be. */
function choose(value: string) {
  const row = Array.from(select().options).find((o) => o.value === value)!;
  if (!row.disabled) fireEvent.change(select(), { target: { value } });
}

/** Let the timers a key started run. */
const settle = () => act(async () => void (await new Promise((r) => setTimeout(r, 0))));

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
  // must not open a dialog nobody asked for (C11 critique 2 M9), nor show a row the value isn't (a
  // save would send something else).
  it("arrowing past the presets never lands on Custom…", async () => {
    const { onChange, rerender, onCustom } = open({ kind: "once" });
    await act(async () => {});
    for (let i = 0; i < 5; i++) {
      step("ArrowDown");
      rerender(onChange.mock.lastCall![0]);
    }
    expect(shown()).toBe("Every weekday (Monday to Friday)");
    expect(onChange).toHaveBeenLastCalledWith({ kind: "rule", rule: PRESETS[4].rule });
    // One more, and End: the step passes Custom… by.
    step("ArrowDown");
    step("End");
    expect(shown()).toBe("Every weekday (Monday to Friday)");
    expect(onChange).toHaveBeenCalledTimes(5);
    expect(onCustom).not.toHaveBeenCalled();
    // Once the key has done its work, the list offers it again.
    await settle();
    expect(customRow().disabled).toBe(false);
    // A type-ahead letter steps too.
    fireEvent.keyDown(select(), { key: "c" });
    expect(customRow().disabled).toBe(true);
    await settle();
    expect(customRow().disabled).toBe(false);
  });

  // The key-up of a key that opened a menu instead of stepping (Up or Down on a Mac) can be swallowed
  // by that menu, so Custom… comes back without waiting for it.
  it("a pick of Custom… opens it, even straight after arrowing with no key-up", async () => {
    const { onChange, onCustom, rerender } = open({ kind: "once" });
    await act(async () => {});
    step("ArrowDown");
    rerender(onChange.mock.lastCall![0]);
    await settle();
    choose("custom");
    expect(onCustom).toHaveBeenCalledTimes(1);
    expect(shown()).toBe("Daily");
  });

  it("Space and Enter open the list: Custom… picked there opens it", async () => {
    const { onCustom } = open({ kind: "once" });
    await act(async () => {});
    fireEvent.keyDown(select(), { key: " " });
    choose("custom");
    expect(onCustom).toHaveBeenCalledTimes(1);
    fireEvent.keyDown(select(), { key: "Enter" });
    choose("custom");
    expect(onCustom).toHaveBeenCalledTimes(2);
    expect(shown()).toBe("Does not repeat");
  });

  it("Alt+Down opens the list rather than stepping: Custom… stays on offer", async () => {
    const { onCustom } = open({ kind: "once" });
    await act(async () => {});
    fireEvent.keyDown(select(), { key: "ArrowDown", altKey: true });
    choose("custom");
    expect(onCustom).toHaveBeenCalledTimes(1);
  });

  // WebKit on a Mac opens the menu on Up and Down: Custom… must be in it, on.
  it("on a Mac, Up and Down open the list with Custom… in it", async () => {
    mac = true;
    const { onCustom } = open({ kind: "once" });
    await act(async () => {});
    fireEvent.keyDown(select(), { key: "ArrowDown" });
    expect(customRow().disabled).toBe(false);
    choose("custom");
    expect(onCustom).toHaveBeenCalledTimes(1);
    // Type-ahead still steps there.
    fireEvent.keyDown(select(), { key: "c" });
    expect(customRow().disabled).toBe(true);
  });

  // A custom rule's words can run long: the select fits its row, its words cut short on screen only.
  it("is never wider than its row", async () => {
    open(monWed, "Weekly on Monday, Wednesday, Thursday and Friday, until 20-12-2026");
    await act(async () => {});
    expect(select().className.split(" ")).toContain("max-w-full");
    expect(shown()).toBe("Weekly on Monday, Wednesday, Thursday and Friday, until 20-12-2026");
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
