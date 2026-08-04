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
/// Pure, so the pairing rules are tested without a database, a model or a vault.
/// `similarity_budget` is stated in DOCUMENTS — the caller passes [`MAX_SIMILARITY_DOCUMENTS`], or 0
/// to run openings only — and spent in COMPARISONS, so that one number bounds an all-vs-all sweep
/// (n(n-1)/2) and a focused one (|focus| × n) honestly rather than only the shape it was written for.
///
/// Results are ordered by strength: pairs both signals agree on first, then same-opening, then
/// similarity alone, and within each by descending similarity. The strongest claims are the ones
/// worth a person's attention first, and a list that opens with its weakest guesses teaches the user
/// to distrust the whole feature.
///
/// `focus` of `None` compares everything against everything. `Some(set)` compares only those
/// documents against the corpus — the incremental mode a background sweep runs after a sync lands
/// something (#711). The full sweep measures 5.0s at 3,000 documents and 13.7s at 5,000, which is
/// why it was on-demand-only: spending that after every fifteen-minute poll would tell most users
/// nothing, repeatedly. Restricting one side to what actually arrived makes the cost proportional to
/// the arrivals rather than to the library, so a sync that lands three files compares three files. A
/// pair needs only ONE side in `focus` — the other side is whatever it duplicates, which is by
/// definition already in the library.
pub fn pair_up(
    docs: &[DupDoc],
    focus: Option<&std::collections::HashSet<i64>>,
    similarity_budget: usize,
) -> Vec<RawPair> {
    use std::collections::HashMap;

    let in_focus = |id: i64| focus.is_none_or(|f| f.contains(&id));

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
                if !in_focus(*a) && !in_focus(*b) {
                    continue;
                }
                let key = ordered(*a, *b);
                merged.entry(key).or_insert(Signals {
                    same_opening: false,
                    similarity: None,
                });
                merged.get_mut(&key).unwrap().same_opening = true;
            }
        }
    }

    // 2. Embeddings — quadratic, and only within budget. Documents with no vector (never indexed, or
    //    index-only rows whose leaves predate the current model) simply don't participate.
    //
    //    The budget is stated in DOCUMENTS but spent in COMPARISONS, so that one number bounds both
    //    shapes honestly: all-vs-all over `n` documents is n(n-1)/2 comparisons, and a focused sweep
    //    is |focus| × n. Without that, an incremental pass over a library past the document limit
    //    would refuse to compare three new files against it — which costs nothing and is the whole
    //    point of running incrementally.
    let vectored: Vec<&DupDoc> = docs.iter().filter(|d| d.vector.is_some()).collect();
    let focused: Vec<&&DupDoc> = vectored.iter().filter(|d| in_focus(d.id)).collect();
    let wanted = match focus {
        None => vectored.len() * vectored.len().saturating_sub(1) / 2,
        Some(_) => focused.len() * vectored.len(),
    };
    let allowed = similarity_budget * similarity_budget.saturating_sub(1) / 2;
    if wanted <= allowed {
        match focus {
            // Every unordered pair, once.
            None => {
                for (i, a) in vectored.iter().enumerate() {
                    for b in &vectored[i + 1..] {
                        note_similarity(&mut merged, a, b);
                    }
                }
            }
            // Each arrival against the whole corpus. Driving the outer loop off `focused` rather
            // than filtering inside an all-vs-all walk is what makes this proportional to the
            // arrivals — the filtered version still visits n²/2 pairs to reject nearly all of them.
            // Two arrivals meeting each other are visited twice; `merged` is keyed on the ordered
            // pair and cosine is symmetric, so the second visit writes the same answer.
            Some(_) => {
                for a in &focused {
                    for b in &vectored {
                        if a.id != b.id {
                            note_similarity(&mut merged, a, b);
                        }
                    }
                }
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

/// Record a near-duplicate cosine for one pair, if it clears [`NEAR_THRESHOLD`]. Both callers hand
/// it vectored documents, so the `unwrap`s cannot fire.
fn note_similarity(
    merged: &mut std::collections::HashMap<(i64, i64), Signals>,
    a: &DupDoc,
    b: &DupDoc,
) {
    let score = cosine(a.vector.as_ref().unwrap(), b.vector.as_ref().unwrap());
    if score < NEAR_THRESHOLD {
        return;
    }
    let entry = merged.entry(ordered(a.id, b.id)).or_insert(Signals {
        same_opening: false,
        similarity: None,
    });
    entry.similarity = Some(score);
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

// --- the identity fold: duplicates PM can PROVE, and therefore resolve ------------------------
//
// Everything above is a REPORT, and has to be: an opening key and a cosine are evidence, not proof,
// so acting on them alone would delete documents the user wanted. What follows is the other kind of
// duplicate entirely — two rows PM can show are the same file, because both source ids resolve to
// the same provider file id ([`crate::locations::provenance_key`]). That is a fact, not a
// judgement, so it is resolved without asking, and nothing is deleted that isn't provably a second
// copy of something kept.

/// What one identity sweep did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct IdentityFold {
    /// Documents folded away — each was a second row for a file PM already had.
    pub folded: usize,
    /// The folded documents' ANCHOR source ids, so the caller can purge them from the portable
    /// manifest. A DB-only fold leaves the old id in the mirror, where the mirror-∪-file union keeps
    /// it and the next Rebuild restores the duplicate — the exact defect this card cites in
    /// `resolve_shared_drive_twins`.
    pub retired: Vec<String>,
}

/// Which of two rows for one file should keep being the document.
///
/// Same policy as [`crate::drive::resolve_owned_swm_duplicate`] settled on, for the same reasons and
/// deliberately not re-litigated: **a My Drive id wins**, because it is the canonical namespace for
/// a file you own, it is reached by the cheap delta cursor rather than a full re-walk, and it
/// survives the sharing being revoked — where `gdrive:swm:<rootId>:` is keyed on a root that can
/// vanish and `gdrive:sd:<driveId>:` on a drive you can be removed from.
///
/// Since #710 this decides much less than it sounds like. The loser is not discarded — it becomes a
/// LOCATION of the survivor, reconciled by its own connector on its own cursor. All this picks is
/// which id stays the immutable identity anchor.
fn anchor_rank(source_id: &str) -> u8 {
    if !source_id.starts_with("gdrive:") {
        return 3;
    }
    if source_id.starts_with("gdrive:swm:") {
        return 2;
    }
    // `gdrive:sd:<driveId>:…`, and the legacy `gdrive:<email>:sd:<driveId>:…` twin shape v19
    // re-keyed away from — the same drive either way.
    if source_id.contains(":sd:") {
        return 1;
    }
    0
}

/// One document as the identity sweep needs to see it.
struct FoldDoc {
    id: i64,
    anchor: String,
}

/// Fold every set of documents that are provably the same file into one document with many
/// locations (#711).
///
/// This is the general case #703 could only reach a corner of. #703 stopped the *enumeration*
/// producing a second row, which works when one account's listing can see both routes; it cannot
/// see the case that actually bites — the owner indexed the file from My Drive, a second connected
/// account reaches it through Shared with me, and neither listing knows about the other. Worse, it
/// was order-dependent: the same two shares healed or didn't depending on which account claimed the
/// shared root first. Keying on the provider's own file id has no such asymmetry.
///
/// One transaction per fold, and the ORDER inside it is load-bearing: the doomed row's locations
/// move to the survivor BEFORE it is deleted, because `document_locations.document_id` cascades. Get
/// that backwards and the folded id stops being known to its connector, comes back as a brand-new
/// file on the very next pass, and the duplicate rebuilds itself forever.
///
/// Caller owns the connection and must purge [`IdentityFold::retired`] from the manifest.
pub fn fold_by_identity(conn: &Connection) -> Result<IdentityFold> {
    use std::collections::HashMap;

    // Only index-only documents have locations, so a promoted local import — which keeps its
    // `gdrive:` id as a claim marker — is invisible here and stays untouched, which is right: it is
    // a stored file now, not a place a connector found one.
    let mut by_key: HashMap<String, Vec<FoldDoc>> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT l.provenance_key, d.id, d.source_id \
             FROM document_locations l JOIN documents d ON d.id = l.document_id \
             WHERE l.provenance_key IS NOT NULL AND d.source_type = 'index_only' \
               AND d.source_id IS NOT NULL \
             ORDER BY l.provenance_key, d.id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                FoldDoc {
                    id: r.get(1)?,
                    anchor: r.get(2)?,
                },
            ))
        })?;
        for row in rows {
            let (key, doc) = row?;
            by_key.entry(key).or_default().push(doc);
        }
    }

    let mut fold = IdentityFold::default();
    // Sorted so a sweep over an unchanged store does the same work in the same order every time —
    // a HashMap's iteration order is not stable, and neither would the reported counts be.
    let mut keys: Vec<&String> = by_key.keys().collect();
    keys.sort();
    for key in keys {
        let group = &by_key[key];
        if group.len() < 2 {
            continue;
        }
        // Best anchor wins; lowest document id breaks a tie, so two accounts racing the same file
        // settle on the same survivor whichever order they were swept in.
        let Some(survivor) = group.iter().min_by_key(|d| (anchor_rank(&d.anchor), d.id)) else {
            continue;
        };
        for doomed in group.iter().filter(|d| d.id != survivor.id) {
            fold_one(conn, survivor.id, doomed.id)?;
            fold.folded += 1;
            fold.retired.push(doomed.anchor.clone());
        }
    }
    // A fold moves the DB mirror AHEAD of the encrypted manifest, which is precisely what the stale
    // flag means. The caller strips the doomed anchors from the file, but the SURVIVOR's item still
    // carries its pre-fold project/tags/importance/reviewed — and `reconcile_on_open` runs moments
    // later in the same call, treats that file as truth, and writes the stale values back over the
    // merged filing. The row that held the correct filing has already been deleted by then, so there
    // is nothing left to recover it from. Marking stale makes the reconcile repair the file from the
    // mirror BEFORE it applies it, which is the case that machinery exists for.
    if fold.folded > 0 {
        crate::index_only::mark_manifest_stale(conn);
    }
    Ok(fold)
}

