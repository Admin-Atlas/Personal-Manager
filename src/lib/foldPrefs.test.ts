// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A remembered fold: a group you never touched follows its default (so Depth still folds it), and one
// you did keeps your choice across a remount, which is what reopening Settings or restarting is.

import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { readFold, useRememberedFold, writeFold } from "./foldPrefs";

beforeEach(() => localStorage.clear());

describe("foldPrefs", () => {
  it("reads nothing for a fold never chosen, and ignores a value it didn't write", () => {
    expect(readFold("settings.connectors.Google")).toBeNull();
    localStorage.setItem("pm.fold.settings.connectors.Google", "true");
    expect(readFold("settings.connectors.Google")).toBeNull();
  });

  it("round-trips open and closed", () => {
    writeFold("g", false);
    expect(readFold("g")).toBe(false);
    writeFold("g", true);
    expect(readFold("g")).toBe(true);
  });

  it("follows the default until chosen, then keeps the choice over a new default and a remount", () => {
    const first = renderHook(({ fallback }) => useRememberedFold("g", fallback), {
      initialProps: { fallback: true },
    });
    expect(first.result.current[0]).toBe(true);
    first.rerender({ fallback: false }); // Depth changed, nothing chosen yet
    expect(first.result.current[0]).toBe(false);

    act(() => first.result.current[1](true));
    first.rerender({ fallback: false });
    expect(first.result.current[0]).toBe(true);
    first.unmount();

    const again = renderHook(() => useRememberedFold("g", false));
    expect(again.result.current[0]).toBe(true);
  });
});
