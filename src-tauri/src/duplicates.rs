// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Finding the documents a user has twice (#282).
//!
//! **Why this cannot be a content-hash scan, which is what the card originally asked for.**
//! `documents.content_hash` is `NOT NULL UNIQUE`, and every local ingest pre-checks it before doing
//! any work — so two vault documents with identical text *cannot exist*; the second is refused at
//! the door with "already ingested". Meanwhile the duplicates a user actually sees are the ones the
//! hash is **deliberately salted to keep apart**: `index_only::pointer_content_hash` folds the
//! source id into the digest precisely so that two different sources sharing identical text stay two
//! items. A scan keyed on `content_hash` would therefore find nothing, always, on every store.
//!
//! So this module compares what is actually comparable, with two independent signals:
//!
//!   1. **The opening text.** [`opening_key`] normalises hard (case, punctuation, whitespace all
//!      folded away) and keeps the first [`OPENING_CHARS`] characters. Two rows with the same key
//!      begin identically once formatting is discounted — which is what survives the same document
//!      arriving through two different converters (MarkItDown locally, a provider's export in the
//!      cloud). Cheap, offline, explainable, and available for **every** document: a vault document
//!      has its first chunk's real text, and an index-only pointer has `stored_summary`.
//!   2. **The embedding.** Two documents whose first leaf vectors sit within [`NEAR_THRESHOLD`]
//!      cosine are near-duplicates even when the text differs — the `.docx` and the `.pdf` it was
//!      exported to, or a lightly edited second copy. This is the only signal that sees the *whole*
//!      of an index-only document, whose body bytes are never stored (its chunk rows hold a
//!      placeholder) but whose leaf embeddings are real.
//!
//! **Neither signal is allowed to act on its own.** Documents generated from a template share an
//! opening; a series of invoices embeds near-identically. Both are false pairs by construction, and
//! no threshold fixes that — so this module only ever *reports*, and the user decides. That is also
//! why a pair carries which signals fired: "these start identically" and "these read very alike" are
//! different claims and deserve different words on screen.

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::error::Result;

/// Characters of normalised opening text compared. Long enough that a shared letterhead alone rarely
/// fills it, short enough to stay inside an index-only [`crate::index_only`] summary (~500 chars),
/// which is the shortest opening any document type can offer.
pub const OPENING_CHARS: usize = 400;

/// Below this many normalised characters an opening is not evidence of anything — a title, a date
/// line, a one-line note. Those documents can still pair on the embedding signal.
pub const OPENING_MIN_CHARS: usize = 120;

/// Cosine above which two first-leaf vectors are called near-duplicates. Deliberately high: the cost
/// of a miss is that the user scrolls past a duplicate, and the cost of a false pair is that they
/// consider deleting a document they wanted.
pub const NEAR_THRESHOLD: f32 = 0.97;

/// The pairwise similarity sweep is O(n²) in documents. Past this many, the opening-text half still
/// runs (it is O(n)) and the similarity half is reported as **skipped** rather than quietly dropped —
/// a scan that silently covered less than it claimed would be worse than one that admits its limit.
pub const MAX_SIMILARITY_DOCUMENTS: usize = 5_000;

/// Normalised opening text: lowercase, every run of non-alphanumerics folded to one space, trimmed,
/// truncated to [`OPENING_CHARS`]. `None` when what remains is too short to mean anything.
///
/// The normalisation is the point. The same document reaching PM twice — once converted from a local
/// `.docx`, once exported as text by a cloud provider — differs in exactly the ways this discards:
/// heading markers, quote characters, table pipes, doubled blank lines. What it must NOT discard is
/// word order or the words themselves, so this is a fold, never a fuzzy match.
pub fn opening_key(text: &str) -> Option<String> {
    let mut out = String::with_capacity(OPENING_CHARS);
    // Counted as we go, never re-measured: `chars().count()` inside the loop would make normalising
    // a long document quadratic in its length, on a path that runs once per document per scan.
    let mut len = 0usize;
    let mut pending_space = false;
    for ch in text.chars() {
        if len >= OPENING_CHARS {
            break;
        }
        if !ch.is_alphanumeric() {
            pending_space = true;
            continue;
        }
        if pending_space && len > 0 {
            out.push(' ');
            len += 1;
        }
        pending_space = false;
        // A codepoint can lowercase to more than one (ẞ → ss), so append under the same cap rather
        // than assuming one-in-one-out.
        for lower in ch.to_lowercase() {
            if len >= OPENING_CHARS {
                break;
            }
            out.push(lower);
            len += 1;
        }
    }
    (len >= OPENING_MIN_CHARS).then_some(out)
}