/// Absorb one document into another: its filing, its memberships, and its locations. The document
/// row itself then goes — there is one file, so there is one document.
///
/// The classification merge is [`crate::drive::merge_classification`], reused rather than restated:
/// it is where "a confirmed filing beats an unconfirmed one, otherwise fill gaps and never
/// overwrite" was decided, and a second copy of that rule would be a second thing to keep right. It
/// lives in `drive` because that is where the first caller needed it; the rule itself is about two
/// documents, not about Drive.
///
/// The doomed row's embeddings are discarded rather than merged. Two locations of one file have one
/// body, so the survivor's chunks already describe it; if the survivor happened to be the
/// summary-indexed one, its `summary_indexed` flag is still set and the next sync of any of its
/// locations upgrades it to the full body, exactly as it would have without the fold.
fn fold_one(conn: &Connection, survivor: i64, doomed: i64) -> Result<()> {
    let Some(survivor_class) = crate::drive::classification_of(conn, survivor)? else {
        return Ok(());
    };
    let Some(doomed_class) = crate::drive::classification_of(conn, doomed)? else {
        return Ok(());
    };
    let merged = crate::drive::merge_classification(survivor_class, &doomed_class);
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE documents SET project = ?2, tags = ?3, importance = ?4, reviewed = ?5, \
             entity_id = ?6 WHERE id = ?1",
        params![
            survivor,
            merged.project,
            merged.tags,
            merged.importance,
            merged.reviewed as i64,
            merged.entity_id
        ],
    )?;
    // Union the join rows (projects AND labels) before the doomed row cascades them away. Additive:
    // a link the survivor already had is untouched, one only the duplicate had is kept, and a home
    // project that lost the tie above lands here as a LINKED project rather than vanishing.
    tx.execute(
        "INSERT OR IGNORE INTO document_tags (document_id, tag_id) \
         SELECT ?1, tag_id FROM document_tags WHERE document_id = ?2",
        params![survivor, doomed],
    )?;
    // Before the delete, never after — see `fold_by_identity`.
    crate::locations::move_all(&tx, doomed, survivor)?;
    crate::ingest::delete_document(&tx, doomed)?;
    tx.commit()?;
    Ok(())
}

