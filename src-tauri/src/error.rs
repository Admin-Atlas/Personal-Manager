// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

use std::path::Path;

use serde::{Serialize, Serializer};

/// Machine-branchable classification of a vault-path failure, so the UI can pick the
/// right recovery action (Repair access / rejoin / passphrase prompt) instead of
/// string-matching. Kebab-case on the wire to match the TS union in `src/lib/types.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum VaultFaultCode {
    /// The OS refused access (Windows `ERROR_ACCESS_DENIED` / POSIX `EACCES`) — the
    /// folder is there but this account can't open it. Repairable; never a brick.
    Denied,
    /// The file/folder is gone (deleted folder, unplugged drive, renamed path).
    NotFound,
    /// The folder answers but holds no PM vault (no `vault-meta.json`).
    NoVault,
    /// The passphrase verifier rejected the derived key — the folder and metadata are fine.
    WrongPassphrase,
    /// The verifier accepted the key but the store itself won't open — damaged files.
    Corrupt,
    /// Everything else (transient file locks, disk I/O, parse errors).
    Other,
}

/// A vault failure the UI can branch on AND display verbatim: the classification, a
/// user-voiced operation ("read the vault's settings"), the path involved, and a
/// ready-to-show sentence. This is what `Error::Vault` serializes to the webview —
/// the one structured rejection shape (every other variant stays a bare string).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VaultFault {
    pub code: VaultFaultCode,
    pub op: String,
    pub path: Option<String>,
    pub message: String,
}

impl VaultFault {
    /// Fold any backend error into a fault for the state slots that must always hold one
    /// (boot, status, engage): a `Vault` error passes its fault through untouched; every
    /// other variant becomes `Other` carrying its display string, attributed to `op`.
    pub fn from_error(op: &str, e: &Error) -> Self {
        match e {
            Error::Vault(f) => f.clone(),
            other => VaultFault {
                code: VaultFaultCode::Other,
                op: op.to_string(),
                path: None,
                message: other.to_string(),
            },
        }
    }
}

/// The one way a vault-path `std::io::Error` becomes a structured `Error::Vault`:
/// `.map_err(io_at("read the vault's settings", &path))`. Classifies by `ErrorKind`
/// (PermissionDenied → Denied, NotFound → NotFound, else Other) and composes the
/// fallback sentence — screens with better copy branch on the code instead.
pub fn io_at(op: &'static str, path: &Path) -> impl FnOnce(std::io::Error) -> Error {
    let path = path.to_path_buf();
    move |e: std::io::Error| {
        let (code, clause) = match e.kind() {
            std::io::ErrorKind::PermissionDenied => (
                VaultFaultCode::Denied,
                "the system refused this account access".to_string(),
            ),
            std::io::ErrorKind::NotFound => {
                (VaultFaultCode::NotFound, "it doesn't exist".to_string())
            }
            _ => (VaultFaultCode::Other, e.to_string()),
        };
        let display = path.display();
        Error::Vault(VaultFault {
            code,
            op: op.to_string(),
            path: Some(display.to_string()),
            message: format!("PM couldn't {op} at {display}: {clause}"),
        })
    }
}

/// One error type for the whole backend. Implements `Serialize` so it can be
/// returned straight out of `#[tauri::command]` functions to the frontend.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("keychain error: {0}")]
    Keyring(#[from] keyring::Error),

    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("archive error: {0}")]
    Zip(#[from] zip::result::ZipError),

    /// A classified vault-path failure (see [`VaultFault`]). Serializes as a flat JSON
    /// object so the frontend can branch on `code`; its Display stays the friendly
    /// sentence so a generic `String(e)` still renders cleanly.
    #[error("{}", .0.message)]
    Vault(VaultFault),

    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Whether this error is an OS access-denial — a raw io `PermissionDenied` or an
    /// already-classified Denied fault. The lock watcher uses it to tell a broken-ACL
    /// vault folder (persistent, worth a fault banner) from transient tick noise.
    pub fn is_denied(&self) -> bool {
        match self {
            Error::Io(e) => e.kind() == std::io::ErrorKind::PermissionDenied,
            Error::Vault(f) => f.code == VaultFaultCode::Denied,
            _ => false,
        }
    }
}

