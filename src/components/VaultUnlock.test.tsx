// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The unlock gate's error line is the one Callout in the tree that must NOT supply its own live
// region, and this pins why.
//
// `useFieldA11y` already returns `errorProps = { id, role: "alert" }`, and the passphrase input's
// `aria-describedby` points at that id. If the Callout also announced, the same sentence would be
// spoken twice; if the spread were dropped in favour of the Callout's own role, the input would
// describe an element that no longer exists and the error would stop being ASSOCIATED with the
// field even while it was still being read out. Both failures are silent in a screenshot, so both
// are asserted: exactly one alert, and the control still points at it.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { VaultUnlock } from "./VaultUnlock";

const unlockVault = vi.fn();

vi.mock("../lib/ipc", () => ({
  unlockVault: (...args: unknown[]) => unlockVault(...(args as [])),
  detachFromSharedVault: vi.fn(),
  vaultFaultOf: () => null,
  repairVaultAccess: vi.fn(),
  acknowledgeDeletedSharedVault: vi.fn(),
}));

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

/** Type a passphrase and submit, so the component reaches its catch branch. */
async function failUnlock(message: string) {
  unlockVault.mockRejectedValue(new Error(message));
  render(<VaultUnlock status={null} onUnlocked={() => {}} />);
  fireEvent.change(screen.getByPlaceholderText("Passphrase"), { target: { value: "hunter2" } });
  fireEvent.click(screen.getByRole("button", { name: /^unlock$/i }));
  return screen.findByRole("alert");
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(cleanup);

describe("VaultUnlock — one alert, still tied to the field", () => {
  it("announces a refused passphrase exactly once", async () => {
    const alert = await failUnlock("that passphrase doesn't match this vault");
    expect(alert.textContent).toMatch(/doesn't match/i);
    expect(screen.getAllByRole("alert")).toHaveLength(1);
  });

  it("keeps the passphrase field describing the very element that announces", async () => {
    const alert = await failUnlock("that passphrase doesn't match this vault");
    const input = screen.getByPlaceholderText("Passphrase");

    expect(alert.id).toBeTruthy();
    expect(input.getAttribute("aria-describedby")).toContain(alert.id);
    expect(input.getAttribute("aria-invalid")).toBe("true");
  });

  it("is silent before an attempt", () => {
    render(<VaultUnlock status={null} onUnlocked={() => {}} />);
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