// --- the command surface ----------------------------------------------------------------

/// Two documents PM believes are the same thing, and why it believes it. Each side is a full
/// [`crate::ingest::Document`] so the UI renders it with the same row, badges and actions as the
/// Documents list — a duplicate is a document, and giving it a bespoke shape would mean a second
/// place to keep "how a document looks" correct.
#[derive(Clone, Serialize)]
pub struct DuplicatePair {
    pub a: crate::ingest::Document,
    pub b: crate::ingest::Document,
    /// Their normalised openings are identical (see [`opening_key`]).
    pub same_opening: bool,
    /// Cosine of their first-leaf embeddings, when it cleared [`NEAR_THRESHOLD`].
    pub similarity: Option<f32>,
}

/// What one scan found, including what it did **not** do.
#[derive(Clone, Serialize)]
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
    /// When this report was produced. The panel shows it because a result that arrived on its own,
    /// with no button pressed, is otherwise indistinguishable from one that is hours stale.
    pub checked_at: String,
    /// True when this covers only what arrived since the last check, rather than the whole library
    /// (#711). Surfaced for the same reason `similarity_skipped` is: a narrower scan reported as a
    /// whole one is a clean bill of health PM has not earned.
    pub incremental: bool,
}

impl DuplicateReport {
    /// Fold a fresh incremental result into what the panel is already showing.
    ///
    /// New pairs are appended rather than replacing: a background sweep only compared the arrivals,
    /// so everything it did NOT find is a question it never asked, and dropping the earlier findings
    /// would read as "those went away". `scanned` and `checked_at` take the fresh values — both
    /// describe the library now.
    fn absorb(&mut self, fresh: DuplicateReport) {
        let seen: std::collections::HashSet<(i64, i64)> =
            self.pairs.iter().map(|p| (p.a.id, p.b.id)).collect();
        self.pairs.extend(
            fresh
                .pairs
                .into_iter()
                .filter(|p| !seen.contains(&(p.a.id, p.b.id))),
        );
        self.scanned = fresh.scanned;
        self.checked_at = fresh.checked_at;
        self.dismissed = fresh.dismissed.max(self.dismissed);
        self.incremental = self.incremental || fresh.incremental;
    }
}

