// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the game REPORTS, which is a different question from what the rules decide.
//
// `game.ts` is pure and thoroughly pinned, and `usePinboard` is tested against the hook directly.
// Between them sat the bug: a verdict game correctly worked out that you had dodged a card and said
// so — `onDraw(id, false)` — and the two adapters in `PinboardView` were declared one parameter
// short, so the flag was dropped and the card was popped onto the board anyway, while the screen
// said "You're off the hook." Both halves were individually green. Nothing tested the sentence they
// were passing between them.
//
// The wiring itself is now the compiler's job (`drawCard`'s third parameter is required, so an
// adapter that forgets it fails to build). These tests own the other end: that the game states the
// outcome at all, and states it correctly for each of the four ways a verdict round can end.

import { cleanup, render, screen, fireEvent } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { FolderGame } from "./FolderGame";
import type { Widget } from "../lib/pinboard/types";

// The same stub the other component tests use: the game's <Button>s reach for `useTheme`, and the
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

const card = (id: string, title: string): Widget => ({
  id,
  kind: "note",
  rect: { x: 0, y: 0, w: 7, h: 5 },
  title,
});

const folder = (over: Partial<Widget> = {}): Widget => ({
  id: "f",
  kind: "folder",
  rect: { x: 0, y: 0, w: 8, h: 6 },
  title: "Chores",
  children: [card("a", "ring the dentist")],
  ...over,
});

/** Pin the CSPRNG so the coin and the throw land where the test needs them. `randomUnit` reads one
 *  `Uint32` and divides by 2^32, so the value below IS the unit interval, scaled. */
function pinRandom(unit: number) {
  vi.spyOn(globalThis.crypto, "getRandomValues").mockImplementation(((buf: Uint32Array) => {
    buf[0] = Math.min(0xffffffff, Math.floor(unit * 2 ** 32));
    return buf;
  }) as typeof crypto.getRandomValues);
}

/** No animation, so a settled round is on screen synchronously and no timer is left to fire. Uses
 *  the in-app signal rather than a `matchMedia` stub because that one is a documented one-way
 *  override, so it cannot be undone by the environment underneath it. */
function noMotion() {
  document.documentElement.dataset.reducedMotion = "on";
}

const handlers = () => ({
  onDraw: vi.fn(),
  onPopOut: vi.fn(),
  onWeight: vi.fn(),
  onResetRound: vi.fn(),
  onAutoPopOut: vi.fn(),
  onRepeat: vi.fn(),
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  delete document.documentElement.dataset.reducedMotion;
});

describe("a verdict game says whether the card is yours", () => {
  it("reports a coin you LOST as an assignment", () => {
    noMotion();
    pinRandom(0.1); // < 0.5 → heads → the job is yours
    const h = handlers();
    render(<FolderGame folder={folder()} game="coin" roomy={false} {...h} />);
    fireEvent.click(screen.getByRole("button", { name: /offer me one/i }));
    fireEvent.click(screen.getByRole("button", { name: /^flip/i }));
    expect(h.onDraw).toHaveBeenCalledWith("a", true);
    expect(screen.getByText("That one's yours.")).toBeTruthy();
  });

  it("reports a coin you WON as NOT an assignment", () => {
    // The regression. A dodged card has had its turn this round, so it is still reported — but it
    // is emphatically not work, and reporting it as one is what put it on the board.
    noMotion();
    pinRandom(0.9); // >= 0.5 → tails → off the hook
    const h = handlers();
    render(<FolderGame folder={folder()} game="coin" roomy={false} {...h} />);
    fireEvent.click(screen.getByRole("button", { name: /offer me one/i }));
    fireEvent.click(screen.getByRole("button", { name: /^flip/i }));
    expect(h.onDraw).toHaveBeenCalledWith("a", false);
    expect(screen.getByText("You're off the hook.")).toBeTruthy();
  });

  it("reports a throw you LOST as an assignment, and one you WON as not", () => {
    noMotion();
    // THROWS is [rock, paper, scissors] and PM's hand is index floor(unit * 3).
    pinRandom(0.5); // → paper. Rock loses to paper; scissors beats it.
    const lost = handlers();
    const { unmount } = render(<FolderGame folder={folder()} game="rps" roomy={false} {...lost} />);
    fireEvent.click(screen.getByRole("button", { name: /offer me one/i }));
    fireEvent.click(screen.getByRole("button", { name: /^rock$/i }));
    expect(lost.onDraw).toHaveBeenCalledWith("a", true);
    unmount();

    const won = handlers();
    render(<FolderGame folder={folder()} game="rps" roomy={false} {...won} />);
    fireEvent.click(screen.getByRole("button", { name: /offer me one/i }));
    fireEvent.click(screen.getByRole("button", { name: /^scissors$/i }));
    expect(won.onDraw).toHaveBeenCalledWith("a", false);
  });

  it("reports NOTHING on a tie — nobody won, so the card has not had its turn", () => {
    noMotion();
    pinRandom(0.5); // → paper, against paper
    const h = handlers();
    render(<FolderGame folder={folder()} game="rps" roomy={false} {...h} />);
    fireEvent.click(screen.getByRole("button", { name: /offer me one/i }));
    fireEvent.click(screen.getByRole("button", { name: /^paper$/i }));
    expect(h.onDraw).not.toHaveBeenCalled();
    expect(screen.getByText("A tie — go again.")).toBeTruthy();
  });

  it("withholds the move-out button on a card you dodged", () => {
    // The other half of the same promise: a dodged card is not offered to the board by hand either.
    // If this button appeared, the copy above it would be the only thing saying you were off the
    // hook — which is exactly the state the bug shipped in.
    noMotion();
    pinRandom(0.9);
    const h = handlers();
    render(<FolderGame folder={folder()} game="coin" roomy={false} {...h} />);
    fireEvent.click(screen.getByRole("button", { name: /offer me one/i }));
    fireEvent.click(screen.getByRole("button", { name: /^flip/i }));
    expect(screen.queryByRole("button", { name: /move out to the board/i })).toBeNull();
  });
});

describe("a draw game always hands out work", () => {
  it("reports its winner as an assignment", () => {
    // The three draw games have no losing side — the wheel only ever names the next thing to do —
    // so their flag is a constant. Pinned so it stays one.
    noMotion();
    pinRandom(0.1); // the first of two equal wedges
    const h = handlers();
    // TWO cards, because a folder down to its last one says "Take it" instead of naming the game —
    // there is nothing left to spin for, which is its own (deliberate) piece of behaviour.
    const two = folder({ children: [card("a", "ring the dentist"), card("b", "book the MOT")] });
    render(<FolderGame folder={two} game="roulette" roomy={false} {...h} />);
    fireEvent.click(screen.getByRole("button", { name: "Spin" }));
    expect(h.onDraw).toHaveBeenCalledWith("a", true);
  });
});
