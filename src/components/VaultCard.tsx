// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Settings "Vault" card (spec §2–6): shows whether the vault is device-only or a
// shareable, passphrase-protected one, and drives every transition through the backend's
// one migration routine. Sharing with other Windows accounts is a single guided flow
// (ShareVaultWizard) — passphrase, move to a reachable folder, and account grants in the
// one order that can't strand the vault in the profile dir (issue #337); joining an
// existing shared vault is the "Open an existing shared vault…" form. Markdown
// encryption is forced on (and the toggle hidden) for a shareable vault, because once it
// can be opened from another account folder isolation no longer protects the notes. The
// plaintext export is always offered — the promise that the user is never locked in.

import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  adoptSharedVault,
  changeVaultPassphrase,
  deleteSharedVault,
  detachFromSharedVault,
  exportPlaintextMarkdown,
  forgetVaultPassphrase,
  makeVaultPrivate,
  vaultStatus,
} from "../lib/ipc";
import type { PassphraseScore, VaultStatus } from "../lib/types";
import { Button, Callout, Input, SectionInfo } from "./ui";
import { formatDate } from "../lib/format";
import { PassphraseStrengthMeter } from "./PassphraseStrengthMeter";
import { ShareVaultWizard } from "./ShareVaultWizard";
import { DetachConfirm, RepairAccessButton } from "./VaultRecovery";
import { joinErrorMessage } from "./VaultJoin";
import { markJustJoinedVault } from "../lib/joinedVault";

/** Which inline form/confirmation is currently open (only one at a time). */
type Pending = "change" | "private" | "adopt" | "delete" | null;