/// The background duplicate check's state, held in memory for the session (#711).
///
/// In memory rather than in a table on purpose: every pair here is recomputable from the store, and
/// a persisted report would be a second copy of a derived thing to keep in step with deletions,
/// re-embeds and dismissals. What survives a restart is the store; the panel offers a full check.
#[derive(Default)]
pub struct DuplicateWatch {
    /// Documents that have landed since the last sweep — the `focus` set for [`pair_new`].
    pending: Vec<i64>,
    /// One sweep at a time, across all three connectors. A Drive run and a local-folder run can
    /// finish within a second of each other, and two all-vs-corpus sweeps racing would spend the
    /// budget twice to reach the same answer.
    running: bool,
    /// What the last sweep found, background or on demand.
    pub report: Option<DuplicateReport>,
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
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    a: i64,
    b: i64,
) -> Result<()> {
    {
        let conn = state.conn()?;
        let (lo, hi) = ordered(a, b);
        conn.execute(
            "INSERT OR IGNORE INTO duplicate_dismissals (a_document_id, b_document_id, dismissed_at)
             VALUES (?1, ?2, ?3)",
            // chrono directly, NOT a helper that takes `&AppState`: the connection guard is already
            // held here, and re-entering `state.conn()` on a non-reentrant mutex self-deadlocks.
            rusqlite::params![lo, hi, chrono::Utc::now().to_rfc3339()],
        )?;
    }
    // Outside the block above, so the connection guard is released before the watch mutex is taken:
    // `sweep_arrivals` locks the watch and then takes a connection, and acquiring the two in the
    // opposite order here is how a deadlock between them would be built.
    forget_pair(&app, a, b);
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
        let report = {
            let conn = state.conn()?;
            scan(&conn, None)?
        };
        // A full check answers every question the incremental ones only sampled, so it REPLACES the
        // snapshot rather than merging into it — that is also what lets the panel stop saying it
        // only looked at the new arrivals.
        if let Ok(mut watch) = state.duplicate_watch.lock() {
            watch.report = Some(report.clone());
            watch.pending.clear();
        }
        Ok(report)
    })
    .await
    .map_err(|e| crate::error::Error::Other(format!("duplicate scan task panicked: {e}")))?
}

/// What the last check found, or `None` if none has run this session — the mount-time read that
/// makes a background result survive a tab switch (the tab router unmounts the view, so a result
/// held only in component state would be thrown away the moment the user looked at something else).
#[tauri::command]
pub fn duplicate_snapshot(
    state: tauri::State<'_, crate::AppState>,
) -> Result<Option<DuplicateReport>> {
    Ok(state
        .duplicate_watch
        .lock()
        .ok()
        .and_then(|w| w.report.clone()))
}

/// The event the background sweep emits when the snapshot changes, so an open Documents tab updates
/// without polling.
pub const DUPLICATES_UPDATED: &str = "duplicates://updated";

/// Drop every cached pair that names `document_id`, because the row is gone.
///
/// [`DuplicateWatch`]'s own doc names the hazard this closes: an in-memory report IS "a second copy
/// of a derived thing to keep in step with deletions", and nothing kept it in step. [`absorb`] only
/// ever APPENDS, so a pair whose document had been deleted survived every later sweep — and the
/// panel went on rendering a card for it, with live Open and Remove buttons, until the user ran a
/// full check.
///
/// [`absorb`]: DuplicateReport::absorb
pub fn forget_document(app: &tauri::AppHandle, document_id: i64) {
    let changed = {
        let state = tauri::Manager::state::<crate::AppState>(app);
        let Ok(mut watch) = state.duplicate_watch.lock() else {
            return;
        };
        let Some(report) = watch.report.as_mut() else {
            return;
        };
        let before = report.pairs.len();
        report
            .pairs
            .retain(|p| p.a.id != document_id && p.b.id != document_id);
        before != report.pairs.len()
    };
    if changed {
        let _ = tauri::Emitter::emit(app, DUPLICATES_UPDATED, ());
    }
}

