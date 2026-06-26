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
pub struct PointerInput {
    pub source_id: String,
    pub title: String,
    pub external_ref: Option<String>,
    pub source_modified_at: Option<String>,
    pub source_content_hash: Option<String>,
    pub body: String,
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
/// constraint — unlike a vault import, where identical content IS the same document.
fn pointer_content_hash(source_id: &str, indexed_text: &str) -> String {
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
    ingest::index_document(state, meta, &chunks, &embeddings)
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
        },
    };
    embed_and_index(state, gateway, &summary, &meta)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MASTER: [u8; 32] = [7u8; 32];
    const VAULT_ID: &str = "vault-abc";

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
}
