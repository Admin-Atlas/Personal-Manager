// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Remove PM data" — the in-app, à-la-carte teardown that clears PM off the machine (the counterpart
// to the Windows uninstaller, which only removes the app + the regenerable runtime). Because this is
// irreversible, it's gated behind a deliberate confirmation ladder:
//
//   1. a "Remove PM data" button unlocks the checkboxes (they start locked),
//   2. the user picks what to remove,
//   3. an "Are you sure?" step itemises exactly what's selected and each consequence,
//   4. an optional Windows Hello / Touch ID check (when the OS can verify),
//   5. a type-to-confirm ("Delete PM Data"),
//   6. then, and only then, the removal runs.
//
// The three storage/secret classes go to the backend (`wipePmData`); browser preferences are cleared
// here in the webview. Backups are deliberately NOT deletable here — the UI directs the user to the
// backup destination (Proton / Google Drive) to remove those at the source.

import { useState } from "react";
import { exit } from "@tauri-apps/plugin-process";
import { confirmWipeIdentity, wipePmData } from "../lib/ipc";
import { formatBytes } from "../lib/format";
import type { WipeReport } from "../lib/types";
import { Button, Input, Modal } from "./ui";

interface Props {
  /** Whether the OS can run a Windows Hello / Touch ID check (from app-lock status). When true, the
   *  wipe requires a successful verification before the final type-to-confirm gate. */
  biometricAvailable: boolean;
}

/** The exact phrase the user must type to arm the final Delete. */
const CONFIRM_PHRASE = "Delete PM Data";

/** Where to finish revoking Microsoft access (no programmatic revoke for a public desktop client). */
const MICROSOFT_APPS_URL = "https://account.live.com/consent/Manage";

interface Selection {
  regenerable: boolean;
  vaultAndDb: boolean;
  keychain: boolean;
  /** Cleared in the webview, not the backend. */
  localStorage: boolean;
}

const EMPTY_SELECTION: Selection = {
  regenerable: false,
  vaultAndDb: false,
  keychain: false,
  localStorage: false,
};

type Stage = "locked" | "select" | "confirm" | "type" | "working" | "done";

interface Item {
  key: keyof Selection;
  label: string;
  /** What it is. */
  detail: string;
  /** What happens if it's removed — the consequence shown in the "are you sure" step. */
  consequence: string;
  /** Irreversible user data / access — tinted and called out. */
  danger?: boolean;
}

const ITEMS: Item[] = [
  {
    key: "regenerable",
    label: "Downloaded components",
    detail:
      "The document engine, the enhanced-map and photo-text add-ons, and the speech model — everything Settings → Storage can free.",
    consequence: "Frees the most space. Re-downloads automatically the next time it's needed.",
  },
  {
    key: "vaultAndDb",
    label: "Vault & database",
    detail:
      "Every document, note, chat, project and setting — your Markdown vault and its encrypted store.",
    consequence: "Permanent. Everything you've put into PM is gone unless you have a backup.",
    danger: true,
  },
  {
    key: "keychain",
    label: "Saved keys & sign-ins",
    detail:
      "Your API keys, the database key, the backup passphrase, and every connected Google / Microsoft account, all held in the OS keychain.",
    consequence:
      "Revokes PM's Google access and forgets every key. Microsoft access is finished separately at account.live.com.",
    danger: true,
  },
  {
    key: "localStorage",
    label: "App preferences (this window)",
    detail: "Theme, panel sizes, and other on-device interface preferences.",
    consequence: "Resets the interface to its defaults.",
  },
];