/// Drop one cached pair the user has decided to keep, counting it among the hidden ones.
///
/// The dismissal is persisted by [`dismiss_duplicate_pair`], but persistence alone never reached the
/// snapshot the panel re-reads on mount — so switching tabs and coming back re-offered a decision
/// the user had already made, with no "you chose to keep this" line to explain it, because
/// `dismissed` had not moved either. Bumping it here is what keeps that line honest.
pub fn forget_pair(app: &tauri::AppHandle, a: i64, b: i64) {
    let pair = ordered(a, b);
    let changed = {
        let state = tauri::Manager::state::<crate::AppState>(app);
        let Ok(mut watch) = state.duplicate_watch.lock() else {
            return;
        };
        let Some(report) = watch.report.as_mut() else {
            return;
        };
        let before = report.pairs.len();
        report.pairs.retain(|p| ordered(p.a.id, p.b.id) != pair);
        let gone = before - report.pairs.len();
        report.dismissed += gone;
        gone > 0
    };
    if changed {
        let _ = tauri::Emitter::emit(app, DUPLICATES_UPDATED, ());
    }
}

/// One scan. `focus` of `None` is the whole library; `Some(ids)` compares only those documents
/// against it. Shared by the on-demand command and the background sweep so there is one definition
/// of what a report contains.
fn scan(
    conn: &Connection,
    focus: Option<&std::collections::HashSet<i64>>,
) -> Result<DuplicateReport> {
    let docs = load_documents(conn)?;
    let with_vectors = docs.iter().filter(|d| d.vector.is_some()).count();
    // Only the all-vs-all sweep can outgrow the budget; a focused one costs |focus| × n, which is
    // why it is allowed to run over a library the full scan would decline.
    let similarity_skipped = focus.is_none() && with_vectors > MAX_SIMILARITY_DOCUMENTS;
    let pairs = pair_up(&docs, focus, MAX_SIMILARITY_DOCUMENTS);
    let dismissals = load_dismissals(conn)?;
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
            crate::ingest::load_document(conn, p.a),
            crate::ingest::load_document(conn, p.b),
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
        checked_at: crate::ingest::iso_now(conn)?,
        incremental: focus.is_some(),
    })
}

/// Note that a document has just landed, so the next finished sync run checks it (#711).
///
/// Called from the one place every index-only connector announces an arrival, and from the local
/// import pipeline — the two doors a new document comes through. Recording the id rather than a
/// bare flag is what keeps the following sweep proportional to the arrivals.
pub fn note_arrival(state: &crate::AppState, document_id: i64) {
    if let Ok(mut watch) = state.duplicate_watch.lock() {
        watch.pending.push(document_id);
    }
}

