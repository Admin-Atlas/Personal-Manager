// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The thinking fold above a reply. Three things are pinned here. The header words, because the
// timing is the point of the feature and lives in them at every depth. The fold rule — open while
// the model thinks, folded once it answers, and a click by the user beats both. And the render
// boundary: thinking is untrusted model output, so it must arrive as plain text, never as markup.

import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Collapsible reaches for `useTheme`, and the real ThemeProvider pulls in IPC.
vi.mock("../theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { LiveThinkingBlock, NO_ROOM_COPY, ThinkingBlock } from "./ThinkingBlock";
import { formatThoughtSeconds, thinkingTitle } from "./thinkingWords";

const scrollIntoView = vi.fn();
beforeEach(() => {
  scrollIntoView.mockClear();
  Element.prototype.scrollIntoView = scrollIntoView;
});
afterEach(cleanup);

const header = (c: HTMLElement) => c.querySelector("button[aria-expanded]")!;
const expanded = (c: HTMLElement) => header(c).getAttribute("aria-expanded");
const box = (c: HTMLElement) => c.querySelector<HTMLElement>(".overflow-y-auto")!;

describe("the thinking header", () => {
  it("counts up while thinking, then says how long it took", () => {
    const live = (seconds: number | null) =>
      thinkingTitle({ live: true, answered: false, seconds });
    const after = (seconds: number | null) =>
      thinkingTitle({ live: false, answered: true, seconds });
    expect(live(0)).toBe("Thinking…");
    expect(live(12)).toBe("Thinking… 12 s");
    expect(after(48)).toBe("Thought for 48 s");
    expect(after(0)).toBe("Thought for under a second");
    expect(after(null)).toBe("Thought");
    expect(after(72)).toBe("Thought for 1 min 12 s");
    // A live block whose answer has started reads as finished thinking, for the rest of the stream.
    expect(thinkingTitle({ live: true, answered: true, seconds: 48 })).toBe("Thought for 48 s");
  });

  it("words minutes and seconds", () => {
    expect(formatThoughtSeconds(59)).toBe("59 s");
    expect(formatThoughtSeconds(60)).toBe("1 min");
    expect(formatThoughtSeconds(301)).toBe("5 min 1 s");
  });
});

describe("ThinkingBlock (a settled turn)", () => {
  it("starts folded, with its body out of reach", () => {
    const { container } = render(
      <ThinkingBlock text="Let me check the invoice." seconds={48} skipped={false} />,
    );
    expect(expanded(container)).toBe("false");
    expect(header(container).textContent).toContain("Thought for 48 s");
    const body = container.querySelector("[inert]");
    expect(body?.textContent).toContain("Let me check the invoice.");
  });

  it("renders the model's text literally, never as Markdown or HTML", () => {
    const hostile = "**x** <img src=x onerror=alert(1)>";
    const { container } = render(<ThinkingBlock text={hostile} seconds={3} skipped={false} />);
    fireEvent.click(header(container));
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("strong")).toBeNull();
    expect(container.querySelector("p")?.textContent).toBe(hostile);
  });

  it("says so, with no fold, when the turn was answered without thinking", () => {
    const { container } = render(<ThinkingBlock text="" seconds={null} skipped />);
    expect(container.textContent).toBe(NO_ROOM_COPY);
    expect(container.querySelector("button")).toBeNull();
  });

  describe("carrying on from the live fold", () => {
    // jsdom lays nothing out, so give every box a height: 900 px of thought in a 240 px window.
    beforeEach(() => {
      vi.spyOn(Element.prototype, "scrollHeight", "get").mockReturnValue(900);
      vi.spyOn(Element.prototype, "clientHeight", "get").mockReturnValue(240);
    });
    afterEach(() => vi.restoreAllMocks());

    const settled = (fold: { open: boolean | null; scrollTop: number | null }) =>
      render(<ThinkingBlock text="Let me check." seconds={48} skipped={false} fold={fold} />)
        .container;

    it("stays open, at the place the user had scrolled to", () => {
      const c = settled({ open: true, scrollTop: 120 });
      expect(expanded(c)).toBe("true");
      expect(box(c).scrollTop).toBe(120);
    });

    it("stays open at the end when the user was following it", () => {
      expect(box(settled({ open: true, scrollTop: null })).scrollTop).toBe(900);
    });

    it("stays folded, and reads from the top, when the user never opened it", () => {
      // The live box reports its scroll even when only its own following-the-end moved it.
      const c = settled({ open: null, scrollTop: 120 });
      expect(expanded(c)).toBe("false");
      expect(box(c).scrollTop).toBe(0);
    });

    it("stays folded when the user closed it", () => {
      expect(expanded(settled({ open: false, scrollTop: 120 }))).toBe("false");
    });
  });
});

