// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The Repeat menu (#884, C11 §6): Google's order, a rule that is none of the ready-made ones shown as
// its own row in the backend's words, the button always naming the rule held, the arrow keys moving
// the choice (the button following) and never opening Custom…, which opens on a click, Enter or
// Space and leaves the value as it was, the keyboard getting in and out of the list (Escape closing
// only it), the list inside the dialog around it, and a rule PM can only show kept until it's
// replaced.

import { useState } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
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

import { RecurrenceSelect, type Standing } from "./RecurrenceSelect";

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
const MON_WED = "Weekly on Monday and Wednesday";

// jsdom ships no ResizeObserver, and the list's popover (escapeClipping) builds one to re-place
// itself; placement isn't what these assert.
beforeAll(() => {
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
});

beforeEach(() => {
  recurrencePresets.mockResolvedValue(PRESETS);
});

afterEach(() => {
  cleanup();
  vi.resetAllMocks();
});

interface Opts {
  /** The backend's words for the rule held, if any. */
  words?: (value: RepeatValue) => string | null;
  /** The parent takes every change, as the editors do; otherwise it keeps its value. */
  follow?: boolean;
  /** An existing event's own rule, as the event editor gives it. */
  standing?: Standing;
}

/** The menu inside a dialog, as both editors hold it, with the value it's given. */
function mount(start: RepeatValue, { words = () => null, follow = true, standing }: Opts = {}) {
  const onChange = vi.fn();
  const onCustom = vi.fn();
  let setValue: (v: RepeatValue) => void = () => {};
  function Harness({ disabled }: { disabled: boolean }) {
    const [value, set] = useState(start);
    setValue = set;
    return (
      <div role="dialog" aria-label="Editor">
        <RecurrenceSelect
          value={value}
          startDate="2026-10-13"
          disabled={disabled}
          ruleWords={words(value)}
          standing={standing}
          onChange={(v) => {
            onChange(v);
            if (follow) set(v);
          }}
          onCustom={onCustom}
        />
      </div>
    );
  }
  const view = render(<Harness disabled={false} />);
  return {
    onChange,
    onCustom,
    /** The parent's value changed by something else (a moved start, the Custom dialog). */
    give: (v: RepeatValue) => act(() => setValue(v)),
    disable: (disabled: boolean) => view.rerender(<Harness disabled={disabled} />),
  };
}

const trigger = () => screen.getByRole<HTMLButtonElement>("button", { name: /^Repeat: / });
/** What the button names. */
const shown = () =>
  trigger()
    .getAttribute("aria-label")!
    .replace(/^Repeat: /, "");
const list = () => screen.queryByRole("group", { name: "Repeat" });
const rows = () =>
  within(list()!)
    .getAllByRole<HTMLInputElement>("radio")
    .map((r) => r.closest("label")!.textContent);
const radio = (name: string) => within(list()!).getByRole<HTMLInputElement>("radio", { name });
const checked = () =>
  within(list()!)
    .getAllByRole<HTMLInputElement>("radio")
    .find((r) => r.checked);
const customRow = () => within(list()!).getByRole("button", { name: "Custom…" });

/** Open the list (the ready-made rules in), focus on the rule picked as it opens. */
async function openList() {
  await act(async () => {});
  fireEvent.click(trigger());
  await waitFor(() => expect(document.activeElement).toBe(checked()));
}

/** An arrow key in the list, as the browser takes it: focus and the choice move to `name`, with a
 *  click no pointer made. */
function arrowTo(name: string, key = "ArrowDown") {
  fireEvent.keyDown(document.activeElement!, { key });
  const next = radio(name);
  next.focus();
  fireEvent.click(next, { detail: 0 });
}

