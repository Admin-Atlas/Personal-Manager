// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The copy that outlives the padded-passphrase lockout (fixed in 3.19.1). Vaults created or
// re-keyed BEFORE that release derived their key from the trimmed passphrase, while every
// unlock has always hashed the exact bytes — so such a vault opens only under the trimmed
// form, and nothing on disk records which vaults are affected. Whoever set it up never
// notices (their key is cached); it bites whoever has to actually type the passphrase — a
// second Windows account joining, or an unlock after "forget passphrase here". The nudge is
// shown only when what was typed really carries padding, which is exactly the population it
// can help; everyone else sees the unchanged message.
//
// This advice is one-directional — it can say "try it without", never "try it with" — which is
// sound ONLY because 3.19.1 also made create/change REFUSE a padded passphrase (kdf.rs policy
// Rule 2). No vault keyed to genuinely padded bytes can be minted from here on, so every padded
// vault that will ever exist predates 3.19.1 and is keyed to its trimmed form, and dropping the
// padding is always the right move. Were padding ever accepted again, this would start telling
// some users the exact opposite of their real passphrase — so the two changes travel together.

/** The recovery nudge for a pre-3.19.1 padded-passphrase vault, or `""` when the typed
 *  passphrase has no leading/trailing whitespace and the hint would only be noise.
 *  Appended to the wrong-passphrase copy on every surface that takes a passphrase. */
export function paddedPassphraseHint(passphrase: string): string {
  return passphrase === passphrase.trim()
    ? ""
    : " It starts or ends with a space — vaults set up before PM 3.19.1 left those out, so try it without.";
}
