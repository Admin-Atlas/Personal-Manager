// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Photo / screenshot ingestion (board card #135).
//!
//! Photos are a new ingestion source type that REUSES the document pipeline wholesale: each
//! ingested image becomes a `documents` row with `source_type='photo'` (so the existing
//! split/embed/FTS/vector/retrieval/Map/citation/rebuild machinery works unchanged) PLUS one row in
//! the `photos` satellite table (migration v22) holding the image-specific truth — capture date, GPS,
//! the OCR text, the on-disk hash, and the opt-in vault copy. The vault truth is a synthetic Markdown
//! frontmatter file, so a photo rebuilds from the vault for free and OCR is never re-run on Rebuild.
//!
//! This module owns the photo-specific logic; the ingest orchestration lives in [`crate::ingest`] and
//! the OCR + EXIF extraction is done by the Python sidecar's `analyze_image` (see [`ImageAnalysis`]).
//! OCR is an OPTIONAL on-demand component (rapidocr + pillow-heif) — installable and removable from
//! Settings, exactly like the t-SNE reducer — so a user who declines it still ingests photos with
//! their EXIF metadata chunk (`ocr_text` then stays `None`).

use std::path::Path;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::error::{Error, Result};
use crate::AppState;

/// How a photo entered PM — its capture provenance, the `photos.source_type` enum. Orthogonal to
/// whether the original was copied into the vault (`saved_to_vault`): a screenshot stays a screenshot
/// whether or not its bytes were saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotoSourceType {
    Screenshot,
    CameraRoll,
    DraggedFile,
    VaultCopy,
}

impl PhotoSourceType {
    /// The stored DB value (matches the v22 `photos.source_type` CHECK).
    pub fn as_str(self) -> &'static str {
        match self {
            PhotoSourceType::Screenshot => "screenshot",
            PhotoSourceType::CameraRoll => "camera_roll",
            PhotoSourceType::DraggedFile => "dragged_file",
            PhotoSourceType::VaultCopy => "vault_copy",
        }
    }

    /// Parse a stored value back; anything unrecognised falls back to `DraggedFile` (the neutral
    /// default), so a hand-edited or future value can never fail a rebuild.
    pub fn from_db(s: &str) -> Self {
        match s {
            "screenshot" => PhotoSourceType::Screenshot,
            "camera_roll" => PhotoSourceType::CameraRoll,
            "vault_copy" => PhotoSourceType::VaultCopy,
            _ => PhotoSourceType::DraggedFile,
        }
    }

    /// The human noun used in the title + metadata sentence ("Screenshot captured …").
    pub fn noun(self) -> &'static str {
        match self {
            PhotoSourceType::Screenshot => "Screenshot",
            PhotoSourceType::CameraRoll => "Photo",
            PhotoSourceType::DraggedFile | PhotoSourceType::VaultCopy => "Image",
        }
    }
}

