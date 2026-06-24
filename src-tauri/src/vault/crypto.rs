// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Markdown-at-rest encryption (spec §3). When a vault is shared, folder isolation is
//! gone, so the Markdown files themselves must be ciphertext. Each file is a small
//! self-describing container encrypted with XChaCha20-Poly1305 under a subkey derived
//! from the vault master (see [`super::markdown_subkey`]) — never the DB key.
//!
//! Container layout: `magic "PMVAULT1" | version(1) | alg(1) | nonce(24) | ct+tag`.
//! The 192-bit XChaCha nonce is random per write, which is safe even under the many
//! small metadata rewrites the vault performs (a 96-bit nonce would be borderline).
//! The AAD binds the ciphertext to `vault_id:stem`, so a file copied to another vault,
//! renamed, or moved fails authentication instead of silently decrypting.
//!
//! Reads are by magic (tolerating a mixed plaintext/ciphertext folder mid-migration);
//! writes are by the vault's policy.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};

use crate::error::{Error, Result};

const MAGIC: &[u8; 8] = b"PMVAULT1";
const VERSION: u8 = 1;
const ALG_XCHACHA20POLY1305: u8 = 1;
const NONCE_LEN: usize = 24;
/// magic(8) + version(1) + alg(1) + nonce(24).
const HEADER_LEN: usize = MAGIC.len() + 1 + 1 + NONCE_LEN;

/// Whether these bytes are a PM-encrypted Markdown container (by magic prefix). Used
/// on read so a folder part-way through migration (some plaintext, some ciphertext)
/// is handled per file.
pub fn is_encrypted(bytes: &[u8]) -> bool {
    bytes.len() >= MAGIC.len() && &bytes[..MAGIC.len()] == MAGIC
}

/// Additional authenticated data: binds a file to its vault + filename stem.
fn aad(vault_id: &str, stem: &str) -> Vec<u8> {
    format!("{vault_id}:{stem}").into_bytes()
}

/// Encrypt Markdown bytes into a self-describing container.
pub fn encrypt(plaintext: &[u8], mkey: &[u8; 32], vault_id: &str, stem: &str) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(Key::from_slice(mkey));
    let mut nonce_bytes = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce_bytes).map_err(|e| Error::Other(format!("rng failure: {e}")))?;
    let aad = aad(vault_id, stem);
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce_bytes),
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| Error::Other("markdown encryption failed".into()))?;
    let mut out = Vec::with_capacity(HEADER_LEN + ciphertext.len());
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.push(ALG_XCHACHA20POLY1305);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt a container produced by [`encrypt`]. An AEAD failure (wrong key, tamper, or
/// a file moved/renamed so the AAD no longer matches) is distinguished from a
/// malformed/foreign container.
pub fn decrypt(container: &[u8], mkey: &[u8; 32], vault_id: &str, stem: &str) -> Result<Vec<u8>> {
    if !is_encrypted(container) || container.len() < HEADER_LEN {
        return Err(Error::Other("not a PM-encrypted markdown file".into()));
    }
    let version = container[MAGIC.len()];
    let alg = container[MAGIC.len() + 1];
    if version != VERSION || alg != ALG_XCHACHA20POLY1305 {
        return Err(Error::Other(format!(
            "unsupported markdown container (version {version}, alg {alg})"
        )));
    }
    let nonce = XNonce::from_slice(&container[MAGIC.len() + 2..HEADER_LEN]);
    let cipher = XChaCha20Poly1305::new(Key::from_slice(mkey));
    let aad = aad(vault_id, stem);
    cipher
        .decrypt(
            nonce,
            Payload {
                msg: &container[HEADER_LEN..],
                aad: &aad,
            },
        )
        .map_err(|_| {
            Error::Other(
                "could not decrypt markdown (wrong key, or the file was tampered with or moved)"
                    .into(),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 32] = [7u8; 32];
    const OTHER_KEY: [u8; 32] = [9u8; 32];

    #[test]
    fn round_trips() {
        let pt = b"# Note\n\nsome secret body text";
        let ct = encrypt(pt, &KEY, "vault-1", "note-abc").unwrap();
        assert!(is_encrypted(&ct));
        assert!(!is_encrypted(pt));
        let back = decrypt(&ct, &KEY, "vault-1", "note-abc").unwrap();
        assert_eq!(back, pt);
    }

    #[test]
    fn wrong_key_fails() {
        let ct = encrypt(b"body", &KEY, "vault-1", "note-abc").unwrap();
        assert!(decrypt(&ct, &OTHER_KEY, "vault-1", "note-abc").is_err());
    }

    #[test]
    fn wrong_aad_fails_so_moved_or_cross_vault_files_dont_decrypt() {
        let ct = encrypt(b"body", &KEY, "vault-1", "note-abc").unwrap();
        // Different vault id, or different filename stem -> authentication fails.
        assert!(decrypt(&ct, &KEY, "vault-2", "note-abc").is_err());
        assert!(decrypt(&ct, &KEY, "vault-1", "renamed").is_err());
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let mut ct = encrypt(b"body", &KEY, "vault-1", "note-abc").unwrap();
        let last = ct.len() - 1;
        ct[last] ^= 0x01;
        assert!(decrypt(&ct, &KEY, "vault-1", "note-abc").is_err());
    }

    #[test]
    fn nonce_is_random_so_two_encryptions_differ() {
        let a = encrypt(b"same", &KEY, "v", "s").unwrap();
        let b = encrypt(b"same", &KEY, "v", "s").unwrap();
        assert_ne!(a, b, "fresh nonce per write should make ciphertexts differ");
        // ...yet both decrypt back to the same plaintext.
        assert_eq!(decrypt(&a, &KEY, "v", "s").unwrap(), b"same");
        assert_eq!(decrypt(&b, &KEY, "v", "s").unwrap(), b"same");
    }

    #[test]
    fn plaintext_and_garbage_are_not_treated_as_encrypted() {
        assert!(!is_encrypted(b"# plain markdown"));
        assert!(!is_encrypted(b""));
        assert!(decrypt(b"# plain markdown", &KEY, "v", "s").is_err());
    }
}
