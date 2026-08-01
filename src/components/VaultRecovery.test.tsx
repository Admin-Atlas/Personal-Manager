// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The vault-recovery path announces its failures.
//
// This is the reason `Callout` owns the live region rather than leaving it to each call site. Every
// screen in this path is a DEAD END by construction: the app is not running yet, the vault is not
// open, and the tinted line under the button is the only evidence that pressing it did anything at
// all. Before this batch that line was a bare `<p>` with a `--st-due` tint and no role — a sighted
// user saw red, and a screen-reader user heard silence and a button that still said "Continue".
//
// So the assertion here is `role="alert"`, not "the text is on screen". `getByText` would pass on
// the old markup and prove nothing; the announcement IS the fix. It is also asserted to appear only
// on failure, because an alert that is already in the DOM at first render is inconsistently
// announced by assistive tech — that asymmetry is what `live` exists to express.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DeletedVaultNotice } from "./VaultRecovery";

const acknowledgeDeletedSharedVault = vi.fn();

vi.mock("../lib/ipc", () => ({
  acknowledgeDeletedSharedVault: () => acknowledgeDeletedSharedVault(),
  repairVaultAccess: vi.fn(),
}));

// `<Button>` reaches for `useTheme`, and the real provider pulls in IPC. Same stub the other
// component tests use.
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

const NOTICE = { folder: "D:\\Shared\\PM", deleted_at: "2025-01-02T12:00:00Z" };

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(cleanup);

describe("DeletedVaultNotice — the switch-back failure announces", () => {
  it("has no live region until something actually fails", () => {
    render(<DeletedVaultNotice notice={NOTICE} onAcknowledged={() => {}} />);

    // The standing explanation above the button is prose, not an alert. If this ever starts
    // matching, a `live={false}` has been dropped and the whole screen shouts on arrival.
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("announces the backend's refusal, rather than only tinting it red", async () => {
    acknowledgeDeletedSharedVault.mockRejectedValue(new Error("the vault folder is read-only"));
    const onAcknowledged = vi.fn();
    render(<DeletedVaultNotice notice={NOTICE} onAcknowledged={onAcknowledged} />);

    fireEvent.click(screen.getByRole("button", { name: /continue/i }));

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toMatch(/read-only/i);
    // Exactly one live region: a second would announce the same sentence twice.
    expect(screen.getAllByRole("alert")).toHaveLength(1);
    // The screen did NOT move on — which is precisely why the message has to be heard.
    expect(onAcknowledged).not.toHaveBeenCalled();
  });

  it("keeps the tint on the tone token, so no hex can creep back in", async () => {
    acknowledgeDeletedSharedVault.mockRejectedValue(new Error("nope"));
    render(<DeletedVaultNotice notice={NOTICE} onAcknowledged={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: /continue/i }));

    const alert = await screen.findByRole("alert");
    const style = alert.getAttribute("style") ?? "";
    expect(style).toContain("--st-due");
    expect(style).not.toContain("#");
  });

  it("leaves the screen silent when the switch-back succeeds", async () => {
    acknowledgeDeletedSharedVault.mockResolvedValue(undefined);
    const onAcknowledged = vi.fn();
    render(<DeletedVaultNotice notice={NOTICE} onAcknowledged={onAcknowledged} />);

    fireEvent.click(screen.getByRole("button", { name: /continue/i }));

    await waitFor(() => expect(onAcknowledged).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
