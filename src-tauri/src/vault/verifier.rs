// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A small, non-secret "did the passphrase derive the right key?" check. Without it,
//! a wrong passphrase would surface only as SQLCipher's opaque "not a database"
//! error; with it we can say "wrong passphrase" cleanly *before* touching the DB
//! (spec §8). The verifier is a BLAKE3 keyed hash of a stored random salt under the
//! master key — it reveals nothing about the key (preimage-resistant), and the
//! Argon2id work factor, not this check, is what makes guessing expensive.

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

const SALT_LEN: usize = 16;
/// Domain-separation tag mixed into the verifier so it can never collide with the
/// Markdown subkey or any other use of the master key.
const VERIFIER_TAG: &[u8] = b"org.itsatlas.pm passphrase-verifier v1";

/// The non-secret wrong-passphrase check stored in `vault-meta.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verifier {
    /// Always "blake3-keyed".
    pub alg: String,
    pub salt_b64: String,
    pub mac_b64: String,
}

fn compute(master: &[u8; 32], salt: &[u8]) -> blake3::Hash {
    let mut data = Vec::with_capacity(VERIFIER_TAG.len() + salt.len());
    data.extend_from_slice(VERIFIER_TAG);
    data.extend_from_slice(salt);
    blake3::keyed_hash(master, &data)
}

/// Build a verifier from the master key (at vault creation / passphrase change).
pub fn build(master: &[u8; 32]) -> Result<Verifier> {
    let salt: [u8; SALT_LEN] = super::random_array()?;
    let mac = compute(master, &salt);
    Ok(Verifier {
        alg: "blake3-keyed".to_string(),
        salt_b64: B64.encode(salt),
        mac_b64: B64.encode(mac.as_bytes()),
    })
}

/// Constant-time check that `master` matches the key this verifier was built from.
pub fn check(verifier: &Verifier, master: &[u8; 32]) -> Result<bool> {
    if verifier.alg != "blake3-keyed" {
        return Err(Error::Other(format!(
            "unsupported verifier algorithm: {}",
            verifier.alg
        )));
    }
    let salt = B64
        .decode(&verifier.salt_b64)
        .map_err(|e| Error::Other(format!("corrupt verifier salt: {e}")))?;
    let expected: [u8; 32] = B64
        .decode(&verifier.mac_b64)
        .map_err(|e| Error::Other(format!("corrupt verifier mac: {e}")))?
        .try_into()
        .map_err(|_| Error::Other("verifier mac must be 32 bytes".into()))?;
    let got = compute(master, &salt);
    // blake3::Hash equality is constant-time.
    Ok(got == blake3::Hash::from(expected))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_building_key_and_rejects_others() {
        let master = [9u8; 32];
        let other = [10u8; 32];
        let v = build(&master).unwrap();
        assert_eq!(v.alg, "blake3-keyed");
        assert!(check(&v, &master).unwrap());
        assert!(!check(&v, &other).unwrap());
    }

    #[test]
    fn round_trips_through_json() {
        let v = build(&[1u8; 32]).unwrap();
        let json = serde_json::to_string(&v).unwrap();
        let back: Verifier = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }
}