/// Cosine similarity. Returns 0.0 for a zero or mismatched-width vector rather than a NaN, so one
/// bad row can never poison a comparison into looking like a perfect match.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0f32, 0f32, 0f32);
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na <= 0.0 || nb <= 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// Decode the raw little-endian `f32` blob `chunk_vec` stores — the inverse of
/// [`crate::ingest::embedding_blob`]. A trailing partial float is ignored rather than panicking.
pub fn decode_embedding(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}

/// One document as the sweep sees it: the two comparable signals plus enough identity to report it.
#[derive(Clone, Debug)]
pub struct DupDoc {
    pub id: i64,
    pub opening: Option<String>,
    pub vector: Option<Vec<f32>>,
}

/// Why a pair was reported. Both flags can be true; at least one always is.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Signals {
    /// The normalised openings are identical.
    pub same_opening: bool,
    /// Cosine of the two first-leaf vectors, when both had one and it cleared [`NEAR_THRESHOLD`].
    pub similarity: Option<f32>,
}

/// A candidate pair, lower document id first so a pair has one stable identity however it was found.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct RawPair {
    pub a: i64,
    pub b: i64,
    pub signals: Signals,
}

/// Pair up documents by both signals, merging a pair found by both into one entry.
///
/// Pure, so the pairing rules are tested without a database, a model or a vault. `similarity_budget`
/// is how many documents the O(n²) half may consider — the caller passes
/// [`MAX_SIMILARITY_DOCUMENTS`] or 0 to run openings only.
///
/// Results are ordered by strength: pairs both signals agree on first, then same-opening, then
/// similarity alone, and within each by descending similarity. The strongest claims are the ones
/// worth a person's attention first, and a list that opens with its weakest guesses teaches the user
/// to distrust the whole feature.
pub fn pair_up(docs: &[DupDoc], similarity_budget: usize) -> Vec<RawPair> {
    use std::collections::HashMap;

    let mut merged: HashMap<(i64, i64), Signals> = HashMap::new();

    // 1. Opening text — O(n): group by key, pair everything inside a group.
    let mut by_opening: HashMap<&str, Vec<i64>> = HashMap::new();
    for d in docs {
        if let Some(key) = d.opening.as_deref() {
            by_opening.entry(key).or_default().push(d.id);
        }
    }
    for ids in by_opening.values() {
        for (i, a) in ids.iter().enumerate() {
            for b in &ids[i + 1..] {
                let key = ordered(*a, *b);
                merged.entry(key).or_insert(Signals {
                    same_opening: false,
                    similarity: None,
                });
                merged.get_mut(&key).unwrap().same_opening = true;
            }
        }
    }

    // 2. Embeddings — O(n²), and only within budget. Documents with no vector (never indexed, or
    //    index-only rows whose leaves predate the current model) simply don't participate.
    let vectored: Vec<&DupDoc> = docs.iter().filter(|d| d.vector.is_some()).collect();
    if vectored.len() <= similarity_budget {
        for (i, a) in vectored.iter().enumerate() {
            for b in &vectored[i + 1..] {
                let score = cosine(a.vector.as_ref().unwrap(), b.vector.as_ref().unwrap());
                if score < NEAR_THRESHOLD {
                    continue;
                }
                let key = ordered(a.id, b.id);
                let entry = merged.entry(key).or_insert(Signals {
                    same_opening: false,
                    similarity: None,
                });
                entry.similarity = Some(score);
            }
        }
    }

    let mut out: Vec<RawPair> = merged
        .into_iter()
        .map(|((a, b), signals)| RawPair { a, b, signals })
        .collect();
    out.sort_by(|x, y| {
        rank(&y.signals)
            .cmp(&rank(&x.signals))
            .then(
                y.signals
                    .similarity
                    .unwrap_or(0.0)
                    .total_cmp(&x.signals.similarity.unwrap_or(0.0)),
            )
            // Ties break on id so the list never reshuffles between two scans of an unchanged store.
            .then(x.a.cmp(&y.a))
            .then(x.b.cmp(&y.b))
    });
    out
}

