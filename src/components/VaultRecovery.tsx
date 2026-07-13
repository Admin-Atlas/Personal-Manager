// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The two small recovery pieces shared by every vault surface (issue #343):
//
//  - RepairAccessButton — the one-click way out of an access-DENIED vault folder. It
//    drives `repair_vault_access` (re-grant this account, restore the lockdown, reopen);
//    when even that fails, it reveals a copyable run-as-Administrator recipe instead of
//    a dead end — PM itself never elevates.
//  - DetachConfirm — the consent gate in front of "use a vault on this account instead".
//    Detaching an owner whose vault moved into the shared folder lands on a fresh EMPTY
//    vault; doing that silently is how the lockout incident read as total data loss. The
//    copy states exactly which vault the user is about to get, and that nothing in the
//    shared folder is deleted.

import { useState } from "react";
import { acknowledgeDeletedSharedVault, repairVaultAccess } from "../lib/ipc";
import type { DeletedVaultNotice as DeletedNotice, VaultStatus } from "../lib/types";
import { formatDateOnly } from "../lib/format";
import { Button, Modal } from "./ui";

/** The admin fallback recipe for `path`, shown when in-app repair fails. `%USERNAME%`
 *  is expanded by the user's own shell, so the line is copyable as-is. */
function repairRecipe(path: string): { grant: string; takeown: string } {
  return {
    grant: `icacls "${path}" /grant "%USERNAME%:(OI)(CI)F" /T /C`,
    takeown: `takeown /f "${path}" /r /d y`,
  };
}

/** A copyable one-liner in a monospace block (the wizard's `pre` treatment). */
function CommandLine({ command }: { command: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="flex items-center gap-2">
      <pre className="min-w-0 flex-1 overflow-x-auto whitespace-pre rounded-[var(--radius-sm)] border border-border bg-bg px-3 py-2 font-mono text-xs text-ink3">
        {command}
      </pre>
      <Button
        variant="tertiary"
        onClick={() => {
          void navigator.clipboard.writeText(command).then(() => {
            setCopied(true);
            setTimeout(() => setCopied(false), 1500);
          });
        }}
      >
        {copied ? "Copied ✓" : "Copy"}
      </Button>
    </div>
  );
}

export function RepairAccessButton({
  path,
  onRepaired,
  variant = "primary",
}: {
  /** The vault folder being repaired (for the fallback recipe). */
  path: string | null;
  /** Called when the folder answers again — reload vault status; a repaired-but-still-
   *  locked vault falls through to the unlock prompt from there. */
  onRepaired: () => void;
  variant?: "primary" | "secondary";
}) {
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [warnings, setWarnings] = useState<string[]>([]);

  async function repair() {
    if (busy) return;
    setBusy(true);
    setFailure(null);
    try {
      const outcome = await repairVaultAccess();
      setWarnings(outcome.warnings);
      onRepaired();
    } catch (e) {
      setFailure(String(e));
    } finally {
      setBusy(false);
    }
  }

  const recipe = path ? repairRecipe(path) : null;
  return (
    <div className="flex w-full max-w-sm flex-col items-center gap-2">
      <Button variant={variant} disabled={busy} onClick={() => void repair()}>
        {busy ? "Repairing…" : "Repair access"}
      </Button>
      {warnings.map((w, i) => (
        <p key={i} className="break-words text-xs text-ink4">
          {w}
        </p>
      ))}
      {failure && (
        <div className="w-full rounded-[var(--radius)] border border-border bg-surface p-3 text-left">
          <p className="break-words text-xs text-st-due">{failure}</p>
          <p className="mt-2 text-xs text-ink3">
            PM couldn't repair it from this account. If this vault is <strong>yours</strong>, open
            PowerShell as Administrator (Win+X → Terminal (Admin)), run the line below, then try
            again:
          </p>
          {recipe && (
            <div className="mt-2 space-y-2">
              <CommandLine command={recipe.grant} />
              <p className="text-xs text-ink4">
                If Windows still says access is denied, take ownership first, then re-run the line
                above:
              </p>
              <CommandLine command={recipe.takeown} />
            </div>
          )}
          <p className="mt-2 text-xs text-ink4">
            If someone else set the vault up, ask them to open PM on their account and re-add you:
            Settings → Vault → Manage sharing.
          </p>
        </div>
      )}
    </div>
  );
}

