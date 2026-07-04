// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Index-only foundation (Stage 3, board card 3 / spec §8.1) — the shared substrate for sources we
//! index but don't fully import (the cloud connectors and local-folder watch build on this; they
//! supply the per-source change *detection*, this module owns the source-agnostic *semantics*).
//!
//! **Index-only is a mode, not a source.** An index-only document flows through the SAME pipeline as
//! a fully-imported file — chunk, embed, stage in the review queue, resolve an `entity_id` — but
//! stores a metadata row + the leaf embeddings + a **pointer** (stable source id, external ref, the
//! source's last-modified + content hash) and a short summary, NOT the body bytes. The body is
//! fetched live on demand; only the summary stays readable offline (see [`crate::ingest`]'s
//! index-only storage branch).
//!
//! **The classification has nowhere portable to live**, because there is no Markdown file whose
//! front-matter could carry it. So it lives in a **portable, always-encrypted manifest** at the
//! data-home root, next to `pm.sqlite` and the entity-rules file — the `documents` rows are its
//! queryable mirror, restored from it on a Rebuild (which re-embeds each item from its summary,
//! since the body is remote). The manifest reuses the Markdown-at-rest crypto primitive under the
//! same vault subkey as the #61 rules file, separated only by a distinct AAD stem.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::{Error, Result};
use crate::ingest::{self, DocMeta, SourceMeta};
use crate::model_gateway::ModelGateway;
use crate::vault::crypto;
use crate::AppState;

/// Current manifest schema version.
const MANIFEST_SCHEMA: u32 = 1;
/// Filename of the encrypted index-only manifest, at the data-home root next to `pm.sqlite` and
/// `entities.pmrules`.
pub const MANIFEST_FILENAME: &str = "index-only.pmindex";
/// AAD stem binding the manifest ciphertext to its logical identity. MUST differ from the rules
/// file's stem (`"entities"`): both files share the same vault subkey + id, so the distinct stem is
/// the only thing stopping one file's ciphertext from authenticating against the other's reader.
const MANIFEST_AAD_STEM: &str = "index_only";

// --- the portable manifest (the encrypted source of truth for index-only classification) ---

/// The portable manifest shape. Integer ids are an index detail and deliberately absent — each item
/// carries its canonical project NAME and is re-resolved to an `entity_id` through the rules mirror
/// on reconcile (mirrors [`crate::entities::Rules`]).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Manifest {
    pub schema: u32,
    pub items: Vec<ManifestItem>,
}

/// One index-only item's portable truth: where its body lives (the pointer), its reachability state,
/// its offline summary, and its canonical classification.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestItem {
    /// The stable source id — the manifest key and the rename-survives identity.
    pub source_id: String,
    pub title: String,
    /// CANONICAL project name (never a variant) — re-resolved to an `entity_id` on reconcile.
    pub project: String,
    pub tags: Vec<String>,
    pub importance: Option<String>,
    pub reviewed: bool,
    pub last_activity: Option<String>,
    pub external_ref: Option<String>,
    pub source_modified_at: Option<String>,
    pub source_content_hash: Option<String>,
    /// `'ok' | 'source_missing' | 'unreachable'` — the first-class reachability state.
    pub source_state: String,
    pub stored_summary: Option<String>,
}

// --- encryption (reuses the Markdown-at-rest primitive, like the #61 rules file) ---

/// Always-on encryption for the manifest, reusing the Markdown subkey (XChaCha20-Poly1305 via
/// [`crate::vault::crypto`]). A near-copy of [`crate::entities::RulesCipher`] differing only in the
/// AAD stem, so the two encrypted files at the data-home root can never be confused for one another.
#[derive(Clone)]
pub struct ManifestCipher {
    vault_id: String,
    subkey: Zeroizing<[u8; 32]>,
}

impl ManifestCipher {
    /// Build from the vault id + the resolved 32-byte master (the same input the Markdown + rules
    /// ciphers use), deriving the Markdown subkey regardless of the vault's Markdown policy.
    pub fn from_master(vault_id: &str, master: &[u8; 32]) -> Self {
        Self {
            vault_id: vault_id.to_string(),
            subkey: crate::vault::markdown_subkey(master),
        }
    }

    fn encrypt(&self, manifest: &Manifest) -> Result<Vec<u8>> {
        let json = serde_json::to_vec(manifest)
            .map_err(|e| Error::Other(format!("encode manifest: {e}")))?;
        crypto::encrypt(&json, &self.subkey, &self.vault_id, MANIFEST_AAD_STEM)
    }

    fn decrypt(&self, bytes: &[u8]) -> Result<Manifest> {
        let plain = crypto::decrypt(bytes, &self.subkey, &self.vault_id, MANIFEST_AAD_STEM)?;
        serde_json::from_slice(&plain).map_err(|e| Error::Other(format!("decode manifest: {e}")))
    }
}

// --- file IO (atomic, returns prior bytes for rollback — mirrors entities) ---

/// Path to the manifest at the vault root (next to `pm.sqlite`, one level up from `vault/`).
pub fn manifest_path(vault_root: &Path) -> PathBuf {
    vault_root.join(MANIFEST_FILENAME)
}

