// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The open-error gate never offers a destructive recovery for a store a newer PM wrote
// (INVARIANTS.md I-28). That store opened fine and is whole; "Start fresh" would delete the user's
// library to fix a version gap an update closes. The backend refuses the reset for this code too
// (wipe.rs `reset_refusal`), but the screen must not lead anyone to it in the first place, so this
// pins both halves of the screen: the update story is told, and neither escape hatch is drawn.

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { VaultFaultCode, VaultStatus } from "../lib/types";
import { VaultOpenError } from "./VaultOpenError";

vi.mock("../lib/ipc", () => ({
  createReplacementStore: vi.fn(),
  detachFromSharedVault: vi.fn(),
  resetAfterOpenError: vi.fn(),
  retryOpenVault: vi.fn(),
  acknowledgeDeletedSharedVault: vi.fn(),
  repairVaultAccess: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn() }));

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

afterEach(cleanup);

/** A locally-owned vault (no pointer) whose boot open failed with `code`. */
function statusWith(code: VaultFaultCode, message: string): VaultStatus {
  return {
    mode: "device",
    needs_unlock: false,
    markdown_encrypted: false,
    location: "/data/pm",
    vault_id: "v-1",
    retrieval_rebuild_needed: false,
    fault: { code, op: "open the vault's database", path: null, message },
    pointed_root: null,
    has_set_aside_vault: false,
    retired_root: null,
    deleted_notice: null,
    is_owner: true,
    ownership: "device",
    ownership_transfer: null,
    meta_warning: null,
  };
}

const NEWER = "This data was written by a newer version of PM. Update PM to open it.";

describe("VaultOpenError — a newer PM's store", () => {
  it("tells the update story and offers no destructive recovery", () => {
    render(<VaultOpenError status={statusWith("newer-store", NEWER)} onResolved={() => {}} />);
    expect(screen.getByRole("heading", { name: "This vault needs a newer PM" })).toBeTruthy();
    expect(screen.getByText(NEWER)).toBeTruthy();
    // Neither escape hatch is drawn: no "Start fresh" reveal, no empty-store replacement.
    expect(screen.queryByText("Still won't open?")).toBeNull();
    expect(screen.queryByText(/start fresh/i)).toBeNull();
    expect(screen.queryByText("Start a new empty store")).toBeNull();
    // Trying again stays harmless and available.
    expect(screen.getByRole("button", { name: "Try again" })).toBeTruthy();
  });

  it("still offers Start fresh for a genuine brick, so the gate above is the code, not the screen", () => {
    render(
      <VaultOpenError
        status={statusWith("other", "could not open database (wrong key or corrupt file)")}
        onResolved={() => {}}
      />,
    );
    expect(screen.getByText("Still won't open?")).toBeTruthy();
  });
});