describe("LiveThinkingBlock (while a reply streams)", () => {
  const T0 = 1_000_000;

  it("is open while the model thinks and folds when the answer starts", () => {
    const { container, rerender } = render(
      <LiveThinkingBlock thought={{ text: "hm", startedAt: T0, answeredAt: null }} />,
    );
    expect(expanded(container)).toBe("true");
    rerender(
      <LiveThinkingBlock thought={{ text: "hm", startedAt: T0, answeredAt: T0 + 48_000 }} />,
    );
    expect(expanded(container)).toBe("false");
    expect(header(container).textContent).toContain("Thought for 48 s");
  });

  it("lets a click by the user win over the automatic fold", () => {
    const { container, rerender } = render(
      <LiveThinkingBlock thought={{ text: "hm", startedAt: T0, answeredAt: null }} />,
    );
    // Closed and reopened by hand: the user has chosen OPEN, so the answer starting leaves it open.
    fireEvent.click(header(container));
    expect(expanded(container)).toBe("false");
    fireEvent.click(header(container));
    expect(expanded(container)).toBe("true");
    rerender(<LiveThinkingBlock thought={{ text: "hm", startedAt: T0, answeredAt: T0 + 5_000 }} />);
    expect(expanded(container)).toBe("true");
  });

  describe("where the user leaves it", () => {
    beforeEach(() => {
      vi.spyOn(Element.prototype, "scrollHeight", "get").mockReturnValue(900);
      vi.spyOn(Element.prototype, "clientHeight", "get").mockReturnValue(240);
    });
    afterEach(() => vi.restoreAllMocks());

    it("reports a click and a scroll, for the settled fold to carry on from", () => {
      const onFold = vi.fn();
      const { container } = render(
        <LiveThinkingBlock
          thought={{ text: "hm", startedAt: T0, answeredAt: T0 + 5_000 }}
          onFold={onFold}
        />,
      );
      // Answered, so folded by itself. The user opens it: still at its end, where it followed to.
      fireEvent.click(header(container));
      expect(onFold).toHaveBeenLastCalledWith({ open: true, scrollTop: null });
      // Scrolled up to read.
      box(container).scrollTop = 120;
      fireEvent.scroll(box(container));
      expect(onFold).toHaveBeenLastCalledWith({ open: true, scrollTop: 120 });
      // Back down to the end: following it again.
      box(container).scrollTop = 660;
      fireEvent.scroll(box(container));
      expect(onFold).toHaveBeenLastCalledWith({ open: true, scrollTop: null });
    });

    it("mounts again open, at the same place, when the user comes back mid-answer", () => {
      const { container } = render(
        <LiveThinkingBlock
          thought={{
            text: "hm",
            startedAt: T0,
            answeredAt: T0 + 5_000,
            fold: { open: true, scrollTop: 120 },
          }}
        />,
      );
      expect(expanded(container)).toBe("true");
      expect(box(container).scrollTop).toBe(120);
    });

    it("keeps a place the user scrolled up to while it thinks, instead of jumping to the end", () => {
      const thought = { text: "one", startedAt: T0, answeredAt: null };
      const fold = { open: null, scrollTop: 120 };
      const { container, rerender } = render(<LiveThinkingBlock thought={{ ...thought, fold }} />);
      expect(expanded(container)).toBe("true");
      act(() => {
        rerender(<LiveThinkingBlock thought={{ ...thought, text: "one two", fold }} />);
      });
      expect(box(container).scrollTop).toBe(120);
    });
  });

  it("follows the thought inside its own box, never by scrolling the page", () => {
    const { rerender } = render(
      <LiveThinkingBlock thought={{ text: "one", startedAt: T0, answeredAt: null }} />,
    );
    act(() => {
      rerender(
        <LiveThinkingBlock thought={{ text: "one two three", startedAt: T0, answeredAt: null }} />,
      );
    });
    expect(scrollIntoView).not.toHaveBeenCalled();
  });
});
