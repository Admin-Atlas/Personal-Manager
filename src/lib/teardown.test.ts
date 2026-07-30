// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { beforeEach, describe, expect, it, vi } from "vitest";
import { beginTeardown, isTearingDown, onTeardown, resetTeardownForTests } from "./teardown";

describe("teardown signal", () => {
  beforeEach(() => resetTeardownForTests());

  it("starts disarmed, so nothing changes until an erase actually begins", () => {
    expect(isTearingDown()).toBe(false);
  });

  it("tells every subscriber to stand down", () => {
    const poller = vi.fn();
    const other = vi.fn();
    onTeardown(poller);
    onTeardown(other);

    beginTeardown();

    expect(isTearingDown()).toBe(true);
    expect(poller).toHaveBeenCalledOnce();
    expect(other).toHaveBeenCalledOnce();
  });

  it("fires immediately for anything that subscribes late", () => {
    // The window that matters: a component mounting after the wipe has started must not sit there
    // polling a machine that is being erased, just because it missed the announcement.
    beginTeardown();
    const late = vi.fn();
    onTeardown(late);
    expect(late).toHaveBeenCalledOnce();
  });

  it("is idempotent, so a retried wipe does not double-notify", () => {
    const listener = vi.fn();
    onTeardown(listener);
    beginTeardown();
    beginTeardown();
    expect(listener).toHaveBeenCalledOnce();
  });

  it("keeps going when one subscriber throws", () => {
    // These run during an irreversible erase. One broken unsubscribe must not leave the rest armed.
    const after = vi.fn();
    onTeardown(() => {
      throw new Error("boom");
    });
    onTeardown(after);

    expect(() => beginTeardown()).not.toThrow();
    expect(after).toHaveBeenCalledOnce();
  });

  it("stops notifying once unsubscribed", () => {
    const listener = vi.fn();
    const off = onTeardown(listener);
    off();
    beginTeardown();
    expect(listener).not.toHaveBeenCalled();
  });
});
