// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// An Advanced-Protection account can't use the shared Google project, so it signs in with a project
// it owns. Once PM holds that project for one service, connecting the same account to the other
// service must reuse it — before this, the second service asked for the project again, and the
// normal Connect button used the shared client, which Google blocks for such an account.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const connectDrive = vi.fn();
const connectGoogleCalendarAccount = vi.fn();
const googleSavedProjects = vi.fn();

vi.mock("../lib/ipc", () => ({
  connectDrive: (...a: unknown[]) => connectDrive(...a),
  connectGoogleCalendarAccount: (...a: unknown[]) => connectGoogleCalendarAccount(...a),
  googleSavedProjects: () => googleSavedProjects(),
}));

// Same stub the other component tests use: the <Button>s reach for `useTheme`, and the real
// ThemeProvider pulls in IPC.
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

import { GoogleOwnProjectConnect } from "./GoogleOwnProjectConnect";

beforeEach(() => {
  connectDrive.mockResolvedValue({});
  connectGoogleCalendarAccount.mockResolvedValue({});
  googleSavedProjects.mockResolvedValue(["ap@example.com"]);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

function open() {
  fireEvent.click(screen.getByText(/Advanced Protection account\? Use its own project/));
}

describe("GoogleOwnProjectConnect", () => {
  it("offers the project already saved for an account, and connects with it", async () => {
    const onConnected = vi.fn();
    render(<GoogleOwnProjectConnect service="calendar" onConnected={onConnected} />);
    open();
    const reuse = await screen.findByRole("button", { name: "Use ap@example.com’s project" });
    fireEvent.click(reuse);
    await waitFor(() => expect(onConnected).toHaveBeenCalled());
    expect(connectGoogleCalendarAccount).toHaveBeenCalledWith(
      undefined,
      undefined,
      "ap@example.com",
    );
  });

  it("reuses a saved project for Drive too", async () => {
    render(<GoogleOwnProjectConnect service="drive" onConnected={vi.fn()} />);
    open();
    fireEvent.click(await screen.findByRole("button", { name: "Use ap@example.com’s project" }));
    await waitFor(() =>
      expect(connectDrive).toHaveBeenCalledWith(undefined, undefined, "ap@example.com"),
    );
  });

  it("shows only the paste form when no project is saved", async () => {
    googleSavedProjects.mockResolvedValue([]);
    render(<GoogleOwnProjectConnect service="calendar" onConnected={vi.fn()} />);
    open();
    await waitFor(() => expect(googleSavedProjects).toHaveBeenCalled());
    expect(screen.queryByText(/Already saved a project/)).toBeNull();
    expect(screen.getByRole("button", { name: "Connect with this project" })).toBeTruthy();
  });

  it("still connects with a pasted project", async () => {
    render(<GoogleOwnProjectConnect service="calendar" onConnected={vi.fn()} />);
    open();
    fireEvent.change(screen.getByPlaceholderText(/Client ID/), { target: { value: " id-1 " } });
    fireEvent.change(screen.getByPlaceholderText("Client secret"), {
      target: { value: "secret-1" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Connect with this project" }));
    await waitFor(() =>
      expect(connectGoogleCalendarAccount).toHaveBeenCalledWith("id-1", "secret-1"),
    );
  });
});
