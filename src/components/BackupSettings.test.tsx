// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The three one-way doors on the Backup tab, each of which used to fire on a single unguarded click.
//
// These are not render tests. Each one asserts the two halves that make a confirmation worth having:
// the destructive IPC does NOT fire until the user confirms, and the copy in front of them names the
// actual consequence. Copy assertions look like over-testing until you notice what the copy is
// protecting — "Forget" can leave every archive the user owns permanently undecryptable, and it also
// switches automatic backups off, which nothing on screen used to say.
//
// The panel talks to thirty-odd IPC commands; a factory mock REPLACES the module, so every one the
// component imports has to appear below or it is `undefined` at module-eval.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { BackupSchedule, DriveAccount, GdriveBackupStatus } from "../lib/types";

const backupGdriveDisconnect = vi.fn();
const backupGdriveStatus = vi.fn();
const backupStatus = vi.fn();
const backupArchivePrefix = vi.fn();
const forgetBackupPassphrase = vi.fn();
const getBackupSchedule = vi.fn();
const listGdriveBackups = vi.fn();
const listProtonBackups = vi.fn();
const onBackupProgress = vi.fn();
const protonCliStatus = vi.fn();
const protonDisconnect = vi.fn();
const protonStatus = vi.fn();
const scorePassphrase = vi.fn();

vi.mock("../lib/ipc", () => ({
  backupArchivePrefix: () => backupArchivePrefix(),
  backupGdriveConnect: vi.fn(),
  backupGdriveDisconnect: () => backupGdriveDisconnect(),
  backupGdriveStatus: () => backupGdriveStatus(),
  backupNow: vi.fn(),
  backupStatus: () => backupStatus(),
  backupToGdrive: vi.fn(),
  backupToProton: vi.fn(),
  clearBackupReport: vi.fn(),
  createLocalBackup: vi.fn(),
  forgetBackupPassphrase: () => forgetBackupPassphrase(),
  getBackupSchedule: () => getBackupSchedule(),
  listGdriveBackups: () => listGdriveBackups(),
  listProtonBackups: () => listProtonBackups(),
  onBackupProgress: (...a: unknown[]) => onBackupProgress(...a),
  openUrl: vi.fn(),
  protonCliStatus: () => protonCliStatus(),
  protonConnect: vi.fn(),
  protonDisconnect: () => protonDisconnect(),
  protonStatus: () => protonStatus(),
  pruneOwnBackups: vi.fn(),
  restoreFromGdrive: vi.fn(),
  restoreFromProton: vi.fn(),
  restoreLocalBackup: vi.fn(),
  scorePassphrase: (...a: unknown[]) => scorePassphrase(...a),
  setBackupDestinations: vi.fn(),
  setBackupPassphrase: vi.fn(),
  setBackupSchedule: vi.fn(),
  setProtonCliPath: vi.fn(),
  stopBackup: vi.fn(),
  switchToVault: vi.fn(),
}));

// Reached only by clicks no test here makes, but imported at eval time, so it is stubbed rather
// than left to touch a Tauri plugin under jsdom.
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

// The same stub the other component tests use: the panel's primitives reach for `useTheme`, and the
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
    mapVisible: true,
    setSystem: () => {},
    setModePref: () => {},
    setAccent: () => {},
    setDepth: () => {},
    setAutoLocation: () => {},
    setTeachVisible: () => {},
  }),
}));

import { BackupSettings } from "./BackupSettings";

const BACKUP_ACCOUNT = "me@example.com";

function schedule(over: Partial<BackupSchedule> = {}): BackupSchedule {
  return {
    frequency: "weekly",
    retention_n: 5,
    passphrase_stored: true,
    last_backup_at: null,
    proton_enabled: true,
    gdrive_enabled: true,
    gdrive_account: BACKUP_ACCOUNT,
    proton_last_backup_at: null,
    gdrive_last_backup_at: null,
    ...over,
  };
}

