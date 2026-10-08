// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The failed-install banner. On Windows the updater plugin tears PM down for exit (every window
// hidden, the tray dropped, the staged update cleared) BEFORE it learns the installer won't launch,
// so a "Try again" there can only fail again: this session no longer holds the update. The banner
// must say to reopen PM instead. macOS keeps its staged update through a failed install, and a
// prior attempt's silent block was downloaded afresh this session, so both stay retryable.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { UpdateBanner } from "./UpdateBanner";
import type { AppUpdate } from "../lib/useUpdater";

// Same stub the other component tests use: <Button> reaches for `useTheme`, and the real
// ThemeProvider pulls in IPC.
vi.mock("../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

afterEach(cleanup);

function update(over: Partial<AppUpdate> = {}): AppUpdate {
  return {
    status: "ready",
    version: "3.140.0-alpha",
    progress: 1,
    dismissed: false,
    installFailed: false,
    sac: "off",
    blockedByPriorAttempt: false,
    packageManaged: false,
    releasesUrl: "https://example.invalid/releases",
    restart: vi.fn(),
    dismiss: vi.fn(),
    ...over,
  };
}

const tryAgain = () => screen.queryByRole("button", { name: "Try again" });

describe("UpdateBanner after a failed install", () => {
  it("on Windows, says to reopen PM and offers no retry", () => {
    const u = update({ installFailed: true });
    render(<UpdateBanner update={u} platform="windows" />);
    expect(screen.getByText(/Couldn.t install the update automatically/).textContent).toContain(
      "Close PM and open it again to retry, or download it from the releases page.",
    );
    expect(tryAgain()).toBeNull();
    expect(screen.getByRole("link", { name: "Download it manually" }).getAttribute("href")).toBe(
      u.releasesUrl,
    );
    // It can still be put away — without "Later" a banner with no retry would sit there all session.
    fireEvent.click(screen.getByRole("button", { name: "Later" }));
    expect(u.dismiss).toHaveBeenCalledTimes(1);
    expect(u.restart).not.toHaveBeenCalled();
  });

  it("on Windows, the collapsed chip points at the download instead of a restart", () => {
    const u = update({ installFailed: true, dismissed: true });
    render(<UpdateBanner update={u} platform="windows" />);
    expect(screen.getByText(/couldn't install — reopen PM to try again/)).toBeTruthy();
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.getByRole("link", { name: "Get it" }).getAttribute("href")).toBe(u.releasesUrl);
  });

  it("on macOS, keeps Try again (the staged update survives the failure)", () => {
    const u = update({ installFailed: true });
    render(<UpdateBanner update={u} platform="mac" />);
    expect(screen.queryByText(/open it again to retry/)).toBeNull();
    fireEvent.click(tryAgain()!);
    expect(u.restart).toHaveBeenCalledTimes(1);
  });

  it("on macOS, the collapsed chip still restarts", () => {
    const u = update({ installFailed: true, dismissed: true });
    render(<UpdateBanner update={u} platform="mac" />);
    fireEvent.click(screen.getByRole("button", { name: "Restart to update" }));
    expect(u.restart).toHaveBeenCalledTimes(1);
  });

  it("on Windows, a prior attempt's silent block stays retryable (this session re-downloaded it)", () => {
    const u = update({ blockedByPriorAttempt: true });
    render(<UpdateBanner update={u} platform="windows" />);
    expect(screen.queryByText(/open it again to retry/)).toBeNull();
    fireEvent.click(tryAgain()!);
    expect(u.restart).toHaveBeenCalledTimes(1);
  });
});
