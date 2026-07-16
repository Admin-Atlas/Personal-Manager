// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server-side validation of webview-supplied filesystem paths (L-5, defense-in-depth).
//!
//! PM bundles the dialog plugin but *not* the fs plugin, so the webview has no direct filesystem
//! access and every file operation runs in Rust `std::fs` on a path string the webview passed into
//! a `#[tauri::command]`. The dialog picker runs in the webview and hands its result back through
//! `invoke(...)` as an ordinary argument — so a compromised webview could call any path-accepting
//! command directly with an *arbitrary* path, never touching the picker. These guards reject
//! malformed / non-existent / out-of-bounds paths server-side, before the operation runs.
//!
//! Two shapes, matched to how the command uses the path:
//! - [`sanitize_source`] / [`sanitize_destination`] validate a single path by *shape and existence*
//!   (fail-closed) without an allowlist — for the commands where the user legitimately points at a
//!   NEW / arbitrary location (add a folder, choose a backup destination, pick a vault). This is a
//!   partial guard by nature: it rejects fabricated, relative, NUL, traversal and reserved-name
//!   paths, but a well-formed absolute path to a real location still passes (there is no allowlist
//!   to reject it against). The two highest-severity sinks (subprocess spawn, decrypted export)
//!   are instead driven through a backend-owned dialog so the webview never supplies their path.
//! - [`is_allowed`] additionally requires the path to sit inside the user's tracked folders or the
//!   app's own data dir — for commands that must stay within already-known locations (reveal).
//!
//! The containment primitive ([`within_root`]) is shared with `ingest::symlink_escapes_root`, so
//! there is one place that decides "is P inside allowed root R".

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use tauri::AppHandle;

use crate::backup::bundle::is_windows_reserved;
use crate::error::{Error, Result};
use crate::{localfolder, paths};

fn reject(msg: &str) -> Error {
    Error::Other(msg.to_string())
}

/// Whether `candidate` sits at or below `root`. PURE: both paths MUST already be canonicalized by
/// the caller. `Path::starts_with` is component-wise, so mixing a canonical root with a raw
/// candidate (or vice-versa) silently misjudges containment — on Windows `std::fs::canonicalize`
/// returns the `\\?\` verbatim form, and NTFS is case-insensitive while `starts_with` is not, so
/// only canonicalizing *both* sides makes the comparison correct. Same contract as
/// `ingest::symlink_escapes_root`, which now calls this.
pub fn within_root(candidate: &Path, root: &Path) -> bool {
    candidate.starts_with(root)
}

/// Whether `candidate` sits within ANY of `roots` (all canonical). Pure fold over [`within_root`].
pub fn within_any_root(candidate: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|r| within_root(candidate, r))
}

/// Validate a webview-supplied path that names an EXISTING location to read or open (ingest inputs,
/// a folder to track, a vault folder to load, a backup file to restore). Requires an absolute path
/// with no NUL that canonicalizes successfully — which resolves any `..`, symlink, junction and
/// case difference and **fails closed** if the target is missing or unreachable. Returns the
/// canonical path. Does NOT allowlist: pointing at a brand-new file or folder is the whole point of
/// these commands, so there is no set of roots to reject against here (see [`is_allowed`]).
pub fn sanitize_source(path: &str) -> Result<PathBuf> {
    if path.is_empty() {
        return Err(reject("No location was given."));
    }
    if path.contains('\0') {
        return Err(reject("That location has an invalid character."));
    }
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err(reject("That location isn't an absolute path."));
    }
    std::fs::canonicalize(p).map_err(|_| reject("That location doesn't exist or can't be reached."))
}

/// Validate a webview-supplied path that names a WRITE destination (a backup/export file or a new
/// vault folder). The final component is a new-or-existing plain name: it must be an ordinary name
/// (no `..`, `.`, root or drive prefix), must not be a Windows reserved device name, and must not
/// end in a dot or space (Windows strips those, landing the write somewhere else). The CONTAINING
/// folder must already exist — canonicalizing it resolves any `..`/symlink/junction/case in the
/// path up to the leaf and fails closed if it doesn't exist, so a compromised webview can neither
/// traverse out of a real folder nor cause an arbitrary new tree to be created. Reserved/trailing
/// rules are enforced on every OS (as `backup::bundle::validate_path` does) so the check behaves
/// identically everywhere. Returns the canonical-parent path re-joined with the validated leaf.
///
/// Honest scope: with no allowlist, a well-formed destination inside a real folder still passes —
/// this bounds the *shape* of the path, not the *choice* of an otherwise-valid location.
pub fn sanitize_destination(path: &str) -> Result<PathBuf> {
    if path.is_empty() {
        return Err(reject("No location was given."));
    }
    if path.contains('\0') {
        return Err(reject("That location has an invalid character."));
    }
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err(reject("That location isn't an absolute path."));
    }
    // `file_name()` is `None` when the path ends in `..`, `.` or a separator — i.e. it is not a
    // plain destination name — which rejects traversal leaves outright.
    let leaf = p
        .file_name()
        .ok_or_else(|| reject("That location isn't a valid destination."))?;
    let leaf_str = leaf.to_string_lossy();
    if leaf_str.ends_with('.') || leaf_str.ends_with(' ') || is_windows_reserved(&leaf_str) {
        return Err(reject("That location uses a reserved or malformed name."));
    }
    let parent = p
        .parent()
        .ok_or_else(|| reject("That location has no containing folder."))?;
    let parent_canon = std::fs::canonicalize(parent)
        .map_err(|_| reject("That folder doesn't exist or can't be reached."))?;
    Ok(parent_canon.join(leaf))
}

