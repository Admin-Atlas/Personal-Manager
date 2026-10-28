// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The sign-in strip. PM opens a sign-in page in the default browser, which isn't always the one
// signed in to the right account, so these pin that a strip offers that page's link to copy and to
// open again for exactly as long as the backend waits on it; that every sign-in waiting gets its own
// strip, named for what it signs in to, and one ending never hides another's; that a browser which
// wouldn't open is said plainly, even when the backend says so after first showing the page, and
// stops being said once it opens from here; that focus on a strip that goes stays in the strip's
// place rather than falling out of the Settings dialog; that the strip draws wherever it's placed
// from the one feed App follows (App's place, or Settings' own, since Settings' scrim covers App's);
// and that the link itself, which carries the sign-in's one-time state, is never written out.

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

const DRIVE: SignInPage = {
  id: 1,
  provider: "Google",
  label: "Google Drive",
  url: "https://accounts.google.com/o/oauth2/v2/auth?client_id=x&state=s1",
  opened: true,
};
const CALENDAR: SignInPage = {
  id: 2,
  provider: "Google",
  label: "Google Calendar",
  url: "https://accounts.google.com/o/oauth2/v2/auth?client_id=x&state=s2",
  opened: true,
};
const PROTON: SignInPage = {
  id: 3,
  provider: "Proton",
  label: "Proton Drive",
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
const copyName = (label: string) => `Copy link to the ${label} sign-in`;
const againName = (label: string) => `Open again: the ${label} sign-in`;

beforeEach(() => {
  openUrl.mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
  // The feed is the module's, so what one test left waiting would show in the next.
  for (const id of [1, 2, 3]) signInClosed(id);
  vi.clearAllMocks();
  Object.defineProperty(navigator, "clipboard", { value: undefined, configurable: true });
});

describe("SignInStrip", () => {
  it("shows nothing until a sign-in starts, and goes when it ends", async () => {
    const { container } = await mount();
    expect(container.textContent).toBe("");
    open(DRIVE);
    expect(screen.getByRole("status").textContent).toContain(
      "Google Drive: finish signing in to Google in your browser.",
    );
    close(1);
    expect(container.textContent).toBe("");
  });

  it("copies the page's link and opens it again, without writing it out", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const { container } = await mount();
    open(DRIVE);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: copyName("Google Drive") }));
    });
    expect(writeText).toHaveBeenCalledWith(DRIVE.url);
    fireEvent.click(screen.getByRole("button", { name: againName("Google Drive") }));
    expect(openUrl).toHaveBeenCalledWith(DRIVE.url);
    expect(container.innerHTML).not.toContain("accounts.google.com");
  });

  it("says so when the browser couldn't be opened, also when that comes after the page", async () => {
    await mount();
    open(DRIVE);
    // The backend shows the page first, then says its browser wouldn't open: the same id again.
    open({ ...DRIVE, opened: false });
    expect(screen.getAllByRole("status")).toHaveLength(1);
    expect(screen.getByRole("status").textContent).toContain(
      "Google Drive: PM couldn't open your browser to sign in to Google.",
    );
    // Opening it from here works: from then on it reads as opened.
    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: "Try opening it: the Google Drive sign-in" }),
      );
    });
    expect(openUrl).toHaveBeenCalledWith(DRIVE.url);
    expect(screen.getByRole("status").textContent).toContain("finish signing in to Google");
  });

  it("a retry that fails still says the browser couldn't open", async () => {
    openUrl.mockRejectedValue(new Error("no browser"));
    await mount();
    open({ ...DRIVE, opened: false });
    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: "Try opening it: the Google Drive sign-in" }),
      );
    });
    expect(screen.getByRole("status").textContent).toContain("PM couldn't open your browser");
  });

  it("every sign-in waiting has its own strip, named for what it signs in to", async () => {
    await mount();
    open(DRIVE);
    open(CALENDAR);
    open(PROTON);
    // Two of Google's at once: each says which, and so do its buttons.
    expect(screen.getAllByRole("status").map((s) => s.textContent?.split(":")[0])).toEqual([
      "Google Drive",
      "Google Calendar",
      "Proton Drive",
    ]);
    expect(screen.getByRole("button", { name: copyName("Google Calendar") })).toBeTruthy();
    expect(screen.getByRole("button", { name: againName("Google Calendar") })).toBeTruthy();
    close(1);
    expect(screen.getAllByRole("status")).toHaveLength(2);
    expect(screen.queryByRole("button", { name: copyName("Google Drive") })).toBeNull();
    close(2);
    close(3);
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("a copy of one sign-in's link never shows as copied on another's", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    await mount();
    open(DRIVE);
    open(PROTON);
    // Drive's copied, then Drive's sign-in ends: Proton's strip takes the first place.
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: copyName("Google Drive") }));
    });
    close(1);
    expect(screen.getByRole("button", { name: copyName("Proton Drive") }).textContent).toBe(
      "Copy link",
    );
  });

  it("focus on a strip that goes stays where the strips are, not on the page's body", async () => {
    await mount();
    open(DRIVE);
    open(PROTON);
    const copy = screen.getByRole("button", { name: copyName("Google Drive") });
    act(() => copy.focus());
    close(1);
    const place = screen.getByTestId("place0");
    expect(document.activeElement).not.toBe(document.body);
    expect(place.contains(document.activeElement)).toBe(true);
    // Focus that had moved on elsewhere is left alone.
    const outside = document.createElement("button");
    document.body.appendChild(outside);
    act(() => outside.focus());
    close(3);
    expect(document.activeElement).toBe(outside);
    outside.remove();
  });

  it("focus kept through PM's window losing focus to the browser, then the strip going", async () => {
    await mount();
    open(DRIVE);
    open(PROTON);
    const copy = screen.getByRole("button", { name: copyName("Google Drive") });
    act(() => copy.focus());
    // The window goes to the browser: the button blurs with nowhere to go, and the document has
    // no focus.
    const hasFocus = vi.spyOn(document, "hasFocus").mockReturnValue(false);
    fireEvent.blur(copy, { relatedTarget: null });
    hasFocus.mockRestore();
    close(1);
    expect(screen.getByTestId("place0").contains(document.activeElement)).toBe(true);
  });

  it("an end that never matched a start changes nothing", async () => {
    await mount();
    open(DRIVE);
    close(99);
    expect(screen.getByRole("status").textContent).toContain("Google Drive");
  });

  it("a failed reopen stays quiet", async () => {
    openUrl.mockRejectedValue(new Error("no browser"));
    await mount();
    open(DRIVE);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: againName("Google Drive") }));
    });
    expect(screen.getByRole("status").textContent).toContain("Google Drive");
  });

  it("draws wherever it's placed, from the one feed", async () => {
    await mount(2);
    open(DRIVE);
    expect(screen.getByTestId("place0").textContent).toContain("Google Drive");
    expect(screen.getByTestId("place1").textContent).toContain("Google Drive");
  });

  it("a strip drawn later still shows a sign-in already waiting", async () => {
    await mount();
    open(DRIVE);
    // Settings opening mid-sign-in mounts its own strip then.
    const later = render(<SignInStrip />);
    expect(later.container.textContent).toContain("Google Drive");
  });
});
