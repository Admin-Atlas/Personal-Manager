// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Argon2id key derivation for shareable vaults. A user passphrase becomes the raw
//! 256-bit SQLCipher key via Argon2id; the *cost* parameters are calibrated once at
//! vault creation and written (non-secret) to `vault-meta.json`, then read back
//! verbatim on every unlock so the same passphrase derives the identical key on any
//! machine, regardless of speed (spec §2.2). Never hardcode the cost on the read
//! path — always use the stored params.

use std::time::Instant;

use argon2::{Algorithm, Argon2, Params, Version};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::{Error, Result};

/// Derived master-key length in bytes (the 256-bit SQLCipher key).
pub const KEY_LEN: usize = 32;
/// KDF salt length in bytes (comfortably above Argon2's 8-byte minimum).
pub const SALT_LEN: usize = 16;
/// The only Argon2 version we emit/accept: 0x13 (== 19), i.e. Argon2 v1.3.
const ARGON2_VERSION: u32 = 0x13;

/// Argon2id cost parameters. These are **not secret** — they live in
/// `vault-meta.json` alongside the salt so any profile/machine can reproduce the
/// key. Stored as plain numbers (not a PHC string) because we own the salt+params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    /// Always "argon2id".
    pub algorithm: String,
    /// Argon2 version number; always 0x13.
    pub version: u32,
    /// Memory cost in KiB.
    pub m_cost_kib: u32,
    /// Time cost (number of passes).
    pub t_cost: u32,
    /// Degree of parallelism (lanes).
    pub p_cost: u32,
    /// Output length in bytes; always 32 here.
    pub key_len: u32,
}

impl KdfParams {
    fn argon2(&self) -> Result<Argon2<'static>> {
        if self.algorithm != "argon2id" {
            return Err(Error::Other(format!(
                "unsupported KDF algorithm in vault-meta.json: {}",
                self.algorithm
            )));
        }
        if self.version != ARGON2_VERSION {
            return Err(Error::Other(format!(
                "unsupported Argon2 version in vault-meta.json: {}",
                self.version
            )));
        }
        let params = Params::new(
            self.m_cost_kib,
            self.t_cost,
            self.p_cost,
            Some(self.key_len as usize),
        )
        .map_err(|e| Error::Other(format!("invalid Argon2 parameters: {e}")))?;
        Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
    }

    /// Cost params at a given memory tier and pass count, parallelism fixed at 1.
    fn at(m_cost_kib: u32, t_cost: u32) -> Self {
        Self {
            algorithm: "argon2id".to_string(),
            version: ARGON2_VERSION,
            m_cost_kib,
            t_cost,
            p_cost: 1,
            key_len: KEY_LEN as u32,
        }
    }
}

/// Derive the raw 32-byte master key from a passphrase, the stored salt, and the
/// stored cost params. Deterministic across runs and machines — this is the whole
/// point of the shareable key model. The result is zeroized on drop.
pub fn derive_master(
    passphrase: &str,
    salt: &[u8],
    params: &KdfParams,
) -> Result<Zeroizing<[u8; KEY_LEN]>> {
    if params.key_len as usize != KEY_LEN {
        return Err(Error::Other("vault KDF key_len must be 32".into()));
    }
    let argon = params.argon2()?;
    let mut out = Zeroizing::new([0u8; KEY_LEN]);
    argon
        .hash_password_into(passphrase.as_bytes(), salt, out.as_mut_slice())
        .map_err(|e| Error::Other(format!("Argon2 derivation failed: {e}")))?;
    Ok(out)
}

/// Memory tiers (KiB) tried during calibration, strongest first: 256 / 128 / 64 MiB.
const MEM_TIERS_KIB: [u32; 3] = [256 * 1024, 128 * 1024, 64 * 1024];

/// Pick Argon2id cost params that take roughly `target_ms` on *this* machine, so
/// unlock is ~250–500 ms regardless of hardware (spec §2.2). Prefers the most
/// memory-hard tier the machine can run without blowing far past the target; within
/// a tier it raises the time cost until the target is met. The chosen params are
/// stored and thereafter used verbatim — calibration never runs on the read path.
pub fn calibrate(target_ms: u64) -> KdfParams {
    // A box so slow that even the lightest tier at one pass blows past this is
    // treated as "give up tuning, take the lightest tier".
    let ceiling_ms = target_ms.saturating_mul(2);
    let lightest = MEM_TIERS_KIB[MEM_TIERS_KIB.len() - 1];

    for &m_cost_kib in &MEM_TIERS_KIB {
        let one_pass = KdfParams::at(m_cost_kib, 1);
        let baseline = measure_ms(&one_pass);

        // This much memory is already too heavy even at a single pass — try less
        // (unless this is already the lightest tier, in which case accept it).
        if baseline > ceiling_ms && m_cost_kib != lightest {
            continue;
        }
        if baseline >= target_ms {
            return one_pass;
        }
        // Raise passes until we reach the target (cap to keep unlock bounded).
        for t_cost in 2..=10u32 {
            let candidate = KdfParams::at(m_cost_kib, t_cost);
            if measure_ms(&candidate) >= target_ms {
                return candidate;
            }
        }
        // Even the max passes at this tier stay under target (fast machine): accept
        // the strongest we tried here rather than dropping memory.
        return KdfParams::at(m_cost_kib, 10);
    }

    // Unreachable in practice (the last tier always returns above), but be explicit.
    KdfParams::at(lightest, 1)
}

/// Time one derivation with the given params. A derivation failure (shouldn't happen
/// for params we construct) is reported as "very slow" so calibration moves on
/// rather than panicking.
fn measure_ms(params: &KdfParams) -> u64 {
    // Salt content is irrelevant to Argon2 timing; draw it from the CSPRNG anyway so
    // no constant salt ever appears on a derivation path. An RNG failure is reported
    // as "very slow" so calibration moves on rather than panicking.
    let salt: [u8; SALT_LEN] = match super::random_array() {
        Ok(s) => s,
        Err(_) => return u64::MAX,
    };
    let start = Instant::now();
    match derive_master("calibration-probe", &salt, params) {
        Ok(_) => start.elapsed().as_millis() as u64,
        Err(_) => u64::MAX,
    }
}