export function RemovePmData({ biometricAvailable }: Props) {
  const [stage, setStage] = useState<Stage>("locked");
  const [sel, setSel] = useState<Selection>(EMPTY_SELECTION);
  const [confirmText, setConfirmText] = useState("");
  const [verifying, setVerifying] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<WipeReport | null>(null);

  const anySelected = sel.regenerable || sel.vaultAndDb || sel.keychain || sel.localStorage;
  const selectedItems = ITEMS.filter((i) => sel[i.key]);
  // The database's only key lives in the keychain, so a store left behind after the keychain is wiped
  // can never be opened again. We therefore FORCE "Vault & database" on whenever "Saved keys" is
  // selected — the vault checkbox is locked on in that case — and keep the warning explaining why.
  const vaultForcedByKeychain = sel.keychain;

  function reset() {
    setSel(EMPTY_SELECTION);
    setConfirmText("");
    setError(null);
    setReport(null);
    setStage("locked");
  }

  function toggle(key: keyof Selection) {
    setSel((s) => {
      const next = { ...s, [key]: !s[key] };
      // Enforce the invariant "keychain implies vault & database" in both directions: ticking saved
      // keys pulls in the store, and the store can't be unticked while saved keys are selected.
      if (key === "keychain" && next.keychain) next.vaultAndDb = true;
      if (key === "vaultAndDb" && !next.vaultAndDb && next.keychain) next.vaultAndDb = true;
      return next;
    });
  }

  // "Continue to deletion" → the optional Hello gate, then the type-to-confirm step.
  async function proceedFromConfirm() {
    setError(null);
    if (biometricAvailable) {
      setVerifying(true);
      try {
        const ok = await confirmWipeIdentity();
        if (!ok) {
          setError("Verification was cancelled — nothing was removed.");
          return;
        }
      } catch (e) {
        setError(`Couldn't verify: ${String(e)}`);
        return;
      } finally {
        setVerifying(false);
      }
    }
    setConfirmText("");
    setStage("type");
  }

  async function runWipe() {
    setStage("working");
    setError(null);
    try {
      const backendSelected = sel.regenerable || sel.vaultAndDb || sel.keychain;
      let rep: WipeReport | null = null;
      if (backendSelected) {
        rep = await wipePmData({
          regenerable: sel.regenerable,
          vaultAndDb: sel.vaultAndDb,
          keychain: sel.keychain,
        });
      }
      if (sel.localStorage) {
        try {
          localStorage.clear();
        } catch {
          /* a locked webview store just keeps its prefs — nothing sensitive there */
        }
      }
      setReport(rep);
      setStage("done");
    } catch (e) {
      setError(String(e));
      setStage("type"); // back to the last gate so they can retry or cancel
    }
  }

  return (
    <div className="mt-5 border-t border-border pt-4" data-help="settings-remove-data">
      <label className="block text-sm font-medium text-st-due">Remove PM data</label>
      <p className="mt-1 text-xs text-ink4">
        Erase PM from this machine — choose exactly what to remove. Uninstalling PM the usual way
        already clears the big re-downloadable components; this is for clearing your actual data,
        saved keys, and sign-ins as well. Some of this can&apos;t be undone.
      </p>

      {stage === "locked" && (
        <div className="mt-3">
          <Button
            variant="secondary"
            onClick={() => {
              setSel(EMPTY_SELECTION);
              setStage("select");
            }}
            style={{ color: "var(--st-due)" }}
          >
            Remove PM data…
          </Button>
        </div>
      )}

      {stage === "select" && (
        <div className="mt-3">
          <div className="space-y-2">
            {ITEMS.map((item) => {
              // "Vault & database" is locked on while "Saved keys" is selected (see vaultForcedByKeychain).
              const locked = item.key === "vaultAndDb" && vaultForcedByKeychain;
              return (
                <label
                  key={item.key}
                  className={`flex items-start gap-3 rounded-[var(--radius)] border border-border bg-surface px-3 py-2.5 transition ${
                    locked ? "cursor-default opacity-90" : "cursor-pointer hover:border-border2"
                  }`}
                >
                  <input
                    type="checkbox"
                    className="mt-0.5 h-4 w-4 shrink-0 accent-[var(--st-due)] disabled:opacity-60"
                    checked={sel[item.key]}
                    disabled={locked}
                    onChange={() => toggle(item.key)}
                  />
                  <span className="min-w-0">
                    <span
                      className={`block text-sm font-medium ${item.danger ? "text-st-due" : "text-ink2"}`}
                    >
                      {item.label}
                      {locked && (
                        <span className="ml-1.5 font-normal text-ink4">
                          · included with saved keys
                        </span>
                      )}
                    </span>
                    <span className="mt-0.5 block text-xs text-ink4">{item.detail}</span>
                  </span>
                </label>
              );
            })}
          </div>

          {vaultForcedByKeychain && (
            <p
              className="mt-2 rounded-[var(--radius)] px-3 py-2 text-xs text-st-due"
              style={{ background: "color-mix(in oklab, var(--st-due) 12%, transparent)" }}
            >
              Removing your saved keys also removes the vault &amp; database — the database&apos;s
              only key lives in the keychain, so the store can&apos;t be kept without it. Both are
              permanent.
            </p>
          )}

          {/* Backups sit apart — PM won't delete them here on purpose. */}
          <div className="mt-4 border-t border-border pt-3">
            <p className="text-xs text-ink4">
              <span className="font-medium text-ink3">Encrypted backups</span> aren&apos;t removed
              here. Any local <span className="font-mono">.pmbackup</span> files and your Proton /
              Google Drive backups live outside PM — delete those yourself at the destination if you
              want them gone.
            </p>
          </div>

          {error && <p className="mt-2 text-xs text-st-due">{error}</p>}

          <div className="mt-4 flex justify-end gap-2">
            <Button variant="tertiary" onClick={reset}>
              Cancel
            </Button>
            <Button
              variant="secondary"
              disabled={!anySelected}
              onClick={() => {
                setError(null);
                setStage("confirm");
              }}
              style={anySelected ? { color: "var(--st-due)" } : undefined}
            >
              Continue
            </Button>
          </div>
        </div>
      )}

      {/* Step 3 — "Are you sure?": itemise the selection + consequences. */}
      <Modal
        open={stage === "confirm"}
        onClose={verifying ? () => {} : () => setStage("select")}
        widthClassName="max-w-md"
      >
        <div className="p-5">
          <h2 className="font-head text-base font-semibold text-st-due">Remove this data?</h2>
          <p className="mt-2 text-sm text-ink3">
            You&apos;re about to remove the following from this machine:
          </p>
          <ul className="mt-3 space-y-2">
            {selectedItems.map((item) => (
              <li key={item.key} className="text-sm">
                <span className={`font-medium ${item.danger ? "text-st-due" : "text-ink2"}`}>
                  {item.label}
                </span>
                <span className="mt-0.5 block text-xs text-ink4">{item.consequence}</span>
              </li>
            ))}
          </ul>
          {vaultForcedByKeychain && (
            <p className="mt-3 text-xs text-st-due">
              Removing your saved keys removes the vault &amp; database along with them — the
              database&apos;s only key is in the keychain, so it can&apos;t be kept.
            </p>
          )}
          {(sel.vaultAndDb || sel.keychain) && (
            <p className="mt-3 text-xs text-ink4">
              This can&apos;t be undone, and PM will close afterwards.
            </p>
          )}
          {error && <p className="mt-3 text-xs text-st-due">{error}</p>}
          <div className="mt-5 flex justify-end gap-2">
            <Button variant="tertiary" onClick={() => setStage("select")} disabled={verifying}>
              Back
            </Button>
            <Button
              variant="primary"
              onClick={() => void proceedFromConfirm()}
              disabled={verifying}
              style={{
                background: "color-mix(in oklab, var(--st-due) 15%, transparent)",
                color: "var(--st-due)",
              }}
            >
              {verifying ? "Verifying…" : "Continue to deletion"}
            </Button>
          </div>
        </div>
      </Modal>

      {/* Step 5 — type-to-confirm. */}
      <Modal open={stage === "type"} onClose={() => setStage("select")} widthClassName="max-w-md">
        <div className="p-5">
          <h2 className="font-head text-base font-semibold text-st-due">Final confirmation</h2>
          <p className="mt-2 text-sm text-ink3">
            Type <span className="font-mono font-medium text-ink2">{CONFIRM_PHRASE}</span> to
            confirm you want to permanently remove the selected data.
          </p>
          <Input
            value={confirmText}
            onChange={(e) => setConfirmText(e.target.value)}
            placeholder={CONFIRM_PHRASE}
            autoFocus
            autoComplete="off"
            className="mt-3"
          />
          {error && <p className="mt-3 text-xs text-st-due">{error}</p>}
          <div className="mt-5 flex justify-end gap-2">
            <Button variant="tertiary" onClick={() => setStage("select")}>
              Cancel
            </Button>
            <Button
              variant="primary"
              disabled={confirmText !== CONFIRM_PHRASE}
              onClick={() => void runWipe()}
              style={
                confirmText === CONFIRM_PHRASE
                  ? {
                      background: "color-mix(in oklab, var(--st-due) 15%, transparent)",
                      color: "var(--st-due)",
                    }
                  : undefined
              }
            >
              Delete PM data
            </Button>
          </div>
        </div>
      </Modal>

      {/* Working / done. */}
      <Modal
        open={stage === "working" || stage === "done"}
        onClose={() => {}}
        widthClassName="max-w-md"
      >
        <div className="p-5">
          {stage === "working" ? (
            <>
              <h2 className="font-head text-base font-semibold text-ink">Removing…</h2>
              <p className="mt-2 text-sm text-ink3">
                Clearing the selected data{sel.keychain ? " and revoking access" : ""}. This can
                take a moment.
              </p>
            </>
          ) : (
            <>
              <h2 className="font-head text-base font-semibold text-ink">PM data removed</h2>
              <ul className="mt-3 space-y-1 text-sm text-ink3">
                {(report?.removed ?? []).map((r) => (
                  <li key={r}>• {r}</li>
                ))}
                {sel.localStorage && <li>• App preferences (this window)</li>}
              </ul>
              {report && (
                <div className="mt-3 space-y-1 text-xs text-ink4">
                  {report.freedBytes > 0 && <p>Freed about {formatBytes(report.freedBytes)}.</p>}
                  {report.keychainDeleted > 0 && (
                    <p>{report.keychainDeleted} saved key(s) removed from the keychain.</p>
                  )}
                  {report.googleRevoked > 0 && (
                    <p>
                      {report.googleRevoked} Google sign-in(s) revoked
                      {report.googleRevokeFailures > 0
                        ? ` (${report.googleRevokeFailures} couldn't be reached, but were removed locally)`
                        : ""}
                      .
                    </p>
                  )}
                  {report.microsoftAccounts.length > 0 && (
                    <p className="text-st-due">
                      Finish removing PM&apos;s access to {report.microsoftAccounts.join(", ")} at{" "}
                      <a
                        href={MICROSOFT_APPS_URL}
                        target="_blank"
                        rel="noreferrer"
                        className="underline hover:brightness-110"
                      >
                        account.live.com
                      </a>
                      .
                    </p>
                  )}
                </div>
              )}
              <div className="mt-5 flex justify-end gap-2">
                {report?.quitRequired ? (
                  <Button variant="primary" onClick={() => void exit(0)}>
                    Close PM
                  </Button>
                ) : sel.localStorage ? (
                  <Button variant="primary" onClick={() => window.location.reload()}>
                    Reload
                  </Button>
                ) : (
                  <Button variant="primary" onClick={reset}>
                    Done
                  </Button>
                )}
              </div>
            </>
          )}
        </div>
      </Modal>
    </div>
  );
}
