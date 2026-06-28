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
