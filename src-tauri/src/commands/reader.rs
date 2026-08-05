// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Read-only views onto already-indexed state: document bodies, chunk spans, images, and
//! the guarded paths that open a source outside PM.

use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use tauri::{AppHandle, State};

use crate::error::{Error, Result};
use crate::ingest;
use crate::{pathguard, vault, AppState};

/// Open a URL in the system browser, but ONLY if it's http/https — never a `file:`, app, or custom
/// scheme, so a stray or injected href can't launch a local handler (the inputs are app constants and
/// Drive-supplied links, treated as untrusted — rule #6).
fn open_external_url(url: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|_| Error::Other("That doesn't look like a valid link.".into()))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(Error::Other("Only http(s) links can be opened.".into()));
    }
    open::that(parsed.as_str()).map_err(|e| Error::Other(format!("Couldn't open the link: {e}")))
}

/// Open an arbitrary http(s) URL in the system browser. The webview can't open `target="_blank"`
/// links itself (no shell/opener plugin), so the frontend's app-wide link handler routes them here.
#[tauri::command]
pub fn open_url(url: String) -> Result<()> {
    open_external_url(&url)
}

// --- Document reader (Documents tab): read-only views onto already-indexed state ---
//
// The reader renders a document's on-disk body and, for power users, paints the chunk boundaries the
// splitter placed. These commands are the first consumers of the write-only `chunks.start_offset`/
// `end_offset` byte columns. They read and decrypt through the same `MarkdownCipher` the ingest path
// uses, so what the reader shows is byte-identical to what was chunked. Nothing here mutates the store.

/// A document's chunk span — one row of the boundary overlay, and the first reader of the offset columns.
/// Leaves (`kind = "leaf"`) are the embedded units; `parent_id` groups sibling leaves under their parent.
/// Offsets are BYTE offsets into the document body (see [`read_document_body`]); they are `None` for chunk
/// kinds that predate the offset columns (e.g. chat turns).
#[derive(Serialize)]
pub struct ChunkSpan {
    pub id: i64,
    pub ordinal: i64,
    pub parent_id: Option<i64>,
    pub kind: String,
    pub start_offset: Option<i64>,
    pub end_offset: Option<i64>,
}

/// A decrypted image handed to the webview as base64 + mime (for a `data:` URL). The asset protocol is
/// off and an opt-in saved original follows the vault cipher (possibly ciphertext), so image bytes come
/// back through a command rather than a file URL — the same base64 hop `transcribe_audio` uses.
#[derive(Serialize)]
pub struct ImageData {
    pub base64: String,
    pub mime: String,
}

