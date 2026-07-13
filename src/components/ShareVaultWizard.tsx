// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The guided "share this vault with other Windows accounts" flow (issue #337). It
// replaces three formerly-independent VaultCard actions (make shareable, move, link)
// whose split let a vault become "shareable" while still sitting in the owner's
// per-user profile folder — unreachable by every other account, with the link action
// granting ACEs that could never work. The wizard runs the steps in the only order
// that can't produce that trap: passphrase → move to a cross-account location (one
// crash-recoverable migration) → grant accounts on the REAL folder → hand-off
// instructions for the other side. Re-running it on an already-shareable vault skips
// the passphrase step and offers the move/link steps again — which is also the
// upgrade path for vaults stuck in the old in-place state.

import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  createShareableVault,
  linkVaultAccount,
  listLocalAccounts,
  moveVault,
  suggestSharedVaultLocation,
  vaultFaultOf,
} from "../lib/ipc";
import type { LocalAccount, PassphraseScore, VaultStatus } from "../lib/types";
import { Button, Card, Collapsible, Input, Modal } from "./ui";
import { PassphraseStrengthMeter } from "./PassphraseStrengthMeter";
import { RepairAccessButton } from "./VaultRecovery";

type Step = "passphrase" | "location" | "accounts" | "done";

interface Props {
  open: boolean;
  onClose: () => void;
  /** The vault's status when the wizard opened — decides which steps apply. */
  status: VaultStatus | null;
  /** Reload the caller's vault status after anything committed. */
  onChanged: () => void | Promise<void>;
}

/** Per-account link state on the accounts step. `denied` is the folder itself refusing
 *  THIS account (the owner-lockout moment, not a per-account grant problem) — it gets
 *  the repair treatment instead of an inert error line. */
type LinkState =
  { kind: "linked" } | { kind: "error"; message: string } | { kind: "denied"; message: string };