/// The truth a photo carries beyond a plain document — written to the `photos` satellite row and
/// round-tripped through the vault frontmatter so a Rebuild reconstructs it without re-running OCR.
#[derive(Debug, Clone, PartialEq)]
pub struct PhotoRecord {
    pub source_path: Option<String>,
    pub source_type: PhotoSourceType,
    pub capture_date: String,
    pub file_hash: String,
    pub ocr_text: Option<String>,
    pub saved_to_vault: bool,
    pub vault_path: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

/// The heading under which the OCR text lives in the synthetic photo body. Its own section so the
/// splitter makes it independent leaves; [`ocr_text_from_body`] reads it back on rebuild.
const TEXT_HEADING: &str = "## Text";

/// Infer the capture provenance from the filename and whether the image carries camera EXIF. A name
/// like "Screenshot 2026-03-12 at 09.41.png" → Screenshot; a file with real camera EXIF (a date or
/// GPS a screenshot never has) → CameraRoll; otherwise a plain dropped file.
pub fn infer_source_type(path: &Path, has_camera_exif: bool) -> PhotoSourceType {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if name.contains("screenshot") || name.contains("screen shot") || name.contains("screen_shot") {
        PhotoSourceType::Screenshot
    } else if has_camera_exif {
        PhotoSourceType::CameraRoll
    } else {
        PhotoSourceType::DraggedFile
    }
}

/// Resolve a photo's capture date (YYYY-MM-DD) by the spec's fallback ladder: EXIF DateTimeOriginal
/// first, then an 8-digit or `YYYY-MM-DD` run in the filename (how phones/screenshot tools name
/// files), then the ingest timestamp's date as a last resort.
pub fn resolve_capture_date(exif_date: Option<&str>, path: &Path, ingest_ts: &str) -> String {
    if let Some(d) = exif_date.map(str::trim).filter(|d| is_iso_date(d)) {
        return d.to_string();
    }
    if let Some(d) = date_from_filename(path) {
        return d;
    }
    // Ingest ts is ISO ("YYYY-MM-DDT…"); take its date, or today is unknowable so fall back to it raw.
    ingest_ts.get(..10).unwrap_or(ingest_ts).to_string()
}

/// A `YYYY-MM-DD` string, loosely validated (digits + plausible month/day). Good enough to reject the
/// EXIF all-zero placeholder and obvious junk without pulling a date library.
fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[8..10].iter().all(u8::is_ascii_digit)
        && &s[0..4] != "0000"
}

/// Pull a date out of a filename: a literal `YYYY-MM-DD`, or the first run of exactly 8 digits read
/// as `YYYYMMDD` (e.g. "IMG_20260312_094135.jpg"). Returns the normalised `YYYY-MM-DD`, or None.
fn date_from_filename(path: &Path) -> Option<String> {
    let name = path.file_stem()?.to_string_lossy().to_string();
    // 1) explicit YYYY-MM-DD
    let bytes = name.as_bytes();
    for w in bytes.windows(10) {
        if let Ok(s) = std::str::from_utf8(w) {
            if is_iso_date(s) {
                return Some(s.to_string());
            }
        }
    }
    // 2) a standalone 8-digit run → YYYYMMDD (bounded by non-digits so 9+ digits don't match)
    let chars: Vec<char> = name.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            if i - start == 8 {
                let run: String = chars[start..i].iter().collect();
                let candidate = format!("{}-{}-{}", &run[0..4], &run[4..6], &run[6..8]);
                if is_iso_date(&candidate) && &run[4..6] <= "12" && &run[6..8] <= "31" {
                    return Some(candidate);
                }
            }
        } else {
            i += 1;
        }
    }
    None
}

/// The document title for a photo — concise and carrying the type + date into every chunk's
/// breadcrumb (which is how each chunk gets the metadata as a context prefix). E.g. "Screenshot — 2026-03-12".
pub fn photo_title(source_type: PhotoSourceType, capture_date: &str) -> String {
    format!("{} — {}", source_type.noun(), capture_date)
}

/// The one-line capture sentence that becomes the always-present metadata chunk: type + date +
/// (when EXIF carried them) coordinates. Kept on its own under "## Capture details" so a query like
/// "that screenshot from March" hits it cleanly, isolated from OCR content.
fn metadata_sentence(
    source_type: PhotoSourceType,
    capture_date: &str,
    lat: Option<f64>,
    lon: Option<f64>,
) -> String {
    let mut s = format!("{} captured {}", source_type.noun(), capture_date);
    if let (Some(lat), Some(lon)) = (lat, lon) {
        s.push_str(&format!(", near {lat:.6}, {lon:.6}"));
    }
    s.push('.');
    s
}

