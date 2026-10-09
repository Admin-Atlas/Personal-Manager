// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Calendar editing is turned on per Google account (#884). Each status offers exactly one action:
// turning it on asks Google, while switching it off and back is PM's alone and never opens a
// sign-in. A connect that asks for reading only turns editing off, and the note has to say so.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CalendarOverview, EditingStatus } from "../lib/types";

const calendarOverview = vi.fn();
const enableCalendarEditing = vi.fn();
const setCalendarEditingPaused = vi.fn();
const connectGoogleCalendarAccount = vi.fn();
const disconnectGoogleCalendarAccount = vi.fn();
const syncCalendar = vi.fn();

vi.mock("../lib/ipc", () => ({
  calendarOverview: () => calendarOverview(),
  enableCalendarEditing: (...a: unknown[]) => enableCalendarEditing(...a),
  setCalendarEditingPaused: (...a: unknown[]) => setCalendarEditingPaused(...a),
  connectGoogleCalendarAccount: (...a: unknown[]) => connectGoogleCalendarAccount(...a),
  disconnectGoogleCalendarAccount: (...a: unknown[]) => disconnectGoogleCalendarAccount(...a),
  connectOutlookCalendar: vi.fn(),
  disconnectOutlookCalendar: vi.fn(),
  setCalendarSelected: vi.fn(),
  setCalendarQuiet: vi.fn(),
  setCalendarKind: vi.fn(),
  syncCalendar: () => syncCalendar(),
  googleSavedProjects: () => Promise.resolve([]),
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

// Developer mode off: the dev panel isn't what's under test.
vi.mock("../lib/capabilities", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useDevMode: () => ({ devMode: false, setDevMode: () => {} }),
}));

import { CalendarConnection } from "./CalendarConnection";

const EMAIL = "me@example.com";
const ID = `gcal:${EMAIL}`;

function overview(editing: EditingStatus): CalendarOverview {
  return {
    google_client_configured: true,
    microsoft_client_configured: false,
    accounts: [
      {
        id: ID,
        provider: "google",
        email: EMAIL,
        label: EMAIL,
        state: "ok",
        last_synced_at: null,
      },
    ],
    calendars: [],
    last_sync: null,
    window_days: 21,
    mirror_start: "2026-09-09T00:00:00Z",
    mirror_end: "2027-11-09T00:00:00Z",
    editing: { [ID]: editing },
  };
}

beforeEach(() => {
  syncCalendar.mockResolvedValue(3);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("CalendarConnection editing switch", () => {
  it("offers to turn editing on, through Google, when it's off", async () => {
    calendarOverview.mockResolvedValueOnce(overview("off")).mockResolvedValue(overview("on"));
    enableCalendarEditing.mockResolvedValue("on");
    render(<CalendarConnection provider="google" />);
    fireEvent.click(await screen.findByRole("button", { name: "Turn on editing…" }));
    await waitFor(() => expect(enableCalendarEditing).toHaveBeenCalledWith(EMAIL));
    expect(await screen.findByText(`Editing is on for ${EMAIL}.`)).toBeTruthy();
    expect(await screen.findByRole("button", { name: "Turn off editing" })).toBeTruthy();
    expect(setCalendarEditingPaused).not.toHaveBeenCalled();
  });

  it("says the account stays read-only when Google's write box was left unticked", async () => {
    calendarOverview.mockResolvedValue(overview("off"));
    enableCalendarEditing.mockResolvedValue("off");
    render(<CalendarConnection provider="google" />);
    fireEvent.click(await screen.findByRole("button", { name: "Turn on editing…" }));
    expect(await screen.findByText(new RegExp(`so ${EMAIL} stays read-only`))).toBeTruthy();
  });

  it("switches editing off and back on without asking Google", async () => {
    calendarOverview.mockResolvedValueOnce(overview("on")).mockResolvedValue(overview("paused"));
    setCalendarEditingPaused.mockResolvedValue("paused");
    render(<CalendarConnection provider="google" />);
    fireEvent.click(await screen.findByRole("button", { name: "Turn off editing" }));
    await waitFor(() => expect(setCalendarEditingPaused).toHaveBeenCalledWith(EMAIL, true));
    fireEvent.click(await screen.findByRole("button", { name: "Turn editing back on" }));
    await waitFor(() => expect(setCalendarEditingPaused).toHaveBeenLastCalledWith(EMAIL, false));
    expect(enableCalendarEditing).not.toHaveBeenCalled();
  });

  it("asks Google again when the permission is gone", async () => {
    calendarOverview.mockResolvedValue(overview("needs_consent"));
    enableCalendarEditing.mockResolvedValue("on");
    render(<CalendarConnection provider="google" />);
    expect(await screen.findByText(/Google no longer gives PM permission/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Ask Google again…" }));
    await waitFor(() => expect(enableCalendarEditing).toHaveBeenCalledWith(EMAIL));
  });

  it("says when connecting again turned editing off", async () => {
    calendarOverview.mockResolvedValueOnce(overview("on")).mockResolvedValue(overview("off"));
    connectGoogleCalendarAccount.mockResolvedValue(overview("off").accounts[0]);
    render(<CalendarConnection provider="google" />);
    fireEvent.click(await screen.findByRole("button", { name: "Add another account" }));
    expect(
      await screen.findByText(
        `Connected. Synced 3 events. Editing is now off for ${EMAIL}, because connecting again only asks Google for reading. Turn it back on under the account.`,
      ),
    ).toBeTruthy();
  });

  // Editing reads "off" here (say a read-only reconnect cleared it), but Google's grant still has the
  // write scope: the note follows what the backend says the kept grant covers, not the status.
  it("names the kept calendar write permission after a disconnect that kept the grant", async () => {
    calendarOverview.mockResolvedValue(overview("off"));
    disconnectGoogleCalendarAccount.mockResolvedValue({
      kept_for: ["drive"],
      calendar_write: true,
    });
    const onGrantOutcome = vi.fn();
    render(<CalendarConnection provider="google" onGrantOutcome={onGrantOutcome} />);
    fireEvent.click(await screen.findByRole("button", { name: "Disconnect" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(
      Array.from(dialog.querySelectorAll("button")).find((b) => b.textContent === "Disconnect")!,
    );
    await waitFor(() =>
      expect(onGrantOutcome).toHaveBeenLastCalledWith({
        service: "calendar",
        email: EMAIL,
        keptFor: ["drive"],
        calendarWrite: true,
      }),
    );
  });

  it("says Waiting for Google only on the account being turned on", async () => {
    const other = "other@example.com";
    const two = overview("off");
    two.accounts.push({ ...two.accounts[0], id: `gcal:${other}`, email: other, label: other });
    two.editing[`gcal:${other}`] = "off";
    calendarOverview.mockResolvedValue(two);
    // Google's page stays open for the whole test.
    enableCalendarEditing.mockReturnValue(new Promise(() => {}));
    render(<CalendarConnection provider="google" />);
    const [first] = await screen.findAllByRole("button", { name: "Turn on editing…" });
    fireEvent.click(first);
    expect(await screen.findByRole("button", { name: "Waiting for Google…" })).toBeTruthy();
    // The other account's button keeps its label (disabled while the consent is open).
    const rest = screen.getAllByRole("button", { name: "Turn on editing…" });
    expect(rest).toHaveLength(1);
    expect((rest[0] as HTMLButtonElement).disabled).toBe(true);
  });

  it("shows no editing switch on Outlook accounts", async () => {
    calendarOverview.mockResolvedValue({
      ...overview("off"),
      microsoft_client_configured: true,
      accounts: [
        { ...overview("off").accounts[0], id: "outlook:me@example.com", provider: "microsoft" },
      ],
      editing: {},
    });
    render(<CalendarConnection provider="microsoft" />);
    await screen.findByText(EMAIL);
    expect(screen.queryByRole("button", { name: /editing/ })).toBeNull();
  });
});