function driveAccount(email: string): DriveAccount {
  return {
    id: email,
    email,
    label: email,
    last_synced_at: null,
    state: "ok",
    indexed: 0,
    has_sheets_scope: true,
  };
}

function gdriveStatus(over: Partial<GdriveBackupStatus> = {}): GdriveBackupStatus {
  return {
    account: BACKUP_ACCOUNT,
    has_write_scope: true,
    enabled: true,
    accounts: [],
    ...over,
  };
}

/** Mount the panel and wait until its four independent loads have landed. */
async function mountPanel() {
  const view = render(<BackupSettings />);
  await waitFor(() =>
    expect(screen.getByText("Passphrase remembered on this device")).toBeTruthy(),
  );
  return view;
}

/** The open ConfirmDialog. */
function dialog(): HTMLElement {
  return screen.getByRole("dialog");
}

/** One destination's block, found by its `SectionLabel` heading — both blocks have a button called
 *  "Disconnect", so the query has to be scoped or it is ambiguous. */
function section(heading: string): HTMLElement {
  return screen.getByRole("heading", { name: heading }).closest("div") as HTMLElement;
}

function clickDisconnect(heading: string) {
  fireEvent.click(within(section(heading)).getByRole("button", { name: "Disconnect" }));
}

beforeEach(() => {
  getBackupSchedule.mockResolvedValue(schedule());
  backupGdriveStatus.mockResolvedValue(gdriveStatus());
  listGdriveBackups.mockResolvedValue([]);
  listProtonBackups.mockResolvedValue([]);
  protonCliStatus.mockResolvedValue({ installed: true, path: "/proton", install_url: "" });
  protonStatus.mockResolvedValue({ connected: true, account: "me@proton.me", error: null });
  backupArchivePrefix.mockResolvedValue("pm-vault-");
  backupStatus.mockResolvedValue({
    running: false,
    phase: null,
    fraction: 0,
    started_at_ms: null,
    last_error: null,
    last_report: null,
    pending_restore: null,
  });
  onBackupProgress.mockReturnValue(Promise.resolve(() => {}));
  scorePassphrase.mockResolvedValue({ score: 4, acceptable: true, feedback: [] });
  forgetBackupPassphrase.mockResolvedValue(undefined);
  protonDisconnect.mockResolvedValue(undefined);
  backupGdriveDisconnect.mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("Forget the backup passphrase", () => {
  it("asks first — the click alone destroys nothing", async () => {
    // The regression this exists for: one click used to delete the keychain entry AND write the
    // cadence to off, with no dialog and no message.
    await mountPanel();
    fireEvent.click(screen.getByRole("button", { name: "Forget" }));

    expect(forgetBackupPassphrase).not.toHaveBeenCalled();
    expect(dialog()).toBeTruthy();
  });

  it("names both consequences: unreadable backups, and the schedule going off", async () => {
    await mountPanel();
    fireEvent.click(screen.getByRole("button", { name: "Forget" }));
    const body = dialog().textContent ?? "";

    // 1. What is actually lost. Deliberately NOT "gone forever" — on macOS the entry is still
    //    visible in Keychain Access, so the only claim true on every platform is that PM keeps no
    //    other copy and cannot show it.
    expect(body).toContain("PM keeps no other copy");
    expect(body).toContain("permanently unreadable");
    // 2. The silent side effect, in the user's own cadence.
    expect(body).toContain("Weekly");
    expect(body).toContain("switches them to Off");
    // 3. And the reassurance the panel already teaches, so a scary dialog doesn't invite the
    //    wrong fear: the app lock is a different secret.
    expect(body).toContain("app lock is a different secret");
  });

  it("raises no false alarm about a schedule that is already off", async () => {
    getBackupSchedule.mockResolvedValue(schedule({ frequency: "off" }));
    await mountPanel();
    fireEvent.click(screen.getByRole("button", { name: "Forget" }));
    const body = dialog().textContent ?? "";

    expect(body).toContain("permanently unreadable");
    expect(body).not.toContain("switches them to Off");
  });

  it("cancelling leaves the passphrase alone", async () => {
    await mountPanel();
    fireEvent.click(screen.getByRole("button", { name: "Forget" }));
    fireEvent.click(within(dialog()).getByRole("button", { name: "Cancel" }));

    expect(forgetBackupPassphrase).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("confirming forgets it once, and says what happened", async () => {
    // The success path used to be silent: the only feedback was the UI mutating underneath you.
    await mountPanel();
    fireEvent.click(screen.getByRole("button", { name: "Forget" }));
    fireEvent.click(within(dialog()).getByRole("button", { name: "Forget passphrase" }));

    await waitFor(() => expect(forgetBackupPassphrase).toHaveBeenCalledTimes(1));
    await waitFor(() =>
      expect(screen.getByText(/Passphrase forgotten\. Automatic backups are off/)).toBeTruthy(),
    );
  });
});

describe("Disconnect a backup destination", () => {
  it("asks first for Proton, and says what survives and what stops", async () => {
    await mountPanel();
    clickDisconnect("Proton Drive");

    expect(protonDisconnect).not.toHaveBeenCalled();
    const body = dialog().textContent ?? "";
    expect(body).toContain("are kept — nothing is deleted");
    expect(body).toContain("can’t restore from Proton until you sign in again");
    // The machine-wide effect nothing else warns about: this is a CLI sign-out, not a PM-local one.
    expect(body).toContain("signs the Proton Drive command-line tool out on this computer");
    // And the flag the backend does NOT clear, so the schedule keeps advertising a dead destination.
    expect(body).toContain("keep listing Proton Drive");
  });

  it("cancelling a Proton disconnect leaves the session alone", async () => {
    await mountPanel();
    clickDisconnect("Proton Drive");
    fireEvent.click(within(dialog()).getByRole("button", { name: "Cancel" }));

    expect(protonDisconnect).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("confirming a Proton disconnect runs it exactly once", async () => {
    await mountPanel();
    clickDisconnect("Proton Drive");
    fireEvent.click(within(dialog()).getByRole("button", { name: "Disconnect" }));

    await waitFor(() => expect(protonDisconnect).toHaveBeenCalledTimes(1));
    expect(backupGdriveDisconnect).not.toHaveBeenCalled();
  });

  it("asks first for Google Drive, and warns about the fresh grant when it is backup-only", async () => {
    // The account is NOT also a read connector, so `backup_gdrive_disconnect` deletes the keychain
    // token — and PM's own Drive code treats a later re-consent as a NEW grant with no authority
    // over files the old one uploaded. Hedged, because disconnect never calls Google's revoke.
    backupGdriveStatus.mockResolvedValue(gdriveStatus({ accounts: [] }));
    await mountPanel();
    clickDisconnect("Google Drive");

    expect(backupGdriveDisconnect).not.toHaveBeenCalled();
    const body = dialog().textContent ?? "";
    expect(body).toContain("are kept — nothing is deleted");
    expect(body).toContain("may no longer be able to trim or replace");
    expect(body).not.toContain("also connected as a read-only source");
  });

  it("reassures instead when the same account is also a read connector", async () => {
    // The #600 cross-wiring is fixed in both directions: the backend keeps the token when the
    // account is also a connector. This is the assertion that pins that behaviour to the UI.
    backupGdriveStatus.mockResolvedValue(
      gdriveStatus({ accounts: [driveAccount(BACKUP_ACCOUNT.toUpperCase())] }),
    );
    await mountPanel();
    clickDisconnect("Google Drive");

    const body = dialog().textContent ?? "";
    expect(body).toContain("also connected as a read-only source");
    expect(body).not.toContain("may no longer be able to trim or replace");
  });

  it("confirming a Google disconnect runs it exactly once", async () => {
    await mountPanel();
    clickDisconnect("Google Drive");
    fireEvent.click(within(dialog()).getByRole("button", { name: "Disconnect" }));

    await waitFor(() => expect(backupGdriveDisconnect).toHaveBeenCalledTimes(1));
    expect(protonDisconnect).not.toHaveBeenCalled();
  });
});
