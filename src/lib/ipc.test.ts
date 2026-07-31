// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The IPC boundary's own behaviour, loaded for real.
//
// `src/lib/ipc.ts` is the single CI-enforced seam between the webview and every Tauri command, and
// it had no test. It is not a pass-through: it normalises the backend's one structured error shape
// into a `VaultError` that five vault-recovery surfaces branch on, and it fires the Settings
// footer's "Saved ✓" on success only. Every component test replaces the module wholesale with a
// bare factory mock (`vi.mock("../lib/ipc", () => …)` with no `importOriginal`), so until this file
// the real module was never loaded by anything.
//
// So the mock here is one level DOWN — `@tauri-apps/api/core` — and the module under test is the
// genuine article.

import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => invokeMock(cmd, args),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const ipc = await import("./ipc");
const { SETTING_SAVED_EVENT } = await import("./settingsSaved");

/** Count "Saved ✓" announcements while `fn` runs. */
async function savedTicks(fn: () => Promise<unknown>): Promise<number> {
  let ticks = 0;
  const onTick = () => {
    ticks++;
  };
  window.addEventListener(SETTING_SAVED_EVENT, onTick);
  try {
    await fn().catch(() => {});
  } finally {
    window.removeEventListener(SETTING_SAVED_EVENT, onTick);
  }
  return ticks;
}

beforeEach(() => {
  invokeMock.mockReset();
});

describe("the structured-fault normalisation", () => {
  const fault = {
    code: "locked",
    op: "open_vault",
    path: "/home/u/vault",
    message: "That vault is locked by another copy of PM.",
  };

  it("turns the backend's one structured shape into a VaultError callers can branch on", () => {
    // Rust's `Error::Vault` serialises `{code, op, path, message}`; every other variant is a bare
    // string. Without this, the five recovery surfaces would be string-matching a message.
    invokeMock.mockRejectedValueOnce(fault);
    return ipc.hasOpenRouterKey().then(
      () => expect.unreachable("should have rejected"),
      (e: unknown) => {
        expect(e).toBeInstanceOf(ipc.VaultError);
        expect(ipc.vaultFaultOf(e)).toEqual(fault);
        // The ~200 existing `String(e)` call sites must keep rendering a sentence, not
        // "[object Object]" and not "Error: …".
        expect(String(e)).toBe(fault.message);
        expect((e as Error).message).toBe(fault.message);
      },
    );
  });

  it("passes a bare-string rejection through untouched", () => {
    invokeMock.mockRejectedValueOnce("no key configured");
    return ipc.hasOpenRouterKey().then(
      () => expect.unreachable("should have rejected"),
      (e: unknown) => {
        expect(e).toBe("no key configured");
        expect(ipc.vaultFaultOf(e)).toBeNull();
      },
    );
  });

  it("leaves an object that is merely fault-ish alone", () => {
    // Missing `op`. Widening the predicate would wrap ordinary rejections in a VaultError and send
    // callers down a vault-recovery path for an error that has nothing to do with the vault.
    const notAFault = { code: "locked", message: "hmm" };
    invokeMock.mockRejectedValueOnce(notAFault);
    return ipc.hasOpenRouterKey().then(
      () => expect.unreachable("should have rejected"),
      (e: unknown) => {
        expect(e).toBe(notAFault);
        expect(e).not.toBeInstanceOf(ipc.VaultError);
      },
    );
  });

  it("returns a resolved value untouched", async () => {
    invokeMock.mockResolvedValueOnce(true);
    await expect(ipc.hasOpenRouterKey()).resolves.toBe(true);
    expect(invokeMock).toHaveBeenCalledWith("has_openrouter_key", undefined);
  });
});

describe("the Saved tick", () => {
  it("announces after a settings write that succeeded", async () => {
    invokeMock.mockResolvedValueOnce(undefined);
    expect(await savedTicks(() => ipc.setReranking(true))).toBe(1);
  });

  it("stays silent when the settings write rejected", async () => {
    // A tick on a failed save is the worst outcome: the user is told their change landed when it
    // did not.
    invokeMock.mockRejectedValueOnce("disk full");
    expect(await savedTicks(() => ipc.setReranking(true))).toBe(0);
  });

  it("stays silent for a command that is not a settings write", async () => {
    invokeMock.mockResolvedValueOnce(true);
    expect(await savedTicks(() => ipc.hasOpenRouterKey())).toBe(0);
  });

  it("announces from the shared invoke, so a new setting command needs no registration", async () => {
    // The point of announcing inside `invoke` rather than per wrapper. Any `set_*` command that is
    // not a content write ticks, including one nobody has written a wrapper for yet.
    invokeMock.mockResolvedValueOnce(undefined);
    expect(await savedTicks(() => ipc.setOnboardingDone())).toBe(1);
  });
});

describe("argument marshalling", () => {
  it("passes each wrapper's arguments under the key the Rust command expects", async () => {
    // Tauri maps `camelCase` here to `snake_case` there, so the KEY is part of the contract and a
    // typo is a runtime "invalid args" nobody sees until that screen is opened.
    invokeMock.mockResolvedValue(undefined);
    await ipc.setOpenRouterKey("sk-test");
    expect(invokeMock).toHaveBeenCalledWith("set_openrouter_key", { key: "sk-test" });

    await ipc.setIndexingSpeed("gentle");
    expect(invokeMock).toHaveBeenCalledWith("set_indexing_speed", { speed: "gentle" });
  });
});