/// The text the reader renders: a locally-stored document's on-disk Markdown **body** (front-matter
/// stripped), or an index-only pointer's offline `stored_summary` (its body is not held locally). The
/// body is returned byte-for-byte as `parse_frontmatter` yields it — the exact string the splitter
/// chunked — so the overlay's stored byte offsets map onto it without drift. Do NOT normalize newlines.
#[tauri::command]
pub fn read_document_body(state: State<'_, AppState>, doc_id: i64) -> Result<String> {
    let (source_type, vault_path, stored_summary): (String, String, Option<String>) = {
        let conn = state.conn()?;
        conn.query_row(
            "SELECT source_type, vault_path, stored_summary FROM documents WHERE id = ?1",
            params![doc_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?
    };
    if source_type == ingest::SOURCE_TYPE_INDEX_ONLY {
        // No local body — the reader shows the offline summary alongside an "Open source" affordance.
        return Ok(stored_summary.unwrap_or_default());
    }
    let (vault, cipher) = state.markdown_io()?;
    let raw = cipher.read(&vault.join(&vault_path))?;
    let (_fields, body) = ingest::parse_frontmatter(&raw)
        .ok_or_else(|| Error::Other("this document's vault file is missing front-matter".into()))?;
    Ok(body.to_string())
}

/// The chunk spans for a document, ordered by `ordinal` — the boundary overlay's data. Includes both
/// leaves and their parents (the frontend uses leaves for spans and `parent_id` for the grouping shade).
#[tauri::command]
pub fn document_chunk_spans(state: State<'_, AppState>, doc_id: i64) -> Result<Vec<ChunkSpan>> {
    let conn = state.conn()?;
    let mut stmt = conn.prepare(
        "SELECT id, ordinal, parent_id, kind, start_offset, end_offset \
         FROM chunks WHERE document_id = ?1 ORDER BY ordinal",
    )?;
    let rows = stmt
        .query_map(params![doc_id], |r| {
            Ok(ChunkSpan {
                id: r.get(0)?,
                ordinal: r.get(1)?,
                parent_id: r.get(2)?,
                kind: r.get(3)?,
                start_offset: r.get(4)?,
                end_offset: r.get(5)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// One place a document's file lives, in the shape the UI renders (#710/#711).
///
/// A flattened `locations::Location` rather than the struct itself: the frontend's own
/// `sourceLabel.ts` already decodes a `source_id` into the words a person would use, and returning
/// the raw id lets one labeller serve documents and locations alike instead of a second copy of the
/// namespace rules living in Rust and drifting from it.
#[derive(Serialize)]
pub struct DocumentPlace {
    pub source_id: String,
    /// `'ok' | 'unreachable' | 'source_missing'` — for THIS place, not the document. A document with
    /// two places is routinely reachable at one and not the other, which is the whole point of
    /// showing them separately.
    pub state: String,
    pub external_ref: Option<String>,
    pub source_modified_at: Option<String>,
    pub source_parent_folder_name: Option<String>,
    /// The folders above THIS place, root-most first (#736) — the breadcrumb, per-place because that
    /// is the level at which two copies of one file actually differ. `null` when PM has not resolved
    /// it; an empty array means it sits at the top of its corpus.
    pub source_folder_path: Option<Vec<String>>,
    /// True for the place whose id is the document's permanent identity anchor. Shown as "the
    /// original" ordering only — never as "the good copy", because it isn't one.
    pub anchor: bool,
}

/// Every place one document's file lives, anchor first then oldest-first.
///
/// Empty for a vault document, a chat or a photo — none of which a connector found — and the UI
/// renders nothing rather than an empty list, because "this has no locations" is a statement about
/// PM's plumbing that means nothing to the person reading it.
#[tauri::command]
pub fn document_locations(state: State<'_, AppState>, doc_id: i64) -> Result<Vec<DocumentPlace>> {
    let conn = state.conn()?;
    Ok(crate::locations::list(&conn, doc_id)?
        .into_iter()
        .map(|l| DocumentPlace {
            source_id: l.source_id,
            state: l.state.as_str().to_string(),
            external_ref: l.external_ref,
            source_modified_at: l.source_modified_at,
            source_parent_folder_name: l.source_parent_folder_name,
            source_folder_path: l.source_folder_path,
            anchor: l.anchor,
        })
        .collect())
}

/// The original image for a `photo` document, as base64 + mime, for the reader to display. Prefers the
/// encrypted copy in the vault when the user opted to save one; otherwise falls back to the original
/// file where PM referenced it on disk (photos are referenced-in-place by default — no vault copy). Only
/// `None` when neither is available — no saved copy and the original has moved/been deleted (e.g. a
/// screenshot in a temp folder that was since cleaned up) — in which case the reader shows the OCR body.
#[tauri::command]
pub fn read_document_image(state: State<'_, AppState>, doc_id: i64) -> Result<Option<ImageData>> {
    use base64::Engine;
    let row: Option<(Option<String>, Option<String>, i64)> = {
        let conn = state.conn()?;
        conn.query_row(
            "SELECT vault_path, source_path, saved_to_vault FROM photos WHERE document_id = ?1",
            params![doc_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
    };
    let Some((vault_path, source_path, saved)) = row else {
        return Ok(None);
    };

    // Preferred: the encrypted vault copy the user chose to keep.
    if saved == 1 {
        if let Some(rel) = vault_path {
            let (vault, cipher) = state.markdown_io()?;
            // Degrade, don't fail: a copy that won't decrypt (stranded under a previous passphrase
            // by a pre-v3.19.2 re-key, or simply missing) must fall through to the original and the
            // OCR body — the same outcome as never having saved one. Erroring here instead took the
            // whole reader down over an image, which is the one thing this row is not worth.
            match cipher.read_bytes(&vault.join(&rel)) {
                Ok(bytes) => {
                    let mime = image_mime(&vault::MarkdownCipher::logical_name(&rel));
                    return Ok(Some(ImageData {
                        base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
                        mime,
                    }));
                }
                Err(e) => {
                    eprintln!("photo {doc_id}: saved vault copy at {rel} is unreadable ({e}); falling back to the original");
                }
            }
        }
    }

    // Fallback: read the original from the path PM recorded at import. It's the user's own file, read
    // straight from disk (never encrypted — the vault copy is the only encrypted one); a missing/moved
    // original falls through to `None` and the reader's OCR body — and so does a path that no longer
    // resolves to a photo (see `photo_original`).
    if let Some(path) = source_path {
        if let Some((p, mime)) = photo_original(&path) {
            if let Ok(bytes) = std::fs::read(&p) {
                return Ok(Some(ImageData {
                    base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
                    mime,
                }));
            }
        }
    }
    Ok(None)
}

/// Validate the stored original path before [`read_document_image`] turns it into bytes in the
/// webview — the one command that does. PURE (no `State`, no `AppHandle`, no DB) so the decision is
/// unit-testable on its own, the repo idiom `classify_source_ref` below already follows. Returns the
/// canonical path plus its MIME, or `None` for anything that isn't a photo original any more.
///
/// Two guards, both re-using machinery that already exists:
///
/// 1. [`pathguard::sanitize_source`] — the SAME function `ingest_paths` applied to this exact string
///    when it was written. Absolute, no NUL, and canonicalizes (resolving `..`, symlink, junction
///    and case), failing closed if the original has moved or gone. It subsumes the old `is_file()`.
///
/// 2. An extension gate on the **canonical** path against [`ingest::PHOTO_EXTS`]. Canonical is the
///    load-bearing word: a symlink `shot.png -> secret.key` canonicalizes to `secret.key` and is
///    refused, which is what closes the post-ingest swap. Without this half the guard would be
///    justified only by its own comment — an absolute, existing `~/.ssh/id_rsa` passes step 1 fine.
///    A `photos` row is only ever created for a `PHOTO_EXTS` file, so a resolved path naming
///    anything else is a planted front-matter row or a swapped original, not a photo.
///
/// [`pathguard::is_allowed`] is deliberately NOT used here, and must not be "harmonised" onto later:
/// it allowlists the app data dir plus the tracked local-folder roots, but photo originals are
/// referenced-in-place from wherever the user dragged them — Desktop, Downloads, `%TEMP%` (this
/// command's own doc comment names the temp-folder screenshot as the expected case). Applying that
/// allowlist would drop most users' photos to the OCR body: a functional regression, not a fix.
/// `reader_tests::photo_original_resolves_a_photo_outside_every_tracked_root` pins that.
///
/// Honest scope, matching `pathguard`'s own: this bounds a planted path STRING and a symlink swap.
/// It cannot bound an attacker who already has arbitrary local write — a hard link is a real
/// directory entry, so `shot.png` hard-linked to a secret canonicalizes to `shot.png` and passes;
/// but that attacker could equally have copied the secret to `shot.png` outright, which no guard
/// here could see. The containment story for local write is the encrypted store, not this function.
fn photo_original(stored: &str) -> Option<(std::path::PathBuf, String)> {
    let p = pathguard::sanitize_source(stored).ok()?;
    // Kept from the pre-guard code: canonicalization proves the target exists, not that it is a
    // regular file, and a directory named `album.png` would otherwise clear the extension gate.
    // Safe to ask on the canonical path — there is no link left to follow at this point.
    if !p.is_file() {
        return None;
    }
    // Take the extension off the returned `PathBuf`, never off a display string: a canonical
    // Windows path carries the `\\?\` verbatim prefix. Lower-cased to match `ingest::extension`,
    // so a `.PNG` original keeps working.
    let ext = p.extension()?.to_string_lossy().to_ascii_lowercase();
    if !ingest::PHOTO_EXTS.contains(&ext.as_str()) {
        return None;
    }
    let mime = mime_for_ext(&ext);
    Some((p, mime))
}

/// Best-effort image MIME from a filename extension, for the reader's `data:` URL.
fn image_mime(name: &str) -> String {
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    mime_for_ext(&ext)
}

/// The extension → MIME table, taking an already-lower-cased extension with no dot. Split out of
/// [`image_mime`] so a caller that has already parsed the extension off a `PathBuf` reaches the same
/// table without round-tripping through a display string.
fn mime_for_ext(ext: &str) -> String {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "tif" | "tiff" => "image/tiff",
        "heic" | "heif" => "image/heic",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Whether a stored `external_ref` is a web link (opened in the browser) or a local path (revealed in the
/// OS file manager). Split out as a pure function so the dispatch is unit-testable without a DB/State.
#[derive(Debug, PartialEq, Eq)]
enum SourceRefKind {
    Web,
    LocalPath,
}

fn classify_source_ref(external_ref: &str) -> SourceRefKind {
    if external_ref.starts_with("http://") || external_ref.starts_with("https://") {
        SourceRefKind::Web
    } else {
        SourceRefKind::LocalPath
    }
}

/// Reveal a local file in the OS file manager, SELECTING it (not opening it — that would launch the
/// file's default app). The path is validated to exist and passed as a single non-shell argument, so a
/// stored path can't inject further arguments. Local-only; the http(s) guard covers web links elsewhere.
fn reveal_in_file_manager(path: &str) -> Result<()> {
    let p = std::path::Path::new(path);
    if !p.exists() {
        return Err(Error::Other(
            "This file is no longer at its saved location.".into(),
        ));
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", p.display()))
            .spawn()
            .map_err(|e| Error::Other(format!("Couldn't open the file manager: {e}")))?;
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(p)
            .spawn()
            .map_err(|e| Error::Other(format!("Couldn't open Finder: {e}")))?;
        return Ok(());
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // No portable "select the file" on Linux; open the containing folder instead.
        let dir = p.parent().unwrap_or(p);
        open::that(dir)
            .map_err(|e| Error::Other(format!("Couldn't open the file manager: {e}")))?;
        return Ok(());
    }
    #[allow(unreachable_code)]
    Err(Error::Other(
        "Revealing files isn't supported on this platform.".into(),
    ))
}

/// Open a document's source. An index-only web link (Drive/OneDrive `webViewLink`) opens in the system
/// browser through the http(s) guard; a local-folder file path is revealed-and-selected in the OS file
/// manager. Web links never reach the file-manager reveal and local paths never reach `open::that`.
/// Supersedes the old `open_external_ref` (which was http(s)-only).
#[tauri::command]
pub fn open_source(app: AppHandle, state: State<'_, AppState>, doc_id: i64) -> Result<()> {
    let external_ref: Option<String> = {
        let conn = state.conn()?;
        conn.query_row(
            "SELECT external_ref FROM documents WHERE id = ?1",
            params![doc_id],
            |r| r.get(0),
        )?
    };
    let refr = external_ref.ok_or_else(|| Error::Other("This item has no source link.".into()))?;
    match classify_source_ref(&refr) {
        SourceRefKind::Web => open_external_url(&refr),
        SourceRefKind::LocalPath => {
            // L-5 defense-in-depth: this path comes from the document row (populated by the
            // now-guarded ingest / local-folder pipeline), but keep the reveal inside the folders
            // PM tracks (or its own data dir) so it can never hand the OS shell an out-of-bounds
            // location. Fails closed if the source has moved out of every tracked root.
            let conn = state.conn()?;
            pathguard::is_allowed(&app, &conn, &refr)?;
            drop(conn);
            reveal_in_file_manager(&refr)
        }
    }
}

#[cfg(test)]
mod reader_tests {
    use super::{classify_source_ref, photo_original, SourceRefKind};

    /// Write `name` under `dir` and hand back the string `photos.source_path` would hold.
    fn planted(dir: &std::path::Path, name: &str, bytes: &[u8]) -> String {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn photo_original_accepts_every_ingestable_photo_extension() {
        let dir = tempfile::tempdir().unwrap();
        for (name, want_mime) in [
            ("shot.png", "image/png"),
            ("scan.jpeg", "image/jpeg"),
            ("scan.jpg", "image/jpeg"),
            ("art.webp", "image/webp"),
            ("phone.heic", "image/heic"),
            // Upper-case must keep working — `ingest::extension` lower-cases, so the read side does too.
            ("LOUD.PNG", "image/png"),
        ] {
            let stored = planted(dir.path(), name, b"not-really-an-image");
            let (got, mime) = photo_original(&stored)
                .unwrap_or_else(|| panic!("{name} should resolve as a photo original"));
            assert_eq!(got, std::fs::canonicalize(dir.path().join(name)).unwrap());
            assert_eq!(mime, want_mime, "{name}");
        }
    }

    #[test]
    fn photo_original_refuses_an_existing_non_photo() {
        // The planted-front-matter case: a `photos` row whose `source_path` was rewritten (via a
        // hand-edited vault file or an adopted backup) to name a secret. Every one of these EXISTS
        // and is absolute, so `sanitize_source` alone would pass it — the extension gate is what
        // stops the command handing the file's bytes to the webview.
        let dir = tempfile::tempdir().unwrap();
        for name in ["id_rsa", "notes.txt", "key.pem", "archive.tar.gz", "photo."] {
            let stored = planted(dir.path(), name, b"secret");
            assert!(
                photo_original(&stored).is_none(),
                "{name} must not resolve as a photo original"
            );
        }
        // A directory named like a photo is not a file to read either.
        let d = dir.path().join("album.png");
        std::fs::create_dir(&d).unwrap();
        assert!(photo_original(&d.to_string_lossy()).is_none());
    }

    #[test]
    fn photo_original_fails_closed_on_malformed_and_missing_paths() {
        // Inherited from `pathguard::sanitize_source`: absolute, no NUL, must canonicalize.
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("gone.png");
        assert!(photo_original(&missing.to_string_lossy()).is_none());
        assert!(photo_original("relative/shot.png").is_none());
        assert!(photo_original("").is_none());
        assert!(photo_original("has\0nul.png").is_none());
    }

    /// The post-ingest swap: `p.is_file()` used `metadata` (which follows links) and `std::fs::read`
    /// follows them too, so replacing the original with a link to a secret used to serve the secret.
    /// The gate runs on the CANONICAL path, so the link resolves to `secret.key` and is refused —
    /// the name it was given is irrelevant. Unix-only because it needs an unprivileged symlink.
    #[cfg(unix)]
    #[test]
    fn photo_original_refuses_a_symlink_that_resolves_to_a_non_photo() {
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("secret.key");
        std::fs::write(&secret, b"-----BEGIN PRIVATE KEY-----").unwrap();
        let link = dir.path().join("shot.png");
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        // The stored string still says `.png`; only canonicalization sees through it.
        assert!(link.to_string_lossy().ends_with("shot.png"));
        assert!(photo_original(&link.to_string_lossy()).is_none());

        // Counterpart: a symlink that really does resolve to a photo still works, so the guard
        // bounds the TARGET's type rather than banning links outright.
        let real = dir.path().join("real.png");
        std::fs::write(&real, b"png").unwrap();
        let ok_link = dir.path().join("alias.png");
        std::os::unix::fs::symlink(&real, &ok_link).unwrap();
        let (got, mime) = photo_original(&ok_link.to_string_lossy()).unwrap();
        assert_eq!(got, std::fs::canonicalize(&real).unwrap());
        assert_eq!(mime, "image/png");
    }

    /// REGRESSION GUARD — do not "harmonise" `photo_original` onto `pathguard::is_allowed`.
    ///
    /// This tempdir is neither the app data dir nor a tracked local-folder root, which is exactly
    /// where photo originals normally live: referenced in place from Desktop, Downloads or `%TEMP%`
    /// (`read_document_image`'s own doc comment names the temp-folder screenshot). `is_allowed`
    /// allowlists only the data dir + `localfolder::tracked_roots`, so adopting `open_source`'s line
    /// here would refuse the majority of real photos and silently drop the reader to the OCR body.
    /// If this test ever goes red, the fix is to revert that change, not to relax this assertion.
    #[test]
    fn photo_original_resolves_a_photo_outside_every_tracked_root() {
        let dir = tempfile::tempdir().unwrap();
        let stored = planted(dir.path(), "screenshot.png", b"png-bytes");
        let (got, mime) = photo_original(&stored)
            .expect("a referenced-in-place original outside every tracked root must still resolve");
        assert_eq!(
            got,
            std::fs::canonicalize(dir.path().join("screenshot.png")).unwrap()
        );
        assert_eq!(mime, "image/png");
    }

    #[test]
    fn classify_source_ref_splits_web_from_local() {
        assert_eq!(
            classify_source_ref("https://drive.google.com/file/d/abc/view"),
            SourceRefKind::Web
        );
        assert_eq!(
            classify_source_ref("http://example.com/x"),
            SourceRefKind::Web
        );
        // A Windows drive path must NOT be mistaken for a URL scheme ("C:" is not http/https).
        assert_eq!(
            classify_source_ref("C:\\Users\\me\\notes\\report.md"),
            SourceRefKind::LocalPath
        );
        assert_eq!(
            classify_source_ref("/home/me/notes/report.md"),
            SourceRefKind::LocalPath
        );
        // A non-web scheme is treated as a local path (revealed), never handed to the browser opener.
        assert_eq!(
            classify_source_ref("file:///home/me/x"),
            SourceRefKind::LocalPath
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_external_url_allows_only_http_schemes() {
        // Rejected before any launch — a stray/injected href can't open a local handler.
        assert!(open_external_url("file:///etc/passwd").is_err());
        assert!(open_external_url("javascript:alert(1)").is_err());
        assert!(open_external_url("not a url").is_err());
        // The http/https success path is deliberately not exercised (it would launch a browser).
    }
}
