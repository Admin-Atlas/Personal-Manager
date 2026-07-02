// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A tiny length-prefixed archive of named byte streams — the innermost layer of a
//! `.pmbackup`, sitting under zstd + the STREAM cipher. Hand-rolled rather than pulling
//! in `tar`: the need is small (a flat allow-list of files), the format is a pure codec
//! over `impl Read`/`impl Write` (so it unit-tests in memory), and we skip tar's unpack
//! path-traversal surface — every entry path is validated here on the way out AND in.
//!
//! Layout: `"PMBNDL1" | entry_count(u32 LE) | [ path_len(u16 LE) | path | content_len(u64 LE) | content ]*`.
//! Paths are relative, `/`-separated, and validated (`..`, absolute, backslash, drive
//! letters, and embedded NULs are rejected) so a hostile archive can't escape staging.

use std::io::{self, Read, Write};

use crate::error::{Error, Result};

const BUNDLE_MAGIC: &[u8; 7] = b"PMBNDL1";
/// Upper bound on a stored path (also the u16 length-prefix ceiling in practice).
const MAX_PATH_LEN: usize = 4096;

/// Write the bundle header. `entry_count` must equal the number of `write_entry` calls
/// that follow (the reader loops exactly that many times).
pub fn write_header<W: Write>(w: &mut W, entry_count: u32) -> Result<()> {
    w.write_all(BUNDLE_MAGIC)?;
    w.write_all(&entry_count.to_le_bytes())?;
    Ok(())
}

/// Stream one entry: its validated path, then exactly `len` bytes drained from
/// `content`. Errors if `content` yields a different number of bytes than `len` (a
/// file that changed size mid-backup), so the archive can never silently desync.
pub fn write_entry<W: Write, R: Read>(
    w: &mut W,
    path: &str,
    len: u64,
    content: &mut R,
) -> Result<()> {
    validate_path(path)?;
    let pb = path.as_bytes();
    if pb.len() > MAX_PATH_LEN {
        return Err(Error::Other("backup entry path is too long".into()));
    }
    w.write_all(&(pb.len() as u16).to_le_bytes())?;
    w.write_all(pb)?;
    w.write_all(&len.to_le_bytes())?;
    let copied = io::copy(content, w)?;
    if copied != len {
        return Err(Error::Other(format!(
            "backup entry '{path}' changed size while being read ({copied} vs {len} bytes)"
        )));
    }
    Ok(())
}

/// Read a bundle, invoking `on_entry(path, len, content)` for each entry in order. The
/// `content` reader is capped at exactly `len` bytes; anything the callback leaves
/// unread is drained so the stream stays aligned for the next entry. Every path is
/// re-validated before the callback sees it.
pub fn read_bundle<R: Read>(
    mut r: R,
    mut on_entry: impl FnMut(&str, u64, &mut dyn Read) -> Result<()>,
) -> Result<()> {
    let mut magic = [0u8; 7];
    // Preserve the cause: on a wrong passphrase the first read here IS the AEAD layer's
    // decrypt failure, so surfacing it keeps the "wrong passphrase" hint (rather than
    // masking it as a generic "missing header").
    r.read_exact(&mut magic)
        .map_err(|e| Error::Other(format!("could not read the backup contents: {e}")))?;
    if &magic != BUNDLE_MAGIC {
        return Err(Error::Other("corrupt backup (bad bundle signature)".into()));
    }
    let mut cnt = [0u8; 4];
    r.read_exact(&mut cnt)?;
    let count = u32::from_le_bytes(cnt);

    for _ in 0..count {
        let mut pl = [0u8; 2];
        r.read_exact(&mut pl)?;
        let plen = u16::from_le_bytes(pl) as usize;
        if plen == 0 || plen > MAX_PATH_LEN {
            return Err(Error::Other(
                "corrupt backup (bad entry path length)".into(),
            ));
        }
        let mut pbuf = vec![0u8; plen];
        r.read_exact(&mut pbuf)?;
        let path = String::from_utf8(pbuf)
            .map_err(|_| Error::Other("corrupt backup (non-UTF-8 entry path)".into()))?;
        validate_path(&path)?;

        let mut lb = [0u8; 8];
        r.read_exact(&mut lb)?;
        let len = u64::from_le_bytes(lb);

        let mut limited = (&mut r).take(len);
        on_entry(&path, len, &mut limited)?;
        // Drain anything the callback didn't consume so the next entry starts aligned.
        io::copy(&mut limited, &mut io::sink())?;
    }
    Ok(())
}

