// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// SettingRow's contract: the label text is written ONCE and every kind of control the Settings tabs
// use is named by it — including the two that had no way to be named at all before this landed
// (`role="switch"` buttons and `role="group"` segmented pickers). Each `getByRole(…, { name })` here
// is a query that resolves to nothing in the pre-primitive markup.

import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

// `Select` reaches for `useTheme` (the terminal System's mono treatment); the real ThemeProvider
// pulls in IPC. Same stub the other component tests use.
vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { SegmentedControl } from "./SegmentedControl";
import { Select } from "./Select";
import { SettingRow } from "./SettingRow";
import { Toggle } from "./Toggle";

afterEach(cleanup);

describe("SettingRow", () => {
  it("names a Toggle from the visible label, with no ariaLabel passed", () => {
    const { getByRole, getAllByText } = render(
      <SettingRow label="Map tab">
        {(a11y) => <Toggle {...a11y} checked onChange={() => {}} />}
      </SettingRow>,
    );
    expect(getByRole("switch", { name: "Map tab" })).toBeTruthy();
    // Written once: the row renders the text, the switch borrows it.
    expect(getAllByText("Map tab")).toHaveLength(1);
  });

  it("names a SegmentedControl group from the visible label", () => {
    const { getByRole } = render(
      <SettingRow label="System">
        {(a11y) => (
          <SegmentedControl
            {...a11y}
            value="slate"
            onChange={() => {}}
            options={[
              { value: "editorial", label: "Editorial" },
              { value: "slate", label: "Slate" },
            ]}
          />
        )}
      </SettingRow>,
    );
    expect(getByRole("group", { name: "System" })).toBeTruthy();
  });

  it("names a native select from the visible label", () => {
    const { getByRole } = render(
      <SettingRow label="Zone">
        {(a11y) => (
          <Select {...a11y} value="Europe/London" onChange={() => {}}>
            <option value="Europe/London">Europe/London</option>
          </Select>
        )}
      </SettingRow>,
    );
    expect(getByRole("combobox", { name: "Zone" })).toBeTruthy();
  });

  it("mints distinct ids for two rows on one page", () => {
    const { getAllByRole } = render(
      <>
        <SettingRow label="Map tab">
          {(a11y) => <Toggle {...a11y} checked onChange={() => {}} />}
        </SettingRow>
        <SettingRow label="Help mode">
          {(a11y) => <Toggle {...a11y} checked={false} onChange={() => {}} />}
        </SettingRow>
      </>,
    );
    const [first, second] = getAllByRole("switch");
    expect(first.getAttribute("aria-labelledby")).not.toBe(second.getAttribute("aria-labelledby"));
    expect(first.id).not.toBe(second.id);
  });

  it("associates a description with the control and switches the row to top alignment", () => {
    const { getByRole, getByText } = render(
      <SettingRow label="Duplicate check" description="Adds a Check for duplicates action.">
        {(a11y) => <Toggle {...a11y} checked onChange={() => {}} />}
      </SettingRow>,
    );
    const sw = getByRole("switch");
    const describedBy = sw.getAttribute("aria-describedby");
    expect(describedBy).toBeTruthy();
    expect(document.getElementById(describedBy!)?.textContent).toBe(
      "Adds a Check for duplicates action.",
    );
    const row = getByText("Duplicate check").closest("div")?.parentElement;
    expect(row?.className).toContain("items-start");
    expect(row?.className).not.toContain("items-center");
  });

  it("centres the row and emits no wrapper when there is no description", () => {
    const { getByRole, getByText } = render(
      <SettingRow label="Map tab">
        {(a11y) => <Toggle {...a11y} checked onChange={() => {}} />}
      </SettingRow>,
    );
    const row = getByText("Map tab").parentElement;
    expect(row?.className).toContain("items-center");
    // The label and the control are siblings — no intermediate div on either side.
    expect(getByRole("switch").parentElement).toBe(row);
  });

  it("puts helpId on the row itself, where HelpOverlay's closest() will find it", () => {
    const { getByRole } = render(
      <SettingRow label="Map tab" helpId="settings-map-tab">
        {(a11y) => <Toggle {...a11y} checked onChange={() => {}} />}
      </SettingRow>,
    );
    const row = getByRole("switch").closest("[data-help]");
    expect(row?.getAttribute("data-help")).toBe("settings-map-tab");
  });

  it("renders `aside` beside the control, not beside the label", () => {
    const { getByRole, getByText } = render(
      <SettingRow label="Confirm before deleting" aside={<button>Reset</button>}>
        {(a11y) => <Toggle {...a11y} checked onChange={() => {}} />}
      </SettingRow>,
    );
    expect(getByText("Reset").parentElement).toBe(getByRole("switch").parentElement);
    expect(getByText("Confirm before deleting").parentElement).not.toBe(
      getByRole("switch").parentElement,
    );
  });

  it("swaps the label recipe for emphasis rather than layering a second one", () => {
    const { getByText, rerender } = render(
      <SettingRow label="App lock">
        {(a11y) => <Toggle {...a11y} checked onChange={() => {}} />}
      </SettingRow>,
    );
    expect(getByText("App lock").className).toBe("text-sm text-ink2");
    rerender(
      <SettingRow label="App lock" emphasis="strong">
        {(a11y) => <Toggle {...a11y} checked onChange={() => {}} />}
      </SettingRow>,
    );
    expect(getByText("App lock").className).toBe("text-sm font-medium text-ink2");
  });
});