describe("RecurrenceSelect", () => {
  it("lists Does not repeat, the presets and Custom… in Google's order", async () => {
    mount({ kind: "once" });
    await openList();
    expect(rows()).toEqual([
      "Does not repeat",
      "Daily",
      "Weekly on Tuesday",
      "Monthly on the second Tuesday",
      "Annually on 13 October",
      "Every weekday (Monday to Friday)",
    ]);
    // Custom… after them, an action of its own rather than one of the choices.
    const all = list()!.querySelectorAll("input, button");
    expect(all[all.length - 1]).toBe(customRow());
    expect(checked()).toBe(radio("Does not repeat"));
    expect(recurrencePresets).toHaveBeenCalledWith("2026-10-13");
  });

  it("the button always names the rule held, in words", async () => {
    const { give } = mount({ kind: "once" }, { words: (v) => (v === monWed ? MON_WED : null) });
    await act(async () => {});
    expect(shown()).toBe("Does not repeat");
    expect(trigger().textContent).toBe("Does not repeat▾");
    expect(trigger().getAttribute("aria-expanded")).toBe("false");
    await give({ kind: "rule", rule: PRESETS[1].rule });
    expect(shown()).toBe("Weekly on Tuesday");
    await give(monWed);
    expect(shown()).toBe(MON_WED);
    expect(trigger().textContent).toBe(`${MON_WED}▾`);
    await give({ kind: "kept", words: "Repeats (custom rule)" });
    expect(shown()).toBe("Repeats (custom rule)");
    await give({ kind: "once" });
    expect(shown()).toBe("Does not repeat");
  });

  it("a preset clicked is the value, and the list closes with focus on the button", async () => {
    const { onChange } = mount({ kind: "once" });
    await openList();
    expect(trigger().getAttribute("aria-expanded")).toBe("true");
    fireEvent.click(radio("Weekly on Tuesday"), { detail: 1 });
    expect(onChange).toHaveBeenCalledWith({ kind: "rule", rule: PRESETS[1].rule });
    expect(list()).toBeNull();
    expect(shown()).toBe("Weekly on Tuesday");
    expect(document.activeElement).toBe(trigger());
  });

  it("a rule outside the presets shows as its own row, picked", async () => {
    mount(monWed, { words: () => MON_WED });
    await openList();
    expect(rows()).toEqual([
      "Does not repeat",
      "Daily",
      "Weekly on Tuesday",
      "Monthly on the second Tuesday",
      "Annually on 13 October",
      "Every weekday (Monday to Friday)",
      MON_WED,
    ]);
    expect(checked()).toBe(radio(MON_WED));
    expect(shown()).toBe(MON_WED);
  });

  it("a rule equal to a preset, its days in another order, picks that preset", async () => {
    mount({
      kind: "rule",
      rule: rule({ kind: "weekly", days: ["fri", "thu", "wed", "tue", "mon"] }),
    });
    await openList();
    expect(checked()).toBe(radio("Every weekday (Monday to Friday)"));
    expect(shown()).toBe("Every weekday (Monday to Friday)");
    expect(rows()).toHaveLength(6);
  });

  it("the arrow keys change the rule and the button, the list staying open", async () => {
    const { onChange, onCustom } = mount({ kind: "once" });
    await openList();
    arrowTo("Daily");
    expect(onChange).toHaveBeenLastCalledWith({ kind: "rule", rule: PRESETS[0].rule });
    expect(shown()).toBe("Daily");
    arrowTo("Weekly on Tuesday");
    expect(shown()).toBe("Weekly on Tuesday");
    expect(checked()).toBe(radio("Weekly on Tuesday"));
    expect(list()).not.toBeNull();
    expect(onCustom).not.toHaveBeenCalled();
  });

  // The button is worked out from the value, never from the list: a change the parent doesn't take
  // leaves both naming the rule it holds.
  it("never names a rule the value doesn't hold", async () => {
    const { onChange } = mount({ kind: "once" }, { follow: false });
    await openList();
    arrowTo("Daily");
    expect(onChange).toHaveBeenCalledWith({ kind: "rule", rule: PRESETS[0].rule });
    expect(shown()).toBe("Does not repeat");
    expect(checked()).toBe(radio("Does not repeat"));
  });

  // Custom… isn't one of the radios, so the arrow keys go round the rules and never reach it (C11
  // critique 2 M9).
  it("the arrow keys never open Custom…", async () => {
    const { onChange, onCustom } = mount({ kind: "once" });
    await openList();
    const names = rows() as string[];
    for (const n of [...names.slice(1), ...names]) arrowTo(n);
    for (const n of [...names].reverse()) arrowTo(n, "ArrowUp");
    expect(onChange).toHaveBeenCalled();
    // On Custom… itself, an arrow key does nothing.
    customRow().focus();
    for (const key of ["ArrowDown", "ArrowUp", "ArrowLeft", "ArrowRight", "Home", "End"]) {
      fireEvent.keyDown(customRow(), { key });
    }
    expect(onCustom).not.toHaveBeenCalled();
    expect(list()).not.toBeNull();
  });

  // Moved off a rule of its own, the rule held keeps its row while the list is open, so an arrow
  // key passing over it doesn't lose it.
  it("the rule held when the list opened can be arrowed back to", async () => {
    let answer: string | null = MON_WED;
    const { onChange } = mount(monWed, { words: (v) => (v === monWed ? answer : null) });
    await openList();
    arrowTo("Does not repeat");
    expect(shown()).toBe("Does not repeat");
    expect(rows()).toContain(MON_WED);
    // Picked again, its words are the ones it had while the backend says them again.
    answer = null;
    arrowTo(MON_WED, "ArrowUp");
    expect(onChange).toHaveBeenLastCalledWith(monWed);
    expect(shown()).toBe(MON_WED);
    expect(checked()).toBe(radio(MON_WED));
    // Closed on another rule, it's gone.
    arrowTo("Daily");
    fireEvent.keyDown(radio("Daily"), { key: "Enter" });
    await openList();
    expect(rows()).not.toContain(MON_WED);
  });

  it("Enter picks the rule the arrow keys moved to and closes, focus on the button", async () => {
    const { onChange } = mount({ kind: "once" });
    await openList();
    arrowTo("Daily");
    expect(fireEvent.keyDown(radio("Daily"), { key: "Enter" })).toBe(false);
    expect(list()).toBeNull();
    expect(document.activeElement).toBe(trigger());
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(shown()).toBe("Daily");
  });

  it("Enter and Space on Custom… open it, the value left as it was", async () => {
    const { onChange, onCustom } = mount({ kind: "once" });
    await openList();
    fireEvent.keyDown(checked()!, { key: "Tab" });
    expect(document.activeElement).toBe(customRow());
    // Enter on a button is its click: nothing in the list may take the key first.
    expect(fireEvent.keyDown(customRow(), { key: "Enter" })).toBe(true);
    expect(list()).not.toBeNull();
    fireEvent.click(customRow(), { detail: 0 });
    expect(onCustom).toHaveBeenCalledTimes(1);
    expect(list()).toBeNull();
    // Focus is on the button, which the dialog Custom… opens hands it back to.
    expect(document.activeElement).toBe(trigger());
    // Space too: its click comes as the key goes up.
    await openList();
    fireEvent.keyDown(checked()!, { key: "Tab" });
    expect(fireEvent.keyDown(customRow(), { key: " " })).toBe(true);
    fireEvent.keyUp(customRow(), { key: " " });
    fireEvent.click(customRow(), { detail: 0 });
    expect(onCustom).toHaveBeenCalledTimes(2);
    expect(onChange).not.toHaveBeenCalled();
    expect(shown()).toBe("Does not repeat");
  });

  it("a click on Custom… opens it and keeps the value", async () => {
    const { onChange, onCustom } = mount(monWed, { words: () => MON_WED });
    await openList();
    fireEvent.click(customRow(), { detail: 1 });
    expect(onCustom).toHaveBeenCalledTimes(1);
    expect(onChange).not.toHaveBeenCalled();
    expect(list()).toBeNull();
    expect(shown()).toBe(MON_WED);
    expect(document.activeElement).toBe(trigger());
  });

  it("Tab goes from the rules to Custom… and back; out of the list, to the button", async () => {
    mount({ kind: "rule", rule: PRESETS[1].rule });
    await openList();
    // Moved here, so the browser's own Tab mustn't move it again.
    expect(fireEvent.keyDown(checked()!, { key: "Tab" })).toBe(false);
    expect(document.activeElement).toBe(customRow());
    fireEvent.keyDown(customRow(), { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(radio("Weekly on Tuesday"));
    // Shift+Tab out of the rules.
    fireEvent.keyDown(radio("Weekly on Tuesday"), { key: "Tab", shiftKey: true });
    expect(list()).toBeNull();
    expect(document.activeElement).toBe(trigger());
    // Tab out of Custom….
    await openList();
    fireEvent.keyDown(checked()!, { key: "Tab" });
    fireEvent.keyDown(customRow(), { key: "Tab" });
    expect(list()).toBeNull();
    expect(document.activeElement).toBe(trigger());
  });

  // A click on the words reaches the radio forwarded by its label, with `detail` 0 in every engine PM
  // runs on, as an arrow key's click has: the pointer going down on the row is what tells.
  it("a click on a row's words picks and closes, as one on its radio does", async () => {
    const { onChange } = mount({ kind: "once" });
    await openList();
    fireEvent.pointerDown(within(list()!).getByText("Daily"));
    fireEvent.click(radio("Daily"), { detail: 0 });
    expect(onChange).toHaveBeenLastCalledWith({ kind: "rule", rule: PRESETS[0].rule });
    expect(list()).toBeNull();
    expect(document.activeElement).toBe(trigger());
  });

  it("a key after a pointer went down on a row is the keyboard's again", async () => {
    mount({ kind: "once" });
    await openList();
    // Pressed on a row and let go off it: no click. Then an arrow key moves the choice.
    fireEvent.pointerDown(within(list()!).getByText("Daily"));
    arrowTo("Weekly on Tuesday");
    expect(shown()).toBe("Weekly on Tuesday");
    expect(list()).not.toBeNull();
  });

  // A click on a Mac leaves focus where it was (jsdom's click does the same).
  it("Escape closes only the list, focus on the button, even after a click that didn't focus it", async () => {
    const outer = vi.fn();
    window.addEventListener("keydown", outer);
    try {
      mount({ kind: "once" });
      await openList();
      fireEvent.keyDown(checked()!, { key: "Escape" });
      expect(list()).toBeNull();
      expect(document.activeElement).toBe(trigger());
      // The dialog around it never hears it.
      expect(outer).not.toHaveBeenCalled();
    } finally {
      window.removeEventListener("keydown", outer);
    }
  });

  // Mounted in the dialog itself, not inside the button's box, and placed against the window: an
  // absolute list would be cut off by the dialog's scrolling body.
  it("the list is inside the dialog around it, never clipped by it", async () => {
    mount({ kind: "once" });
    await openList();
    expect(list()!.parentElement).toBe(screen.getByRole("dialog", { name: "Editor" }));
    expect(list()!.className.split(" ")).toContain("fixed");
  });

  it("closes when it's disabled, and doesn't come back by itself", async () => {
    const { disable } = mount({ kind: "once" });
    await openList();
    disable(true);
    expect(list()).toBeNull();
    expect(trigger().disabled).toBe(true);
    disable(false);
    expect(list()).toBeNull();
  });

  // A custom rule's words can run long: they wrap on the button, which never grows past its row.
  it("a long rule's words wrap on the button, never wider than its row", async () => {
    const LONG = "Weekly on Monday, Wednesday, Thursday and Friday, until 20-12-2026";
    mount(monWed, { words: () => LONG });
    await act(async () => {});
    expect(trigger().className.split(" ")).toContain("max-w-full");
    const words = within(trigger()).getByText(LONG);
    expect(words.className).not.toMatch(/truncate|whitespace-nowrap/);
    expect(shown()).toBe(LONG);
  });

  it("a kept rule shows its words; picking a preset replaces it", async () => {
    const { onChange } = mount({ kind: "kept", words: "Repeats (custom rule)" });
    await openList();
    expect(shown()).toBe("Repeats (custom rule)");
    expect(checked()).toBe(radio("Repeats (custom rule)"));
    // Clicked again, it changes nothing.
    fireEvent.click(radio("Repeats (custom rule)"), { detail: 1 });
    expect(onChange).not.toHaveBeenCalled();
    await openList();
    fireEvent.click(radio("Daily"), { detail: 1 });
    expect(onChange).toHaveBeenCalledWith({ kind: "rule", rule: PRESETS[0].rule });
    expect(shown()).toBe("Daily");
    await openList();
    expect(rows()).not.toContain("Repeats (custom rule)");
  });

  // C11d review 1: an existing event's own rule, when no ready-made rule is it, keeps a row of its
  // own whatever the list closed on, as Google keeps it, so it can be picked back as it was.
  it("an existing event's own rule keeps its row after the list closes on another", async () => {
    const kept: RepeatValue = { kind: "kept", words: "Repeats (custom rule)" };
    for (const [own, words] of [
      [kept, "Repeats (custom rule)"],
      [monWed, MON_WED],
    ] as const) {
      const { onChange } = mount(own, { standing: { value: own, words } });
      await openList();
      arrowTo("Does not repeat");
      fireEvent.keyDown(checked()!, { key: "Escape" });
      expect(list()).toBeNull();
      expect(shown()).toBe("Does not repeat");
      await openList();
      expect(rows()).toEqual(["Does not repeat", ...PRESETS.map((p) => p.label), words]);
      fireEvent.click(radio(words), { detail: 1 });
      expect(onChange).toHaveBeenLastCalledWith(own);
      expect(shown()).toBe(words);
      // Closed on a ready-made rule, it's still there, once.
      await openList();
      fireEvent.click(radio("Daily"), { detail: 1 });
      await openList();
      expect(rows().filter((r) => r === words)).toHaveLength(1);
      cleanup();
    }
    // A ready-made rule of its own has that rule's row and no other.
    const weekly: RepeatValue = { kind: "rule", rule: PRESETS[1].rule };
    mount(weekly, { standing: { value: weekly, words: "Weekly on Tuesday" } });
    await openList();
    expect(rows()).toHaveLength(6);
  });

  it("presets still loading never show Does not repeat as picked for a rule", async () => {
    let arrive: (presets: RepeatPreset[]) => void = () => {};
    recurrencePresets.mockReturnValue(new Promise<RepeatPreset[]>((r) => (arrive = r)));
    mount({ kind: "rule", rule: PRESETS[1].rule });
    await openList();
    expect(rows()).toEqual(["Does not repeat", "Repeats"]);
    expect(checked()).toBe(radio("Repeats"));
    expect(shown()).toBe("Repeats");
    // They come while the list is open: the rule is its preset's row, and has no other.
    await act(async () => arrive(PRESETS));
    expect(rows()).toHaveLength(6);
    expect(checked()).toBe(radio("Weekly on Tuesday"));
    expect(shown()).toBe("Weekly on Tuesday");
    // The row focus was on went with them: focus is on the rule's new row, still in the list.
    await waitFor(() => expect(document.activeElement).toBe(radio("Weekly on Tuesday")));
    expect(list()).not.toBeNull();
  });

  it("presets arriving leave focus where it was in the list", async () => {
    let arrive: (presets: RepeatPreset[]) => void = () => {};
    recurrencePresets.mockReturnValue(new Promise<RepeatPreset[]>((r) => (arrive = r)));
    mount({ kind: "rule", rule: PRESETS[1].rule });
    await openList();
    fireEvent.keyDown(checked()!, { key: "Tab" });
    expect(document.activeElement).toBe(customRow());
    await act(async () => arrive(PRESETS));
    expect(rows()).toHaveLength(6);
    // Custom… is still there: nothing takes focus off it, however long it waits.
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10));
    });
    expect(document.activeElement).toBe(customRow());
  });
});
