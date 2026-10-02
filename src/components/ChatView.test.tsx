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

import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ChatEvent } from "../lib/types";

vi.mock("../lib/capabilities", () => ({
  useDevMode: () => ({ devMode: false, setDevMode: () => {} }),
  isDevBuild: false,
}));

// `sendMessage` is for the end-to-end cases at the bottom, which run the real `useChatStream`: the
// test drives the reply's events through the captured callback, the way the Channel would.
const stream = vi.hoisted(() => ({
  onEvent: null as ((e: ChatEvent) => void) | null,
  resolve: null as (() => void) | null,
}));
vi.mock("../lib/ipc", () => ({
  listTags: () => Promise.resolve([]),
  sendMessage: (_c: number, _t: string, onEvent: (e: ChatEvent) => void) => {
    stream.onEvent = onEvent;
    return new Promise<void>((res) => {
      stream.resolve = res;
    });
  },
}));

vi.mock("../theme", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useDepth: () => ({ depth: "standard", atLeast: () => true, showPower: false }),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { ChatView } from "./ChatView";
import { useChatStream } from "../lib/useChatStream";

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

  it("follows the thinking too, still scoped", () => {
    // While the model thinks, `streaming` stays "" — only the thought grows — so without the
    // thought as a trigger the transcript stops following the reply it is waiting for.
    const thought = (text: string) => ({ text, startedAt: 0, answeredAt: null });
    const { rerender } = render(
      <ChatView messages={[message(1)]} streaming="" streamingThought={thought("Let me")} />,
    );
    scrollIntoView.mockClear();
    rerender(
      <ChatView messages={[message(1)]} streaming="" streamingThought={thought("Let me check")} />,
    );
    expect(scrollIntoView).toHaveBeenCalled();
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

describe("the thinking fold", () => {
  const reply = {
    ...message(2),
    role: "assistant" as const,
    content: "an answer",
    model: "gemma4:12b",
  };
  const fold = (c: HTMLElement) => c.querySelector('[data-help="chat-thinking"]');

  it("draws nothing for a turn with no thinking", () => {
    const { container } = render(<ChatView messages={[message(1), reply]} streaming={null} />);
    expect(fold(container)).toBeNull();
  });

  it("sits directly above its own reply", () => {
    const { container } = render(
      <ChatView
        messages={[message(1), reply]}
        streaming={null}
        thoughts={{ 2: { text: "Let me check.", seconds: 48, skipped: false } }}
      />,
    );
    const block = fold(container);
    expect(block?.textContent).toContain("Thought for 48 s");
    const answer = [...container.querySelectorAll("div")].find(
      (d) => d.textContent === "Assistant said: an answer",
    );
    expect(answer).toBeDefined();
    expect(block!.compareDocumentPosition(answer!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("is the only progress while the model thinks, with no empty reply bubble beside it", () => {
    const { container } = render(
      <ChatView
        messages={[message(1)]}
        streaming=""
        streamingThought={{ text: "Let me check", startedAt: Date.now(), answeredAt: null }}
      />,
    );
    expect(fold(container)).not.toBeNull();
    expect(container.textContent).not.toContain("Assistant said:");
    const placeholder = [...container.querySelectorAll("span")].filter(
      (s) => s.textContent === "…",
    );
    expect(placeholder).toHaveLength(0);
  });

  it("keeps the reply bubble once the answer starts", () => {
    const { container } = render(
      <ChatView
        messages={[message(1)]}
        streaming="Hi"
        streamingThought={{ text: "Let me check", startedAt: 0, answeredAt: 12_000 }}
      />,
    );
    expect(fold(container)).not.toBeNull();
    expect(container.textContent).toContain("Assistant said: Hi");
  });

  it("never announces the thinking to a screen reader", () => {
    const thought = { text: "THOUGHT-SENTINEL-7f3", startedAt: 0, answeredAt: null };
    const { container, rerender } = render(
      <ChatView messages={[message(1)]} streaming="" streamingThought={thought} />,
    );
    rerender(
      <ChatView
        messages={[message(1)]}
        streaming="The answer."
        streamingThought={{ ...thought, answeredAt: 5_000 }}
      />,
    );
    rerender(<ChatView messages={[message(1)]} streaming={null} streamingThought={null} />);
    const announcer = container.querySelector('[role="status"]');
    expect(announcer?.textContent).toBe("The answer.");
    expect(announcer?.textContent).not.toContain("THOUGHT-SENTINEL-7f3");
  });
});

describe("the thinking fold, with the real stream", () => {
  // The live fold and the settled one are separate mounts with a gap between them: the stream
  // clears a round trip before the host reloads the messages. These run the real hook into the real
  // view through that sequence, the way App does, because neither half shows the bug on its own.
  type Chat = ReturnType<typeof useChatStream>;
  function Host({ view, out }: { view: { current: number }; out: { chat?: Chat } }) {
    const chat = useChatStream(() => view.current);
    out.chat = chat;
    return (
      <ChatView
        messages={chat.messages}
        streaming={chat.streaming}
        streamingThought={chat.streamingThought}
        thoughts={chat.thoughts}
        onLiveFold={chat.noteLiveFold}
      />
    );
  }

  function chatting() {
    const view = { current: 1 };
    const out: { chat?: Chat } = {};
    const { container } = render(<Host view={view} out={out} />);
    act(() => {
      void out.chat!.send(1, "Is the invoice paid?");
    });
    const fold = () => container.querySelector('[data-help="chat-thinking"]');
    return {
      view,
      chat: () => out.chat!,
      emit: (e: ChatEvent) => act(() => stream.onEvent!(e)),
      fold,
      header: () => fold()!.querySelector("button[aria-expanded]")!,
      box: () => fold()!.querySelector<HTMLElement>(".overflow-y-auto")!,
      container,
    };
  }

  beforeEach(() => {
    // jsdom lays nothing out: 900 px of thought in its 240 px box.
    vi.spyOn(Element.prototype, "scrollHeight", "get").mockReturnValue(900);
    vi.spyOn(Element.prototype, "clientHeight", "get").mockReturnValue(240);
  });
  afterEach(() => vi.restoreAllMocks());

  it("keeps a fold the user opened mid-answer open, at their place, as the turn settles", async () => {
    const t = chatting();
    t.emit({ type: "thinking", text: "Let me check the invoice." });
    t.emit({ type: "token", text: "Paid." });
    expect(t.header().getAttribute("aria-expanded")).toBe("false"); // folded by itself
    fireEvent.click(t.header());
    t.box().scrollTop = 120;
    fireEvent.scroll(t.box());

    t.emit({
      type: "done",
      message_id: 42,
      content: "Paid.",
      citations: [],
      served_by: "local",
      on_battery: false,
    });
    await act(async () => {
      stream.resolve!();
    });
    expect(t.fold()).toBeNull(); // the gap: the stream has cleared, the reload hasn't landed
    act(() =>
      t
        .chat()
        .setMessages([
          message(41),
          { ...message(42), role: "assistant", content: "Paid.", model: "gemma4:12b" },
        ]),
    );

    expect(t.header().getAttribute("aria-expanded")).toBe("true");
    expect(t.box().scrollTop).toBe(120);
  });

  it("draws the live thinking again for a chat left and returned to mid-thought", () => {
    const t = chatting();
    t.emit({ type: "thinking", text: "a" });
    expect(t.fold()).not.toBeNull();
    t.view.current = 2;
    act(() => t.chat().clearTransient());
    t.view.current = 1;
    act(() => t.chat().clearTransient());
    expect(t.fold()).toBeNull();

    // The next thought chunk, not the first answer token (which can be minutes away), brings it back.
    t.emit({ type: "thinking", text: "b" });
    expect(t.fold()?.querySelector("p")?.textContent).toBe("ab");
    const placeholder = [...t.container.querySelectorAll("span")].filter(
      (s) => s.textContent === "…",
    );
    expect(placeholder).toHaveLength(0);
  });
});
