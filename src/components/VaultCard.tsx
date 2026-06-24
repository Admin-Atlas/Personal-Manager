// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Settings "Vault" card (spec §2–6): shows whether the vault is device-only or a
// shareable, passphrase-protected one, and drives every transition through the backend's
// one migration routine. Markdown encryption is forced on (and the toggle hidden) for a
// shareable vault, because once it can be opened from another account folder isolation no
// longer protects the notes. The plaintext export is always offered — the promise that
// the user is never locked in.

import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  changeVaultPassphrase,
  createShareableVault,
  exportPlaintextMarkdown,
  forgetVaultPassphrase,
  linkVaultAccount,
  makeVaultPrivate,
  moveVault,
  vaultStatus,
} from "../lib/ipc";
import type { VaultStatus } from "../lib/types";
import { Button, Input } from "./ui";

/** Which inline form/confirmation is currently open (only one at a time). */
type Pending = "share" | "change" | "private" | "link" | null;

export function VaultCard() {
  const [status, setStatus] = useState<VaultStatus | null>(null);
  const [pending, setPending] = useState<Pending>(null);
  const [pass, setPass] = useState("");
  const [confirm, setConfirm] = useState("");
  const [account, setAccount] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [msg, setMsg] = useState<string | null>(null);

  async function refresh() {
    try {
      setStatus(await vaultStatus());
    } catch (e) {
      setError(String(e));
    }
  }

  useEffect(() => {
    void refresh();
  }, []);

  function reset() {
    setPending(null);
    setPass("");
    setConfirm("");
    setAccount("");
  }

  /** Run a backend transition, then reset the form and reload status. */
  async function run(action: () => Promise<unknown>, success: string) {
    setBusy(true);
    setError(null);
    setMsg(null);
    try {
      await action();
      setMsg(success);
      reset();
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function pickFolder(): Promise<string | null> {
    const picked = await openDialog({ directory: true, multiple: false });
    return typeof picked === "string" ? picked : null;
  }

  async function exportPlaintext() {
    const dir = await pickFolder();
    if (!dir) return;
    setBusy(true);
    setError(null);
    setMsg(null);
    try {
      const count = await exportPlaintextMarkdown(dir);
      setMsg(`Exported ${count} Markdown file${count === 1 ? "" : "s"} to ${dir}`);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function moveTo() {
    const dir = await pickFolder();
    if (!dir) return;
    await run(() => moveVault(dir), `Vault moved to ${dir}`);
  }

  const passphrasesMatch = pass.length > 0 && pass === confirm;
  const shareable = status?.mode === "passphrase";

  return (
    <div className="mt-5 border-t border-border pt-4" data-help="settings-vault">
      <label className="block text-sm font-medium text-ink2">Vault</label>

      {status && (
        <p className="mt-1 text-xs text-ink4">
          This vault is{" "}
          <span className="font-medium text-ink3">
            {shareable ? "shareable (passphrase-protected)" : "private to this device"}
          </span>
          {shareable && status.markdown_encrypted ? ", with Markdown encrypted at rest" : ""}.{" "}
          <span className="break-all">{status.location}</span>
        </p>
      )}

      {/* Device-only → offer to make shareable. */}
      {status && !shareable && (
        <div className="mt-3">
          {pending !== "share" ? (
            <Button variant="secondary" onClick={() => setPending("share")} disabled={busy}>
              Make shareable…
            </Button>
          ) : (
            <div className="space-y-2 rounded-[var(--radius-sm)] border border-border2 p-3">
              <p className="text-xs text-ink4">
                Choose a passphrase. It derives the encryption key, so it's never stored and can't
                be recovered — any profile or machine that knows it can open this vault.
              </p>
              <Input
                type="password"
                placeholder="Passphrase"
                value={pass}
                onChange={(e) => setPass(e.target.value)}
                autoFocus
              />
              <Input
                type="password"
                placeholder="Confirm passphrase"
                value={confirm}
                onChange={(e) => setConfirm(e.target.value)}
              />
              <div className="flex gap-2">
                <Button
                  variant="primary"
                  disabled={busy || !passphrasesMatch}
                  onClick={() =>
                    run(() => createShareableVault(pass), "This vault is now shareable.")
                  }
                >
                  {busy ? "Working…" : "Make shareable"}
                </Button>
                <Button variant="tertiary" onClick={reset} disabled={busy}>
                  Cancel
                </Button>
              </div>
            </div>
          )}
        </div>
      )}

      {/* Shareable → encryption note + manage actions. */}
      {shareable && (
        <>
          <p className="mt-2 text-xs text-ink4">
            Markdown encryption is on because this vault is shared. Without it, other accounts on
            this device could read your notes directly.
          </p>

          <div className="mt-3 flex flex-wrap gap-2">
            <Button variant="secondary" onClick={() => setPending("change")} disabled={busy}>
              Change passphrase…
            </Button>
            <Button variant="secondary" onClick={moveTo} disabled={busy}>
              Move vault…
            </Button>
            <Button variant="secondary" onClick={() => setPending("link")} disabled={busy}>
              Link another account…
            </Button>
            <Button variant="secondary" onClick={() => setPending("private")} disabled={busy}>
              Make private…
            </Button>
            <Button
              variant="tertiary"
              disabled={busy}
              onClick={() =>
                run(
                  () => forgetVaultPassphrase(),
                  "Passphrase forgotten on this device — you'll be asked for it next launch.",
                )
              }
            >
              Forget passphrase here
            </Button>
          </div>

          {pending === "change" && (
            <div className="mt-3 space-y-2 rounded-[var(--radius-sm)] border border-border2 p-3">
              <p className="text-xs text-ink4">
                Set a new passphrase. The vault is re-keyed and its Markdown re-encrypted.
              </p>
              <Input
                type="password"
                placeholder="New passphrase"
                value={pass}
                onChange={(e) => setPass(e.target.value)}
                autoFocus
              />
              <Input
                type="password"
                placeholder="Confirm new passphrase"
                value={confirm}
                onChange={(e) => setConfirm(e.target.value)}
              />
              <div className="flex gap-2">
                <Button
                  variant="primary"
                  disabled={busy || !passphrasesMatch}
                  onClick={() => run(() => changeVaultPassphrase(pass), "Passphrase changed.")}
                >
                  {busy ? "Working…" : "Change passphrase"}
                </Button>
                <Button variant="tertiary" onClick={reset} disabled={busy}>
                  Cancel
                </Button>
              </div>
            </div>
          )}

          {pending === "link" && (
            <div className="mt-3 space-y-2 rounded-[var(--radius-sm)] border border-border2 p-3">
              <p className="text-xs text-ink4">
                Grant another Windows account access to the shared vault folder. Enter its account
                name (e.g. <span className="font-mono">PC\alice</span>) or SID.
              </p>
              <Input
                placeholder="Account name or SID"
                value={account}
                onChange={(e) => setAccount(e.target.value)}
                autoFocus
              />
              <div className="flex gap-2">
                <Button
                  variant="primary"
                  disabled={busy || account.trim().length === 0}
                  onClick={() =>
                    run(
                      () => linkVaultAccount(account.trim()),
                      "Account linked to the vault folder.",
                    )
                  }
                >
                  {busy ? "Working…" : "Link account"}
                </Button>
                <Button variant="tertiary" onClick={reset} disabled={busy}>
                  Cancel
                </Button>
              </div>
            </div>
          )}

          {pending === "private" && (
            <div className="mt-3 space-y-2 rounded-[var(--radius-sm)] border border-border2 p-3">
              <p className="text-xs text-ink4">
                Make this vault private to this device again? It's re-keyed to a device-only key and
                its Markdown is decrypted back to plaintext. Other profiles will no longer be able
                to open it.
              </p>
              <div className="flex gap-2">
                <Button
                  variant="primary"
                  disabled={busy}
                  onClick={() =>
                    run(() => makeVaultPrivate(), "This vault is private to this device again.")
                  }
                >
                  {busy ? "Working…" : "Make private"}
                </Button>
                <Button variant="tertiary" onClick={reset} disabled={busy}>
                  Cancel
                </Button>
              </div>
            </div>
          )}
        </>
      )}

      {/* Always available: the portability escape hatch. */}
      <p className="mt-3 text-xs text-ink4">
        Your files stay yours. Export to plaintext Markdown anytime with your passphrase —
        encryption protects them at rest, it doesn't lock you in.
      </p>
      <div className="mt-2">
        <Button variant="tertiary" onClick={exportPlaintext} disabled={busy}>
          Export to plaintext Markdown…
        </Button>
      </div>

      {error && <p className="mt-2 break-all text-xs text-st-due">{error}</p>}
      {msg && <p className="mt-2 break-all text-xs text-faint">{msg}</p>}
    </div>
  );
}
