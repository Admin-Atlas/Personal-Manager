// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// Keyboard control of the `@` suggestion list (#276).
//
// The list shipped unable to be walked: `onKeyUp` re-read the token under the caret and reset the
// highlight, and key-up fires on the SAME physical keypress that key-down had just used to move it.
// Every Down snapped straight back to the first row. It is invisible in the markup and invisible in
// a unit test of the grammar — it only exists once both handlers are on one element — so the fix is
// pinned here, at the level the bug actually lived at.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const listTags = vi.fn();
vi.mock("../lib/ipc", () => ({ listTags: () => listTags() }));
// The mic is not under test and useRecorder reaches for browser media APIs jsdom does not have.
vi.mock("../lib/useRecorder", () => ({
  useRecorder: () => ({ state: "idle", start: vi.fn(), stop: vi.fn() }),
}));
// The same stub the other component tests use: <Textarea>/<Button> reach for `useTheme`, and the
// real ThemeProvider pulls in IPC.
vi.mock("../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({
    system: "slate",
    mode: "dark",
    modePref: "system",
    modeSource: "system",
    accent: "mono",
    depth: "standard",
    autoLocation: "",
    teachVisible: true,
    setSystem: () => {},
    setModePref: () => {},
    setAccent: () => {},
    setDepth: () => {},
    setAutoLocation: () => {},
    setTeachVisible: () => {},
  }),
}));

import { Composer } from "./Composer";

afterEach(cleanup);

// jsdom implements no layout: the composer measures its parent to cap its own height, and the
// suggestion list scrolls the highlighted row into view. Neither is what these tests are about.
beforeEach(() => {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
  Element.prototype.scrollIntoView = () => {};
});

beforeEach(() => {
  listTags.mockResolvedValue([
    { name: "marketing", kind: "group", documents: 9 },
    { name: "market-research", kind: "group", documents: 4 },
    { name: "markets", kind: "group", documents: 1 },
  ]);
});

/** Type `value` into the composer, mirroring what a real keypress fires: change, then key-up. */
function type(input: HTMLTextAreaElement, value: string) {
  fireEvent.change(input, { target: { value, selectionStart: value.length } });
  fireEvent.keyUp(input, { key: value.slice(-1) });
}

async function open() {
  const onSend = vi.fn();
  render(<Composer disabled={false} onSend={onSend} />);
  const input = screen.getByPlaceholderText(/Ask anything/) as HTMLTextAreaElement;
  type(input, "ask @mark");
  await waitFor(() => expect(screen.getAllByRole("option").length).toBeGreaterThan(1));
  return { input, onSend };
}

/** The NAME of the option the textarea points at — the same wiring a screen reader follows. (The
 *  first span; the rest of the button is the "project"/"tag" kind label.) */
function highlighted(input: HTMLTextAreaElement): string | null {
  const id = input.getAttribute("aria-activedescendant");
  if (!id) return null;
  return document.getElementById(id)?.querySelector("span")?.textContent ?? null;
}

describe("the @ suggestion list", () => {
  it("walks down and back up with the arrow keys", async () => {
    const { input } = await open();
    const first = highlighted(input);

    fireEvent.keyDown(input, { key: "ArrowDown" });
    // Key-UP for the same press: this is the event that used to undo the line above.
    fireEvent.keyUp(input, { key: "ArrowDown" });
    const second = highlighted(input);
    expect(second).not.toBe(first);

    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyUp(input, { key: "ArrowDown" });
    expect(highlighted(input)).not.toBe(second);

    fireEvent.keyDown(input, { key: "ArrowUp" });
    fireEvent.keyUp(input, { key: "ArrowUp" });
    expect(highlighted(input)).toBe(second);
  });

  it("completes the option the arrows landed on, not the first one", async () => {
    const { input } = await open();
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyUp(input, { key: "ArrowDown" });
    const chosen = highlighted(input);

    fireEvent.keyDown(input, { key: "Enter" });
    expect(input.value).toBe(`ask @${chosen} `);
  });

  // Escape has the same shape of bug: the caret is still sitting in the token, so re-reading it on
  // key-up would put the list straight back up and it could never be dismissed.
  it("stays closed after Escape", async () => {
    const { input } = await open();
    fireEvent.keyDown(input, { key: "Escape" });
    fireEvent.keyUp(input, { key: "Escape" });
    expect(screen.queryAllByRole("option")).toHaveLength(0);
  });

  // Typing genuinely changes the candidate set, so the old index means nothing — it must go back to
  // the top rather than leave the highlight on whatever now happens to sit at that position.
  it("resets the highlight when the query changes", async () => {
    const { input } = await open();
    const first = highlighted(input);
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyUp(input, { key: "ArrowDown" });
    expect(highlighted(input)).not.toBe(first);

    type(input, "ask @marke");
    await waitFor(() =>
      expect(highlighted(input)).toBe(
        screen.getAllByRole("option")[0].querySelector("span")?.textContent,
      ),
    );
  });

  // Escape closes it; typing more of the name asks for it again.
  it("re-offers the list once the user types on", async () => {
    const { input } = await open();
    fireEvent.keyDown(input, { key: "Escape" });
    fireEvent.keyUp(input, { key: "Escape" });
    expect(screen.queryAllByRole("option")).toHaveLength(0);

    type(input, "ask @marke");
    await waitFor(() => expect(screen.getAllByRole("option").length).toBeGreaterThan(0));
  });

  // Enter sends when nothing is being completed; it must not be swallowed by a closed list.
  it("sends on Enter when no suggestion list is open", async () => {
    const onSend = vi.fn();
    render(<Composer disabled={false} onSend={onSend} />);
    const input = screen.getByPlaceholderText(/Ask anything/) as HTMLTextAreaElement;
    type(input, "just a question");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onSend).toHaveBeenCalledWith("just a question");
  });
});

// What Enter does changes mid-answer — it queues instead of sending (#152) — and for one release the
// box said "Ask anything…" either way. An input that silently changes meaning and looks identical is
// the thing people report as "it ignored me", so the wording is pinned here.
describe("what the box says it will do", () => {
  it("offers to queue while a reply is streaming", () => {
    render(<Composer disabled={false} busy onSend={vi.fn()} />);
    expect(screen.getByPlaceholderText(/Queue a message/)).toBeTruthy();
    expect(screen.queryByPlaceholderText(/Ask anything/)).toBeNull();
    // The button too: it is the other half of the same promise.
    expect(screen.getByRole("button", { name: "Queue" })).toBeTruthy();
  });

  it("offers to send when nothing is in flight", () => {
    render(<Composer disabled={false} onSend={vi.fn()} />);
    expect(screen.getByPlaceholderText(/Ask anything/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Send" })).toBeTruthy();
  });

  // The one state where the box genuinely refuses. Saying "queue a message" into a disabled box
  // would be an offer PM is not making.
  it("says why it is refusing when the queue is full", () => {
    render(<Composer disabled busy onSend={vi.fn()} />);
    expect(screen.getByPlaceholderText(/Waiting for the queue to clear/)).toBeTruthy();
  });
});
