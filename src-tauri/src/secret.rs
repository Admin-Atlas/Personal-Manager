// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A redacting wrapper for in-memory secret strings (the DB key, OpenRouter keys,
//! Google OAuth tokens + client secret, ICS bearer URLs). It does two distinct
//! jobs the keychain boundary doesn't:
//!
//!   1. **Memory hygiene** — the value lives in a `Zeroizing` buffer, so the heap
//!      copy is wiped when the `Secret` drops (same as the raw key buffers did).
//!   2. **Redaction** — `Debug` and `Display` print `[REDACTED]`, never the value.
//!      So a `println!`/`format!`/log added to this tree later cannot leak a secret
//!      by accident, and any struct that *derives* `Debug` over a `Secret` field
//!      (e.g. [`crate::google::Token`]) inherits that protection for free. This is
//!      the durable form of the audit's "no secret in a log" property: it survives
//!      someone adding logging, instead of depending on no one ever doing so.
//!
//! Reading the real value is deliberately explicit via [`Secret::expose`] — the one
//! way out, so an audit can `grep` every site a secret is actually read. `Serialize`
//! / `Deserialize` are transparent (the raw string), because the trust boundary is
//! the OS keychain, not this type: the Google token blob must still round-trip to
//! and from its keychain entry unchanged.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::Zeroizing;

/// A secret string that never prints itself. Clone is allowed (each clone owns a
/// zeroizing buffer); equality is intentionally **not** derived to avoid a timing
/// oracle — compare exposed values explicitly with a constant-time check if needed.
#[derive(Clone)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    /// Borrow the underlying secret. The single intentional escape hatch: every read
    /// of a real secret value is one greppable `.expose()` call.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Secret(Zeroizing::new(value))
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Secret(Zeroizing::new(value.to_owned()))
    }
}

impl From<Zeroizing<String>> for Secret {
    /// Adopt an existing zeroizing buffer without copying its contents into a fresh
    /// plaintext `String` first (used by the DB-key generator).
    fn from(value: Zeroizing<String>) -> Self {
        Secret(value)
    }
}

/// Redact in debug output — the whole point of the type. Mirrors a struct field, so
/// `#[derive(Debug)]` on a struct holding a `Secret` prints `Secret([REDACTED])`.
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

/// Redact in display output, so an accidental `format!("{secret}")` is safe too.
impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Transparent: the stored blob carries the real value (the keychain is the boundary,
/// not this wrapper). So `serde_json::to_string(&Token { .. })` still produces the
/// genuine token JSON we persist to the keychain.
impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.expose())
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Secret::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Deliberately generic, non-secret-shaped fixtures so the repo's gitleaks rules
    // (which match the app's real secret shapes) don't fire on test data.
    const FIXTURE: &str = "value-under-redaction-0001";

    #[test]
    fn debug_and_display_never_reveal_the_value() {
        let s = Secret::from(FIXTURE);
        assert_eq!(format!("{s:?}"), "Secret([REDACTED])");
        assert_eq!(format!("{s}"), "[REDACTED]");
        assert!(!format!("{s:?} {s}").contains(FIXTURE));
    }

    #[test]
    fn derived_debug_on_a_holder_struct_is_also_redacted() {
        // The durable guard: a struct that derives Debug over a Secret field cannot
        // leak it — this is what protects google::Token's access/refresh tokens.
        #[derive(Debug)]
        struct Holder {
            token: Secret,
        }
        let h = Holder {
            token: Secret::from(FIXTURE),
        };
        let rendered = format!("{h:?}");
        assert!(rendered.contains("Secret([REDACTED])"));
        assert!(!rendered.contains(FIXTURE));
        // The field still holds the real value — it simply refuses to print it.
        assert_eq!(h.token.expose(), FIXTURE);
    }

    #[test]
    fn expose_returns_the_real_value() {
        assert_eq!(Secret::from(FIXTURE).expose(), FIXTURE);
    }

    #[test]
    fn serde_round_trips_transparently() {
        // Persisted blobs (e.g. the OAuth token) must serialize the real value and
        // read back identically — the redaction is for formatting, not storage.
        #[derive(Serialize, Deserialize, Debug)]
        struct Holder {
            token: Secret,
        }
        let json = serde_json::to_string(&Holder {
            token: Secret::from(FIXTURE),
        })
        .unwrap();
        assert_eq!(json, format!(r#"{{"token":"{FIXTURE}"}}"#));

        let back: Holder = serde_json::from_str(&json).unwrap();
        assert_eq!(back.token.expose(), FIXTURE);
        // ...and the value that just round-tripped still can't be debug-printed.
        assert!(!format!("{back:?}").contains(FIXTURE));
    }

    #[test]
    fn adopts_a_zeroizing_buffer_without_a_plaintext_copy() {
        let buf = Zeroizing::new(String::from(FIXTURE));
        assert_eq!(Secret::from(buf).expose(), FIXTURE);
    }
}
