// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

use serde::{Serialize, Serializer};

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

    #[error("{0}")]
    Other(String),
}

impl Serialize for Error {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
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
}