impl Serialize for Error {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            // The single structured rejection: {"code","op","path","message"}. Everything
            // else keeps the historical bare-string shape (~200 String(e) call sites).
            Error::Vault(fault) => fault.serialize(serializer),
            other => serializer.serialize_str(&other.to_string()),
        }
    }
}

/// Trim an upstream/provider response body before folding it into an error that
/// reaches the webview or logs. Provider error bodies can be long and carry
/// incidental detail (absolute paths, request echoes); keep just enough to
/// diagnose. No key/token is ever interpolated into these (invariant #5 holds).
pub fn truncate_detail(detail: &str) -> String {
    const MAX: usize = 500;
    let detail = detail.trim();
    if detail.chars().count() <= MAX {
        return detail.to_string();
    }
    let head: String = detail.chars().take(MAX).collect();
    format!("{head}… (truncated)")
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_detail_keeps_short_and_cuts_long() {
        assert_eq!(truncate_detail("  short body  "), "short body");
        let long = "y".repeat(2000);
        let out = truncate_detail(&long);
        assert!(out.ends_with("… (truncated)"));
        assert_eq!(out.chars().filter(|c| *c == 'y').count(), 500);
    }

    fn fault_of(e: Error) -> VaultFault {
        match e {
            Error::Vault(f) => f,
            other => panic!("expected Error::Vault, got: {other}"),
        }
    }

    #[test]
    fn io_at_classifies_by_error_kind() {
        let path = Path::new("C:/shared/vault-meta.json");
        let denied = fault_of(io_at("read the vault's settings", path)(
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        ));
        assert_eq!(denied.code, VaultFaultCode::Denied);
        assert!(denied.message.contains("read the vault's settings"));
        assert!(denied.message.contains("vault-meta.json"));

        let missing = fault_of(io_at("read the vault's settings", path)(
            std::io::Error::from(std::io::ErrorKind::NotFound),
        ));
        assert_eq!(missing.code, VaultFaultCode::NotFound);

        let other = fault_of(io_at("read the vault's settings", path)(
            std::io::Error::other("disk exploded"),
        ));
        assert_eq!(other.code, VaultFaultCode::Other);
        assert!(other.message.contains("disk exploded"));
    }

    #[test]
    fn vault_error_serializes_as_flat_object_others_as_strings() {
        let fault = VaultFault {
            code: VaultFaultCode::Denied,
            op: "read the vault's settings".into(),
            path: Some("C:/shared".into()),
            message: "PM couldn't read the vault's settings".into(),
        };
        let v = serde_json::to_value(Error::Vault(fault)).unwrap();
        assert_eq!(v["code"], "denied");
        assert_eq!(v["path"], "C:/shared");
        assert!(v["message"].as_str().unwrap().contains("couldn't"));
        // Every other variant keeps the historical bare-string rejection shape.
        let s = serde_json::to_value(Error::Other("plain".into())).unwrap();
        assert_eq!(s, serde_json::json!("plain"));
    }

    #[test]
    fn from_error_passes_faults_through_and_folds_the_rest() {
        let fault = VaultFault {
            code: VaultFaultCode::WrongPassphrase,
            op: "unlock the vault".into(),
            path: None,
            message: "That passphrase doesn't match this vault.".into(),
        };
        let through = VaultFault::from_error("open the vault", &Error::Vault(fault.clone()));
        assert_eq!(through, fault);
        let folded = VaultFault::from_error("open the vault", &Error::Other("boom".into()));
        assert_eq!(folded.code, VaultFaultCode::Other);
        assert_eq!(folded.op, "open the vault");
        assert_eq!(folded.message, "boom");
    }
}