/// Read + decrypt the manifest, or `None` if it doesn't exist yet. A decrypt failure surfaces so the
/// caller can self-heal from the mirror.
pub fn read_manifest(vault_root: &Path, cipher: &ManifestCipher) -> Result<Option<Manifest>> {
    match std::fs::read(manifest_path(vault_root)) {
        Ok(bytes) => Ok(Some(cipher.decrypt(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Write the manifest atomically (temp + rename), returning the prior raw bytes (empty if none) so a
/// caller can restore it should a surrounding DB transaction fail to commit.
pub fn write_manifest(
    vault_root: &Path,
    cipher: &ManifestCipher,
    manifest: &Manifest,
) -> Result<Vec<u8>> {
    let path = manifest_path(vault_root);
    let prior = std::fs::read(&path).unwrap_or_default();
    let bytes = cipher.encrypt(manifest)?;
    let tmp = path.with_extension("pmindex.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(prior)
}

// --- mirror <-> file reconciliation ---

/// Whether the store has any index-only documents (so a no-op vault never grows an empty manifest).
fn has_index_only(conn: &Connection) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM documents WHERE source_type = ?1",
        params![ingest::SOURCE_TYPE_INDEX_ONLY],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Serialize the index-only documents in the DB mirror into manifest items (canonical names; integer
/// ids stay out of the file).
fn mirror_items(conn: &Connection) -> Result<Vec<ManifestItem>> {
    let mut stmt = conn.prepare(
        "SELECT source_id, title, project, tags, importance, reviewed, last_activity, \
                external_ref, source_modified_at, source_content_hash, source_state, stored_summary \
         FROM documents \
         WHERE source_type = ?1 AND source_id IS NOT NULL \
         ORDER BY source_id",
    )?;
    let rows = stmt
        .query_map(params![ingest::SOURCE_TYPE_INDEX_ONLY], |r| {
            let tags_json: String = r.get(3)?;
            Ok(ManifestItem {
                source_id: r.get(0)?,
                title: r.get(1)?,
                project: r.get(2)?,
                tags: serde_json::from_str(&tags_json).unwrap_or_default(),
                importance: r.get(4)?,
                reviewed: r.get::<_, i64>(5)? != 0,
                last_activity: r.get(6)?,
                external_ref: r.get(7)?,
                source_modified_at: r.get(8)?,
                source_content_hash: r.get(9)?,
                source_state: r.get(10)?,
                stored_summary: r.get(11)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// The manifest to persist: the DB mirror UNIONED with any items already on disk whose source is
/// absent from the DB (e.g. awaiting a Rebuild to re-embed from their summary) — so a classification
/// is never silently dropped (a property the card insists on: a source going away must never read as
/// data loss). The DB row wins when both carry the same source id.
fn merged_manifest(
    conn: &Connection,
    vault_root: &Path,
    cipher: &ManifestCipher,
) -> Result<Manifest> {
    let mut items = mirror_items(conn)?;
    let present: std::collections::HashSet<String> =
        items.iter().map(|i| i.source_id.clone()).collect();
    if let Ok(Some(existing)) = read_manifest(vault_root, cipher) {
        for it in existing.items {
            if !present.contains(&it.source_id) {
                items.push(it);
            }
        }
    }
    items.sort_by(|a, b| a.source_id.cmp(&b.source_id));
    Ok(Manifest {
        schema: MANIFEST_SCHEMA,
        items,
    })
}

/// Push the DB mirror (merged with any awaiting-Rebuild file items) to the encrypted manifest,
/// returning the prior bytes for rollback. The single write path: the post-change sync and the
/// truth-writer manifest arm both go through this.
pub fn write_synced(
    conn: &Connection,
    vault_root: &Path,
    cipher: &ManifestCipher,
) -> Result<Vec<u8>> {
    let manifest = merged_manifest(conn, vault_root, cipher)?;
    write_manifest(vault_root, cipher, &manifest)
}

/// Drop one source from the encrypted manifest — the "promote to full import" strip. Once a document
/// is materialised as a full local import ([`crate::ingest::promote_spreadsheet`]) its `source_type` is
/// no longer `index_only`, so [`mirror_items`] stops emitting it; but the file still lists it, and
/// [`merged_manifest`]'s DB-∪-file union would resurrect it as a ghost on the next [`write_synced`]. So
/// the promote path calls this to remove it from the file DIRECTLY — a surgical read-modify-write, NOT
/// `write_synced` (which would re-merge it straight back). Idempotent: a missing manifest, or a source
/// already absent, is a clean no-op. The DB row is deliberately left untouched — it IS the promoted
/// document now. Call this AFTER the promote's DB transaction commits, so the mirror already excludes
/// the id and no racing sync can re-add it.
pub fn forget_source(vault_root: &Path, cipher: &ManifestCipher, source_id: &str) -> Result<()> {
    let Some(mut manifest) = read_manifest(vault_root, cipher)? else {
        return Ok(());
    };
    let before = manifest.items.len();
    manifest.items.retain(|it| it.source_id != source_id);
    if manifest.items.len() != before {
        write_manifest(vault_root, cipher, &manifest)?;
    }
    Ok(())
}

/// Apply the portable classification in `manifest` onto the matching index-only rows (the file is the
/// source of truth for classification), re-resolving each item's `entity_id` from its canonical name
/// through the rules mirror. Rows present in the file but absent from the DB are left untouched —
/// they await a Rebuild, which re-embeds them from their summary (we can't embed here).
fn apply_classification(conn: &Connection, manifest: &Manifest) -> Result<()> {
    for it in &manifest.items {
        let exists = conn
            .query_row(
                "SELECT 1 FROM documents WHERE source_id = ?1 AND source_type = ?2",
                params![it.source_id, ingest::SOURCE_TYPE_INDEX_ONLY],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !exists {
            continue;
        }
        let entity_id = crate::entities::resolve_project(conn, &it.project, true)?;
        let tags_json = serde_json::to_string(&it.tags)
            .map_err(|e| Error::Other(format!("encode tags: {e}")))?;
        conn.execute(
            "UPDATE documents SET project = ?2, tags = ?3, importance = ?4, reviewed = ?5, \
                    last_activity = ?6, external_ref = ?7, source_modified_at = ?8, \
                    source_content_hash = ?9, source_state = ?10, stored_summary = ?11, \
                    title = ?12, entity_id = ?13 \
             WHERE source_id = ?1 AND source_type = 'index_only'",
            params![
                it.source_id,
                it.project,
                tags_json,
                it.importance,
                it.reviewed as i64,
                it.last_activity,
                it.external_ref,
                it.source_modified_at,
                it.source_content_hash,
                it.source_state,
                it.stored_summary,
                it.title,
                entity_id,
            ],
        )?;
    }
    Ok(())
}

/// Reconcile the encrypted manifest with the DB mirror at session open. The file is the portable
/// truth for index-only CLASSIFICATION (project/tags/state/pointer/summary) — but NOT the embeddings,
/// which live only in the DB and come back only via a Rebuild. So: when the file is present, apply
/// its classification onto existing rows (a Rebuild restores any rows it still lacks); when ABSENT or
/// UNDECRYPTABLE, (re)write it from the mirror if there is anything to persist. Runs AFTER the entity
/// rules reconcile at boot, so each item's project resolves through the rebuilt aliases.
pub fn reconcile_on_open(
    conn: &Connection,
    vault_root: &Path,
    cipher: &ManifestCipher,
) -> Result<()> {
    match read_manifest(vault_root, cipher) {
        Ok(Some(manifest)) => {
            let tx = conn.unchecked_transaction()?;
            apply_classification(&tx, &manifest)?;
            tx.commit()?;
            Ok(())
        }
        Ok(None) => {
            if has_index_only(conn)? {
                write_synced(conn, vault_root, cipher)?;
            }
            Ok(())
        }
        Err(e) => {
            eprintln!("index_only: manifest unreadable ({e}); rewriting it from the DB mirror");
            if has_index_only(conn)? {
                write_synced(conn, vault_root, cipher)?;
            }
            Ok(())
        }
    }
}

// --- pointer-ingest ---

/// A request to index a source by pointer only: its body is fetched once for embedding + summary but
/// never persisted. The body is supplied by the caller (a connector, or the dev affordance); the
/// detection of WHEN to call this lives in the connector cards, not here.
#[derive(Clone)]
pub struct PointerInput {
    pub source_id: String,
    pub title: String,
    pub external_ref: Option<String>,
    pub source_modified_at: Option<String>,
    pub source_content_hash: Option<String>,
    pub body: String,
    /// The source folder this item was found in (Drive today), for sorting-review context only. Rides
    /// alongside the body but is never chunked or embedded. `None` for sources with no folder concept.
    pub source_parent_folder_id: Option<String>,
    pub source_parent_folder_name: Option<String>,
}

/// Length (chars) of the offline summary kept for an index-only item — short enough to stay a
/// pointer, long enough to recognise + keyword-find the item. The full body is a live fetch.
const SUMMARY_CHARS: usize = 500;

/// The readable-offline summary: the first ~[`SUMMARY_CHARS`] characters of the body, trimmed.
fn summarize(body: &str) -> String {
    let trimmed = body.trim();
    let mut s: String = trimmed.chars().take(SUMMARY_CHARS).collect();
    if trimmed.chars().count() > SUMMARY_CHARS {
        s.push('…');
    }
    s
}

/// Content hash for an index-only document: the stable SOURCE id folded into the indexed text's
/// digest. Two different sources that happen to share identical text stay TWO items (index-only
/// dedup is by source id, never by content) and never collide on `documents.content_hash`'s UNIQUE
/// constraint — unlike a vault import, where identical content IS the same document. Also used by
/// the note→vault ingest ([`crate::ingest::ingest_note_document`]), which dedups by `note:<widget_id>`
/// for the same reason, so two notes with identical text stay distinct.
pub(crate) fn pointer_content_hash(source_id: &str, indexed_text: &str) -> String {
    ingest::hex_digest(format!("{source_id}\u{0}{indexed_text}").as_bytes())
}

/// Chunk `indexed_text`, embed its leaves, and store it as the index-only document described by
/// `meta` (no Markdown file). `meta.content_hash` must already be [`pointer_content_hash`] of the
/// same text. Shared by register + Rebuild so the chunk → embed → index path lives once; the body
/// bytes are never persisted (see [`crate::ingest::index_document`]'s index-only branch).
fn embed_and_index(
    state: &AppState,
    gateway: &ModelGateway<'_>,
    indexed_text: &str,
    meta: &DocMeta,
) -> Result<ingest::Document> {
    let chunks = ingest::split_document(gateway, indexed_text, &meta.title, &meta.content_hash)?;
    let texts = ingest::leaf_embed_texts(&chunks);
    let embeddings = gateway.embed_documents(&texts)?;
    ingest::check_embeddings(&embeddings, texts.len(), gateway.embedder().dimension)?;
    ingest::index_document(state, meta, &chunks, &embeddings, None, None)
}

/// Register a source as an index-only document: chunk + embed its body (fetched once), store the leaf
/// embeddings + a short summary + the pointer, and persist its classification to the encrypted
/// manifest. Writes NO Markdown vault file. The new document enters the review queue (project
/// Unsorted, `reviewed = false`), exactly like a freshly imported file — index-only is a mode, not a
/// separate pipeline.
pub fn register_pointer(
    state: &AppState,
    gateway: &ModelGateway<'_>,
    vault_root: &Path,
    cipher: &ManifestCipher,
    input: PointerInput,
) -> Result<ingest::Document> {
    let body = input.body.trim();
    if body.is_empty() {
        return Err(Error::Other(
            "an index-only source has no extractable text".into(),
        ));
    }
    // If this source was already promoted to a full local import, a non-index-only document owns its
    // id — never re-ingest a second, index-only copy. `read_item_state`'s widened match already turns
    // the usual sync re-observation into a `Noop`; this closes the tiny gather-then-apply window where
    // a promote lands mid-sync, from any caller that still reaches `IngestNew`.
    {
        let conn = state.conn()?;
        if let Some(existing) = conn
            .query_row(
                "SELECT id FROM documents WHERE source_id = ?1 AND source_type != ?2",
                params![input.source_id, ingest::SOURCE_TYPE_INDEX_ONLY],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
        {
            return ingest::load_document(&conn, existing);
        }
    }
    let now = {
        let conn = state.conn()?;
        ingest::iso_now(&conn)?
    };
    let meta = DocMeta {
        source_path: None,
        // Synthetic NOT-NULL-UNIQUE sentinel: there is no real Markdown file, identity is keyed on
        // `source_id`, and the rebuild vault-walk skips it (no `.md`/`.pmenc` extension).
        vault_path: format!("idx://{}", input.source_id),
        title: input.title.clone(),
        // The Markdown content hash drives chunk uids + dedupe; distinct from the SOURCE's content
        // hash (the pointer), which a connector supplies for change detection.
        content_hash: pointer_content_hash(&input.source_id, body),
        ext: None,
        byte_size: None,
        created_at: input.source_modified_at.clone(),
        ingested_at: now.clone(),
        project: "Unsorted".into(),
        tags: Vec::new(),
        importance: None,
        reviewed: false,
        last_activity: Some(now),
        source: SourceMeta {
            source_type: ingest::SOURCE_TYPE_INDEX_ONLY.into(),
            source_state: ingest::SOURCE_STATE_OK.into(),
            source_id: Some(input.source_id.clone()),
            external_ref: input.external_ref,
            source_modified_at: input.source_modified_at,
            source_content_hash: input.source_content_hash,
            stored_summary: Some(summarize(body)),
            source_parent_folder_id: input.source_parent_folder_id,
            source_parent_folder_name: input.source_parent_folder_name,
        },
    };
    let document = embed_and_index(state, gateway, body, &meta)?;
    // Persist the new item's classification to the portable manifest — a best-effort skip would lose
    // it on the next Rebuild, so a failure here propagates.
    {
        let conn = state.conn()?;
        write_synced(&conn, vault_root, cipher)?;
    }
    Ok(document)
}

/// Restore the index-only documents from the encrypted manifest after a [`crate::ingest::rebuild`]
/// has cleared the store — they have no Markdown file, so the rebuild vault-walk skipped them. The
/// bodies are remote and not held, so each item is re-embedded from its **stored summary**: a
/// degraded but honest offline index that stays findable + fully classified; a later connector
/// "refresh" re-fetches + re-embeds the full body. Reuses the already-warmed `gateway` and never
/// resizes `chunk_vec` (the vault loop sized it). An item with no summary is skipped (nothing to
/// embed) but kept in the manifest for that later refresh. Returns `(restored, failed)`.
pub fn rebuild_from_manifest(
    state: &AppState,
    gateway: &ModelGateway<'_>,
    vault_root: &Path,
    cipher: &ManifestCipher,
) -> Result<(usize, usize)> {
    let manifest = match read_manifest(vault_root, cipher)? {
        Some(m) => m,
        None => return Ok((0, 0)),
    };
    let (mut restored, mut failed) = (0usize, 0usize);
    for item in &manifest.items {
        match restore_item(state, gateway, item) {
            Ok(()) => restored += 1,
            Err(e) => {
                failed += 1;
                eprintln!("index_only: could not rebuild '{}': {e}", item.source_id);
            }
        }
    }
    // The mirror has the restored rows again; resync (merge preserves any item that had no summary).
    state.sync_index_only();
    Ok((restored, failed))
}

/// Re-create one index-only document row from its manifest item, re-embedding from the summary.
fn restore_item(state: &AppState, gateway: &ModelGateway<'_>, item: &ManifestItem) -> Result<()> {
    let summary = item
        .stored_summary
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();
    if summary.is_empty() {
        return Err(Error::Other("no stored summary to re-embed".into()));
    }
    let ingested_at = {
        let conn = state.conn()?;
        ingest::iso_now(&conn)?
    };
    let meta = DocMeta {
        source_path: None,
        vault_path: format!("idx://{}", item.source_id),
        title: item.title.clone(),
        content_hash: pointer_content_hash(&item.source_id, &summary),
        ext: None,
        byte_size: None,
        created_at: item.source_modified_at.clone(),
        ingested_at: ingested_at.clone(),
        project: item.project.clone(),
        tags: item.tags.clone(),
        importance: item.importance.clone(),
        reviewed: item.reviewed,
        last_activity: item.last_activity.clone().or(Some(ingested_at)),
        source: SourceMeta {
            source_type: ingest::SOURCE_TYPE_INDEX_ONLY.into(),
            source_state: item.source_state.clone(),
            source_id: Some(item.source_id.clone()),
            external_ref: item.external_ref.clone(),
            source_modified_at: item.source_modified_at.clone(),
            source_content_hash: item.source_content_hash.clone(),
            stored_summary: item.stored_summary.clone(),
            // The portable manifest doesn't carry the parent folder, so a Rebuild-from-manifest can't
            // restore it — the folder tag re-populates on the next Drive refresh (no backfill here).
            source_parent_folder_id: None,
            source_parent_folder_name: None,
        },
    };
    embed_and_index(state, gateway, &summary, &meta)?;
    Ok(())
}

// --- observe-and-react: the source-agnostic change-event semantics ---
//
// PM owns the vault as a single writer, but it does NOT own an index-only source — it is a read-only
// OBSERVER reacting to an external writer it cannot lock (the user, a cloud web UI, another sync
// daemon). So the concurrency model inverts: no lock applies here, and the vault's single-writer
// baton-pass must never be reached for. Connectors (the cloud + local-folder cards) supply the
// per-source *detection*; this module owns the *semantics* — the pure [`react`] reducer below, plus
// the [`apply_actions`] executor that performs its decisions.

/// The reachability of an index-only item's source.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SourceState {
    /// Reachable — the body can be fetched (the default).
    Ok,
    /// The item was deleted at the source: kept findable (metadata + embedding stay), body flagged
    /// unretrievable. A soft state, never a hard drop.
    SourceMissing,
    /// The whole source can't be reached (expired auth, unmounted drive). First-class, distinct from
    /// a per-item deletion.
    Unreachable,
}

impl SourceState {
    /// The `documents.source_state` string this maps to.
    pub fn as_str(self) -> &'static str {
        match self {
            SourceState::Ok => ingest::SOURCE_STATE_OK,
            SourceState::SourceMissing => ingest::SOURCE_STATE_MISSING,
            SourceState::Unreachable => ingest::SOURCE_STATE_UNREACHABLE,
        }
    }

    /// Parse a stored `source_state`; anything unrecognised reads as `Ok` (the safe default).
    pub fn from_db(s: &str) -> Self {
        if s == ingest::SOURCE_STATE_MISSING {
            SourceState::SourceMissing
        } else if s == ingest::SOURCE_STATE_UNREACHABLE {
            SourceState::Unreachable
        } else {
            SourceState::Ok
        }
    }
}

/// The persisted state of an index-only item, as the reducer needs to see it. `None` to the reducer
/// means "never seen this source id before".
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ItemState {
    pub source_id: String,
    pub source_modified_at: Option<String>,
    pub source_content_hash: Option<String>,
    pub source_state: SourceState,
}

/// A change reported by some source, normalised. Connectors translate their native events (a notify
/// fs event, the Drive changes feed, the OneDrive delta query) into this; the reducer stays
/// source-agnostic. Items are keyed by a **stable source id** — a connector that namespaces its ids
/// as `<source>:<localid>` lets [`SourceFailure`](ChangeEvent::SourceFailure) group an account's
/// items. A naive watcher reporting a rename as delete-plus-add would strip classification; stable-id
/// keying + [`Rename`](ChangeEvent::Rename) is what prevents that.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ChangeEvent {
    /// A new item appeared at the source.
    Add {
        source_id: String,
        modified_at: Option<String>,
    },
    /// An item changed. `new_content_hash` is the SOURCE's content hash (the pointer), `None` when a
    /// watcher fires mid-write before the file is stable.
    Update {
        source_id: String,
        modified_at: Option<String>,
        new_content_hash: Option<String>,
    },
    /// An item was deleted at the source.
    Delete { source_id: String },
    /// An item was renamed/moved — the stable id is unchanged, only its external ref moved.
    Rename {
        source_id: String,
        new_external_ref: Option<String>,
    },
    /// A whole source became unreachable (expired OAuth, unmounted drive, revoked permission).
    SourceFailure { source: String },
}

/// What the executor should do. Pure data — [`react`] returns these; [`apply_actions`] performs them.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Action {
    /// Ingest a never-seen item (the connector supplies its body).
    IngestNew { source_id: String },
    /// The embedding is stale — re-embed from the connector-supplied body; classification persists.
    ReEmbed {
        source_id: String,
        new_content_hash: String,
    },
    /// A watcher fired mid-write (no stable hash yet): re-stat after a quiet interval and only
    /// proceed once mtime/size settle. The connector re-fires a hashed `Update` when stable.
    DebounceThenCheck {
        source_id: String,
        modified_at: Option<String>,
    },
    /// Transition one item's reachability state.
    SetState {
        source_id: String,
        state: SourceState,
    },
    /// Transition every item of a source (the `SourceFailure` fan-out).
    SetSourceState { source: String, state: SourceState },
    /// A rename/move kept the identity; update the shown external ref.
    UpdatePointer {
        source_id: String,
        external_ref: Option<String>,
    },
    /// Nothing to do (an idempotent re-fire, or a touch that didn't change the content).
    Noop,
}

/// Decide what a change means for an index-only item — the heart of observe-and-react. **PURE**: no
/// DB, no IO, no clock; the same `(event, current)` always yields the same actions, which is what
/// makes it exhaustively unit-testable. `current` is the item's persisted state, or `None` if its
/// source id has never been seen.
pub fn react(event: ChangeEvent, current: Option<&ItemState>) -> Vec<Action> {
    match event {
        ChangeEvent::Add { source_id, .. } => match current {
            // Never seen → ingest + stage like any new item.
            None => vec![Action::IngestNew { source_id }],
            // Present and reachable → idempotent against a watcher double-fire; never re-stage a
            // classified item.
            Some(s) if s.source_state == SourceState::Ok => vec![Action::Noop],
            // It came back (was missing/unreachable) → mark reachable; a following Update re-embeds
            // if the content actually changed.
            Some(_) => vec![Action::SetState {
                source_id,
                state: SourceState::Ok,
            }],
        },
        ChangeEvent::Update {
            source_id,
            modified_at,
            new_content_hash,
        } => match (current, new_content_hash) {
            // Unknown item → treat as an Add.
            (None, _) => vec![Action::IngestNew { source_id }],
            // No stable hash yet (watcher mid-write) → debounce, re-stat, proceed only once stable.
            (Some(_), None) => vec![Action::DebounceThenCheck {
                source_id,
                modified_at,
            }],
            // Hash differs → the embedding is stale; re-embed only (classification persists). If the
            // item was missing/unreachable, it has effectively come back, so clear that first.
            (Some(s), Some(h)) if Some(&h) != s.source_content_hash.as_ref() => {
                let mut acts = Vec::new();
                if s.source_state != SourceState::Ok {
                    acts.push(Action::SetState {
                        source_id: source_id.clone(),
                        state: SourceState::Ok,
                    });
                }
                acts.push(Action::ReEmbed {
                    source_id,
                    new_content_hash: h,
                });
                acts
            }
            // Same hash → a touch, not an edit.
            (Some(_), Some(_)) => vec![Action::Noop],
        },
        // Soft delete: keep metadata + embedding (still findable), flag the body unretrievable. Never
        // a hard drop. Unknown id → nothing to mark.
        ChangeEvent::Delete { source_id } => match current {
            Some(_) => vec![Action::SetState {
                source_id,
                state: SourceState::SourceMissing,
            }],
            None => vec![Action::Noop],
        },
        // Stable id preserves identity + classification; only the external ref moved. If it was
        // missing/unreachable, a rename means it's reachable again.
        ChangeEvent::Rename {
            source_id,
            new_external_ref,
        } => match current {
            None => vec![Action::Noop],
            Some(s) => {
                let mut acts = Vec::new();
                if s.source_state != SourceState::Ok {
                    acts.push(Action::SetState {
                        source_id: source_id.clone(),
                        state: SourceState::Ok,
                    });
                }
                acts.push(Action::UpdatePointer {
                    source_id,
                    external_ref: new_external_ref,
                });
                acts
            }
        },
        // The whole source is unreachable — fan out to every item of it, never a per-item deletion.
        ChangeEvent::SourceFailure { source } => vec![Action::SetSourceState {
            source,
            state: SourceState::Unreachable,
        }],
    }
}

// --- the executor: perform a reducer's decisions against the store + manifest ---

/// The connector-supplied current content for the item an `IngestNew`/`ReEmbed` concerns, asserting
/// its source id matches the action's (the connector fetches the body before applying the change).
fn require_fetched<'a>(
    fetched: Option<&'a PointerInput>,
    source_id: &str,
) -> Result<&'a PointerInput> {
    match fetched {
        Some(f) if f.source_id == source_id => Ok(f),
        Some(_) => Err(Error::Other(
            "the fetched content's source id does not match the change".into(),
        )),
        None => Err(Error::Other(
            "this change needs the item's body, but none was supplied".into(),
        )),
    }
}

/// Re-embed an existing index-only item from freshly-fetched content (its source changed), keeping
/// its classification (project/tags/importance/reviewed/entity) — only the chunks, pointer hashes,
/// summary, and title are replaced. Embeds OFF the DB lock (like ingest), then swaps the chunks in
/// one short transaction.
fn reembed_item(
    state: &AppState,
    gateway: &ModelGateway<'_>,
    source_id: &str,
    new_content_hash: &str,
    input: &PointerInput,
) -> Result<()> {
    let body = input.body.trim();
    if body.is_empty() {
        return Err(Error::Other("re-embed received an empty body".into()));
    }
    // If this source was promoted to a full local import, it is no longer index-only. Do NOT re-embed
    // (that would clobber the imported content with an index-only summary). Just advance its tracked
    // pointer so the next sync sees no change — the local copy is the user's snapshot; they re-import
    // to pull a fresh version. Checked before the (expensive) split/embed below.
    {
        let conn = state.conn()?;
        let is_index_only = conn
            .query_row(
                "SELECT 1 FROM documents WHERE source_id = ?1 AND source_type = 'index_only'",
                params![source_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !is_index_only {
            conn.execute(
                "UPDATE documents SET source_content_hash = ?2, source_modified_at = ?3 \
                 WHERE source_id = ?1",
                params![source_id, new_content_hash, input.source_modified_at],
            )?;
            return Ok(());
        }
    }
    let content_hash = pointer_content_hash(source_id, body);
    let chunks = ingest::split_document(gateway, body, &input.title, &content_hash)?;
    let texts = ingest::leaf_embed_texts(&chunks);
    let embeddings = gateway.embed_documents(&texts)?;
    ingest::check_embeddings(&embeddings, texts.len(), gateway.embedder().dimension)?;
    let summary = summarize(body);

    let mut conn = state.conn()?;
    let tx = conn.transaction()?;
    let doc_id: i64 = tx.query_row(
        "SELECT id FROM documents WHERE source_id = ?1 AND source_type = 'index_only'",
        params![source_id],
        |r| r.get(0),
    )?;
    ingest::replace_chunks(&tx, doc_id, &chunks, &embeddings, true, Some(&summary))?;
    tx.execute(
        "UPDATE documents SET content_hash = ?2, source_content_hash = ?3, source_modified_at = ?4, \
                stored_summary = ?5, title = ?6, source_state = 'ok' \
         WHERE id = ?1",
        params![
            doc_id,
            content_hash,
            new_content_hash,
            input.source_modified_at,
            summary,
            input.title,
        ],
    )?;
    tx.commit()?;
    Ok(())
}

/// Set one index-only item's reachability state (`documents.source_state`).
fn set_item_state(conn: &Connection, source_id: &str, state: SourceState) -> Result<()> {
    conn.execute(
        "UPDATE documents SET source_state = ?2 WHERE source_id = ?1 AND source_type = 'index_only'",
        params![source_id, state.as_str()],
    )?;
    Ok(())
}

/// Set the reachability state of EVERY item of a source — the source-failure fan-out. Matches an
/// exact `source_id`, or any id namespaced as `<source>:<localid>` (the convention a connector
/// follows so an account's items move together). Never deletes: a failed source must not read as a
/// deletion.
fn set_source_state(conn: &Connection, source: &str, state: SourceState) -> Result<()> {
    conn.execute(
        "UPDATE documents SET source_state = ?2 \
         WHERE source_type = 'index_only' AND (source_id = ?1 OR source_id LIKE ?1 || ':%')",
        params![source, state.as_str()],
    )?;
    Ok(())
}

/// Update one index-only item's external ref (a rename/move kept the stable id, so its
/// classification + chunks are untouched).
fn set_external_ref(conn: &Connection, source_id: &str, external_ref: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE documents SET external_ref = ?2 WHERE source_id = ?1 AND source_type = 'index_only'",
        params![source_id, external_ref],
    )?;
    Ok(())
}

/// Perform the [`Action`]s [`react`] produced, against the live store + manifest. `fetched` is the
/// connector-supplied current content for the item an `IngestNew`/`ReEmbed` concerns (the dev
/// affordance passes a pasted body). State/pointer transitions are short synchronous DB writes; a
/// re-embed runs the sidecar OFF the DB lock, exactly like ingest. The manifest is re-synced once at
/// the end if anything changed (an `IngestNew` syncs itself via [`register_pointer`]).
pub fn apply_actions(
    state: &AppState,
    gateway: &ModelGateway<'_>,
    vault_root: &Path,
    cipher: &ManifestCipher,
    actions: &[Action],
    fetched: Option<&PointerInput>,
) -> Result<()> {
    let mut changed = false;
    for action in actions {
        match action {
            Action::IngestNew { source_id } => {
                let input = require_fetched(fetched, source_id)?.clone();
                register_pointer(state, gateway, vault_root, cipher, input)?;
            }
            Action::ReEmbed {
                source_id,
                new_content_hash,
            } => {
                let input = require_fetched(fetched, source_id)?;
                reembed_item(state, gateway, source_id, new_content_hash, input)?;
                changed = true;
            }
            // Nothing to do in the substrate: the connector re-stats and re-fires a hashed Update
            // once the source is stable. This arm is where that decision lands.
            Action::DebounceThenCheck { .. } => {}
            Action::SetState {
                source_id,
                state: new_state,
            } => {
                let conn = state.conn()?;
                set_item_state(&conn, source_id, *new_state)?;
                changed = true;
            }
            Action::SetSourceState {
                source,
                state: new_state,
            } => {
                let conn = state.conn()?;
                set_source_state(&conn, source, *new_state)?;
                changed = true;
            }
            Action::UpdatePointer {
                source_id,
                external_ref,
            } => {
                let conn = state.conn()?;
                set_external_ref(&conn, source_id, external_ref.as_deref())?;
                changed = true;
            }
            Action::Noop => {}
        }
    }
    if changed {
        let conn = state.conn()?;
        write_synced(&conn, vault_root, cipher)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MASTER: [u8; 32] = [7u8; 32];
    const VAULT_ID: &str = "vault-abc";

    // --- reducer (pure) ---

    fn st(state: SourceState, hash: Option<&str>) -> ItemState {
        ItemState {
            source_id: "s1".into(),
            source_modified_at: None,
            source_content_hash: hash.map(String::from),
            source_state: state,
        }
    }

    #[test]
    fn add_ingests_new_but_is_idempotent_and_recovers() {
        assert_eq!(
            react(
                ChangeEvent::Add {
                    source_id: "s1".into(),
                    modified_at: None
                },
                None
            ),
            vec![Action::IngestNew {
                source_id: "s1".into()
            }]
        );
        // A watcher double-fire on a present, reachable item must not re-stage it.
        assert_eq!(
            react(
                ChangeEvent::Add {
                    source_id: "s1".into(),
                    modified_at: None
                },
                Some(&st(SourceState::Ok, Some("h1")))
            ),
            vec![Action::Noop]
        );
        // A previously-missing item reappearing comes back to reachable.
        assert_eq!(
            react(
                ChangeEvent::Add {
                    source_id: "s1".into(),
                    modified_at: None
                },
                Some(&st(SourceState::SourceMissing, Some("h1")))
            ),
            vec![Action::SetState {
                source_id: "s1".into(),
                state: SourceState::Ok
            }]
        );
    }

    fn update(hash: Option<&str>) -> ChangeEvent {
        ChangeEvent::Update {
            source_id: "s1".into(),
            modified_at: Some("2026-06-26T00:00:00Z".into()),
            new_content_hash: hash.map(String::from),
        }
    }

    #[test]
    fn update_debounces_without_a_hash_and_re_embeds_only_on_a_real_change() {
        // Unknown item → treat as an Add.
        assert_eq!(
            react(update(Some("h1")), None),
            vec![Action::IngestNew {
                source_id: "s1".into()
            }]
        );
        // No hash (watcher mid-write) → debounce, don't touch the index.
        assert_eq!(
            react(update(None), Some(&st(SourceState::Ok, Some("h1")))),
            vec![Action::DebounceThenCheck {
                source_id: "s1".into(),
                modified_at: Some("2026-06-26T00:00:00Z".into())
            }]
        );
        // Same hash → a touch, not an edit.
        assert_eq!(
            react(update(Some("h1")), Some(&st(SourceState::Ok, Some("h1")))),
            vec![Action::Noop]
        );
        // Changed hash → re-embed only (classification persists).
        assert_eq!(
            react(update(Some("h2")), Some(&st(SourceState::Ok, Some("h1")))),
            vec![Action::ReEmbed {
                source_id: "s1".into(),
                new_content_hash: "h2".into()
            }]
        );
        // Changed hash on a missing item → recover, then re-embed.
        assert_eq!(
            react(
                update(Some("h2")),
                Some(&st(SourceState::SourceMissing, Some("h1")))
            ),
            vec![
                Action::SetState {
                    source_id: "s1".into(),
                    state: SourceState::Ok
                },
                Action::ReEmbed {
                    source_id: "s1".into(),
                    new_content_hash: "h2".into()
                }
            ]
        );
    }

    #[test]
    fn delete_is_soft_and_rename_preserves_identity() {
        // Delete → soft source-missing, never a hard drop.
        assert_eq!(
            react(
                ChangeEvent::Delete {
                    source_id: "s1".into()
                },
                Some(&st(SourceState::Ok, Some("h1")))
            ),
            vec![Action::SetState {
                source_id: "s1".into(),
                state: SourceState::SourceMissing
            }]
        );
        // Delete of an unknown id → nothing.
        assert_eq!(
            react(
                ChangeEvent::Delete {
                    source_id: "s1".into()
                },
                None
            ),
            vec![Action::Noop]
        );
        // Rename → only the external ref moves; classification untouched.
        assert_eq!(
            react(
                ChangeEvent::Rename {
                    source_id: "s1".into(),
                    new_external_ref: Some("drive://new".into())
                },
                Some(&st(SourceState::Ok, Some("h1")))
            ),
            vec![Action::UpdatePointer {
                source_id: "s1".into(),
                external_ref: Some("drive://new".into())
            }]
        );
        // Rename of a missing item → recover then update the ref.
        assert_eq!(
            react(
                ChangeEvent::Rename {
                    source_id: "s1".into(),
                    new_external_ref: None
                },
                Some(&st(SourceState::SourceMissing, None))
            ),
            vec![
                Action::SetState {
                    source_id: "s1".into(),
                    state: SourceState::Ok
                },
                Action::UpdatePointer {
                    source_id: "s1".into(),
                    external_ref: None
                }
            ]
        );
    }

    #[test]
    fn source_failure_fans_out_to_unreachable() {
        assert_eq!(
            react(
                ChangeEvent::SourceFailure {
                    source: "gdrive-acct".into()
                },
                None
            ),
            vec![Action::SetSourceState {
                source: "gdrive-acct".into(),
                state: SourceState::Unreachable
            }]
        );
    }

    fn sample() -> Manifest {
        Manifest {
            schema: MANIFEST_SCHEMA,
            items: vec![ManifestItem {
                source_id: "drive:42".into(),
                title: "Quarterly plan".into(),
                project: "Project Falcon".into(),
                tags: vec!["plan".into()],
                importance: Some("high".into()),
                reviewed: true,
                last_activity: Some("2026-06-26T00:00:00Z".into()),
                external_ref: Some("https://drive/42".into()),
                source_modified_at: Some("2026-06-25T00:00:00Z".into()),
                source_content_hash: Some("deadbeef".into()),
                source_state: "ok".into(),
                stored_summary: Some("A short readable summary.".into()),
            }],
        }
    }

    #[test]
    fn manifest_is_encrypted_at_rest_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let cipher = ManifestCipher::from_master(VAULT_ID, &MASTER);
        let manifest = sample();

        let prior = write_manifest(dir.path(), &cipher, &manifest).unwrap();
        assert!(prior.is_empty(), "no prior file on first write");

        // On disk it is ciphertext, and a secret (the canonical project name) is not in plaintext.
        let raw = std::fs::read(manifest_path(dir.path())).unwrap();
        assert!(
            crypto::is_encrypted(&raw),
            "manifest must be ciphertext at rest"
        );
        assert!(
            !String::from_utf8_lossy(&raw).contains("Project Falcon"),
            "classification must not leak in plaintext"
        );

        // Round-trips under the right key.
        assert_eq!(read_manifest(dir.path(), &cipher).unwrap(), Some(manifest));

        // A different vault id cannot decrypt it (AAD binds the file to its vault).
        let other = ManifestCipher::from_master("vault-other", &MASTER);
        assert!(read_manifest(dir.path(), &other).is_err());

        // Absent file → None.
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(read_manifest(empty.path(), &cipher).unwrap(), None);
    }

    #[test]
    fn manifest_stem_is_distinct_from_the_rules_file() {
        // The manifest shares the rules file's subkey + vault_id; only the AAD stem keeps them apart.
        // Decrypting the manifest bytes with the RULES stem ("entities") must fail, while the manifest
        // stem succeeds — proving a rules-file reader can't authenticate a manifest (and vice-versa).
        let dir = tempfile::tempdir().unwrap();
        let cipher = ManifestCipher::from_master(VAULT_ID, &MASTER);
        write_manifest(dir.path(), &cipher, &sample()).unwrap();
        let raw = std::fs::read(manifest_path(dir.path())).unwrap();

        let subkey = crate::vault::markdown_subkey(&MASTER);
        assert!(
            crypto::decrypt(&raw, &subkey, VAULT_ID, "entities").is_err(),
            "the rules-file stem must not authenticate the manifest"
        );
        assert!(
            crypto::decrypt(&raw, &subkey, VAULT_ID, MANIFEST_AAD_STEM).is_ok(),
            "the manifest stem must authenticate the manifest"
        );
    }

    const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn item(source_id: &str, project: &str) -> ManifestItem {
        ManifestItem {
            source_id: source_id.into(),
            title: "T".into(),
            project: project.into(),
            tags: vec![],
            importance: None,
            reviewed: false,
            last_activity: None,
            external_ref: None,
            source_modified_at: None,
            source_content_hash: None,
            source_state: "ok".into(),
            stored_summary: Some("s".into()),
        }
    }

    fn insert_index_only(conn: &Connection, source_id: &str, project: &str) {
        conn.execute(
            "INSERT INTO documents(vault_path, title, content_hash, source_type, source_id, project, \
                    stored_summary) \
             VALUES (?1, 'T', ?2, 'index_only', ?3, ?4, 's')",
            params![
                format!("idx://{source_id}"),
                format!("h-{source_id}"),
                source_id,
                project
            ],
        )
        .unwrap();
    }

    #[test]
    fn reconcile_applies_the_file_classification_onto_existing_rows() {
        // The manifest is the portable truth for classification: when it disagrees with the DB row
        // (e.g. the vault was copied and the row drifted), the file wins on reconcile.
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        insert_index_only(&conn, "s1", "Unsorted");

        let cipher = ManifestCipher::from_master(VAULT_ID, &MASTER);
        write_manifest(
            dir.path(),
            &cipher,
            &Manifest {
                schema: MANIFEST_SCHEMA,
                items: vec![item("s1", "Taxes")],
            },
        )
        .unwrap();

        reconcile_on_open(&conn, dir.path(), &cipher).unwrap();

        let project: String = conn
            .query_row(
                "SELECT project FROM documents WHERE source_id = 's1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            project, "Taxes",
            "the file's classification wins on reconcile"
        );
    }

    #[test]
    fn sync_preserves_a_file_item_absent_from_the_db() {
        // A crash mid-Rebuild can leave the manifest with an item the DB doesn't yet have (awaiting
        // re-embed). A later sync must NOT drop it — losing a classification would read as the data
        // loss the card forbids.
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        let cipher = ManifestCipher::from_master(VAULT_ID, &MASTER);

        // The file has s2 (awaiting Rebuild); the DB has only s1.
        write_manifest(
            dir.path(),
            &cipher,
            &Manifest {
                schema: MANIFEST_SCHEMA,
                items: vec![item("s2", "Archive")],
            },
        )
        .unwrap();
        insert_index_only(&conn, "s1", "Unsorted");

        write_synced(&conn, dir.path(), &cipher).unwrap();

        let manifest = read_manifest(dir.path(), &cipher).unwrap().unwrap();
        let ids: Vec<&str> = manifest
            .items
            .iter()
            .map(|i| i.source_id.as_str())
            .collect();
        assert!(ids.contains(&"s1"), "the DB row is in the manifest");
        assert!(
            ids.contains(&"s2"),
            "the awaiting-Rebuild file item is preserved, not dropped"
        );
    }

    #[test]
    fn forget_source_strips_only_the_promoted_item() {
        // The promote-to-full strip: the named source leaves the manifest, every other item stays, and
        // it is NOT re-merged (a surgical file edit, unlike `write_synced`). This is what stops a
        // promoted document from being resurrected as an index-only ghost.
        let dir = tempfile::tempdir().unwrap();
        let cipher = ManifestCipher::from_master(VAULT_ID, &MASTER);
        write_manifest(
            dir.path(),
            &cipher,
            &Manifest {
                schema: MANIFEST_SCHEMA,
                items: vec![item("keep:1", "A"), item("drop:2", "B")],
            },
        )
        .unwrap();

        forget_source(dir.path(), &cipher, "drop:2").unwrap();
        let ids: Vec<String> = read_manifest(dir.path(), &cipher)
            .unwrap()
            .unwrap()
            .items
            .into_iter()
            .map(|i| i.source_id)
            .collect();
        assert_eq!(
            ids,
            vec!["keep:1".to_string()],
            "only the promoted id is gone"
        );

        // Idempotent: forgetting an absent source, or forgetting on a vault with no manifest at all, is a
        // clean no-op (never creates or errors on an empty manifest).
        forget_source(dir.path(), &cipher, "drop:2").unwrap();
        let empty = tempfile::tempdir().unwrap();
        forget_source(empty.path(), &cipher, "anything").unwrap();
        assert!(read_manifest(empty.path(), &cipher).unwrap().is_none());
    }

    // --- executor state transitions (no embedder needed) ---

    #[test]
    fn delete_is_soft_and_keeps_the_item_and_its_index_rows() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        insert_index_only(&conn, "s1", "Unsorted");
        let doc_id: i64 = conn
            .query_row("SELECT id FROM documents WHERE source_id='s1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        // A chunk + vector + FTS row, so we can prove the soft delete leaves it findable.
        conn.execute(
            "INSERT INTO chunks(document_id, ordinal, content, char_count, kind) \
             VALUES (?1, 0, '(body available at the source)', 1, 'leaf')",
            params![doc_id],
        )
        .unwrap();
        let chunk_id = conn.last_insert_rowid();
        let vec = format!("[{}]", vec!["0.1"; 384].join(", "));
        conn.execute(
            "INSERT INTO chunk_vec(rowid, embedding) VALUES (?1, ?2)",
            params![chunk_id, vec],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chunks_fts(rowid, content) VALUES (?1, 'a short summary')",
            params![chunk_id],
        )
        .unwrap();

        set_item_state(&conn, "s1", SourceState::SourceMissing).unwrap();

        let state: String = conn
            .query_row(
                "SELECT source_state FROM documents WHERE source_id='s1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "source_missing");
        // The row, its vector, and its FTS entry all survive — a soft delete, never a hard drop.
        let docs: i64 = conn
            .query_row(
                "SELECT count(*) FROM documents WHERE source_id='s1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let vecs: i64 = conn
            .query_row(
                "SELECT count(*) FROM chunk_vec WHERE rowid=?1",
                params![chunk_id],
                |r| r.get(0),
            )
            .unwrap();
        let fts: i64 = conn
            .query_row(
                "SELECT count(*) FROM chunks_fts WHERE rowid=?1",
                params![chunk_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            (docs, vecs, fts),
            (1, 1, 1),
            "a soft delete keeps the item + its index rows so it stays findable"
        );
    }

    #[test]
    fn source_failure_fans_out_only_to_that_source() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        insert_index_only(&conn, "acct:1", "Unsorted");
        insert_index_only(&conn, "acct:2", "Unsorted");
        insert_index_only(&conn, "other:1", "Unsorted");

        set_source_state(&conn, "acct", SourceState::Unreachable).unwrap();

        let unreachable: i64 = conn
            .query_row(
                "SELECT count(*) FROM documents WHERE source_state='unreachable'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            unreachable, 2,
            "both items of the failed source go unreachable"
        );
        let other: String = conn
            .query_row(
                "SELECT source_state FROM documents WHERE source_id='other:1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(other, "ok", "a different source is untouched");
    }

    #[test]
    fn rename_updates_ref_without_disturbing_classification() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        insert_index_only(&conn, "s1", "Taxes");
        let entity = crate::entities::resolve_project(&conn, "Taxes", true)
            .unwrap()
            .unwrap();
        conn.execute(
            "UPDATE documents SET entity_id = ?1 WHERE source_id = 's1'",
            params![entity],
        )
        .unwrap();

        set_external_ref(&conn, "s1", Some("drive://moved")).unwrap();

        let (ext, proj, ent): (Option<String>, String, Option<i64>) = conn
            .query_row(
                "SELECT external_ref, project, entity_id FROM documents WHERE source_id='s1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(ext.as_deref(), Some("drive://moved"));
        assert_eq!(proj, "Taxes", "a rename leaves classification untouched");
        assert_eq!(ent, Some(entity), "the entity link survives a rename");
    }
}
