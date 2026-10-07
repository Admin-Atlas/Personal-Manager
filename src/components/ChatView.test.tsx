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
afterEach(() => {
  cleanup();
  delete document.documentElement.dataset.reducedMotion;
});

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

describe("Reduced motion", () => {
  // The autoscroll was hard-coded `behavior: "smooth"`, which bypasses CSS entirely — `index.css`'s
  // `scroll-behavior: auto !important` is only consulted when `behavior` is "auto", so the in-app
  // Accessibility → Motion → "Reduced" setting could not reach it and never had. The `../theme` mock
  // above spreads the real module, so `scrollBehavior` here is the real helper reading the real DOM
  // stamp — which is precisely why it needed no addition to that partial `useTheme` mock.
  it("snaps instantly under the in-app setting, without losing the nearest-scroller guarantee", () => {
    document.documentElement.dataset.reducedMotion = "on";
    render(<ChatView messages={[message(1)]} streaming={null} />);
    expect(scrollIntoView).toHaveBeenCalled();
    for (const call of scrollIntoView.mock.calls) {
      expect(
        call[0]?.behavior,
        "an explicit smooth scroll ignores the Reduced motion setting",
      ).toBe("auto");
      // Pinned together on purpose: the two guarantees live in the same options object, and a fix
      // to one has already been the way the other got dropped.
      expect(call[0]?.block).toBe("nearest");
    }
  });

  it("glides when nothing asks for reduced motion", () => {
    render(<ChatView messages={[message(1)]} streaming={null} />);
    expect(scrollIntoView).toHaveBeenCalled();
    for (const call of scrollIntoView.mock.calls) {
      expect(call[0]?.behavior).toBe("smooth");
    }
  });
});

describe("the provenance footer", () => {
  const reply = {
    ...message(2),
    role: "assistant" as const,
    content: "an answer",
    model: "openai/gpt-test",
  };

  it("says when a reply went to the cloud because of the battery", () => {
    // A power-routed turn is a deliberate route (#432), so it is worded in the footer — plainly,
    // with its reason on hover — rather than surfacing as the fallback strip.
    const { container } = render(
      <ChatView
        messages={[message(1), reply]}
        streaming={null}
        showProvenance
        providers={{ 2: "cloud-on-battery" }}
      />,
    );
    const footer = container.querySelector('[data-help="chat-provenance"]');
    expect(footer?.textContent).toBe("via gpt-test · cloud, on battery");
    expect(footer?.getAttribute("title")).toBe(
      "Sent to your cloud model because you were on battery.",
    );
  });

  it("keeps the plain words, and no battery title, for an ordinary turn", () => {
    const { container } = render(
      <ChatView
        messages={[message(1), reply]}
        streaming={null}
        showProvenance
        providers={{ 2: "cloud" }}
      />,
    );
    const footer = container.querySelector('[data-help="chat-provenance"]');
    expect(footer?.textContent).toBe("via gpt-test · cloud");
    expect(footer?.hasAttribute("title")).toBe(false);
  });
});