/// Build the synthetic Markdown body for a photo: a "## Capture details" section (→ the stable
/// metadata chunk) and, when OCR produced text, a "## Text" section (→ the OCR chunk(s); the existing
/// packer keeps short text in one chunk and splits long text normally). No splitter changes needed —
/// the two headings give two independent leaf sections. OCR is never re-run on rebuild because the
/// text lives here in the vault body.
pub fn photo_markdown(
    source_type: PhotoSourceType,
    capture_date: &str,
    lat: Option<f64>,
    lon: Option<f64>,
    ocr_text: &str,
) -> String {
    let sentence = metadata_sentence(source_type, capture_date, lat, lon);
    let ocr = ocr_text.trim();
    if ocr.is_empty() {
        format!("## Capture details\n\n{sentence}\n")
    } else {
        format!("## Capture details\n\n{sentence}\n\n{TEXT_HEADING}\n\n{ocr}\n")
    }
}

/// Recover the OCR text from a rebuilt photo body — everything under the "## Text" heading. None when
/// the photo had no OCR (no Text section). Mirrors [`photo_markdown`].
pub fn ocr_text_from_body(body: &str) -> Option<String> {
    let marker = format!("\n{TEXT_HEADING}\n");
    let idx = body.find(&marker)?;
    let text = body[idx + marker.len()..].trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// What the sidecar's `analyze_image` extracts from one image: the OCR text (empty when OCR was not
/// run) plus best-effort EXIF capture metadata. `ocr_ran` distinguishes "OCR ran and found nothing"
/// from "OCR was not requested" (the user declined the optional component). Any EXIF field is `None`
/// when absent or unreadable (e.g. a HEIC opened without the pillow-heif codec) — the caller then
/// falls back to a filename- or ingest-time capture date.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImageAnalysis {
    /// Recognised text, joined by newlines. Empty string when OCR found nothing or was not run.
    pub ocr_text: String,
    /// Whether OCR actually ran (the optional component was installed and requested).
    pub ocr_ran: bool,
    /// EXIF DateTimeOriginal normalised to `YYYY-MM-DD`, or `None` if the image carries no date.
    pub capture_date: Option<String>,
    /// EXIF GPS latitude in signed decimal degrees, or `None`.
    pub lat: Option<f64>,
    /// EXIF GPS longitude in signed decimal degrees, or `None`.
    pub lon: Option<f64>,
    /// Pixel width, or `None` if the image could not be opened.
    pub width: Option<u32>,
    /// Pixel height, or `None` if the image could not be opened.
    pub height: Option<u32>,
}

// ---- optional OCR component: status + install commands --------------------
//
// OCR (rapidocr + pillow-heif) is delivered exactly like the t-SNE reducer (see [`crate::layout`]):
// an OPTIONAL on-demand component the user installs once. These two commands are the install surface
// (the drop-time prompt and the Storage tab). Removal goes through the Storage manager's guarded
// cascade ([`crate::components::remove_storage_component`] with id `"ocr"`), which reclaims the heavy
// image deps in order — so there is deliberately no standalone uninstall command here.

/// Whether the optional photo-OCR component (rapidocr + pillow-heif) is installed.
#[derive(Serialize)]
pub struct OcrStatus {
    installed: bool,
}

/// Progress for the optional OCR component download (broadcast on `ocr://install`). Like the t-SNE
/// download it has no file count, so `fraction` (0.0..=1.0, monotonic) renders as a percentage bar.
#[derive(Clone, Serialize)]
pub struct OcrInstallEvent {
    fraction: f32,
}

/// Whether the optional photo-OCR component is installed in the managed venv. Cheap (a marker read),
/// so the UI can check it before a photo drop and the Storage tab can show the install/remove state.
#[tauri::command]
pub fn optional_ocr_status(state: State<'_, AppState>) -> Result<OcrStatus> {
    Ok(OcrStatus {
        installed: state.sidecar.optional_ocr_ready(),
    })
}

