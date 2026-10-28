// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The sign-in strip. PM opens a sign-in page in the default browser, which isn't always the one
// signed in to the right account, so these pin that a strip offers that page's link to copy and to
// open again for exactly as long as the backend waits on it; that every sign-in waiting gets its own
// strip, and one ending never hides another's; that a browser which wouldn't open is said plainly,
// even when the backend says so after first showing the page; that the strip draws wherever it's
// placed from the one feed App follows (App's place, or Settings' own, since Settings' scrim covers
// App's); and that the link itself, which carries the sign-in's one-time state, is never written out
// on the page.

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { SignInPage } from "../lib/types";

let opened: ((page: SignInPage) => void) | null = null;
let closed: ((id: number) => void) | null = null;
const openUrl = vi.fn();

vi.mock("../lib/ipc", () => ({
  onSignInOpened: (h: (page: SignInPage) => void) => {
    opened = h;
    return Promise.resolve(() => {
      opened = null;
    });
  },
  onSignInClosed: (h: (id: number) => void) => {
    closed = h;
    return Promise.resolve(() => {
      closed = null;
    });
  },
  openUrl: (...a: unknown[]) => openUrl(...a),
}));

vi.mock("../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { signInClosed, useFollowSignIns } from "../lib/signInPages";
import { SignInStrip } from "./SignInStrip";

const GOOGLE: SignInPage = {
  id: 1,
  provider: "Google",
  url: "https://accounts.google.com/o/oauth2/v2/auth?client_id=x&state=s1",
  opened: true,
};
const PROTON: SignInPage = {
  id: 2,
  provider: "Proton",
  url: "https://account.proton.me/authorize?x=1",
  opened: true,
};

/** App's part: follow the sign-ins, and draw the strip in `strips` places. */
function Harness({ strips }: { strips: number }) {
  useFollowSignIns();
  return (
    <>
      {Array.from({ length: strips }, (_, i) => (
        <div key={i} data-testid={`place${i}`}>
          <SignInStrip />
        </div>
      ))}
    </>
  );
}

/** Mount it, and let both subscriptions land, as App does long before any sign-in. */
async function mount(strips = 1) {
  const view = render(<Harness strips={strips} />);
  await act(async () => {});
  return view;
}

const open = (page: SignInPage) => act(() => opened?.(page));
const close = (id: number) => act(() => closed?.(id));
const copyName = (provider: string) => `Copy link to the ${provider} sign-in`;

beforeEach(() => {
  openUrl.mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
  // The feed is the module's, so what one test left waiting would show in the next.
  for (const id of [1, 2]) signInClosed(id);
  vi.clearAllMocks();
  Object.defineProperty(navigator, "clipboard", { value: undefined, configurable: true });
});

describe("SignInStrip", () => {
  it("shows nothing until a sign-in starts, and goes when it ends", async () => {
    const { container } = await mount();
    expect(container.textContent).toBe("");
    open(GOOGLE);
    expect(screen.getByRole("status").textContent).toContain(
      "Finish signing in to Google in your browser.",
    );
    close(1);
    expect(container.textContent).toBe("");
  });

  it("copies the page's link and opens it again, without writing it out", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const { container } = await mount();
    open(GOOGLE);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: copyName("Google") }));
    });
    expect(writeText).toHaveBeenCalledWith(GOOGLE.url);
    fireEvent.click(screen.getByRole("button", { name: "Open again" }));
    expect(openUrl).toHaveBeenCalledWith(GOOGLE.url);
    expect(container.innerHTML).not.toContain("accounts.google.com");
  });

  it("says so when the browser couldn't be opened, also when that comes after the page", async () => {
    await mount();
    open(GOOGLE);
    // The backend shows the page first, then says its browser wouldn't open: the same id again.
    open({ ...GOOGLE, opened: false });
    expect(screen.getAllByRole("status")).toHaveLength(1);
    expect(screen.getByRole("status").textContent).toContain(
      "PM couldn't open your browser to sign in to Google.",
    );
    fireEvent.click(screen.getByRole("button", { name: "Try opening it" }));
    expect(openUrl).toHaveBeenCalledWith(GOOGLE.url);
  });

  it("every sign-in waiting has its own strip, and one ending leaves the other's", async () => {
    await mount();
    open(GOOGLE);
    open(PROTON);
    expect(screen.getByRole("button", { name: copyName("Google") })).toBeTruthy();
    expect(screen.getByRole("button", { name: copyName("Proton") })).toBeTruthy();
    close(1);
    expect(screen.getAllByRole("status")).toHaveLength(1);
    expect(screen.getByRole("status").textContent).toContain("Proton");
    close(2);
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("a copy of one sign-in's link never shows as copied on another's", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    await mount();
    open(GOOGLE);
    open(PROTON);
    // Google's copied, then Google's sign-in ends: Proton's strip takes the first place.
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: copyName("Google") }));
    });
    close(1);
    expect(screen.getByRole("button", { name: copyName("Proton") }).textContent).toBe("Copy link");
  });

  it("an end that never matched a start changes nothing", async () => {
    await mount();
    open(GOOGLE);
    close(99);
    expect(screen.getByRole("status").textContent).toContain("Google");
  });

  it("a failed reopen stays quiet", async () => {
    openUrl.mockRejectedValue(new Error("no browser"));
    await mount();
    open(GOOGLE);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Open again" }));
    });
    expect(screen.getByRole("status").textContent).toContain("Google");
  });

  it("draws wherever it's placed, from the one feed", async () => {
    await mount(2);
    open(GOOGLE);
    expect(screen.getByTestId("place0").textContent).toContain("Google");
    expect(screen.getByTestId("place1").textContent).toContain("Google");
  });

  it("a strip drawn later still shows a sign-in already waiting", async () => {
    await mount();
    open(GOOGLE);
    // Settings opening mid-sign-in mounts its own strip then.
    const later = render(<SignInStrip />);
    expect(later.container.textContent).toContain("Google");
  });
});
