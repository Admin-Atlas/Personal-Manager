// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The zone picker the time grid and the event editor share: filtering, leaving out zones already
// chosen, and handing back the picked IANA id.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ZoneSearchList } from "./ZoneSearchList";

afterEach(cleanup);

const filterTo = (text: string) =>
  fireEvent.change(screen.getByLabelText("Filter timezones"), { target: { value: text } });

describe("ZoneSearchList", () => {
  it("filters, and hands back the picked zone", () => {
    const onPick = vi.fn();
    render(<ZoneSearchList onPick={onPick} />);
    filterTo("tokyo");
    fireEvent.click(screen.getByTitle("Asia/Tokyo"));
    expect(onPick).toHaveBeenCalledWith("Asia/Tokyo");
    // The filter clears for the next pick.
    expect(screen.getByLabelText<HTMLInputElement>("Filter timezones").value).toBe("");
  });

  it("leaves out zones already chosen", () => {
    render(<ZoneSearchList onPick={() => {}} exclude={["Asia/Tokyo"]} />);
    filterTo("tokyo");
    expect(screen.queryByTitle("Asia/Tokyo")).toBeNull();
    expect(screen.getByText("No match.")).toBeTruthy();
  });
});
