// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The autoscroll's blast radius.
//
// `scrollIntoView` scrolls EVERY scrollable ancestor, and the document is always one of them. Called
// without `block`, it defaults to "start" — so the instant anything makes the page a pixel taller
// than the viewport, snapping to the newest turn scrolls the entire app out of the window, and
// nothing scrolls it back. That shipped: a one-frame element above the composer was enough, and the
// whole UI slid up leaving the page background behind it.
//
// `block: "nearest"` is what confines the scroll to the transcript's own scroller. It is invisible,
// it looks like a cosmetic argument, and removing it breaks the entire app rather than the chat — so
// it is pinned here.

import { cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../lib/capabilities", () => ({
  useDevMode: () => ({ devMode: false, setDevMode: () => {} }),
  isDevBuild: false,
}));

vi.mock("../lib/ipc", () => ({
  listTags: () => Promise.resolve([]),
}));

vi.mock("../theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useDepth: () => ({ depth: "standard", atLeast: () => true, showPower: false }),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { ChatView } from "./ChatView";

const scrollIntoView = vi.fn();

beforeEach(() => {
  vi.clearAllMocks();
  Element.prototype.scrollIntoView = scrollIntoView;
});
afterEach(cleanup);

function message(id: number) {
  return {
    id,
    conversation_id: 1,
    role: "user" as const,
    content: `turn ${id}`,
    model: null,
    created_at: "2026-07-28T10:00:00Z",
  };
}

describe("snapping to the newest turn", () => {
  it("never scrolls anything but the nearest scroller", () => {
    render(<ChatView messages={[message(1)]} streaming={null} />);
    expect(scrollIntoView).toHaveBeenCalled();
    for (const call of scrollIntoView.mock.calls) {
      expect(
        call[0]?.block,
        "an unscoped scrollIntoView scrolls the DOCUMENT and slides the whole app out of view",
      ).toBe("nearest");
    }
  });

  it("stays scoped while a reply streams in", () => {
    // The streaming effect fires per token, so an unscoped call here would drag the app up
    // repeatedly rather than once.
    const { rerender } = render(<ChatView messages={[message(1)]} streaming="" />);
    scrollIntoView.mockClear();
    rerender(<ChatView messages={[message(1)]} streaming="partial answer" />);
    for (const call of scrollIntoView.mock.calls) {
      expect(call[0]?.block).toBe("nearest");
    }
  });
});