export function DeletedVaultNotice({
  notice,
  onAcknowledged,
}: {
  /** The tombstone record: which shared folder was deleted, and when. */
  notice: DeletedNotice;
  /** Called after the switch-to-local completes — reload boot state; this screen unmounts. */
  onAcknowledged: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const when = notice.deleted_at ? formatDateOnly(notice.deleted_at) : null;

  async function acknowledge() {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await acknowledgeDeletedSharedVault();
      onAcknowledged();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  }

  return (
    <div className="flex h-full flex-col items-center justify-center gap-4 bg-bg px-6 text-center">
      <div className="flex h-12 w-12 items-center justify-center rounded-full border border-border text-ink2">
        {/* Info glyph — no icon dependency (matches the other vault gates). */}
        <svg width="22" height="22" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <circle cx="12" cy="12" r="9" stroke="currentColor" strokeWidth="1.6" />
          <path d="M12 11v5" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
          <circle cx="12" cy="7.6" r="0.9" fill="currentColor" />
        </svg>
      </div>
      <div>
        <h1 className="font-ui text-lg font-semibold text-ink">The shared vault was deleted</h1>
        <p className="mt-1 max-w-sm text-sm text-ink4">
          The shared vault
          {when ? ` was deleted by its owner on ${when}` : " was deleted by its owner"}. PM will
          switch you back to the vault on this account — anything that was only in the shared vault
          is no longer available here.
        </p>
      </div>
      {error && (
        <p
          className="max-w-xs break-words rounded-[var(--radius)] px-3 py-2 text-xs text-st-due"
          style={{ background: "color-mix(in oklab, var(--st-due) 15%, transparent)" }}
        >
          {error}
        </p>
      )}
      <Button variant="primary" disabled={busy} onClick={() => void acknowledge()}>
        {busy ? "Switching…" : "Continue"}
      </Button>
      <p className="max-w-sm break-all text-xs text-faint">{notice.folder}</p>
    </div>
  );
}

export function DetachConfirm({
  open,
  onClose,
  status,
  onConfirm,
}: {
  open: boolean;
  onClose: () => void;
  /** Drives the copy: which vault the user is about to land on, and the folder left behind. */
  status: VaultStatus | null;
  /** The caller runs the actual detach (each gate reloads differently afterwards). */
  onConfirm: () => void;
}) {
  const setAside = status?.has_set_aside_vault ?? false;
  const folder = status?.pointed_root;
  return (
    <Modal
      open={open}
      onClose={onClose}
      labelledBy="detach-confirm-title"
      widthClassName="max-w-md"
    >
      <div className="space-y-3 p-6">
        <h1 id="detach-confirm-title" className="font-head text-lg font-semibold text-ink">
          Switch to a vault on this account?
        </h1>
        <p className="text-sm text-ink2">
          {setAside
            ? "PM will switch back to the vault that was set aside on this account when you joined the shared one."
            : "PM will start a new, empty vault on this account — the shared vault's contents won't be in it."}
        </p>
        <p className="text-xs text-ink4">
          Nothing{folder ? ` in ${folder}` : " in the shared folder"} is deleted. You can rejoin it
          any time from Settings → Vault with the passphrase.
        </p>
        <div className="flex justify-end gap-2 pt-1">
          <Button variant="tertiary" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            onClick={() => {
              onClose();
              onConfirm();
            }}
          >
            Switch vault
          </Button>
        </div>
      </div>
    </Modal>
  );
}
