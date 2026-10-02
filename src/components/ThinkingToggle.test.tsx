// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The composer's Thinking button. The load-bearing test is the zero-pixel one: it only means
// something while chat is really going to a local model, so a cloud-only user — and a chat the On
// battery policy has moved — must see nothing at all. The edge is pinned to `--ink4` in both states
// because the obvious alternative, an accent edge, measures 1.36:1 on a pale accent (contrast.test).

import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { THINKING_TITLE_OFF, THINKING_TITLE_ON, ThinkingToggle } from "./ThinkingToggle";
import type { LocalLlmStatus } from "../lib/types";
import { INERT_POWER_VIEW } from "../lib/powerRoute";
import { readShowThinking, resetShowThinking } from "../lib/chatPrefs";

const KEY = "pm.chat.showThinking";

const st = (over: Partial<LocalLlmStatus>): LocalLlmStatus => ({
  configured: true,
  reachable: true,
  in_cooldown: false,
  cooldown_remaining_s: 0,
  probed_now: true,
  chat_local_model: "gemma4:12b",
  background_local_model: null,
  served_window: null,
  served_window_proven: false,
  window_source: null,
  chat_answering: false,
  background_answering: false,
  chat_loaded: null,
  background_loaded: null,
  chat_released: false,
  background_released: false,
  power: INERT_POWER_VIEW,
  ...over,
});

const button = (c: HTMLElement) => c.querySelector("button")!;

beforeEach(() => localStorage.clear());
afterEach(cleanup);

describe("ThinkingToggle", () => {
  it("renders nothing until chat is going to a local model", () => {
    expect(render(<ThinkingToggle status={null} />).container.firstChild).toBeNull();
    // Cloud-only, or a chat the On battery policy has moved: the backend reports no chat model.
    expect(
      render(<ThinkingToggle status={st({ chat_local_model: null })} />).container.firstChild,
    ).toBeNull();
  });

  it("starts off, and reads a stored on", () => {
    const off = render(<ThinkingToggle status={st({})} />).container;
    expect(button(off).getAttribute("aria-pressed")).toBe("false");
    cleanup();
    localStorage.setItem(KEY, "1");
    const on = render(<ThinkingToggle status={st({})} />).container;
    expect(button(on).getAttribute("aria-pressed")).toBe("true");
  });

  it("writes and announces a click, and removes the key on the second", () => {
    let heard = 0;
    const bump = () => void heard++;
    window.addEventListener("pm:settings-changed", bump);
    const { container } = render(<ThinkingToggle status={st({})} />);

    fireEvent.click(button(container));
    expect(localStorage.getItem(KEY)).toBe("1");
    expect(heard).toBe(1);
    expect(button(container).getAttribute("aria-pressed")).toBe("true");

    fireEvent.click(button(container));
    expect(localStorage.getItem(KEY)).toBeNull();
    expect(button(container).getAttribute("aria-pressed")).toBe("false");
    window.removeEventListener("pm:settings-changed", bump);
  });

  it("shows what was stored when the write fails, because that is what the next send reads", () => {
    // Storage full or blocked: the pref stays off, so a button showing "on" would misreport every
    // send that follows.
    const setItem = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new DOMException("full", "QuotaExceededError");
    });
    try {
      const { container } = render(<ThinkingToggle status={st({})} />);
      fireEvent.click(button(container));
      expect(readShowThinking()).toBe(false);
      expect(button(container).getAttribute("aria-pressed")).toBe("false");
      expect(container.querySelector("span[aria-hidden]")).toBeNull();
    } finally {
      setItem.mockRestore();
    }
  });

  it("follows a reset made elsewhere", () => {
    // "Reset General" runs in the Settings overlay while this composer stays mounted under it.
    localStorage.setItem(KEY, "1");
    const { container } = render(<ThinkingToggle status={st({})} />);
    expect(button(container).getAttribute("aria-pressed")).toBe("true");
    act(() => resetShowThinking());
    expect(button(container).getAttribute("aria-pressed")).toBe("false");
  });

  it("edges both states in --ink4, never the accent", () => {
    const { container } = render(<ThinkingToggle status={st({})} />);
    const off = button(container).className;
    fireEvent.click(button(container));
    const on = button(container).className;
    for (const cls of [off, on]) {
      expect(cls.split(" ")).toContain("border-ink4");
      expect(cls).not.toContain("border-accent");
    }
    // The on state's fill pairs with its own ink, the pair contrast.test measures.
    expect(on.split(" ")).toEqual(expect.arrayContaining(["bg-accent", "text-accent-ink"]));
  });

  it("marks the on state with a dot as well as the fill", () => {
    const { container } = render(<ThinkingToggle status={st({})} />);
    expect(container.querySelector("span[aria-hidden]")).toBeNull();
    fireEvent.click(button(container));
    expect(container.querySelector("span[aria-hidden]")).not.toBeNull();
    expect(button(container).textContent).toBe("Thinking");
  });

  it("says what each state does", () => {
    const { container } = render(<ThinkingToggle status={st({})} />);
    expect(button(container).getAttribute("title")).toBe(
      "Show your model's thinking above each reply. On Ollama this also lets the model think before it answers, so replies take longer — often tens of seconds.",
    );
    expect(button(container).getAttribute("title")).toBe(THINKING_TITLE_OFF);
    fireEvent.click(button(container));
    expect(button(container).getAttribute("title")).toBe(
      "Thinking is on: your model's thinking appears above each reply. Click to turn it off — on Ollama, replies come back quicker without it.",
    );
    expect(button(container).getAttribute("title")).toBe(THINKING_TITLE_ON);
  });
});
