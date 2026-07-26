// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import {
  CONNECTOR_POLL_MS,
  SHARED_WITH_ME_POLL_MS,
  shouldIncludeSharedWithMe,
} from "./connectorPoll";

describe("shouldIncludeSharedWithMe", () => {
  const T0 = 1_700_000_000_000;

  it("includes it on the first pass of a session, so opening PM is a full refresh", () => {
    expect(shouldIncludeSharedWithMe(null, T0)).toBe(true);
  });

  it("skips it on the frequent ticks in between", () => {
    expect(shouldIncludeSharedWithMe(T0, T0 + CONNECTOR_POLL_MS)).toBe(false);
    expect(shouldIncludeSharedWithMe(T0, T0 + CONNECTOR_POLL_MS * 3)).toBe(false);
  });

  it("includes it once the interval has elapsed", () => {
    expect(shouldIncludeSharedWithMe(T0, T0 + SHARED_WITH_ME_POLL_MS)).toBe(true);
    expect(shouldIncludeSharedWithMe(T0, T0 + SHARED_WITH_ME_POLL_MS + 1)).toBe(true);
  });

  it("is exclusive just below the boundary", () => {
    expect(shouldIncludeSharedWithMe(T0, T0 + SHARED_WITH_ME_POLL_MS - 1)).toBe(false);
  });

  it("copes with a clock that jumped backwards rather than firing forever", () => {
    // A machine waking from sleep, or an NTP correction, can move `now` behind the recorded stamp.
    // The comparison must simply be false until real time catches up — never negative-elapsed logic
    // that reads as "an hour ago".
    expect(shouldIncludeSharedWithMe(T0, T0 - 60_000)).toBe(false);
  });

  it("honours an injected interval", () => {
    expect(shouldIncludeSharedWithMe(T0, T0 + 500, 1000)).toBe(false);
    expect(shouldIncludeSharedWithMe(T0, T0 + 1000, 1000)).toBe(true);
  });

  it("polls shared-with-me far less often than the delta corpora — it has no cursor", () => {
    expect(SHARED_WITH_ME_POLL_MS).toBeGreaterThan(CONNECTOR_POLL_MS);
  });
});
