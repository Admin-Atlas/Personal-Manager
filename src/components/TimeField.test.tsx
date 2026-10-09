// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The event editor's time picker. What matters: the time Google holds is always on offer (so an
// untouched field sends nothing, and a slip can be undone), a pick hands back a whole `HH:MM`, and a
// real `<label>` names it.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TimeField } from "./TimeField";

// Select reaches for `useTheme`; the real ThemeProvider pulls in IPC.
vi.mock("../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate" }),
}));

afterEach(cleanup);

const options = (select: HTMLSelectElement) => [...select.options].map((o) => o.value);

describe("TimeField", () => {
  it("offers every quarter hour and shows the value", () => {
    render(<TimeField value="09:15" onChange={() => {}} ariaLabel="Starts" />);
    const select = screen.getByLabelText<HTMLSelectElement>("Starts");
    expect(select.value).toBe("09:15");
    expect(select.options).toHaveLength(96);
    expect(select.options[0].value).toBe("00:00");
    expect(select.options[95].value).toBe("23:45");
  });

  it("keeps Google's off-grid time on offer, in order, after a slip", () => {
    const { rerender } = render(
      <TimeField value="09:10" held="09:10" onChange={() => {}} ariaLabel="Starts" />,
    );
    const select = screen.getByLabelText<HTMLSelectElement>("Starts");
    expect(select.value).toBe("09:10");
    expect(options(select).slice(36, 39)).toEqual(["09:00", "09:10", "09:15"]);
    rerender(<TimeField value="09:15" held="09:10" onChange={() => {}} ariaLabel="Starts" />);
    expect(options(select)).toContain("09:10");
    expect(options(select)).toHaveLength(97);
  });

  it("hands the picked time to the caller", () => {
    const onChange = vi.fn();
    render(<TimeField value="09:00" onChange={onChange} ariaLabel="Starts" />);
    fireEvent.change(screen.getByLabelText("Starts"), { target: { value: "13:45" } });
    expect(onChange).toHaveBeenCalledWith("13:45");
  });

  it("takes its name from a wrapping label", () => {
    render(
      <label>
        Ends
        <TimeField value="10:00" onChange={() => {}} />
      </label>,
    );
    // By role, not by label text: an aria-label would win the name and still pass getByLabelText.
    expect(screen.getByRole("combobox", { name: "Ends" })).toBeTruthy();
  });
});