export function VaultCard() {
  const [status, setStatus] = useState<VaultStatus | null>(null);
  const [pending, setPending] = useState<Pending>(null);
  const [pass, setPass] = useState("");
  const [confirm, setConfirm] = useState("");
  const [passScore, setPassScore] = useState<PassphraseScore | null>(null);
  const [adoptFolder, setAdoptFolder] = useState<string | null>(null);
  // Ticked by a joiner who means to take a shared vault over by re-keying it. Deliberately NOT
  // sticky: `reset()` clears it with the rest of the form, so re-opening the panel asks again.
  const [takeover, setTakeover] = useState(false);
  const [confirmDetach, setConfirmDetach] = useState(false);
  const [wizardOpen, setWizardOpen] = useState(false);
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
    setPassScore(null);
    setAdoptFolder(null);
    setTakeover(false);
  }

  // A create/change passphrase clears the meter's floor unless the meter explicitly says it's too
  // weak — a scoring hiccup (null) never soft-locks the button; the backend floor is the real gate.
  const strongEnough = passScore?.acceptable !== false;

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

  async function pickAdoptFolder() {
    const dir = await pickFolder();
    if (dir) setAdoptFolder(dir);
  }

  async function exportPlaintext() {
    // The backend opens the folder picker itself (L-5), so we don't pass a path.
    setBusy(true);
    setError(null);
    setMsg(null);
    try {
      const res = await exportPlaintextMarkdown();
      if (!res) return; // cancelled
      setMsg(`Exported ${res.count} Markdown file${res.count === 1 ? "" : "s"} to ${res.dest}`);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  /** Join an existing shared vault from this account: unlock + point this profile at
   *  it, then reload the whole webview so every view reboots on the new store (the same
   *  pattern as the backup-restore switch). The previous vault stays on disk, set aside. */
  async function adopt() {
    if (!adoptFolder) return;
    setBusy(true);
    setError(null);
    try {
      await adoptSharedVault(adoptFolder, pass);
      // Explain what stays personal (own key + sign-ins) on the Connectors tab after the reload,
      // exactly like the boot-time join gate.
      markJustJoinedVault();
      window.location.reload();
    } catch (e) {
      // Classified copy: folder-denied ≠ wrong passphrase ≠ no vault ≠ damaged store.
      setError(joinErrorMessage(e, pass));
      setBusy(false);
    }
  }

  /** Delete the shared vault for everyone, then reload the whole webview onto the local
   *  vault this account switched to (the same store-swap reload as adopt/detach). */
  async function deleteShared() {
    setBusy(true);
    setError(null);
    try {
      await deleteSharedVault();
      window.location.reload();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  }

  /** Leave the shared vault and reopen the one set aside at join time (or a fresh, empty one) —
   *  the non-destructive way out for a joiner, and what "Make private" is refused in favour of. */
  async function detach() {
    setBusy(true);
    setError(null);
    try {
      await detachFromSharedVault();
      window.location.reload();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  }

  const passphrasesMatch = pass.length > 0 && pass === confirm;
  const shareable = status?.mode === "passphrase";
  // A shared vault provably stamped with ANOTHER Windows account's SID. Only this case is gated:
  // "unknown" (every shared vault off Windows, and every vault from before ownership was recorded)
  // deliberately behaves as it always has — see the backend's `gate_for`.
  const joined = status?.ownership === "joined";

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

      {/* The one live reader of the ownership-transfer record. It is written under the vault
          metadata's MAC by a confirmed takeover, so it cannot be erased without PM saying the file
          was altered — but a record nobody displays is a record nobody can act on. */}
      {status?.ownership_transfer && (
        <p className="mt-1 text-xs text-ink4">
          Ownership of this vault was transferred on{" "}
          <span className="text-ink3">{formatDate(status.ownership_transfer.at)}</span>
          {status.ownership === "owned"
            ? " — to this account."
            : status.ownership === "joined"
              ? " — to another account on this PC."
              : "."}
        </p>
      )}

      {/* The mid-session heal: PM lost access to the vault folder (the watcher or a command
          noticed) — name it and offer the one-click repair right here, while other views can
          only report their operations failing. */}
      {status?.fault && (
        <div className="mt-3 space-y-2 rounded-[var(--radius-sm)] border border-border2 p-3">
          <Callout as="p" className="break-words">
            {status.fault.message}
          </Callout>
          {status.fault.code === "denied" && (
            <RepairAccessButton
              path={status.fault.path ?? status.pointed_root ?? null}
              onRepaired={() => void refresh()}
              variant="secondary"
            />
          )}
        </div>
      )}

      {/* The way back to a shared vault this account once left: one click re-opens the
          adopt form on the recorded folder (passphrase still required — nothing silent). */}
      {status?.retired_root && !status.pointed_root && pending !== "adopt" && (
        <div className="mt-3">
          <Button
            variant="tertiary"
            disabled={busy}
            onClick={() => {
              setAdoptFolder(status.retired_root);
              setPending("adopt");
            }}
          >
            Rejoin the shared vault at {status.retired_root}…
          </Button>
        </div>
      )}

      {/* Device-only → the guided share flow (passphrase → shared folder → accounts). */}
      {status && !shareable && (
        <div className="mt-3">
          <Button
            variant="secondary"
            onClick={() => setWizardOpen(true)}
            disabled={busy}
            data-help="settings-vault-share"
          >
            Share with other accounts…
          </Button>
        </div>
      )}

      {/* Shareable → manage actions (why encryption is forced folds into the card's info block). */}
      {shareable && (
        <>
          <div className="mt-3 flex flex-wrap gap-2">
            <Button
              variant="secondary"
              onClick={() => setWizardOpen(true)}
              disabled={busy}
              data-help="settings-vault-share"
            >
              Manage sharing…
            </Button>
            <Button variant="secondary" onClick={() => setPending("change")} disabled={busy}>
              Change passphrase…
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
            {/* Deleting a shared vault only makes sense once it's actually in a shared
                folder (pointed) — it removes the vault for every account that uses it. */}
            {/* Hidden outright for a vault another account created: it is theirs to delete, and
                PM should not hand a joiner the button. Still offered when ownership is UNKNOWN (a
                vault from before ownership was recorded, or an account whose SID changed), because
                hiding it there would lock a genuine owner out of their own vault — the confirmation
                warns instead. Not a security boundary: anyone with write access to the folder can
                delete the files themselves. The backend refuses the joined case too. */}
            {status?.pointed_root && status.ownership !== "joined" && (
              <Button variant="tertiary" onClick={() => setPending("delete")} disabled={busy}>
                Delete shared vault…
              </Button>
            )}
          </div>

          {pending === "change" && (
            <div className="mt-3 space-y-2 rounded-[var(--radius-sm)] border border-border2 p-3">
              <p className="text-xs text-ink4">
                Set a new passphrase. The vault is re-keyed and its Markdown re-encrypted.
                {status?.pointed_root
                  ? " Every other account using this shared vault is locked out until you give them the new passphrase."
                  : ""}
              </p>
              {/* A joiner re-keying a vault someone else created also becomes its recorded owner —
                  the backend refuses without this box, which is a speed bump against doing it by
                  accident, not an authorization control. `live={false}` on the frame because the
                  inner <p> is already the alert: two nested live regions say it twice. */}
              {joined && (
                <Callout body="ink" live={false} className="space-y-2">
                  <p className="text-xs text-st-due" role="alert">
                    This shared vault was created by another account on this PC. Changing its
                    passphrase locks everyone else out until you tell them the new one, and makes
                    this account the vault&rsquo;s owner. Ask its owner to change it instead if you
                    can.
                  </p>
                  <label className="flex items-start gap-2 text-xs text-ink3">
                    <input
                      type="checkbox"
                      className="mt-0.5"
                      checked={takeover}
                      onChange={(e) => setTakeover(e.currentTarget.checked)}
                      disabled={busy}
                    />
                    <span>
                      I&rsquo;m taking over ownership of this vault — everyone else needs the new
                      passphrase from me.
                    </span>
                  </label>
                </Callout>
              )}
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
              <PassphraseStrengthMeter passphrase={pass} onScored={setPassScore} />
              <div className="flex gap-2">
                <Button
                  variant="primary"
                  disabled={busy || !passphrasesMatch || !strongEnough || (joined && !takeover)}
                  onClick={() =>
                    run(
                      () => changeVaultPassphrase(pass, joined && takeover),
                      "Passphrase changed.",
                    )
                  }
                >
                  {busy ? "Working…" : "Change passphrase"}
                </Button>
                <Button variant="tertiary" onClick={reset} disabled={busy}>
                  Cancel
                </Button>
              </div>
            </div>
          )}

          {pending === "private" && (
            <div className="mt-3 space-y-2 rounded-[var(--radius-sm)] border border-border2 p-3">
              {/* For a vault another account created there is no confirmation on offer, because
                  there is no way back: making it private re-keys it to THIS account's device key
                  (held in this keychain alone), decrypts the notes, and moves the shared folder
                  into this profile. Unlike a re-key, no passphrase gets the owner back in. So the
                  button is replaced by the non-destructive exit, which the backend's refusal names
                  too — the two must not disagree about what a joiner is offered. */}
              {joined ? (
                <>
                  <p className="text-xs text-st-due" role="alert">
                    This shared vault was created by another account on this PC, so it isn&rsquo;t
                    yours to make private &mdash; that would re-key it to this account alone and
                    move it out of the shared folder, with no way back in for anyone else.
                  </p>
                  {/* Leaving is what `detachFromSharedVault` does, and it retires this profile's
                      POINTER — so it is only on offer when there is a pointer to retire. Without
                      one it is a silent no-op, and a button that does nothing is worse than no
                      button: the honest answer here is that PM has nothing to offer. (Reachable
                      when a shareable vault sits in this profile's own folder and the account's SID
                      has since changed, which reads as `joined` on the user's own vault.) */}
                  {status?.pointed_root && (
                    <p className="text-xs text-ink4">
                      You can leave it instead. The shared vault stays exactly where it is for
                      everyone still using it, and you can rejoin any time with the passphrase.
                    </p>
                  )}
                  <div className="flex gap-2">
                    {status?.pointed_root && (
                      <Button
                        variant="secondary"
                        disabled={busy}
                        onClick={() => setConfirmDetach(true)}
                      >
                        Use a vault on this account instead
                      </Button>
                    )}
                    <Button variant="tertiary" onClick={reset} disabled={busy}>
                      Cancel
                    </Button>
                  </div>
                </>
              ) : (
                <>
                  <p className="text-xs text-ink4">
                    Make this vault private to this device again? It's re-keyed to a device-only key
                    and its Markdown is decrypted back to plaintext. Other profiles will no longer
                    be able to open it.
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
                </>
              )}
            </div>
          )}

          {pending === "delete" && (
            <div className="mt-3 space-y-2 rounded-[var(--radius-sm)] border border-st-due p-3">
              <p className="text-xs text-ink3">
                Delete this shared vault for <span className="font-medium text-st-due">every</span>{" "}
                account that uses it? Its documents, chats, and projects are{" "}
                <span className="font-medium text-st-due">permanently removed</span> from the shared
                folder. Connected accounts lose access at their next launch and are moved back to a
                vault of their own.
              </p>
              <p className="text-xs text-ink4">
                {status?.has_set_aside_vault
                  ? "This account switches back to the vault that was set aside when you joined."
                  : "This account switches to a new, empty vault (your data was moved into the shared copy when you shared it)."}
              </p>
              {status?.ownership === "unknown" && (
                <p className="text-xs text-st-due" role="alert">
                  PM can&rsquo;t confirm this account created this vault &mdash; it was made before
                  PM recorded that, or this account has changed since. If someone else set it up,
                  ask them to delete it from their own PM instead.
                </p>
              )}
              <p className="text-xs text-ink4">
                Back up anything you still want first. This cannot be undone, and PM keeps no copy.
              </p>
              <div className="flex gap-2">
                <Button variant="danger" disabled={busy} onClick={() => void deleteShared()}>
                  {busy ? "Deleting…" : "Delete for everyone"}
                </Button>
                <Button variant="tertiary" onClick={reset} disabled={busy}>
                  Cancel
                </Button>
              </div>
            </div>
          )}
        </>
      )}

      {/* Always available: join a shared vault someone else set up on this PC. */}
      <div className="mt-3" data-help="settings-vault-join">
        {pending !== "adopt" ? (
          <Button variant="tertiary" onClick={() => setPending("adopt")} disabled={busy}>
            Open an existing shared vault…
          </Button>
        ) : (
          <div className="space-y-2 rounded-[var(--radius-sm)] border border-border2 p-3">
            <p className="text-xs text-ink4">
              Point PM at a shared vault folder someone set up on this PC (or a copied vault) and
              open it with its passphrase. PM switches to that vault; the one you're using now is
              kept on disk, set aside — nothing is deleted.
            </p>
            <div className="flex flex-wrap items-center gap-2">
              <Button variant="secondary" onClick={() => void pickAdoptFolder()} disabled={busy}>
                Choose the vault folder…
              </Button>
              {adoptFolder && (
                <span className="break-all font-mono text-xs text-ink3">{adoptFolder}</span>
              )}
            </div>
            <Input
              type="password"
              placeholder="Vault passphrase"
              value={pass}
              onChange={(e) => setPass(e.target.value)}
            />
            <div className="flex gap-2">
              <Button
                variant="primary"
                disabled={busy || !adoptFolder || pass.length === 0}
                onClick={() => void adopt()}
              >
                {busy ? "Joining…" : "Open shared vault"}
              </Button>
              <Button variant="tertiary" onClick={reset} disabled={busy}>
                Cancel
              </Button>
            </div>
          </div>
        )}
      </div>

      {/* Always available: the portability escape hatch. */}
      <div className="mt-3">
        <Button variant="tertiary" onClick={exportPlaintext} disabled={busy}>
          Export to plaintext Markdown…
        </Button>
      </div>

      {error && <p className="mt-2 break-all text-xs text-st-due">{error}</p>}
      {msg && <p className="mt-2 break-all text-xs text-faint">{msg}</p>}

      <SectionInfo title="How the vault works">
        {shareable && (
          <p>
            Markdown encryption is on because this vault is shared. Without it, other accounts on
            this device could read your notes directly.
          </p>
        )}
        <p>
          Your files stay yours. Export to plaintext Markdown anytime with your passphrase —
          encryption protects them at rest, it doesn't lock you in.
        </p>
      </SectionInfo>

      <ShareVaultWizard
        open={wizardOpen}
        onClose={() => setWizardOpen(false)}
        status={status}
        onChanged={refresh}
      />

      {/* The same consent gate the unlock and open-error screens put in front of leaving, so a
          joiner refused "Make private" gets the identical, honest story about what happens next. */}
      <DetachConfirm
        open={confirmDetach}
        onClose={() => setConfirmDetach(false)}
        status={status}
        onConfirm={() => void detach()}
      />
    </div>
  );
}