/// Check what a finished run actually landed, in the background (#711).
///
/// **Only when something arrived.** A fifteen-minute poll that found nothing has changed no
/// document, so a sweep could only reproduce the last answer at the cost of reading every embedding
/// in the library. Single-flight for the same reason: three connectors can finish within a second of
/// each other, and three sweeps racing spend the budget three times to agree.
///
/// Best-effort throughout: this is a convenience that saves the user a click, and there is no state
/// it can leave wrong — the on-demand full check is always one button away.
pub fn sweep_arrivals(app: &tauri::AppHandle) {
    let state = tauri::Manager::state::<crate::AppState>(app);
    let focus: std::collections::HashSet<i64> = {
        let Ok(mut watch) = state.duplicate_watch.lock() else {
            return;
        };
        if watch.running || watch.pending.is_empty() {
            return;
        }
        watch.running = true;
        // Drained under the same lock that claimed the run, so an arrival landing mid-sweep queues
        // for the next one instead of being swept and forgotten.
        watch.pending.drain(..).collect()
    };
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = tauri::Manager::state::<crate::AppState>(&app);
        let fresh = (|| -> Result<DuplicateReport> {
            let conn = state.conn()?;
            scan(&conn, Some(&focus))
        })();
        if let Ok(mut watch) = state.duplicate_watch.lock() {
            watch.running = false;
            match fresh {
                Ok(fresh) => match watch.report.as_mut() {
                    Some(existing) => existing.absorb(fresh),
                    None => watch.report = Some(fresh),
                },
                Err(e) => eprintln!("duplicates: background check skipped ({e})"),
            }
        }
        let _ = tauri::Emitter::emit(&app, DUPLICATES_UPDATED, ());
    });
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
        let pairs = pair_up(&docs, None, MAX_SIMILARITY_DOCUMENTS);
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
        let pairs = pair_up(&docs, None, MAX_SIMILARITY_DOCUMENTS);
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
        let pairs = pair_up(&docs, None, MAX_SIMILARITY_DOCUMENTS);
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
        let pairs = pair_up(&docs, None, 0);
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
        assert!(pair_up(&docs, None, MAX_SIMILARITY_DOCUMENTS).is_empty());
    }

    // --- the identity fold ---------------------------------------------------------------

    const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    /// An index-only document with one anchor location, exactly as `register_pointer` leaves it.
    fn indexed(conn: &Connection, source_id: &str, project: &str, reviewed: bool) -> i64 {
        conn.execute(
            "INSERT INTO documents (source_type, source_id, title, project, tags, reviewed, \
                 vault_path, content_hash, source_state) \
             VALUES ('index_only', ?1, ?2, ?3, '[]', ?4, ?5, ?6, 'ok')",
            params![
                source_id,
                format!("t-{source_id}"),
                project,
                reviewed as i64,
                format!("idx://{source_id}"),
                format!("h-{source_id}")
            ],
        )
        .unwrap();
        let id = conn.last_insert_rowid();
        crate::locations::record(
            conn,
            id,
            &crate::locations::Location {
                source_id: source_id.to_string(),
                state: crate::index_only::SourceState::Ok,
                external_ref: Some(format!("/at/{source_id}")),
                source_modified_at: None,
                source_content_hash: None,
                source_parent_folder_id: None,
                source_parent_folder_name: None,
                anchor: true,
            },
        )
        .unwrap();
        id
    }

    fn open_store() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        (dir, conn)
    }

    fn places_of(conn: &Connection, id: i64) -> Vec<String> {
        crate::locations::list(conn, id)
            .unwrap()
            .into_iter()
            .map(|l| l.source_id)
            .collect()
    }

    #[test]
    fn two_routes_to_one_file_become_one_document_with_two_places() {
        // #711's headline case, and the one #703 structurally could not reach: account A owns the
        // file, account B reaches it through Shared with me. Under B's token `owned_by_me` is false,
        // so B's corpus keeps it deliberately — and neither account's listing can see the other's.
        // The provider's own file id can.
        let (_dir, conn) = open_store();
        let owner = indexed(&conn, "gdrive:a@x.com:1AbC", "Work", false);
        let recipient = indexed(&conn, "gdrive:swm:rootB:1AbC", "Unsorted", false);

        let fold = fold_by_identity(&conn).unwrap();
        assert_eq!(fold.folded, 1);
        assert_eq!(fold.retired, vec!["gdrive:swm:rootB:1AbC".to_string()]);

        // One document, two places — the recipient's row is not deleted, it is a location now.
        assert_eq!(
            places_of(&conn, owner),
            vec!["gdrive:a@x.com:1AbC", "gdrive:swm:rootB:1AbC"]
        );
        let gone: i64 = conn
            .query_row(
                "SELECT count(*) FROM documents WHERE id = ?1",
                params![recipient],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(gone, 0);
        // Idempotent: a second sweep of a settled store folds nothing.
        assert_eq!(fold_by_identity(&conn).unwrap().folded, 0);
    }

    #[test]
    fn a_fold_marks_the_manifest_stale_so_the_merged_filing_is_not_reverted() {
        // The blocker. `reconcile_index_only` runs this fold and then `index_only::reconcile_on_open`
        // against the same connection. The fold writes the merged filing to the DB and deletes the
        // doomed row; its caller strips only that doomed anchor from the manifest, so the SURVIVOR's
        // item there still carries its pre-fold project/tags/reviewed. The reconcile then reads that
        // file as truth and writes the stale values straight back over the merge — silently, and
        // with the row that held the correct filing already gone, so nothing could recover it.
        //
        // The stale flag is exactly the "mirror is ahead of the file" signal, and it makes the
        // reconcile repair the file from the mirror BEFORE applying it.
        let (_dir, conn) = open_store();
        let survivor = indexed(&conn, "gdrive:a@x.com:1AbC", "Unsorted", false);
        indexed(&conn, "gdrive:swm:rootB:1AbC", "Taxes", true);

        assert_eq!(
            crate::db::get_setting(&conn, "index_only_manifest_stale").unwrap(),
            None,
            "nothing has moved the mirror ahead of the file yet"
        );
        assert_eq!(fold_by_identity(&conn).unwrap().folded, 1);

        // The merge happened…
        let project: String = conn
            .query_row(
                "SELECT project FROM documents WHERE id = ?1",
                params![survivor],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(project, "Taxes", "the confirmed filing won the merge");
        // …and the manifest is now known to be behind it.
        assert_eq!(
            crate::db::get_setting(&conn, "index_only_manifest_stale").unwrap(),
            Some("1".into()),
            "so the next reconcile rewrites the file rather than applying a stale one"
        );
    }

    #[test]
    fn a_fold_that_changes_nothing_leaves_the_manifest_alone() {
        // The flag forces a full manifest rewrite at the next open, so an idempotent sweep over a
        // settled store must not set it — otherwise every launch pays for a write nothing needed.
        let (_dir, conn) = open_store();
        indexed(&conn, "gdrive:a@x.com:1AbC", "Work", false);
        indexed(&conn, "gdrive:b@x.com:9ZzZ", "Work", false);

        assert_eq!(fold_by_identity(&conn).unwrap().folded, 0);
        assert_eq!(
            crate::db::get_setting(&conn, "index_only_manifest_stale").unwrap(),
            None
        );
    }

    #[test]
    fn the_order_the_two_accounts_claimed_it_in_does_not_change_the_outcome() {
        // The property #703 lacked: it healed the pair only if the OWNER happened to claim the
        // shared root first, and left it forever if the recipient did. Same shares, different result
        // by claim order. Here the My Drive row wins either way, because the key has no asymmetry.
        for owner_first in [true, false] {
            let (_dir, conn) = open_store();
            let (owner, _other) = if owner_first {
                (
                    indexed(&conn, "gdrive:a@x.com:1AbC", "Work", false),
                    indexed(&conn, "gdrive:swm:rootB:1AbC", "Unsorted", false),
                )
            } else {
                let swm = indexed(&conn, "gdrive:swm:rootB:1AbC", "Unsorted", false);
                (indexed(&conn, "gdrive:a@x.com:1AbC", "Work", false), swm)
            };
            fold_by_identity(&conn).unwrap();
            let survivor: i64 = conn
                .query_row("SELECT id FROM documents", [], |r| r.get(0))
                .unwrap();
            assert_eq!(survivor, owner, "the My Drive row keeps being the document");
        }
    }

    #[test]
    fn one_file_under_two_shared_roots_and_a_shared_drive_all_collapse() {
        // The card's other two open cases, which are the same case once the key exists: a file under
        // two differently-owned shared-with-me roots (the #703 claim is keyed on the ROOT, so two
        // roots never met), and a shared-drive file that is also directly shared with an account.
        let (_dir, conn) = open_store();
        let sd = indexed(&conn, "gdrive:sd:drive9:1AbC", "Work", false);
        indexed(&conn, "gdrive:swm:rootB:1AbC", "Unsorted", false);
        indexed(&conn, "gdrive:swm:rootC:1AbC", "Unsorted", false);

        assert_eq!(fold_by_identity(&conn).unwrap().folded, 2);
        assert_eq!(places_of(&conn, sd).len(), 3, "three places, one document");
        let documents: i64 = conn
            .query_row("SELECT count(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(documents, 1);
    }

    #[test]
    fn a_fold_never_loses_a_filing() {
        // The doomed row is the one the user actually filed. `merge_classification` exists for
        // exactly this: a confirmed filing beats an unconfirmed one, whichever side happens to win
        // the anchor. Losing it would make deduplication feel like data loss, which is how a feature
        // that is right on the merits gets turned off.
        let (_dir, conn) = open_store();
        let owner = indexed(&conn, "gdrive:a@x.com:1AbC", "Unsorted", false);
        let recipient = indexed(&conn, "gdrive:swm:rootB:1AbC", "Taxes", true);
        conn.execute(
            "INSERT INTO tags (name, norm, kind) VALUES ('receipts', 'receipts', 'group')",
            [],
        )
        .unwrap();
        let tag = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO document_tags (document_id, tag_id) VALUES (?1, ?2)",
            params![recipient, tag],
        )
        .unwrap();

        fold_by_identity(&conn).unwrap();
        let (project, reviewed): (String, i64) = conn
            .query_row(
                "SELECT project, reviewed FROM documents WHERE id = ?1",
                params![owner],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(project, "Taxes", "the confirmed filing survived the fold");
        assert_eq!(reviewed, 1);
        let kept: i64 = conn
            .query_row(
                "SELECT count(*) FROM document_tags WHERE document_id = ?1 AND tag_id = ?2",
                params![owner, tag],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept, 1, "the label came across too");
    }

    #[test]
    fn documents_that_are_not_the_same_file_are_never_folded() {
        // The fold acts without asking, so it must only ever act on proof. Two different Drive files
        // (including a pair whose ids differ only where `_` would have been a LIKE wildcard), and a
        // provider with no global file id at all, all stay exactly as they were.
        let (_dir, conn) = open_store();
        indexed(&conn, "gdrive:a@x.com:1A_C4", "Work", false);
        indexed(&conn, "gdrive:a@x.com:1AbC4", "Work", false);
        indexed(&conn, "onedrive:a@x.com:01ITEM", "Work", false);
        indexed(&conn, "onedrive:b@x.com:01ITEM", "Work", false);
        assert_eq!(fold_by_identity(&conn).unwrap(), IdentityFold::default());
        let documents: i64 = conn
            .query_row("SELECT count(*) FROM documents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(documents, 4);
    }

    #[test]
    fn a_promoted_import_is_not_a_place_and_is_left_alone() {
        // A document promoted to a full local import keeps its `gdrive:` id as a claim marker but is
        // a stored file now, not a place a connector found one — so it has no locations and the fold
        // cannot see it. Folding it would delete the user's actual copy.
        let (_dir, conn) = open_store();
        let pointer = indexed(&conn, "gdrive:swm:rootB:1AbC", "Work", false);
        conn.execute(
            "INSERT INTO documents (source_type, source_id, title, project, tags, reviewed, \
                 vault_path, content_hash, source_state) \
             VALUES ('vault', 'gdrive:a@x.com:1AbC', 't', 'Work', '[]', 1, 'v/a.md', 'h-a', 'ok')",
            [],
        )
        .unwrap();
        assert_eq!(fold_by_identity(&conn).unwrap().folded, 0);
        assert_eq!(places_of(&conn, pointer).len(), 1);
    }

    #[test]
    fn an_incremental_pass_compares_only_what_arrived() {
        // What the background sweep runs after a sync lands something. Two documents already in the
        // library that duplicate each other are NOT re-reported — that pair was already answered —
        // while the arrival is compared against everything.
        let shared = long_body("identical opening");
        let docs = vec![
            doc(1, opening_key(&shared).as_deref(), Some(vec![1.0, 0.0])),
            doc(2, opening_key(&shared).as_deref(), Some(vec![1.0, 0.0])),
            doc(3, opening_key(&shared).as_deref(), Some(vec![1.0, 0.0])),
        ];
        let focus = std::collections::HashSet::from([3i64]);
        let pairs = pair_up(&docs, Some(&focus), MAX_SIMILARITY_DOCUMENTS);
        let ids: Vec<(i64, i64)> = pairs.iter().map(|p| (p.a, p.b)).collect();
        assert!(ids.contains(&(1, 3)) && ids.contains(&(2, 3)));
        assert!(!ids.contains(&(1, 2)), "the pair that predates the arrival");
        // Both signals still fire on the pairs it does look at — the focus narrows WHICH pairs are
        // considered, never how carefully.
        assert!(pairs.iter().all(|p| p.signals.same_opening));
        assert!(pairs.iter().all(|p| p.signals.similarity.is_some()));
    }

    #[test]
    fn an_incremental_pass_still_runs_where_a_full_one_would_decline() {
        // The budget is stated in documents and spent in comparisons, so one number bounds both
        // shapes. Three arrivals against a library past the document limit is 3n comparisons, which
        // costs nothing — refusing it would defeat the whole point of running incrementally.
        let docs: Vec<DupDoc> = (1..=40)
            .map(|id| doc(id, None, Some(vec![0.6, 0.8])))
            .collect();
        let budget = 10; // 45 comparisons allowed; all-vs-all wants 780, one arrival wants 40
        assert!(
            pair_up(&docs, None, budget).is_empty(),
            "the full sweep declines and says so"
        );
        let focus = std::collections::HashSet::from([1i64]);
        assert_eq!(
            pair_up(&docs, Some(&focus), budget).len(),
            39,
            "one arrival against the corpus is affordable"
        );
    }

    #[test]
    fn pairing_is_stable_between_two_scans_of_an_unchanged_store() {
        let shared = long_body("identical opening");
        let docs = vec![
            doc(7, opening_key(&shared).as_deref(), Some(vec![1.0, 0.0])),
            doc(3, opening_key(&shared).as_deref(), Some(vec![1.0, 0.0])),
            doc(5, opening_key(&shared).as_deref(), Some(vec![1.0, 0.0])),
        ];
        let first = pair_up(&docs, None, MAX_SIMILARITY_DOCUMENTS);
        let second = pair_up(&docs, None, MAX_SIMILARITY_DOCUMENTS);
        assert_eq!(first, second, "same input, same order");
        // Lower id first within every pair, so a pair has one identity however it was discovered.
        assert!(first.iter().all(|p| p.a < p.b));
    }
}