/// Install the optional photo-OCR component (rapidocr + pillow-heif) on demand — a pip download into
/// the managed venv. The blocking install runs off the async runtime; progress rides `ocr://install`
/// so the Storage tab and the drop-time prompt can show a real percentage bar. Errors surface to the
/// caller so the UI can show them. Idempotent (a no-op once installed).
#[tauri::command]
pub async fn install_optional_ocr(app: AppHandle) -> Result<()> {
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let progress_app = app2.clone();
        app2.state::<AppState>()
            .sidecar
            .install_optional_ocr(move |fraction| {
                let _ = progress_app.emit("ocr://install", OcrInstallEvent { fraction });
            })
    })
    .await
    .map_err(|e| Error::Other(format!("OCR install task panicked: {e}")))??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn capture_date_prefers_exif_then_filename_then_ingest() {
        let p = PathBuf::from("/x/IMG_20260312_094135.jpg");
        // EXIF wins when valid.
        assert_eq!(
            resolve_capture_date(Some("2025-11-02"), &p, "2026-06-28T10:00:00Z"),
            "2025-11-02"
        );
        // Invalid/placeholder EXIF → filename's 8-digit run.
        assert_eq!(
            resolve_capture_date(Some("0000:00:00"), &p, "2026-06-28T10:00:00Z"),
            "2026-03-12"
        );
        // No EXIF, no date in the name → the ingest timestamp's date.
        let plain = PathBuf::from("/x/note.png");
        assert_eq!(
            resolve_capture_date(None, &plain, "2026-06-28T10:00:00Z"),
            "2026-06-28"
        );
    }

    #[test]
    fn filename_dates_parse_iso_and_compact() {
        assert_eq!(
            date_from_filename(&PathBuf::from("/x/Screenshot 2026-03-12 at 9.41.png")).as_deref(),
            Some("2026-03-12")
        );
        assert_eq!(
            date_from_filename(&PathBuf::from("/x/IMG_20260312.jpg")).as_deref(),
            Some("2026-03-12")
        );
        // A 9-digit id is not a date; an impossible month is rejected.
        assert_eq!(date_from_filename(&PathBuf::from("/x/123456789.jpg")), None);
        assert_eq!(date_from_filename(&PathBuf::from("/x/20269912.jpg")), None);
    }

    #[test]
    fn source_type_inference() {
        assert_eq!(
            infer_source_type(&PathBuf::from("/x/Screenshot 2026.png"), false),
            PhotoSourceType::Screenshot
        );
        assert_eq!(
            infer_source_type(&PathBuf::from("/x/IMG_1234.jpg"), true),
            PhotoSourceType::CameraRoll
        );
        assert_eq!(
            infer_source_type(&PathBuf::from("/x/diagram.png"), false),
            PhotoSourceType::DraggedFile
        );
    }

    #[test]
    fn markdown_has_metadata_chunk_and_optional_text_roundtrips() {
        // With OCR: two sections; the metadata sentence carries type/date/coords; OCR round-trips.
        let body = photo_markdown(
            PhotoSourceType::Screenshot,
            "2026-03-12",
            Some(55.95),
            Some(-3.19),
            "Invoice total £42.00",
        );
        assert!(body.contains("## Capture details"));
        assert!(body.contains("Screenshot captured 2026-03-12, near 55.950000, -3.190000."));
        assert!(body.contains("## Text"));
        assert_eq!(
            ocr_text_from_body(&body).as_deref(),
            Some("Invoice total £42.00")
        );

        // Without OCR: only the metadata section, and nothing to recover.
        let bare = photo_markdown(PhotoSourceType::CameraRoll, "2026-03-12", None, None, "  ");
        assert!(bare.contains("Photo captured 2026-03-12."));
        assert!(!bare.contains("## Text"));
        assert_eq!(ocr_text_from_body(&bare), None);
    }

    #[test]
    fn source_type_db_roundtrip() {
        for st in [
            PhotoSourceType::Screenshot,
            PhotoSourceType::CameraRoll,
            PhotoSourceType::DraggedFile,
            PhotoSourceType::VaultCopy,
        ] {
            assert_eq!(PhotoSourceType::from_db(st.as_str()), st);
        }
        assert_eq!(
            PhotoSourceType::from_db("nonsense"),
            PhotoSourceType::DraggedFile
        );
    }
}
