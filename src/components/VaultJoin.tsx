// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The join gate (issue #337): shown on a fresh profile when another Windows account has
// advertised a shared vault on this PC. One passphrase away from all the shared
// documents — this is the screen account B never had, which made a shared vault
// silently boot into a fresh empty one instead. Visual family of VaultUnlock (same
// glyph treatment, same error box); joining is handled by `adopt_shared_vault`, which
// validates the real vault metadata — the advertisement is only a signpost.

import { useState } from "react";
import { adoptSharedVault, vaultFaultOf } from "../lib/ipc";
import type { SharedVaultAd } from "../lib/types";
import { Button, Input } from "./ui";

/** The join-failure story by classified fault code — a joiner-persona message for each
 *  distinct cause, so "the owner hasn't added you" is never read as "wrong passphrase"
 *  (the lockout incident's most damaging conflation). Falls back to the raw message. */
export function joinErrorMessage(e: unknown): string {
  const fault = vaultFaultOf(e);
  switch (fault?.code) {
    case "denied":
      return (
        "The vault's owner needs to add this Windows account before you can join — on " +
        "their side: Settings → Vault → Manage sharing. (If this vault is yours, open PM " +
        "on the account that set it up and use Repair access.)"
      );
    case "wrong-passphrase":
      return "That passphrase doesn't match this vault. The folder itself is fine.";
    case "no-vault":
      return "No PM vault in that folder — pick the folder that holds vault-meta.json and pm.sqlite.";
    case "corrupt":
      return (
        "The passphrase is right, but the vault's database won't open — it may be damaged. " +
        "Its owner can restore it from a backup."
      );
    default:
      return String(e);
  }
}

export function VaultJoin({
  vaults,
  onJoined,
  onSkip,
}: {
  /** The advertised vaults this profile could join (non-empty when rendered). */
  vaults: SharedVaultAd[];
  /** Called after a successful join — reload boot state; this gate unmounts. */
  onJoined: () => void;
  /** "Set up my own PM instead" — fall through to normal onboarding this launch. */
  onSkip: () => void;
}) {
  const [selected, setSelected] = useState(0);
  const [pass, setPass] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const vault = vaults[Math.min(selected, vaults.length - 1)];

  async function join() {
    if (busy || pass.length === 0 || !vault) return;
    setBusy(true);
    setError(null);
    try {
      await adoptSharedVault(vault.vault_root, pass);
      onJoined();
    } catch (e) {
      setError(joinErrorMessage(e));
      setBusy(false);
    }
  }

  return (
    <div className="flex h-full flex-col items-center justify-center gap-4 bg-bg px-6 text-center">
      <div className="flex h-12 w-12 items-center justify-center rounded-full border border-border text-ink2">
        {/* Two-people glyph — no icon dependency (matches the other vault gates). */}
        <svg width="22" height="22" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <circle cx="9" cy="8" r="3" stroke="currentColor" strokeWidth="1.6" />
          <path
            d="M3.5 19c.7-3 2.8-4.5 5.5-4.5s4.8 1.5 5.5 4.5"
            stroke="currentColor"
            strokeWidth="1.6"
            strokeLinecap="round"
          />
          <circle cx="16.5" cy="9.5" r="2.3" stroke="currentColor" strokeWidth="1.6" />
          <path
            d="M16.5 14.2c2.2.1 3.6 1.4 4.1 3.8"
            stroke="currentColor"
            strokeWidth="1.6"
            strokeLinecap="round"
          />
        </svg>
      </div>

      <div>
        <h1 className="font-ui text-lg font-semibold text-ink">Join the shared vault?</h1>
        <p className="mt-1 max-w-sm text-sm text-ink4">
          {vaults.length > 1
            ? "More than one shared Personal Manager vault was found on this PC — pick yours and enter its passphrase."
            : `A shared Personal Manager vault was found on this PC${
                vault?.owner ? ` — set up by ${vault.owner}` : ""
              }. Enter its passphrase to open the same documents, chats, and projects here.`}
        </p>
      </div>

      <form
        onSubmit={(e) => {
          e.preventDefault();
          void join();
        }}
        className="flex w-full max-w-sm flex-col gap-2"
      >
        {vaults.length > 1 && (
          <div className="flex flex-col gap-1 text-left">
            {vaults.map((v, i) => (
              <label
                key={v.vault_id}
                className="flex cursor-pointer items-center gap-2 rounded-[var(--radius-sm)] border border-border2 px-3 py-2 text-sm text-ink2"
              >
                <input
                  type="radio"
                  name="shared-vault"
                  checked={selected === i}
                  onChange={() => setSelected(i)}
                  disabled={busy}
                />
                <span className="min-w-0">
                  <span className="block truncate">
                    {v.label}
                    {v.owner ? ` — ${v.owner}` : ""}
                  </span>
                  <span className="block truncate font-mono text-xs text-ink4">{v.vault_root}</span>
                </span>
              </label>
            ))}
          </div>
        )}
        <Input
          type="password"
          autoComplete="current-password"
          autoFocus
          placeholder="Vault passphrase"
          value={pass}
          onChange={(e) => setPass(e.target.value)}
          disabled={busy}
        />
        {error && (
          <p
            className="rounded-[var(--radius)] px-3 py-2 text-xs text-st-due"
            style={{ background: "color-mix(in oklab, var(--st-due) 15%, transparent)" }}
          >
            {error}
          </p>
        )}
        <Button variant="primary" type="submit" disabled={busy || pass.length === 0}>
          {busy ? "Joining…" : "Join this vault"}
        </Button>
      </form>

      <div className="max-w-sm space-y-1">
        <p className="text-xs text-ink4">
          Anything already set up on this account is kept safely aside — nothing is deleted.
        </p>
        <p className="text-xs text-faint">
          If PM is open on the other account right now, you'll be asked to take over — only one
          account writes at a time.
        </p>
      </div>

      <button
        className="text-xs text-ink4 underline underline-offset-2 hover:text-ink3"
        disabled={busy}
        onClick={onSkip}
      >
        Set up my own PM instead
      </button>

      {vaults.length === 1 && vault && (
        <p className="max-w-sm break-all text-xs text-faint">{vault.vault_root}</p>
      )}
    </div>
  );
}