/// Validate that a webview-reachable path stays inside a location PM already knows: the app's own
/// data dir or one of the user's tracked folders. Used for the reveal-in-file-manager path, whose
/// value comes from the document row (populated by the now-guarded ingest / local-folder pipeline)
/// — this is the defense-in-depth layer that keeps a reveal from ever pointing outside those roots.
/// Fails closed on any canonicalize error (unmounted root, missing target). Returns the canonical
/// path when allowed.
pub fn is_allowed(app: &AppHandle, conn: &Connection, candidate: &str) -> Result<PathBuf> {
    let canon = sanitize_source(candidate)?;
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(data) = std::fs::canonicalize(paths::data_dir(app)?) {
        roots.push(data);
    }
    for r in localfolder::tracked_roots(conn)? {
        // A currently-unmounted/deleted root simply can't contain anything, so skip it rather than
        // fail the whole check.
        if let Ok(rc) = std::fs::canonicalize(&r.root) {
            roots.push(rc);
        }
    }
    if within_any_root(&canon, &roots) {
        Ok(canon)
    } else {
        Err(reject("That location is outside the folders PM tracks."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // POSIX-style absolute paths keep the containment semantics identical on Windows and Linux
    // (component-wise `starts_with`), so these pure cases run the same everywhere.
    #[test]
    fn within_root_matches_nested_and_the_root_itself() {
        assert!(within_root(
            Path::new("/home/me/docs/a.md"),
            Path::new("/home/me")
        ));
        assert!(within_root(Path::new("/home/me"), Path::new("/home/me")));
    }

    #[test]
    fn within_root_rejects_siblings_and_prefix_lookalikes() {
        // A string-prefix sibling is NOT a path-prefix child — the boundary is per component.
        assert!(!within_root(
            Path::new("/home/me2/x"),
            Path::new("/home/me")
        ));
        assert!(!within_root(
            Path::new("/home/other"),
            Path::new("/home/me")
        ));
    }

    #[test]
    fn within_any_root_is_true_when_any_root_contains_it() {
        let roots = vec![PathBuf::from("/data"), PathBuf::from("/home/me/notes")];
        assert!(within_any_root(Path::new("/home/me/notes/x.md"), &roots));
        assert!(within_any_root(Path::new("/data/db.sqlite"), &roots));
        assert!(!within_any_root(Path::new("/home/me/other/x.md"), &roots));
    }

    #[test]
    fn sanitize_source_accepts_an_existing_absolute_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.md");
        std::fs::write(&file, b"hi").unwrap();
        let got = sanitize_source(file.to_str().unwrap()).unwrap();
        // Canonical form of the same file (may carry the `\\?\` prefix on Windows).
        assert_eq!(got, std::fs::canonicalize(&file).unwrap());
    }

    #[test]
    fn sanitize_source_rejects_missing_relative_and_empty() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.md");
        assert!(sanitize_source(missing.to_str().unwrap()).is_err()); // fail-closed on non-existent
        assert!(sanitize_source("relative/path.md").is_err());
        assert!(sanitize_source("").is_err());
        assert!(sanitize_source("has\0nul").is_err());
    }

    #[test]
    fn sanitize_destination_accepts_a_new_leaf_under_an_existing_folder() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("backup.pmbackup"); // parent exists, leaf is new
        let got = sanitize_destination(dest.to_str().unwrap()).unwrap();
        assert_eq!(
            got,
            std::fs::canonicalize(dir.path())
                .unwrap()
                .join("backup.pmbackup")
        );
    }

    #[test]
    fn sanitize_destination_rejects_missing_parent_relative_and_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let missing_parent = dir.path().join("no-such-dir").join("x.zip");
        assert!(sanitize_destination(missing_parent.to_str().unwrap()).is_err());
        assert!(sanitize_destination("relative.zip").is_err());
        // A `..` leaf has no `file_name`, so it is rejected.
        let traversal = dir.path().join("..");
        assert!(sanitize_destination(traversal.to_str().unwrap()).is_err());
        assert!(sanitize_destination("").is_err());
    }

    #[test]
    fn sanitize_destination_rejects_reserved_and_trailing_names_on_every_os() {
        let dir = tempfile::tempdir().unwrap();
        // Reserved Windows device name as the leaf — refused everywhere for a consistent format.
        assert!(sanitize_destination(dir.path().join("CON").to_str().unwrap()).is_err());
        assert!(sanitize_destination(dir.path().join("LPT9.zip").to_str().unwrap()).is_err());
        // Trailing dot/space would be stripped by Windows, landing the write elsewhere.
        assert!(sanitize_destination(dir.path().join("report.").to_str().unwrap()).is_err());
        assert!(sanitize_destination(dir.path().join("report ").to_str().unwrap()).is_err());
    }

    #[test]
    fn reserved_name_rule_matches_backup_bundle() {
        // The reserved-name decision is shared with the backup archive validator, so the two agree.
        assert!(is_windows_reserved("CON"));
        assert!(is_windows_reserved("nul.txt"));
        assert!(!is_windows_reserved("contacts"));
    }
}