export function ShareVaultWizard({ open, onClose, status, onChanged }: Props) {
  const stepTitles: Record<Step, string> = {
    passphrase: "Choose a passphrase",
    location: "Where it lives",
    accounts: "Who can open it",
    done: "All set",
  };

  // Freeze the step list at OPEN time. Committing the share flips the vault to passphrase mode and
  // refreshes the parent's `status` mid-wizard; deriving `steps` from the live prop would then drop
  // the passphrase step and make the "Step 2 of 4" counter jump backward to "Step 2 of 3".
  const [steps, setSteps] = useState<Step[]>(["passphrase", "location", "accounts", "done"]);
  const [step, setStep] = useState<Step>("passphrase");
  const [pass, setPass] = useState("");
  const [confirm, setConfirm] = useState("");
  const [passScore, setPassScore] = useState<PassphraseScore | null>(null);
  const [location, setLocation] = useState<string | null>(null);
  const [suggestedWritable, setSuggestedWritable] = useState(true);
  const [accounts, setAccounts] = useState<LocalAccount[]>([]);
  const [linkStates, setLinkStates] = useState<Record<string, LinkState>>({});
  const [manualAccount, setManualAccount] = useState("");
  const [linkedAny, setLinkedAny] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [warnings, setWarnings] = useState<string[]>([]);

  // Reset and pre-fill each time the wizard opens: fresh step, and the location
  // defaults to where the vault already lives (a re-run) or the suggested shared spot.
  useEffect(() => {
    if (!open) return;
    const alreadyShared = status?.mode === "passphrase";
    setSteps(
      alreadyShared
        ? ["location", "accounts", "done"]
        : ["passphrase", "location", "accounts", "done"],
    );
    setStep(alreadyShared ? "location" : "passphrase");
    setPass("");
    setConfirm("");
    setPassScore(null);
    setError(null);
    setWarnings([]);
    setLinkStates({});
    setManualAccount("");
    setLinkedAny(false);
    if (alreadyShared && status?.pointed_root) {
      // Already moved out of the profile — the current folder is the default.
      setLocation(status.location);
      setSuggestedWritable(true);
    } else {
      setLocation(null);
      suggestSharedVaultLocation()
        .then((s) => {
          setLocation(s.path);
          setSuggestedWritable(s.path === null || s.writable);
        })
        .catch(() => {
          setLocation(null);
          setSuggestedWritable(true);
        });
    }
    // The wizard captures the status it opened with; a mid-flight external change is
    // caught by the backend's own guards.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const passphrasesMatch = pass.length > 0 && pass === confirm;
  // A scoring hiccup (null) never soft-locks the button; the backend floor is the real gate.
  const strongEnough = passScore?.acceptable !== false;
  const stepIndex = steps.indexOf(step) + 1;
  // Frozen at open (via `steps`): a fresh-share flow has the passphrase step, a re-run doesn't.
  const isFreshShare = steps.includes("passphrase");

  async function pickFolder() {
    const picked = await openDialog({ directory: true, multiple: false });
    if (typeof picked === "string") setLocation(picked);
  }

  /** The location step's commit: one migration for a new share (rekey + move), or a
   *  plain move (or no-op) on a re-run. Every later ACE lands on the final folder. */
  async function commitShare() {
    if (!location) return;
    setBusy(true);
    setError(null);
    try {
      const outcome = isFreshShare
        ? await createShareableVault(pass, location)
        : location === status?.location
          ? { warnings: [] }
          : await moveVault(location);
      setWarnings(outcome.warnings);
      setPass("");
      setConfirm("");
      await onChanged();
      const found = await listLocalAccounts().catch(() => []);
      setAccounts(found.filter((a) => !a.is_current));
      setStep("accounts");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function link(principal: string, key: string) {
    setBusy(true);
    setError(null);
    try {
      const outcome = await linkVaultAccount(principal);
      setWarnings((w) => [...w, ...outcome.warnings]);
      setLinkStates((s) => ({ ...s, [key]: { kind: "linked" } }));
      setLinkedAny(true);
    } catch (e) {
      // The folder itself refusing THIS account is not a per-account grant failure — it
      // means the move-time lockdown went wrong and the owner is on the way to being
      // locked out (issue #343). Say so and offer the repair right here.
      const fault = vaultFaultOf(e);
      setLinkStates((s) => ({
        ...s,
        [key]:
          fault?.code === "denied"
            ? {
                kind: "denied",
                message:
                  "PM just lost access to the shared folder itself — its permissions went " +
                  "wrong during the move. Repair access, then add accounts again.",
              }
            : { kind: "error", message: String(e) },
      }));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Modal
      open={open}
      onClose={onClose}
      labelledBy="share-vault-wizard-title"
      widthClassName="max-w-xl"
      className="flex max-h-[85vh] flex-col"
    >
      <div className="flex items-center justify-between border-b border-border px-6 py-4">
        <div>
          <p className="font-mono text-xs uppercase tracking-wide text-ink4">
            Step {stepIndex} of {steps.length} · {stepTitles[step]}
          </p>
          <h1 id="share-vault-wizard-title" className="font-head text-lg font-semibold text-ink">
            Share this vault with other accounts
          </h1>
        </div>
        <Button variant="tertiary" onClick={onClose}>
          {step === "done" ? "Close" : "Cancel"}
        </Button>
      </div>

      <div className="flex-1 overflow-y-auto px-6 py-4">
        {step === "passphrase" && (
          <div className="space-y-2">
            <p className="text-sm text-ink2">
              This passphrase becomes the vault's key. Anyone who knows it can open the vault, and
              it can't be recovered — so make it strong, and share it privately with the people you
              trust.
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
            <PassphraseStrengthMeter passphrase={pass} onScored={setPassScore} />
            <div className="pt-2">
              <Button
                variant="primary"
                disabled={!passphrasesMatch || !strongEnough}
                onClick={() => setStep("location")}
              >
                Continue
              </Button>
            </div>
          </div>
        )}

        {step === "location" && (
          <div className="space-y-2">
            <p className="text-sm text-ink2">
              The vault moves to a folder every account on this PC can reach — that's what makes
              sharing actually work. The suggested spot is right for almost everyone.
            </p>
            {location ? (
              <p className="break-all rounded-[var(--radius-sm)] border border-border bg-bg px-3 py-2 font-mono text-xs text-ink3">
                {location}
              </p>
            ) : (
              <p className="text-xs text-ink4">
                No suggested spot on this system — pick a folder that other accounts can reach.
              </p>
            )}
            {!suggestedWritable && (
              <p className="text-xs text-st-due">
                PM couldn't confirm it can write there — you can still try, or pick another folder.
              </p>
            )}
            <div className="flex flex-wrap items-center gap-2 pt-2">
              <Button variant="primary" disabled={busy || !location} onClick={commitShare}>
                {busy ? "Sharing…" : isFreshShare ? "Share vault" : "Continue"}
              </Button>
              <Button variant="tertiary" onClick={pickFolder} disabled={busy}>
                Choose a different folder…
              </Button>
              {/* A fresh share can step back to fix a passphrase the backend rejected. */}
              {isFreshShare && (
                <Button variant="tertiary" onClick={() => setStep("passphrase")} disabled={busy}>
                  Back
                </Button>
              )}
            </div>
          </div>
        )}

        {step === "accounts" && (
          <div className="space-y-3">
            <p className="text-sm text-ink2">
              Pick the accounts that should be able to open the vault. You can add more later from
              this same screen.
            </p>
            {accounts.length > 0 ? (
              <div className="space-y-2">
                {accounts.map((a) => {
                  const state = linkStates[a.sid];
                  return (
                    <div
                      key={a.sid}
                      className="flex items-center justify-between rounded-[var(--radius-sm)] border border-border2 px-3 py-2"
                    >
                      <div className="min-w-0">
                        <p className="truncate text-sm text-ink2">{a.name}</p>
                        {(state?.kind === "error" || state?.kind === "denied") && (
                          <p className="break-all text-xs text-st-due">{state.message}</p>
                        )}
                        {state?.kind === "denied" && (
                          <div className="mt-2">
                            <RepairAccessButton
                              path={location}
                              onRepaired={() => setLinkStates({})}
                              variant="secondary"
                            />
                          </div>
                        )}
                      </div>
                      <Button
                        variant="secondary"
                        disabled={busy || state?.kind === "linked"}
                        onClick={() => link(a.sid, a.sid)}
                      >
                        {state?.kind === "linked" ? "Added ✓" : "Add"}
                      </Button>
                    </div>
                  );
                })}
              </div>
            ) : (
              <p className="text-xs text-ink4">
                PM couldn't list this PC's accounts — add one by name or SID below.
              </p>
            )}
            <Collapsible title="Account not listed? Enter a name or SID">
              <div className="mt-2 space-y-2">
                <p className="text-xs text-ink4">
                  Enter the account's <strong>name</strong> (like{" "}
                  <span className="font-mono">PC\alice</span>) or its <strong>SID</strong> (
                  <span className="font-mono">S-1-5-21-…</span>, survives renames). To find either,
                  open <strong>PowerShell</strong> (Win+X → Terminal) and run:
                </p>
                <pre className="overflow-x-auto whitespace-pre-wrap rounded-[var(--radius-sm)] border border-border bg-bg px-3 py-2 font-mono text-xs text-ink3">
                  Get-LocalUser | Select Name, SID
                </pre>
                <div className="flex gap-2">
                  <Input
                    placeholder="Account name or SID"
                    value={manualAccount}
                    onChange={(e) => setManualAccount(e.target.value)}
                  />
                  <Button
                    variant="secondary"
                    disabled={busy || manualAccount.trim().length === 0}
                    onClick={() => {
                      const value = manualAccount.trim();
                      setManualAccount("");
                      void link(value, value);
                    }}
                  >
                    Add
                  </Button>
                </div>
                {manualAccount.trim().length === 0 &&
                  Object.entries(linkStates)
                    .filter(([key]) => !accounts.some((a) => a.sid === key))
                    .map(([key, state]) =>
                      state.kind === "error" || state.kind === "denied" ? (
                        <div key={key}>
                          <p className="break-all text-xs text-st-due">
                            {key}: {state.message}
                          </p>
                          {state.kind === "denied" && (
                            <div className="mt-2">
                              <RepairAccessButton
                                path={location}
                                onRepaired={() => setLinkStates({})}
                                variant="secondary"
                              />
                            </div>
                          )}
                        </div>
                      ) : null,
                    )}
              </div>
            </Collapsible>
            <div className="flex items-center gap-3 pt-1">
              <Button variant="primary" disabled={busy} onClick={() => setStep("done")}>
                {linkedAny ? "Continue" : "I'll add accounts later"}
              </Button>
            </div>
            {!linkedAny && (
              <p className="text-xs text-ink4">
                Until an account is added here, Windows won't let it open the vault folder.
              </p>
            )}
          </div>
        )}

        {step === "done" && (
          <div className="space-y-3">
            <p className="text-sm text-ink2">All set — here's what happens on the other account:</p>
            <Card className="p-4">
              <ol className="space-y-3">
                {[
                  "Sign in to Windows as them and open Personal Manager (install it first if needed).",
                  "PM spots this shared vault and offers to join it — they enter the passphrase you just chose.",
                  "They add their own OpenRouter key. Keys and sign-ins never travel between Windows accounts.",
                  "They reconnect any cloud accounts and calendars as themselves — everything already indexed is there waiting.",
                ].map((line, i) => (
                  <li key={i} className="flex gap-3 text-sm text-ink2">
                    <span className="mt-0.5 select-none font-mono text-xs text-ink4">{i + 1}.</span>
                    <span>{line}</span>
                  </li>
                ))}
              </ol>
            </Card>
            <p className="text-xs text-ink4">
              Only one account writes at a time — if PM is open here when they join, they'll be
              offered a hand-over, and you'll get it back the same way.
            </p>
          </div>
        )}

        {warnings.length > 0 && (
          <div className="mt-4 space-y-1">
            {warnings.map((w, i) => (
              <p key={i} className="break-all text-xs text-ink4">
                {w}
              </p>
            ))}
          </div>
        )}
        {error && <p className="mt-4 break-all text-xs text-st-due">{error}</p>}
      </div>
    </Modal>
  );
}
