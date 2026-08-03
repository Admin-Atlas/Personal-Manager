// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The vault owner gate, at the surface the user actually touches. The backend refuses a joiner's
// re-key without an explicit takeover flag and refuses their "Make private" outright; if this card
// disagreed about either, the user would meet a raw error string instead of a decision they could
// make. Both halves are worth pinning here rather than reading, because the failure modes are
// asymmetric and both bad: a checkbox that doesn't gate hands a joiner a silent takeover in one
// click, and a checkbox shown on an OWNED vault teaches every legitimate owner to tick a warning
// away every time they rotate their own passphrase.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { VaultCard } from "./VaultCard";
import type { VaultStatus } from "../lib/types";

const changeVaultPassphrase = vi.fn(async () => ({ warnings: [] }));
const makeVaultPrivate = vi.fn(async () => ({ warnings: [] }));
const vaultStatus = vi.fn();

const PASS = "correct horse battery staple";

/** `jest-dom` is not installed (no `toBeDisabled`), so read the property the suite already has. */
const disabled = (el: HTMLElement) => (el as HTMLButtonElement).disabled;

// The whole module is replaced, so every wrapper this card OR its children import must be present —
// ShareVaultWizard, PassphraseStrengthMeter, VaultRecovery and VaultJoin all reach for their own.
vi.mock("../lib/ipc", () => ({
  adoptSharedVault: vi.fn(),
  changeVaultPassphrase: (...args: unknown[]) => changeVaultPassphrase(...(args as [])),
  deleteSharedVault: vi.fn(),
  detachFromSharedVault: vi.fn(),
  exportPlaintextMarkdown: vi.fn(),
  forgetVaultPassphrase: vi.fn(),
  makeVaultPrivate: (...args: unknown[]) => makeVaultPrivate(...(args as [])),
  vaultStatus: () => vaultStatus(),
  // Children.
  scorePassphrase: vi.fn(async () => ({ score: 4, acceptable: true, feedback: [] })),
  acknowledgeDeletedSharedVault: vi.fn(),
  repairVaultAccess: vi.fn(),
  vaultFaultOf: () => null,
  createShareableVault: vi.fn(),
  linkVaultAccount: vi.fn(),
  listLocalAccounts: vi.fn(async () => []),
  moveVault: vi.fn(),
  suggestSharedVaultLocation: vi.fn(async () => null),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));

// Same stub the other component tests use: <Button> reaches for `useTheme`, and the real
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

function status(over: Partial<VaultStatus> = {}): VaultStatus {
  return {
    mode: "passphrase",
    needs_unlock: false,
    markdown_encrypted: true,
    location: "D:\\Shared\\PM",
    vault_id: "v1",
    retrieval_rebuild_needed: false,
    fault: null,
    pointed_root: "D:\\Shared\\PM",
    has_set_aside_vault: true,
    retired_root: null,
    deleted_notice: null,
    is_owner: false,
    ownership: "joined",
    ownership_transfer: null,
    meta_warning: null,
    ...over,
  };
}

/** Render the card and expand the sharing disclosure the controls now live behind (#712).
 *
 *  Not optional politeness: `Collapsible` keeps its body MOUNTED and marks it `inert`, and jsdom
 *  implements neither `inert` nor the CSS that hides it — so every query below would keep passing
 *  against controls a real user cannot reach until they expand this. */
async function openSharing() {
  render(<VaultCard />);
  fireEvent.click(
    await screen.findByRole("button", { name: /share this vault with other accounts/i }),
  );
}

/** Render the card and open its "Change passphrase…" panel with a matching, strong passphrase. */
async function openChangePanel() {
  await openSharing();
  fireEvent.click(await screen.findByRole("button", { name: /change passphrase…/i }));
  fireEvent.change(screen.getByPlaceholderText("New passphrase"), { target: { value: PASS } });
  fireEvent.change(screen.getByPlaceholderText("Confirm new passphrase"), {
    target: { value: PASS },
  });
  return screen.getByRole("button", { name: /^change passphrase$/i });
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(cleanup);

describe("VaultCard — the vault owner gate", () => {
  it("starts with sharing folded away", async () => {
    // #712 demoted it deliberately: sharing one vault between accounts on one PC is niche, and it
    // had been sitting at the same level as "where is my data", which is a question everybody has.
    // The disclosure closed is the demotion — an open one would be the same prominence with extra
    // chrome.
    vaultStatus.mockResolvedValue(status());
    render(<VaultCard />);
    const toggle = await screen.findByRole("button", {
      name: /share this vault with other accounts/i,
    });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    // What must NOT fold: the state readout. It says what this vault currently is, and a status
    // hidden behind a click is a status nobody reads.
    expect(screen.getByText(/shareable \(passphrase-protected\)/i)).toBeTruthy();
  });

  it("keeps a joiner's Change disabled until the takeover box is ticked, then sends true", async () => {
    vaultStatus.mockResolvedValue(status({ ownership: "joined" }));
    const change = await openChangePanel();

    // The warning is present and the action is unavailable, even though the passphrase itself is
    // perfectly valid — the block is the ownership decision, not the passphrase.
    expect(screen.getByRole("alert").textContent).toMatch(/created by another account/i);
    expect(disabled(change)).toBe(true);

    fireEvent.click(screen.getByRole("checkbox"));
    expect(disabled(change)).toBe(false);
    fireEvent.click(change);

    await waitFor(() => expect(changeVaultPassphrase).toHaveBeenCalledTimes(1));
    expect(changeVaultPassphrase).toHaveBeenCalledWith(PASS, true);
  });

  it.each(["owned", "unknown"] as const)(
    "shows no checkbox for an %s vault and sends false",
    async (ownership) => {
      // `unknown` is every shared vault off Windows and every vault from before ownership was
      // recorded. It must rotate exactly as it always did — the gate falls open there by design.
      vaultStatus.mockResolvedValue(status({ ownership }));
      const change = await openChangePanel();

      expect(screen.queryByRole("checkbox")).toBeNull();
      expect(disabled(change)).toBe(false);
      fireEvent.click(change);

      await waitFor(() => expect(changeVaultPassphrase).toHaveBeenCalledTimes(1));
      expect(changeVaultPassphrase).toHaveBeenCalledWith(PASS, false);
    },
  );

  it("offers a joiner the leave affordance instead of Make private, and never calls it", async () => {
    vaultStatus.mockResolvedValue(status({ ownership: "joined" }));
    await openSharing();
    fireEvent.click(await screen.findByRole("button", { name: /make private…/i }));

    expect(screen.queryByRole("button", { name: /^make private$/i })).toBeNull();
    expect(
      disabled(screen.getByRole("button", { name: /use a vault on this account instead/i })),
    ).toBe(false);
    expect(makeVaultPrivate).not.toHaveBeenCalled();
  });

  it("offers no dead leave button when a joined vault isn't pointed", async () => {
    // Reachable without any second account: a shareable vault left in this profile's own folder,
    // whose owner's SID has since changed (domain move, account recreated), reads as `joined` on the
    // user's OWN vault. `detachFromSharedVault` retires a POINTER, so with none it silently does
    // nothing — PM must not offer it. The way forward is the re-key hatch, not this panel.
    vaultStatus.mockResolvedValue(status({ ownership: "joined", pointed_root: null }));
    await openSharing();
    fireEvent.click(await screen.findByRole("button", { name: /make private…/i }));

    expect(screen.queryByRole("button", { name: /^make private$/i })).toBeNull();
    expect(
      screen.queryByRole("button", { name: /use a vault on this account instead/i }),
    ).toBeNull();
    expect(makeVaultPrivate).not.toHaveBeenCalled();
  });

  it("still offers Make private on a vault this account owns", async () => {
    vaultStatus.mockResolvedValue(status({ ownership: "owned", is_owner: true }));
    await openSharing();
    fireEvent.click(await screen.findByRole("button", { name: /make private…/i }));

    expect(disabled(screen.getByRole("button", { name: /^make private$/i }))).toBe(false);
    expect(
      screen.queryByRole("button", { name: /use a vault on this account instead/i }),
    ).toBeNull();
  });

  it("states a recorded takeover, so the field is not written and never read", async () => {
    // The record rides under the vault metadata's MAC; if nothing displayed it, a takeover would be
    // tamper-evident to PM and invisible to the person it happened to.
    vaultStatus.mockResolvedValue(
      status({
        ownership: "joined",
        ownership_transfer: {
          from_sid: "S-1-5-21-1-2-3-1001",
          to_sid: "S-1-5-21-1-2-3-1002",
          // A PAST year, so `formatDate` keeps it (it drops the year within the current one) and
          // this assertion doesn't quietly change meaning on 1 January. Midday UTC, so no timezone
          // the suite runs in can shift the calendar day.
          at: "2025-01-02T12:00:00Z",
        },
      }),
    );
    render(<VaultCard />);
    expect(await screen.findByText(/ownership of this vault was transferred/i)).toBeTruthy();
    expect(screen.getByText("02-01-2025")).toBeTruthy();
  });
});
