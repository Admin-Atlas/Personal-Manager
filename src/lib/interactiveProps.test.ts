// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { KeyboardEvent } from "react";
import { describe, expect, it, vi } from "vitest";
import { interactiveProps } from "./interactiveProps";

function keyEvent(key: string) {
  return { key, preventDefault: vi.fn() } as unknown as KeyboardEvent<HTMLElement>;
}

describe("interactiveProps", () => {
  it("exposes a button role and a tab stop", () => {
    const p = interactiveProps(() => {});
    expect(p.role).toBe("button");
    expect(p.tabIndex).toBe(0);
  });

  it("activates on Enter and Space, preventing the default", () => {
    const onActivate = vi.fn();
    const p = interactiveProps(onActivate);
    for (const key of ["Enter", " "]) {
      const e = keyEvent(key);
      p.onKeyDown(e);
      expect(e.preventDefault).toHaveBeenCalledOnce();
    }
    expect(onActivate).toHaveBeenCalledTimes(2);
  });

  it("ignores other keys", () => {
    const onActivate = vi.fn();
    const p = interactiveProps(onActivate);
    const e = keyEvent("ArrowDown");
    p.onKeyDown(e);
    expect(onActivate).not.toHaveBeenCalled();
    expect(e.preventDefault).not.toHaveBeenCalled();
  });

  it("onClick calls the activator", () => {
    const onActivate = vi.fn();
    interactiveProps(onActivate).onClick();
    expect(onActivate).toHaveBeenCalledOnce();
  });
});