/// Both signals > opening alone > similarity alone. Opening outranks similarity because it is an
/// exact claim about the text; similarity is a judgement with a threshold behind it.
fn rank(s: &Signals) -> u8 {
    match (s.same_opening, s.similarity.is_some()) {
        (true, true) => 2,
        (true, false) => 1,
        _ => 0,
    }
}

fn ordered(a: i64, b: i64) -> (i64, i64) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

// --- reading the store ------------------------------------------------------------------

/// Every document the sweep can compare, with its opening text resolved per source type.
///
/// A **vault** document's opening comes from its first chunk's stored text — real content already in
/// the database, so the sweep never reads or decrypts a single vault file. An **index-only** row's
/// chunk text is a placeholder by design, so its opening comes from `stored_summary`, which is the
/// first ~500 characters of the body captured at index time. Comparable after [`opening_key`]'s fold.
pub fn load_documents(conn: &Connection) -> Result<Vec<DupDoc>> {
    let mut stmt = conn.prepare(
        "SELECT d.id, \
                CASE WHEN d.source_type = ?1 THEN d.stored_summary \
                     ELSE (SELECT c.content FROM chunks c \
                            WHERE c.document_id = d.id ORDER BY c.ordinal, c.id LIMIT 1) END \
         FROM documents d ORDER BY d.id",
    )?;
    let rows: Vec<(i64, Option<String>)> = stmt
        .query_map(params![crate::ingest::SOURCE_TYPE_INDEX_ONLY], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<std::result::Result<_, _>>()?;
    drop(stmt);

    let mut out = Vec::with_capacity(rows.len());
    for (id, opening) in rows {
        out.push(DupDoc {
            id,
            opening: opening.as_deref().and_then(opening_key),
            vector: first_leaf_vector(conn, id)?,
        });
    }
    Ok(out)
}

/// The embedding of a document's first **leaf** chunk, or `None` if it has none.
///
/// Only leaves get vectors (parents are gaps in `chunk_vec`), so this walks the document's chunks in
/// order and takes the first that has one — rather than joining `chunk_vec`, which is a vec0 virtual
/// table and answers rowid lookups far more predictably than scans.
fn first_leaf_vector(conn: &Connection, document_id: i64) -> Result<Option<Vec<f32>>> {
    let mut ids =
        conn.prepare("SELECT id FROM chunks WHERE document_id = ?1 ORDER BY ordinal, id LIMIT ?2")?;
    let chunk_ids: Vec<i64> = ids
        .query_map(params![document_id, MAX_CHUNKS_PROBED as i64], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    drop(ids);
    for chunk_id in chunk_ids {
        let blob: Option<Vec<u8>> = conn
            .query_row(
                "SELECT embedding FROM chunk_vec WHERE rowid = ?1",
                params![chunk_id],
                |r| r.get(0),
            )
            .ok();
        if let Some(blob) = blob {
            let v = decode_embedding(&blob);
            if !v.is_empty() {
                return Ok(Some(v));
            }
        }
    }
    Ok(None)
}

/// How far into a document to look for its first leaf. A document that is all parents for its first
/// dozen chunks is pathological; bounding this keeps a broken row from costing a scan its runtime.
const MAX_CHUNKS_PROBED: usize = 12;

// --- the command surface ----------------------------------------------------------------

/// Two documents PM believes are the same thing, and why it believes it. Each side is a full
/// [`crate::ingest::Document`] so the UI renders it with the same row, badges and actions as the
/// Documents list — a duplicate is a document, and giving it a bespoke shape would mean a second
/// place to keep "how a document looks" correct.
#[derive(Serialize)]
pub struct DuplicatePair {
    pub a: crate::ingest::Document,
    pub b: crate::ingest::Document,
    /// Their normalised openings are identical (see [`opening_key`]).
    pub same_opening: bool,
    /// Cosine of their first-leaf embeddings, when it cleared [`NEAR_THRESHOLD`].
    pub similarity: Option<f32>,
}

/// What one scan found, including what it did **not** do.
#[derive(Serialize)]
pub struct DuplicateReport {
    pub scanned: usize,
    pub pairs: Vec<DuplicatePair>,
    /// The store was past [`MAX_SIMILARITY_DOCUMENTS`], so only the opening-text signal ran. Surfaced
    /// rather than swallowed: "no duplicates found" from a scan that skipped half its method is a
    /// claim PM has not earned.
    pub similarity_skipped: bool,
    pub similarity_limit: usize,
    /// Pairs hidden because the user already chose to keep both. Reported rather than silently
    /// subtracted: a narrowed result the user cannot see the shape of is the same defect as the
    /// skipped-similarity case above.
    pub dismissed: usize,
}

/// The pairs the user has already decided to keep, lower-id first to match [`ordered`].
fn load_dismissals(conn: &Connection) -> Result<std::collections::HashSet<(i64, i64)>> {
    let mut stmt = conn.prepare("SELECT a_document_id, b_document_id FROM duplicate_dismissals")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
    let mut out = std::collections::HashSet::new();
    for row in rows {
        out.insert(row?);
    }
    Ok(out)
}

/// Record that the user looked at a pair and is keeping both.
///
/// Without this the report was stateless by construction — `scan_duplicates` recomputes everything
/// and writes nothing back, and the only "resolved" state was a component-local set cleared at the
/// top of every scan. So a decision the user had already made was re-offered on every scan forever.
#[tauri::command]
pub fn dismiss_duplicate_pair(
    state: tauri::State<'_, crate::AppState>,
    a: i64,
    b: i64,
) -> Result<()> {
    let conn = state.conn()?;
    let (lo, hi) = ordered(a, b);
    conn.execute(
        "INSERT OR IGNORE INTO duplicate_dismissals (a_document_id, b_document_id, dismissed_at)
         VALUES (?1, ?2, ?3)",
        // chrono directly, NOT a helper that takes `&AppState`: the connection guard is already
        // held here, and re-entering `state.conn()` on a non-reentrant mutex self-deadlocks.
        rusqlite::params![lo, hi, chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// Un-hide every dismissed pair, so a narrowing the user made is one they can undo.
#[tauri::command]
pub fn restore_duplicate_dismissals(state: tauri::State<'_, crate::AppState>) -> Result<()> {
    let conn = state.conn()?;
    conn.execute("DELETE FROM duplicate_dismissals", [])?;
    Ok(())
}

/// Scan the whole library for documents the user has twice (#282).
///
/// On demand, never on a timer: it reads every document's opening and first-leaf vector, and the
/// similarity half is O(n²) in documents. An automatic version would spend that budget repeatedly to
/// tell most users nothing.
///
/// Reports only — nothing is deleted, merged or hidden. Both signals produce false pairs by
/// construction (a template shares an opening; a run of invoices embeds alike), so the decision is
/// always the user's, made against two documents they can open.
#[tauri::command]
pub async fn scan_duplicates(app: tauri::AppHandle) -> Result<DuplicateReport> {
    tokio::task::spawn_blocking(move || {
        let state = tauri::Manager::state::<crate::AppState>(&app);
        let conn = state.conn()?;
        let docs = load_documents(&conn)?;
        let with_vectors = docs.iter().filter(|d| d.vector.is_some()).count();
        let similarity_skipped = with_vectors > MAX_SIMILARITY_DOCUMENTS;
        let pairs = pair_up(&docs, MAX_SIMILARITY_DOCUMENTS);
        let dismissals = load_dismissals(&conn)?;
        let mut dismissed = 0usize;

        let mut out = Vec::with_capacity(pairs.len());
        for p in pairs {
            // `pair_up` already emits ids lower-first, and the table stores them the same way, so
            // one lookup settles a pair whichever way round it was discovered.
            if dismissals.contains(&ordered(p.a, p.b)) {
                dismissed += 1;
                continue;
            }
            // A row deleted between the sweep and this load is not an error — drop the pair. Both
            // sides must resolve, or there is nothing to compare on screen.
            let (Ok(a), Ok(b)) = (
                crate::ingest::load_document(&conn, p.a),
                crate::ingest::load_document(&conn, p.b),
            ) else {
                continue;
            };
            out.push(DuplicatePair {
                a,
                b,
                same_opening: p.signals.same_opening,
                similarity: p.signals.similarity,
            });
        }
        Ok(DuplicateReport {
            scanned: docs.len(),
            pairs: out,
            similarity_skipped,
            similarity_limit: MAX_SIMILARITY_DOCUMENTS,
            dismissed,
        })
    })
    .await
    .map_err(|e| crate::error::Error::Other(format!("duplicate scan task panicked: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(id: i64, opening: Option<&str>, vector: Option<Vec<f32>>) -> DupDoc {
        DupDoc {
            id,
            opening: opening.map(str::to_string),
            vector,
        }
    }

    /// A body long enough to clear `OPENING_MIN_CHARS` after normalisation.
    fn long_body(seed: &str) -> String {
        format!("{seed} ").repeat(60)
    }

    #[test]
    fn opening_key_folds_formatting_but_not_words() {
        // THE case the whole signal exists for: the same document converted twice. MarkItDown emits
        // ATX headings and pipe tables; a provider's plain-text export emits neither. Everything they
        // differ by is punctuation and whitespace, and everything they agree on is the prose.
        let markitdown = format!("## {}\n\n| a | b |\n", long_body("Quarterly Report Notes"));
        let export = format!("{}\n   a   b\n", long_body("quarterly report NOTES"));
        assert_eq!(
            opening_key(&markitdown),
            opening_key(&export),
            "case, heading markers and table pipes must not separate two copies"
        );
        // Word order is meaning, never noise — folding it away would pair unrelated documents.
        assert_ne!(
            opening_key(&long_body("alpha beta")),
            opening_key(&long_body("beta alpha"))
        );
    }

    #[test]
    fn opening_key_refuses_to_speak_for_a_short_document() {
        // A title, a date line, a one-line note: these collide constantly and mean nothing. They can
        // still pair on the embedding signal, which sees the whole document.
        assert_eq!(opening_key("Invoice"), None);
        assert_eq!(opening_key("2026-07-28"), None);
        assert_eq!(opening_key(""), None);
        // Punctuation alone normalises to nothing at all — and must not become an empty key that
        // every other punctuation-only document then "matches".
        assert_eq!(opening_key("--- *** ---"), None);
    }

    #[test]
    fn opening_key_is_bounded_so_a_long_document_costs_no_more_than_a_short_one() {
        // Comfortably past the cap (~1100 normalised chars) — the key is what makes a scan's cost
        // independent of document length, and an off-by-one that let it overshoot would also make
        // two copies that diverge just past the boundary stop matching.
        let key = opening_key(&long_body("a paragraph of ordinary prose")).unwrap();
        assert_eq!(key.chars().count(), OPENING_CHARS);
        // Exactly at the cap, not one over: the truncation lands mid-word, and must do so identically
        // for both copies of a document.
        let same = opening_key(&format!(
            "{} …and then it differs",
            long_body("a paragraph of ordinary prose")
        ));
        assert_eq!(same.as_deref(), Some(key.as_str()));
    }

    #[test]
    fn cosine_is_zero_rather_than_nan_on_a_degenerate_vector() {
        // A NaN here would sort as neither above nor below the threshold, and a zero vector compared
        // to itself must never read as a perfect match — that would pair every unindexed document
        // with every other.
        assert_eq!(cosine(&[0.0, 0.0], &[0.0, 0.0]), 0.0);
        assert_eq!(cosine(&[1.0, 0.0], &[]), 0.0);
        assert_eq!(cosine(&[1.0, 0.0], &[1.0, 0.0, 0.0]), 0.0, "width mismatch");
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
    }

    #[test]
    fn embedding_blob_round_trips() {
        // Guards the pair against drift: `chunk_vec` stores what `ingest::embedding_blob` wrote, and
        // a decoder that disagreed about endianness would produce plausible garbage — vectors that
        // compare cleanly and mean nothing.
        let v = vec![0.5f32, -1.25, 3.0e-3];
        assert_eq!(decode_embedding(&crate::ingest::embedding_blob(&v)), v);
    }

    #[test]
    fn a_pair_is_reported_once_however_many_signals_found_it() {
        let a = long_body("shared opening text");
        let docs = vec![
            doc(1, opening_key(&a).as_deref(), Some(vec![1.0, 0.0])),
            doc(2, opening_key(&a).as_deref(), Some(vec![1.0, 0.0])),
        ];
        let pairs = pair_up(&docs, MAX_SIMILARITY_DOCUMENTS);
        assert_eq!(pairs.len(), 1, "one pair, not one per signal");
        assert_eq!(pairs[0].a, 1);
        assert_eq!(pairs[0].b, 2);
        assert!(pairs[0].signals.same_opening);
        assert!(pairs[0].signals.similarity.unwrap() > NEAR_THRESHOLD);
    }

    #[test]
    fn each_signal_finds_what_the_other_cannot() {
        let shared = long_body("identical opening");
        let docs = vec![
            // 1 & 2: same opening, orthogonal vectors — the cloud/local converter case where the
            // bodies later diverge.
            doc(1, opening_key(&shared).as_deref(), Some(vec![1.0, 0.0])),
            doc(2, opening_key(&shared).as_deref(), Some(vec![0.0, 1.0])),
            // 3 & 4: different openings, near-identical vectors — the .docx/.pdf case.
            doc(
                3,
                opening_key(&long_body("one wording")).as_deref(),
                Some(vec![0.6, 0.8]),
            ),
            doc(
                4,
                opening_key(&long_body("another wording")).as_deref(),
                Some(vec![0.61, 0.79]),
            ),
        ];
        let pairs = pair_up(&docs, MAX_SIMILARITY_DOCUMENTS);
        let by_ids: Vec<(i64, i64)> = pairs.iter().map(|p| (p.a, p.b)).collect();
        assert!(by_ids.contains(&(1, 2)), "opening-only pair found");
        assert!(by_ids.contains(&(3, 4)), "similarity-only pair found");
        let opening_only = pairs.iter().find(|p| (p.a, p.b) == (1, 2)).unwrap();
        assert!(opening_only.signals.similarity.is_none());
        let similar_only = pairs.iter().find(|p| (p.a, p.b) == (3, 4)).unwrap();
        assert!(!similar_only.signals.same_opening);
    }

    #[test]
    fn the_strongest_claims_come_first() {
        // A list that opens with its weakest guesses teaches the user to distrust all of it.
        let shared = long_body("identical opening");
        let docs = vec![
            doc(1, opening_key(&shared).as_deref(), Some(vec![1.0, 0.0])),
            doc(2, opening_key(&shared).as_deref(), Some(vec![1.0, 0.0])), // both signals
            doc(3, opening_key(&shared).as_deref(), Some(vec![0.0, 1.0])), // opening only (vs 1,2)
            doc(4, None, Some(vec![0.6, 0.8])),
            doc(5, None, Some(vec![0.6, 0.8])), // similarity only
        ];
        let pairs = pair_up(&docs, MAX_SIMILARITY_DOCUMENTS);
        assert_eq!(rank(&pairs[0].signals), 2, "both signals lead");
        assert_eq!(
            rank(&pairs.last().unwrap().signals),
            0,
            "similarity alone trails"
        );
    }

    #[test]
    fn a_zero_similarity_budget_still_runs_the_opening_half() {
        // What a store past `MAX_SIMILARITY_DOCUMENTS` gets: the cheap signal in full, and an honest
        // statement that the expensive one did not run — never a quietly narrower scan.
        let shared = long_body("identical opening");
        let docs = vec![
            doc(1, opening_key(&shared).as_deref(), Some(vec![1.0, 0.0])),
            doc(2, opening_key(&shared).as_deref(), Some(vec![1.0, 0.0])),
        ];
        let pairs = pair_up(&docs, 0);
        assert_eq!(pairs.len(), 1);
        assert!(pairs[0].signals.same_opening);
        assert!(
            pairs[0].signals.similarity.is_none(),
            "the O(n²) half was skipped, and says so by its absence"
        );
    }

    #[test]
    fn documents_with_neither_signal_pair_with_nothing() {
        // The failure that would make the feature useless: an empty opening key and a missing vector
        // must not collapse into "everything matches everything".
        let docs = vec![
            doc(1, None, None),
            doc(2, None, None),
            doc(3, None, Some(vec![0.0, 0.0])), // zero vector — cosine 0, never a match
            doc(4, None, Some(vec![0.0, 0.0])),
        ];
        assert!(pair_up(&docs, MAX_SIMILARITY_DOCUMENTS).is_empty());
    }

    #[test]
    fn pairing_is_stable_between_two_scans_of_an_unchanged_store() {
        let shared = long_body("identical opening");
        let docs = vec![
            doc(7, opening_key(&shared).as_deref(), Some(vec![1.0, 0.0])),
            doc(3, opening_key(&shared).as_deref(), Some(vec![1.0, 0.0])),
            doc(5, opening_key(&shared).as_deref(), Some(vec![1.0, 0.0])),
        ];
        let first = pair_up(&docs, MAX_SIMILARITY_DOCUMENTS);
        let second = pair_up(&docs, MAX_SIMILARITY_DOCUMENTS);
        assert_eq!(first, second, "same input, same order");
        // Lower id first within every pair, so a pair has one identity however it was discovered.
        assert!(first.iter().all(|p| p.a < p.b));
    }
}