/// Reject any path that could escape the staging directory or isn't a plain relative
/// forward-slash path. Pure — the whole point is that it's exhaustively testable.
pub fn validate_path(path: &str) -> Result<()> {
    if path.is_empty() {
        return Err(Error::Other("corrupt backup (empty entry path)".into()));
    }
    if path.len() > MAX_PATH_LEN {
        return Err(Error::Other("backup entry path is too long".into()));
    }
    if path.starts_with('/') {
        return Err(Error::Other(format!(
            "unsafe backup path (absolute): {path}"
        )));
    }
    if path.contains('\\') {
        return Err(Error::Other(format!(
            "unsafe backup path (backslash): {path}"
        )));
    }
    if path.contains(':') {
        return Err(Error::Other(format!(
            "unsafe backup path (drive/colon): {path}"
        )));
    }
    if path.contains('\0') {
        return Err(Error::Other("unsafe backup path (embedded NUL)".into()));
    }
    for comp in path.split('/') {
        if comp.is_empty() || comp == "." || comp == ".." {
            return Err(Error::Other(format!(
                "unsafe backup path (traversal): {path}"
            )));
        }
        // Windows strips a trailing dot/space and resolves reserved device names (CON,
        // NUL, COM1…) even inside a subdirectory — either would make a restored file land
        // somewhere other than a plain file under staging. Refuse them on every OS so the
        // format is validated identically everywhere.
        if comp.ends_with('.') || comp.ends_with(' ') {
            return Err(Error::Other(format!(
                "unsafe backup path (trailing dot or space): {path}"
            )));
        }
        if is_windows_reserved(comp) {
            return Err(Error::Other(format!(
                "unsafe backup path (reserved device name): {path}"
            )));
        }
    }
    Ok(())
}

/// Whether a path component is a Windows reserved device name (CON/PRN/AUX/NUL,
/// COM1–COM9, LPT1–LPT9). The reservation applies to the stem before the first dot,
/// case-insensitively (`NUL.txt` is still the null device).
fn is_windows_reserved(comp: &str) -> bool {
    let stem = comp.split('.').next().unwrap_or(comp);
    let upper = stem.to_ascii_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    let bytes = upper.as_bytes();
    (upper.starts_with("COM") || upper.starts_with("LPT"))
        && bytes.len() == 4
        && bytes[3].is_ascii_digit()
        && bytes[3] != b'0'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_multiple_entries() {
        let mut buf = Vec::new();
        write_header(&mut buf, 2).unwrap();
        write_entry(&mut buf, "vault/a.md", 5, &mut &b"hello"[..]).unwrap();
        write_entry(&mut buf, "pm.sqlite", 3, &mut &b"DB!"[..]).unwrap();

        let mut got: Vec<(String, Vec<u8>)> = Vec::new();
        read_bundle(&buf[..], |path, _len, content| {
            let mut v = Vec::new();
            content.read_to_end(&mut v)?;
            got.push((path.to_string(), v));
            Ok(())
        })
        .unwrap();

        assert_eq!(got.len(), 2);
        assert_eq!(got[0], ("vault/a.md".to_string(), b"hello".to_vec()));
        assert_eq!(got[1], ("pm.sqlite".to_string(), b"DB!".to_vec()));
    }

    #[test]
    fn drains_unconsumed_entry_bytes_and_stays_aligned() {
        let mut buf = Vec::new();
        write_header(&mut buf, 2).unwrap();
        write_entry(&mut buf, "big", 10, &mut &b"0123456789"[..]).unwrap();
        write_entry(&mut buf, "next", 2, &mut &b"ok"[..]).unwrap();

        let mut names = Vec::new();
        read_bundle(&buf[..], |path, _len, _content| {
            // Deliberately read NOTHING from the first entry.
            names.push(path.to_string());
            Ok(())
        })
        .unwrap();
        assert_eq!(names, vec!["big".to_string(), "next".to_string()]);
    }

    #[test]
    fn rejects_unsafe_paths() {
        for bad in [
            "",
            "/etc/passwd",
            "..",
            "a/../b",
            "../escape",
            "a/./b",
            r"a\b",
            "C:/x",
            "a\0b",
            "vault/NUL",
            "vault/con",
            "vault/COM1",
            "vault/LPT9.txt",
            "vault/note.md.",
            "vault/trailing ",
        ] {
            assert!(validate_path(bad).is_err(), "should reject {bad:?}");
        }
        for ok in [
            "vault/a.md",
            "pm.sqlite",
            "vault/sub/dir/note.md.pmenc",
            "vault/COM0",
            "vault/comfort.md",
            "vault/lpt.md",
        ] {
            assert!(validate_path(ok).is_ok(), "should accept {ok:?}");
        }
    }

    #[test]
    fn a_hostile_traversal_entry_is_refused_on_read() {
        // Hand-craft a bundle whose stored path tries to escape.
        let mut buf = Vec::new();
        buf.extend_from_slice(BUNDLE_MAGIC);
        buf.extend_from_slice(&1u32.to_le_bytes());
        let evil = b"../escape";
        buf.extend_from_slice(&(evil.len() as u16).to_le_bytes());
        buf.extend_from_slice(evil);
        buf.extend_from_slice(&0u64.to_le_bytes());
        let err = read_bundle(&buf[..], |_, _, _| Ok(()));
        assert!(err.is_err());
    }
}
