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

/// How many mirror-changing items a bulk sync may ingest between manifest rewrites. Each
/// [`write_synced`] is an O(n) read-merge-encrypt-write of the WHOLE manifest, so writing it once per
/// item made a pass O(n²); a connector loop instead batches through [`crate::connector_sync::ManifestFlusher`],
/// flushing every `MANIFEST_FLUSH_EVERY` items and once at the end. The bound also caps the crash-exposed
/// window: a crash between flushes leaves at most this many committed DB rows without a manifest entry,
/// which [`reconcile_on_open`] self-heals from the mirror on next open (and the interrupted account's
/// cursor stays unadvanced, so the items re-observe anyway).
pub const MANIFEST_FLUSH_EVERY: usize = 256;
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
    /// The item's OTHER project memberships (#275) — the manifest's counterpart to a vault file's
    /// `linked_projects:` line, so an index-only document can belong to several projects exactly
    /// like a stored one.
    ///
    /// `#[serde(default)]` because this file is the portable truth and manifests written before
    /// #275 have no such key. Without it, every pre-existing `.pmindex` would fail to parse — and
    /// this is the file a restore reads, so that is not a cosmetic failure.
    #[serde(default)]
    pub linked_projects: Vec<String>,
    pub tags: Vec<String>,
    pub importance: Option<String>,
    pub reviewed: bool,
    pub last_activity: Option<String>,
    pub external_ref: Option<String>,
    pub source_modified_at: Option<String>,
    pub source_content_hash: Option<String>,
    /// `'ok' | 'source_missing' | 'unreachable'` — the first-class reachability state. Since #710
    /// this is the ROLLUP across `locations`, not one place's answer, so an older build reading a
    /// newer manifest still gets the honest "is this body reachable at all".
    pub source_state: String,
    pub stored_summary: Option<String>,
    /// Every OTHER place this file lives (#710) — the anchor is `source_id` above, so this holds
    /// only its siblings and is empty for all but a folded duplicate.
    ///
    /// `#[serde(default)]` because this file is the portable truth and every manifest written before
    /// #710 lacks the key; without it a restore — the one moment this file has to work — would fail
    /// to parse outright. The reverse direction is the honest cost of a portable format: an OLDER
    /// build reading a newer manifest drops these on its next write, and the duplicate then
    /// re-appears as two documents on that machine. It cannot corrupt anything, because a location
    /// is a claim PM can re-derive, and the newer build folds it again.
    #[serde(default)]
    pub locations: Vec<ManifestLocation>,
}

/// One non-anchor place a file lives, as the portable manifest carries it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestLocation {
    pub source_id: String,
    /// THIS location's reachability — not the document's. The two differ exactly when one copy is
    /// gone and another is fine, which is the case the whole model exists for.
    pub source_state: String,
    pub external_ref: Option<String>,
    pub source_modified_at: Option<String>,
    /// This location's own change pointer. Sharing one across locations would make each connector's
    /// `Update` compare against the other's hash, and the two would re-embed each other forever.
    pub source_content_hash: Option<String>,
    pub source_parent_folder_id: Option<String>,
    pub source_parent_folder_name: Option<String>,
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
    crate::vault::write_atomic(&path, &bytes)?;
    Ok(prior)
}

/// Re-encrypt the manifest from `old` to `new` after a vault rekey. A passphrase change moves the
/// master, and with it this file's subkey, so without this the file stops decrypting — and
/// [`reconcile_on_open`]'s heal then rewrites it from the DB mirror ALONE, because the union in
/// [`merged_manifest`] that would have preserved file-only items works by reading the very file it
/// cannot read. The items that union exists to protect (classified, awaiting a Rebuild) are exactly
/// the ones a rekey would destroy, and the old file is overwritten, so they are gone for good
/// (#517). Converting here means that heal never has to run.
///
/// Idempotent and best-effort. Absent, or already readable under `new` (an interrupted migration
/// re-running), is a no-op; unreadable under BOTH keys is left to the heal, which is the only thing
/// left to do with it. Returns whether the file was rewritten.
pub fn reencrypt_manifest(
    vault_root: &Path,
    old: &ManifestCipher,
    new: &ManifestCipher,
) -> Result<bool> {
    if !manifest_path(vault_root).exists() {
        return Ok(false);
    }
    if read_manifest(vault_root, new).is_ok() {
        return Ok(false);
    }
    let Ok(Some(manifest)) = read_manifest(vault_root, old) else {
        return Ok(false);
    };
    write_manifest(vault_root, new, &manifest)?;
    Ok(true)
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

/// Whether the DB mirror holds an index-only source that `manifest` (the file) is missing — the
/// signature of the F-20 crash window: [`register_pointer`] commits the row before it writes the
/// manifest, so a crash between the two leaves a mirror row absent from the portable truth. A cheap
/// single-column scan compared against the file's id set; drives the gated mirror→file self-heal in
/// [`reconcile_on_open`] so a normal boot (file already complete) rewrites nothing.
fn mirror_has_unfiled(conn: &Connection, manifest: &Manifest) -> Result<bool> {
    let filed: std::collections::HashSet<&str> = manifest
        .items
        .iter()
        .map(|i| i.source_id.as_str())
        .collect();
    let mut stmt = conn.prepare(
        "SELECT source_id FROM documents WHERE source_type = ?1 AND source_id IS NOT NULL",
    )?;
    let mut rows = stmt.query(params![ingest::SOURCE_TYPE_INDEX_ONLY])?;
    while let Some(row) = rows.next()? {
        let sid: String = row.get(0)?;
        if !filed.contains(sid.as_str()) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Serialize the index-only documents in the DB mirror into manifest items (canonical names; integer
/// ids stay out of the file).
fn mirror_items(conn: &Connection) -> Result<Vec<ManifestItem>> {
    // Memberships come from the join, in one pass rather than a query per item — an estate of
    // connected files is the case this path is built for, and per-item lookups would scale with it.
    let memberships = crate::tags::all_project_memberships(conn)?;
    // Sibling locations in one pass, for the same reason memberships are: a folded estate is the
    // case this path exists for, and a query per item would scale with it.
    let mut siblings = std::collections::HashMap::<i64, Vec<ManifestLocation>>::new();
    {
        let mut stmt = conn.prepare(
            "SELECT l.document_id, l.source_id, l.source_state, l.external_ref, \
                    l.source_modified_at, l.source_content_hash, l.source_parent_folder_id, \
                    l.source_parent_folder_name \
             FROM document_locations l JOIN documents d ON d.id = l.document_id \
             WHERE d.source_type = ?1 AND l.source_id IS NOT d.source_id \
             ORDER BY l.first_seen_at, l.id",
        )?;
        let rows = stmt.query_map(params![ingest::SOURCE_TYPE_INDEX_ONLY], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                ManifestLocation {
                    source_id: r.get(1)?,
                    source_state: r.get(2)?,
                    external_ref: r.get(3)?,
                    source_modified_at: r.get(4)?,
                    source_content_hash: r.get(5)?,
                    source_parent_folder_id: r.get(6)?,
                    source_parent_folder_name: r.get(7)?,
                },
            ))
        })?;
        for row in rows {
            let (doc_id, loc) = row?;
            siblings.entry(doc_id).or_default().push(loc);
        }
    }
    let mut stmt = conn.prepare(
        "SELECT source_id, title, project, tags, importance, reviewed, last_activity, \
                external_ref, source_modified_at, source_content_hash, source_state, stored_summary, \
                id \
         FROM documents \
         WHERE source_type = ?1 AND source_id IS NOT NULL \
         ORDER BY source_id",
    )?;
    let rows = stmt
        .query_map(params![ingest::SOURCE_TYPE_INDEX_ONLY], |r| {
            let tags_json: String = r.get(3)?;
            let project: String = r.get(2)?;
            let doc_id: i64 = r.get(12)?;
            let home_norm = crate::tags::normalize(&project);
            let linked_projects = memberships
                .get(&doc_id)
                .map(|names| {
                    names
                        .iter()
                        .filter(|n| crate::tags::normalize(n) != home_norm)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            Ok(ManifestItem {
                source_id: r.get(0)?,
                title: r.get(1)?,
                project,
                linked_projects,
                tags: serde_json::from_str(&tags_json).unwrap_or_default(),
                importance: r.get(4)?,
                reviewed: r.get::<_, i64>(5)? != 0,
                last_activity: r.get(6)?,
                external_ref: r.get(7)?,
                source_modified_at: r.get(8)?,
                source_content_hash: r.get(9)?,
                source_state: r.get(10)?,
                stored_summary: r.get(11)?,
                locations: siblings.remove(&doc_id).unwrap_or_default(),
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

/// Settings key recording that the DB mirror has moved AHEAD of the encrypted manifest — i.e. that a
/// classification change committed but its mirror→file push did not land.
///
/// The manifest is the portable truth for index-only classification, and [`reconcile_on_open`]
/// applies it file→DB at every boot. That is only sound while the file is known to be current. The
/// push is best-effort by design (it must never fail a user's edit, and it is a no-op on a locked
/// vault), so a failed push used to leave a file that was silently OLDER than the DB — and the next
/// boot then applied it, quietly reverting the user's filing to a previous value with no error and
/// nothing in the UI to see. Recording the divergence lets the next boot repair the file BEFORE
/// reading it as truth.
const MANIFEST_STALE_KEY: &str = "index_only_manifest_stale";

/// Note that the manifest is behind the DB mirror — see [`MANIFEST_STALE_KEY`]. Best-effort: this is
/// itself called from error paths, so a failure to record only costs the repair, never the caller.
pub fn mark_manifest_stale(conn: &Connection) {
    if let Err(e) = crate::db::set_setting(conn, MANIFEST_STALE_KEY, "1") {
        eprintln!("index_only: could not record a stale manifest ({e})");
    }
}

/// Whether a mirror→file push is known to have been lost since the manifest was last written.
fn manifest_is_stale(conn: &Connection) -> bool {
    matches!(
        crate::db::get_setting(conn, MANIFEST_STALE_KEY),
        Ok(Some(v)) if v == "1"
    )
}

/// Push the DB mirror (merged with any awaiting-Rebuild file items) to the encrypted manifest,
/// returning the prior bytes for rollback. The single write path: the post-change sync and the
/// truth-writer manifest arm both go through this.
///
/// Clears [`MANIFEST_STALE_KEY`] on success — the file now matches the mirror, whatever was lost
/// before. Inside the truth-writer's transaction that clear rolls back with the rest, which is the
/// behaviour we want: an abandoned batch leaves the flag exactly as it found it.
pub fn write_synced(
    conn: &Connection,
    vault_root: &Path,
    cipher: &ManifestCipher,
) -> Result<Vec<u8>> {
    let manifest = merged_manifest(conn, vault_root, cipher)?;
    let prior = write_manifest(vault_root, cipher, &manifest)?;
    crate::db::set_setting(conn, MANIFEST_STALE_KEY, "0")?;
    Ok(prior)
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

/// Re-key sources in the encrypted manifest, in one read-modify-write, so a `source_id` change made
/// in the DB travels to the portable truth with its classification intact.
///
/// The counterpart to a bare `UPDATE documents SET source_id`. [`merged_manifest`] unions the DB
/// mirror with every FILE item whose id is absent from the mirror, so an id re-keyed only in the DB
/// survives in the manifest forever as a ghost — and a Rebuild, which restores from the manifest,
/// then resurrects it as a SECOND document alongside the re-keyed one. Dropping the old id instead
/// ([`forget_source`]) would fix the duplicate but throw away the user's filing; renaming in place
/// keeps it.
///
/// Skips any pair whose new id is already in the file (nothing to carry over, and a rename would
/// duplicate it) by dropping the stale old entry. Idempotent: a missing manifest, or a pair already
/// applied, is a clean no-op that writes nothing. Returns how many items were re-keyed. Call AFTER
/// the DB update commits, so the mirror already reports the new id.
pub fn rekey_sources(
    vault_root: &Path,
    cipher: &ManifestCipher,
    pairs: &[(String, String)],
) -> Result<usize> {
    if pairs.is_empty() {
        return Ok(0);
    }
    let Some(mut manifest) = read_manifest(vault_root, cipher)? else {
        return Ok(0);
    };
    let mut changed = 0usize;
    for (old, new) in pairs {
        if old == new || !manifest.items.iter().any(|it| &it.source_id == old) {
            continue;
        }
        if manifest.items.iter().any(|it| &it.source_id == new) {
            manifest.items.retain(|it| &it.source_id != old);
        } else {
            for it in &mut manifest.items {
                if &it.source_id == old {
                    it.source_id = new.clone();
                }
            }
        }
        changed += 1;
    }
    if changed > 0 {
        manifest.items.sort_by(|a, b| a.source_id.cmp(&b.source_id));
        write_manifest(vault_root, cipher, &manifest)?;
    }
    Ok(changed)
}

/// Apply the portable classification in `manifest` onto the matching index-only rows (the file is the
/// source of truth for classification), re-resolving each item's `entity_id` from its canonical name
/// through the rules mirror. Rows present in the file but absent from the DB are left untouched —
/// they await a Rebuild, which re-embeds them from their summary (we can't embed here). Runs on every
/// boot/unlock, so the common already-in-sync row is detected up front and skipped: the existence
/// probe reads the row's current values and the UPDATE only fires when something actually differs.
/// Returns whether any item MINTED a project entity — see the F-04 note on the probe below. The
/// caller must push the rules file when it did, or the next boot rolls the mint back.
fn apply_classification(conn: &Connection, manifest: &Manifest) -> Result<bool> {
    /// The row's current values for every column the UPDATE below writes (minus `entity_id`, which
    /// is re-resolved fresh each pass and compared alongside).
    struct CurrentRow {
        project: Option<String>,
        tags: Option<String>,
        importance: Option<String>,
        reviewed: Option<i64>,
        last_activity: Option<String>,
        external_ref: Option<String>,
        source_modified_at: Option<String>,
        source_content_hash: Option<String>,
        source_state: Option<String>,
        stored_summary: Option<String>,
        title: Option<String>,
        entity_id: Option<i64>,
        id: i64,
    }
    // F-04 ("mirror ⊆ rules after any mint"). The loop's `resolve_project(.., true)` below MINTS a
    // project entity when the manifest names one the mirror lacks — but nothing here pushed it to
    // the portable rules file, and this runs at boot right AFTER `entities::reconcile_on_open`,
    // which treats that file as truth. So the next boot rolled the entity back, this pass minted it
    // again, and round it went: a permanent every-boot churn loop, with any project-scoped
    // preference attached to that entity left dormant (`entity_id = NULL`) for good.
    //
    // Probed BEFORE the loop, since the loop is what creates them, and once per DISTINCT name — the
    // items overwhelmingly share one project, and one mint is enough to owe the sync.
    let minted = {
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut found = false;
        for it in &manifest.items {
            if seen.insert(it.project.as_str())
                && crate::entities::resolve_project(conn, &it.project, false)?.is_none()
            {
                found = true;
                break;
            }
        }
        found
    };
    for it in &manifest.items {
        let current = conn
            .query_row(
                "SELECT project, tags, importance, reviewed, last_activity, external_ref, \
                        source_modified_at, source_content_hash, source_state, stored_summary, \
                        title, entity_id, id \
                 FROM documents WHERE source_id = ?1 AND source_type = ?2",
                params![it.source_id, ingest::SOURCE_TYPE_INDEX_ONLY],
                |r| {
                    Ok(CurrentRow {
                        project: r.get(0)?,
                        tags: r.get(1)?,
                        importance: r.get(2)?,
                        reviewed: r.get(3)?,
                        last_activity: r.get(4)?,
                        external_ref: r.get(5)?,
                        source_modified_at: r.get(6)?,
                        source_content_hash: r.get(7)?,
                        source_state: r.get(8)?,
                        stored_summary: r.get(9)?,
                        title: r.get(10)?,
                        entity_id: r.get(11)?,
                        id: r.get(12)?,
                    })
                },
            )
            .optional()?;
        let Some(current) = current else {
            continue;
        };
        // Always re-resolve (never skipped even when the rest matches): the boot reconcile runs
        // AFTER the entity-rules reconcile precisely so an alias edit from another machine re-points
        // items — and `resolve_project(.., true)` also recreates a missing project entity.
        let entity_id = crate::entities::resolve_project(conn, &it.project, true)?;
        let tags_json = serde_json::to_string(&it.tags)
            .map_err(|e| Error::Other(format!("encode tags: {e}")))?;
        // Compares EVERY column the UPDATE writes, so any drift still rewrites exactly as before;
        // the every-boot in-sync case becomes read-only.
        // Memberships join the drift comparison: a link added on another machine arrives only in
        // the manifest, and without this the every-boot in-sync shortcut below would skip it
        // forever.
        let current_linked = crate::tags::linked_projects(conn, current.id, &it.project)?;
        let unchanged = current.project.as_deref() == Some(it.project.as_str())
            && current_linked == it.linked_projects
            && current.tags.as_deref() == Some(tags_json.as_str())
            && current.importance == it.importance
            && current.reviewed == Some(it.reviewed as i64)
            && current.last_activity == it.last_activity
            && current.external_ref == it.external_ref
            && current.source_modified_at == it.source_modified_at
            && current.source_content_hash == it.source_content_hash
            && current.source_state.as_deref() == Some(it.source_state.as_str())
            && current.stored_summary == it.stored_summary
            && current.title.as_deref() == Some(it.title.as_str())
            && current.entity_id == entity_id;
        if unchanged {
            continue;
        }
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
        crate::tags::set_document_projects(conn, current.id, &it.project, &it.linked_projects)?;
        crate::tags::set_document_group_tags(conn, current.id, &it.tags)?;
    }
    Ok(minted)
}

/// Reconcile the encrypted manifest with the DB mirror at session open. The file is the portable
/// truth for index-only CLASSIFICATION (project/tags/state/pointer/summary) — but NOT the embeddings,
/// which live only in the DB and come back only via a Rebuild. So: when the file is present, apply
/// its classification onto existing rows (a Rebuild restores any rows it still lacks); when ABSENT or
/// UNDECRYPTABLE, (re)write it from the mirror if there is anything to persist. Runs AFTER the entity
/// rules reconcile at boot, so each item's project resolves through the rebuilt aliases.
/// Returns whether a project entity was MINTED, so the caller can push the portable rules file
/// (this can't do it itself — it holds the connection, and `sync_entity_rules` takes its own).
pub fn reconcile_on_open(
    conn: &Connection,
    vault_root: &Path,
    cipher: &ManifestCipher,
) -> Result<bool> {
    match read_manifest(vault_root, cipher) {
        Ok(Some(manifest)) => {
            // Repair BEFORE reading the file as truth. A recorded stale flag means a classification
            // change committed to the DB and its mirror→file push was lost, so the file on disk is
            // OLDER than the mirror for at least one item — and applying it would silently revert the
            // user's filing to a previous value. `write_synced` resolves that the only defensible
            // way: the DB wins for ids present in both, while `merged_manifest`'s union preserves
            // every file-only item awaiting a Rebuild. Then we re-read and carry on as normal, so
            // the entity re-resolution and mint accounting below are unchanged.
            let manifest = if manifest_is_stale(conn) {
                eprintln!(
                    "index_only: the manifest is behind the DB mirror (a previous push was lost); \
                     rewriting it from the mirror before applying it"
                );
                write_synced(conn, vault_root, cipher)?;
                read_manifest(vault_root, cipher)?.unwrap_or(manifest)
            } else {
                manifest
            };
            let tx = conn.unchecked_transaction()?;
            let minted = apply_classification(&tx, &manifest)?;
            tx.commit()?;
            // F-20 self-heal (mirror→file). `register_pointer` commits the DB row before it writes the
            // manifest, so a crash in that window leaves an index-only row in the mirror but absent from
            // the file (the portable truth). `apply_classification` only flows file→DB; without a pass
            // in the other direction the orphan persists until an unrelated `write_synced`, and a
            // Rebuild-from-manifest before that drops the item entirely. Union the mirror back into the
            // file when it holds ids the file lacks — gated so a normal boot rewrites nothing.
            if mirror_has_unfiled(conn, &manifest)? {
                write_synced(conn, vault_root, cipher)?;
            }
            Ok(minted)
        }
        // No manifest, or an unreadable one: nothing was read, so nothing was minted from a file.
        Ok(None) => {
            if has_index_only(conn)? {
                write_synced(conn, vault_root, cipher)?;
            }
            Ok(false)
        }
        Err(e) => {
            eprintln!("index_only: manifest unreadable ({e}); rewriting it from the DB mirror");
            if has_index_only(conn)? {
                write_synced(conn, vault_root, cipher)?;
            }
            Ok(false)
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
    /// What the SOURCE says about the item (#701) — its author, who last edited it, when it was
    /// created there, and how big it is. `None` means the provider did not say, which the UI renders
    /// as "Unknown". Rides alongside the body exactly like the parent folder above: never chunked,
    /// never embedded.
    pub source_author: Option<String>,
    pub source_last_modified_by: Option<String>,
    pub source_created_at: Option<String>,
    pub source_size_bytes: Option<i64>,
}

/// What the SOURCE says about an item, as of the last time PM looked (#701 wrote them, #708 keeps
/// them true).
///
/// The flow is one-directional on purpose: each connector builds its `SourceFacts` from the payload
/// in hand, and a [`PointerInput`] is then assembled FROM those facts. So first sight and every
/// later sighting read one definition of what a file's facts are, and the two cannot drift.
///
/// Held separately from [`PointerInput`] because the two paths that write these columns have very
/// different costs: first-sight ingest has the body in hand and is about to embed it, while a
/// refresh runs against an item whose content has not changed at all and must therefore be free
/// when there is nothing to say.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct SourceFacts {
    pub author: Option<String>,
    pub last_modified_by: Option<String>,
    pub created_at: Option<String>,
    pub size_bytes: Option<i64>,
    pub modified_at: Option<String>,
    pub parent_folder_id: Option<String>,
    /// Resolving a Drive folder's NAME costs an API call, so it is filled in only when the id has
    /// actually moved. `None` here means "don't touch the stored name", not "the folder is unnamed"
    /// — which is why [`refresh_source_facts`] treats it separately from the rest.
    pub parent_folder_name: Option<String>,
}

impl SourceFacts {
    /// True when the provider told us nothing at all, so there is no point taking the DB lock.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The parent folder id currently stored for an item, so a caller can tell whether it has moved.
///
/// Exists to keep the folder-NAME lookup rare: on Drive a name costs an API call, and resolving one
/// per distinct folder on every fifteen-minute poll would turn a settled library into a steady
/// stream of requests. The id itself rides on the listing for free, so comparing ids is what decides
/// whether the name is worth fetching. `Ok(None)` covers both "no such row" and "no folder".
pub(crate) fn stored_parent_folder_id(
    conn: &Connection,
    source_id: &str,
) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT source_parent_folder_id FROM documents \
             WHERE source_id = ?1 AND source_type = 'index_only'",
            params![source_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

/// Bring one index-only row's source facts up to date, and stamp when that happened.
///
/// Returns whether anything was actually written. Two rules, both load-bearing:
///
/// **A fact PM knows is never unlearned by silence.** Every column is `COALESCE`d, so a `None`
/// leaves the stored value alone. Drive genuinely stops reporting `owners` once a file moves into a
/// shared drive, and Google-native documents have no `size` — under a wholesale assignment those
/// files would silently lose an author and a size PM had already been told, which is the
/// lifting-a-constraint-drops-the-default shape that has bitten this codebase before. "Unknown"
/// should mean PM was never told, not that PM forgot.
///
/// **An unchanged item writes nothing.** The `WHERE` clause compares every incoming fact against
/// what is stored (`IS NOT`, so NULLs compare safely) and matches no rows unless something differs.
/// This runs for every item on every pass of every account — a fifteen-minute poll over a settled
/// library must not dirty a single page.
pub(crate) fn refresh_source_facts(
    conn: &Connection,
    source_id: &str,
    facts: &SourceFacts,
    now: &str,
) -> Result<bool> {
    if facts.is_empty() {
        return Ok(false);
    }
    // The per-LOCATION half first (#710): where this copy sits and when it last changed THERE.
    // `documents.source_modified_at` is a mirror of the anchor's, so advancing only the mirror would
    // leave the location's own pointer stale — and the local-folder walk reads its mtime gate off the
    // location, so it would re-hash every tracked file on every poll instead of no-opping.
    conn.execute(
        "UPDATE document_locations SET \
             source_modified_at        = COALESCE(?2, source_modified_at), \
             source_parent_folder_id   = COALESCE(?3, source_parent_folder_id), \
             source_parent_folder_name = COALESCE(?4, source_parent_folder_name) \
         WHERE source_id = ?1 AND ( \
             (?2 IS NOT NULL AND source_modified_at        IS NOT ?2) OR \
             (?3 IS NOT NULL AND source_parent_folder_id   IS NOT ?3) OR \
             (?4 IS NOT NULL AND source_parent_folder_name IS NOT ?4))",
        params![
            source_id,
            facts.modified_at,
            facts.parent_folder_id,
            facts.parent_folder_name,
        ],
    )?;
    let changed = conn.execute(
        "UPDATE documents SET \
             source_author            = COALESCE(?2, source_author), \
             source_last_modified_by  = COALESCE(?3, source_last_modified_by), \
             source_created_at        = COALESCE(?4, source_created_at), \
             source_size_bytes        = COALESCE(?5, source_size_bytes), \
             source_modified_at       = COALESCE(?6, source_modified_at), \
             source_parent_folder_id  = COALESCE(?7, source_parent_folder_id), \
             source_parent_folder_name = COALESCE(?8, source_parent_folder_name), \
             pm_refreshed_at          = ?9 \
         WHERE source_id = ?1 AND source_type = 'index_only' AND ( \
             (?2 IS NOT NULL AND source_author            IS NOT ?2) OR \
             (?3 IS NOT NULL AND source_last_modified_by  IS NOT ?3) OR \
             (?4 IS NOT NULL AND source_created_at        IS NOT ?4) OR \
             (?5 IS NOT NULL AND source_size_bytes        IS NOT ?5) OR \
             (?6 IS NOT NULL AND source_modified_at       IS NOT ?6) OR \
             (?7 IS NOT NULL AND source_parent_folder_id  IS NOT ?7) OR \
             (?8 IS NOT NULL AND source_parent_folder_name IS NOT ?8))",
        params![
            source_id,
            facts.author,
            facts.last_modified_by,
            facts.created_at,
            facts.size_bytes,
            facts.modified_at,
            facts.parent_folder_id,
            facts.parent_folder_name,
            now,
        ],
    )?;
    Ok(changed > 0)
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
/// What [`register_pointer`] did: the document, and whether this call is what created it.
///
/// `created: false` means the source was already promoted to a full local import, so an existing
/// (possibly already-reviewed) row owns the id and nothing new entered the review queue.
pub struct Registered {
    pub document: ingest::Document,
    pub created: bool,
}

/// What [`apply_actions`] did: whether the portable mirror needs rewriting, and the documents that
/// came into existence during the call.
///
/// `landed` is deliberately narrower than `dirtied`. A re-embed, a state flip or a pointer update all
/// dirty the mirror while touching a row the user has already seen; only a brand-new row is an
/// arrival. Keeping them separate is what lets a caller announce arrivals without re-announcing the
/// whole corpus on every sync.
#[derive(Default)]
pub struct Applied {
    pub dirtied: bool,
    pub landed: Vec<ingest::Document>,
}

/// embeddings + a short summary + the pointer. Commits the DB row only — the portable manifest is
/// rewritten by the caller's batched flush, not per item (see [`MANIFEST_FLUSH_EVERY`]). Writes NO
/// Markdown vault file. The new document enters the review queue (project Unsorted, `reviewed =
/// false`), exactly like a freshly imported file — index-only is a mode, not a separate pipeline.
pub fn register_pointer(
    state: &AppState,
    gateway: &ModelGateway<'_>,
    input: PointerInput,
) -> Result<Registered> {
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
            // A pre-existing (and possibly already-reviewed) row — nothing landed here.
            return Ok(Registered {
                document: ingest::load_document(&conn, existing)?,
                created: false,
            });
        }
    }
    let now = {
        let conn = state.conn()?;
        ingest::iso_now(&conn)?
    };
    // Held aside before `input` is consumed field-by-field below — the anchor location is recorded
    // from it once the row exists.
    let input_source_id = input.source_id.clone();
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
        linked_projects: Vec::new(),
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
            source_author: input.source_author,
            source_last_modified_by: input.source_last_modified_by,
            source_created_at: input.source_created_at,
            source_size_bytes: input.source_size_bytes,
        },
    };
    // F-04: landing this item resolves its project with `create_if_new`, which MINTS a mirror entity
    // when that project is new. Note whether it already existed BEFORE the insert so we can push a
    // genuine mint out to the portable rules file below — otherwise the next session's mirror rebuild
    // (the file is truth) would silently roll the new entity back. Normally a no-op: index-only items
    // land as the seeded 'Unsorted' project, so `project_existed` is almost always true.
    let project_existed = {
        let conn = state.conn()?;
        crate::entities::resolve_project(&conn, &meta.project, false)?.is_some()
    };
    let document = embed_and_index(state, gateway, body, &meta)?;
    // The anchor location, recorded the moment the row lands (#710). Every index-only document has
    // one from birth, so `document_locations` is never a partial view of the library — the
    // connectors' known sets read it and nothing else, and a document missing from it would be
    // re-ingested as a brand-new file on the very next pass.
    {
        let conn = state.conn()?;
        crate::locations::record(
            &conn,
            document.id,
            &crate::locations::Location {
                source_id: input_source_id,
                state: SourceState::Ok,
                external_ref: meta.source.external_ref.clone(),
                source_modified_at: meta.source.source_modified_at.clone(),
                source_content_hash: meta.source.source_content_hash.clone(),
                source_parent_folder_id: meta.source.source_parent_folder_id.clone(),
                source_parent_folder_name: meta.source.source_parent_folder_name.clone(),
                anchor: true,
            },
        )?;
    }
    // The DB row is now committed; the portable manifest is NOT written here. The caller batches that
    // rewrite ([`apply_actions`] reports it dirtied the mirror, and the connector loop flushes every
    // `MANIFEST_FLUSH_EVERY` items via `ManifestFlusher`) — writing it per item made a bulk sync O(n²).
    // A crash before the next flush leaves this committed row absent from the file; `reconcile_on_open`
    // unions it back from the mirror on next open, so the classification is never lost.
    // Structural guarantee (mirror ⊆ rules after any mint, mirroring `ingest::rebuild`'s own sync):
    // if this item created a new project entity, keep the portable rules file current. Best-effort +
    // gated on an actual mint, so a normal (Unsorted) sync writes nothing.
    if !project_existed {
        state.sync_entity_rules();
    }
    Ok(Registered {
        document,
        created: true,
    })
}

/// Restore the index-only documents from the encrypted manifest during a [`crate::ingest::rebuild`] —
/// they have no Markdown file, so the rebuild vault-walk skips them. The bodies are remote and not held,
/// so a restored item is re-embedded from its **stored summary**: a degraded but honest offline index that
/// stays findable + fully classified, which phase 2 then upgrades to a full body. Reuses the already-warmed
/// `gateway` and never resizes `chunk_vec` (the vault loop sized it). An item with no summary fails (nothing
/// to embed) but is kept in the manifest for a later refresh. Returns `(restored, failed)`.
///
/// Since #371 this only has anything to do on the vector-width-change arm, which is the one that still
/// clears the store: on every other rebuild the rows are all still present, so each item reports "no restore
/// needed" and is left alone rather than being downgraded to its summary. See [`restore_item`].
pub fn rebuild_from_manifest(
    state: &AppState,
    gateway: &ModelGateway<'_>,
    vault_root: &Path,
    cipher: &ManifestCipher,
    on_event: &crate::ingest::ProgressSink,
) -> Result<(usize, usize)> {
    let manifest = match read_manifest(vault_root, cipher)? {
        Some(m) => m,
        None => return Ok((0, 0)),
    };
    let (mut restored, mut failed) = (0usize, 0usize);
    for item in &manifest.items {
        match restore_item(state, gateway, vault_root, cipher, item) {
            Ok(true) => restored += 1,
            // Already present (or a healed promote): no work, and nothing to report — phase 2 owns this row
            // now, and counting it here would double-count it against the same progress total.
            Ok(false) => {}
            Err(e) => {
                failed += 1;
                eprintln!("index_only: could not rebuild '{}': {e}", item.source_id);
                // Name the failure in the UI, and — just as importantly — CLOSE this item's slot on
                // the progress bar.
                //
                // The accounting, because it is not obvious and a plausible-looking "emit progress
                // per restored item" would break it: the bar's total is `files.len() + extra_total`,
                // where `extra_total` budgets exactly ONE slot per reachable index-only row, counted
                // up front for PHASE 2 (the full-body re-index). This restore has no budget of its
                // own, so emitting a terminal event per item would overshoot every slot phase 2 is
                // about to fill and leave the bar reading "150 of 100".
                //
                // A FAILED restore is the one case that doesn't self-balance: no row is inserted, so
                // phase 2 — which walks `documents`, not the manifest — never sees the item and never
                // fills its slot. The bar then stops one short of 100% for a reason the user was
                // never told. Emitting here fills exactly that orphaned slot. `Started` is only for
                // the label (`processed` advances on terminal events alone), and the `ok` gate
                // matches `extra_total`'s own `source_state = 'ok'` filter — an unreachable item was
                // never budgeted, so naming it here would overshoot instead.
                if item.source_state == SourceState::Ok.as_str() {
                    on_event.send(crate::ingest::IngestEvent::Started {
                        path: item.source_id.clone(),
                        name: item.title.clone(),
                    });
                    on_event.send(crate::ingest::IngestEvent::Failed {
                        path: item.source_id.clone(),
                        error: e.to_string(),
                    });
                }
            }
        }
    }
    // The mirror has the restored rows again; resync (merge preserves any item that had no summary).
    state.sync_index_only();
    // F-04, same rule as the boot reconcile: a restored item resolves its project with
    // `create_if_new`, so restoring into a mirror that was cleared (the vector-width arm wipes the
    // store) MINTS every project entity afresh. Without this the rules file — the portable truth —
    // still described the pre-rebuild world, and the next boot rolled the whole lot back. Gated on
    // actual work, and best-effort like every other sync: a rebuild that restored nothing writes
    // nothing.
    if restored > 0 {
        state.sync_entity_rules();
    }
    Ok((restored, failed))
}

/// Self-heal a stale manifest entry left by a promote (F-21). When a source is promoted to a full
/// local import ([`crate::ingest::promote_spreadsheet`] / note-promote), a non-index-only document
/// owns its `source_id`, and the promote strips the manifest entry — but that strip lands AFTER the DB
/// commit, so a crash between them can leave the id in the file. On the next Rebuild the vault walk
/// re-creates the promoted document first, so [`restore_item`] re-inserting an index-only row with the
/// same `source_id` would collide on the `idx_documents_source_id` UNIQUE index and fail that item on
/// EVERY Rebuild forever. Mirror [`register_pointer`]'s already-promoted guard: if a non-index-only doc
/// owns the id, strip the stale entry (idempotent [`forget_source`]) and report the skip. Returns
/// whether the item was a promoted stale entry (and was healed).
fn heal_if_promoted(
    conn: &Connection,
    vault_root: &Path,
    cipher: &ManifestCipher,
    source_id: &str,
) -> Result<bool> {
    let promoted = conn
        .query_row(
            "SELECT 1 FROM documents WHERE source_id = ?1 AND source_type != ?2",
            params![source_id, ingest::SOURCE_TYPE_INDEX_ONLY],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if promoted {
        forget_source(vault_root, cipher, source_id)?;
    }
    Ok(promoted)
}

/// Re-create one index-only document row from its manifest item, re-embedding from the summary. Returns
/// whether a row was actually created — `false` means the item needed no restoring (see below).
fn restore_item(
    state: &AppState,
    gateway: &ModelGateway<'_>,
    vault_root: &Path,
    cipher: &ManifestCipher,
    item: &ManifestItem,
) -> Result<bool> {
    // F-21: skip + self-heal a stale entry whose source was already promoted to a full import, rather
    // than colliding on the `source_id` UNIQUE index and failing this item on every future Rebuild.
    {
        let conn = state.conn()?;
        if heal_if_promoted(&conn, vault_root, cipher, &item.source_id)? {
            return Ok(false);
        }
    }
    // Since #371 the rebuild upserts in place instead of wiping first, so on every arm but a vector-width
    // change this row is still HERE — it never needed restoring. Two reasons that matters, beyond the
    // `source_id`/`vault_path` UNIQUE collision an unconditional insert would now hit:
    //   - Restoring a live full-body row from its ~500-char summary is an active DOWNGRADE — precisely the
    //     retrieval regression #360 shipped a fix for. Leaving the row alone keeps the good index.
    //   - The row's chunks may still be stale (this could be a splitter change), but the body is remote and
    //     not held, so only a re-fetch can re-chunk it. That is exactly what phase 2
    //     (`upgrade_index_only_to_full_body`) does next, and it claims the row for the pass when it lands.
    {
        let conn = state.conn()?;
        let present = conn
            .query_row(
                "SELECT 1 FROM documents WHERE source_id = ?1 AND source_type = ?2",
                params![item.source_id, ingest::SOURCE_TYPE_INDEX_ONLY],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if present {
            return Ok(false);
        }
    }
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
        // The manifest is this document's portable truth, so its memberships restore from it.
        linked_projects: item.linked_projects.clone(),
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
            // Same for what the source knows (#701), and deliberately the same answer: the manifest
            // is a PORTABLE format, so widening it is a compatibility surface of its own, and these
            // four are refetched on the very next sync anyway. A restored row shows "Unknown" until
            // then, which is honest — PM genuinely does not know yet.
            source_author: None,
            source_last_modified_by: None,
            source_created_at: None,
            source_size_bytes: None,
        },
    };
    let document = embed_and_index(state, gateway, &summary, &meta)?;
    // The anchor location, and every sibling the manifest carried (#710). Restoring the document
    // without them would leave every folded copy unknown to its connector, and the next sync of that
    // corpus would ingest the duplicate all over again — undoing the fold on the one path a user
    // reaches for when something has already gone wrong.
    {
        let conn = state.conn()?;
        let mut all = vec![crate::locations::Location {
            source_id: item.source_id.clone(),
            state: SourceState::from_db(&item.source_state),
            external_ref: item.external_ref.clone(),
            source_modified_at: item.source_modified_at.clone(),
            source_content_hash: item.source_content_hash.clone(),
            source_parent_folder_id: None,
            source_parent_folder_name: None,
            anchor: true,
        }];
        all.extend(item.locations.iter().map(|l| crate::locations::Location {
            source_id: l.source_id.clone(),
            state: SourceState::from_db(&l.source_state),
            external_ref: l.external_ref.clone(),
            source_modified_at: l.source_modified_at.clone(),
            source_content_hash: l.source_content_hash.clone(),
            source_parent_folder_id: l.source_parent_folder_id.clone(),
            source_parent_folder_name: l.source_parent_folder_name.clone(),
            anchor: false,
        }));
        for loc in &all {
            crate::locations::record(&conn, document.id, loc)?;
        }
    }
    Ok(true)
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
    /// True when this item's stored chunk map was built from its ~500-char offline SUMMARY rather than
    /// the full body (a rebuild-from-manifest restore). The reducer uses it to force a full-body
    /// re-embed on the next sync even when the source content is unchanged — see [`summary_indexed_flag`].
    pub summary_indexed: bool,
}

/// One item's persisted sync-pointer columns exactly as stored (raw `source_state` string,
/// `external_ref` included) — the superset every connector's per-item lookup needs. The local-folder
/// watcher consumes this shape directly (its `KnownItem` keeps the raw state string); the cloud
/// connectors go through [`read_item_state`] for the reducer's [`ItemState`] view.
pub(crate) struct RawItemState {
    pub external_ref: Option<String>,
    pub source_modified_at: Option<String>,
    pub source_content_hash: Option<String>,
    pub source_state: String,
    /// See [`ItemState::summary_indexed`] — precomputed here so every reader (the cloud connectors'
    /// `ItemState` view and the local folder's `KnownItem`) shares one definition.
    pub summary_indexed: bool,
}

/// Whether an index-only item's stored chunk map was built from its ~500-char offline SUMMARY rather
/// than the full body — the signature of a [`rebuild_from_manifest`] restore ([`restore_item`]), which
/// re-embeds from the summary because the body is remote. True iff the stored `content_hash` is the
/// pointer hash of the summary AND the summary ends in the truncation ellipsis [`summarize`] appends:
/// the ellipsis proves the summary is a *truncation* of a longer body, so there is genuinely more to
/// fetch. A body that fits within the summary is stored in full (summary == body), so it is never
/// flagged — a re-embed would only reproduce identical chunks. The next connector sync uses this to
/// force a full-body re-embed even when the source itself is unchanged; once re-embedded from the full
/// body the `content_hash` no longer matches the summary, so the flag self-clears.
pub(crate) fn summary_indexed_flag(
    source_id: &str,
    content_hash: &str,
    stored_summary: Option<&str>,
) -> bool {
    match stored_summary.map(str::trim) {
        Some(s) if s.ends_with('…') => pointer_content_hash(source_id, s) == content_hash,
        _ => false,
    }
}

/// The ONE per-item state lookup behind `drive`/`onedrive`/`localfolder` (their three copies were
/// near-identical, differing only in a hidden `source_type` filter). `include_promoted` makes that
/// difference explicit at each call site:
///
/// * `false` (OneDrive, local folders): only a live `source_type = 'index_only'` row counts.
/// * `true` (Drive): match on `source_id` ALONE, so a document **promoted to a full local import**
///   (`crate::ingest::promote_spreadsheet` flips its `source_type` off `index_only` but keeps the
///   `gdrive:` source id as a claim marker) is still seen as the item's current state — the sync
///   reducer then treats the promoted file as present-and-reachable (an `Add` re-fires as a `Noop`,
///   never re-ingesting a second, index-only copy). Only index-only and promoted-Drive docs ever
///   carry a `gdrive:` id, so the wider match can't pull in an unrelated row.
pub(crate) fn read_raw_item_state(
    conn: &Connection,
    source_id: &str,
    include_promoted: bool,
) -> Result<Option<RawItemState>> {
    // The pointer columns come from the LOCATION, not the document (#710): each place a file lives
    // is reconciled by its own connector against its own change pointer, and sharing one would make
    // location B's `Update` compare against location A's hash — the two corpora would then re-embed
    // each other in a loop, forever, on every pass.
    //
    // `summary_indexed_flag` is computed from the document's ANCHOR id (`d.source_id`), never the
    // queried one: the stored `content_hash` is derived from the anchor, so asking the question with
    // a sibling's id would compare two unrelated digests and always answer false.
    let by_location = conn
        .query_row(
            "SELECT l.external_ref, l.source_modified_at, l.source_content_hash, l.source_state, \
                    d.content_hash, d.stored_summary, d.source_id \
             FROM document_locations l JOIN documents d ON d.id = l.document_id \
             WHERE l.source_id = ?1 AND d.source_type = 'index_only'",
            params![source_id],
            raw_item_state_row,
        )
        .optional()?;
    if by_location.is_some() || !include_promoted {
        return Ok(by_location);
    }
    // Drive only. A document PROMOTED to a full local import keeps its `gdrive:` id as a claim
    // marker but is no longer index-only, so it has no locations — this table describes places a
    // CONNECTOR found a file, and a promoted one is a stored file now. Matching it here is what
    // makes the sync see it as present-and-reachable (an `Add` re-fires as a `Noop`) rather than
    // ingesting a second, index-only copy beside it.
    conn.query_row(
        "SELECT external_ref, source_modified_at, source_content_hash, source_state, \
                content_hash, stored_summary, source_id \
         FROM documents WHERE source_id = ?1",
        params![source_id],
        raw_item_state_row,
    )
    .optional()
    .map_err(Error::from)
}

/// The shared row-mapper behind both arms of [`read_raw_item_state`]. Column 6 is the ANCHOR source
/// id, which is what the stored `content_hash` was derived from.
fn raw_item_state_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawItemState> {
    let content_hash: String = r.get(4)?;
    let stored_summary: Option<String> = r.get(5)?;
    let anchor_id: Option<String> = r.get(6)?;
    Ok(RawItemState {
        external_ref: r.get(0)?,
        source_modified_at: r.get(1)?,
        source_content_hash: r.get(2)?,
        source_state: r.get(3)?,
        summary_indexed: summary_indexed_flag(
            anchor_id.as_deref().unwrap_or_default(),
            &content_hash,
            stored_summary.as_deref(),
        ),
    })
}

/// [`read_raw_item_state`] mapped into the reducer's [`ItemState`] — the shape the cloud
/// connectors' `read_item_state` delegates return.
pub(crate) fn read_item_state(
    conn: &Connection,
    source_id: &str,
    include_promoted: bool,
) -> Result<Option<ItemState>> {
    Ok(
        read_raw_item_state(conn, source_id, include_promoted)?.map(|raw| ItemState {
            source_id: source_id.to_string(),
            source_modified_at: raw.source_modified_at,
            source_content_hash: raw.source_content_hash,
            source_state: SourceState::from_db(&raw.source_state),
            summary_indexed: raw.summary_indexed,
        }),
    )
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
            // Same source hash → normally a touch, not an edit. BUT if our stored chunk map was built
            // from the ~500-char offline summary (a rebuild-from-manifest restore), an unchanged source
            // is exactly when we must still upgrade it to the full body — so force a re-embed against
            // the freshly-fetched body. Self-clearing: the re-embed rewrites `content_hash` from the
            // full body, so `summary_indexed` goes false and this no-ops on every later unchanged pass.
            (Some(s), Some(h)) => {
                if s.summary_indexed {
                    vec![Action::ReEmbed {
                        source_id,
                        new_content_hash: h,
                    }]
                } else {
                    vec![Action::Noop]
                }
            }
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

// --- folder-scoped reconcile: diff a live enumeration against the known-healthy set ---

/// A connector's enumerated file, reduced to the three fields a folder-scoped reconcile needs: the
/// provider-local id, a last-modified timestamp, and a content hash (the change pointer). Implemented
/// by `drive::DriveFile` and `onedrive::DriveItem`, so the Add/Update/Delete decision lives here once
/// rather than being hand-copied per connector (the bug that let F-30's fix get written twice).
pub trait EnumeratedFile {
    /// The provider-local file id — the caller namespaces it into a `source_id`.
    fn local_id(&self) -> &str;
    /// The source's last-modified timestamp, if it reported one.
    fn modified_at(&self) -> Option<String>;
    /// The source content hash — the pointer used to tell an edit from a no-op touch.
    fn content_hash(&self) -> Option<String>;
}

/// One entry of a folder-scoped reconcile plan: the namespaced `source_id`, the [`ChangeEvent`] the
/// reducer will apply, and the enumerated `payload` (present for `Add`/`Update`, `None` for `Delete`,
/// so the caller can fetch a body only when it exists).
pub struct ReconcileItem<T> {
    pub source_id: String,
    pub event: ChangeEvent,
    pub payload: Option<T>,
}

/// Diff a live folder-scoped enumeration against the known-healthy set → reconcile events. **PURE**:
/// no DB, no IO, no clock. A present file already known → [`Update`](ChangeEvent::Update) (catches
/// edits; the reducer no-ops an unchanged hash); a present file new or previously missing/unreachable
/// → [`Add`](ChangeEvent::Add) (ingests, or reactivates a folder removed then re-added); a known file
/// no longer present → [`Delete`](ChangeEvent::Delete). `source_id_of` namespaces a provider-local id,
/// so My Drive, shared drives, and OneDrive all share this core.
///
/// `complete` is the enumeration's own report of whether it saw *everything* (the `truncated` flag from
/// [`crate::connector_sync::paginate`], inverted). When it's `false` the listing was cut short by a
/// page/folder guard, so a file's absence proves nothing — the deletion pass is **skipped entirely**
/// and only the positively-observed Adds/Updates are returned. Without this, a truncated enumeration
/// would soft-delete every still-present file it simply didn't reach (audit F-30). Adds/Updates are
/// always safe: they're only emitted for files actually seen.
pub fn reconcile_enumeration<T: EnumeratedFile>(
    files: Vec<T>,
    known: std::collections::HashSet<String>,
    complete: bool,
    source_id_of: impl Fn(&str) -> String,
) -> Vec<ReconcileItem<T>> {
    let mut present: std::collections::HashSet<String> =
        std::collections::HashSet::with_capacity(files.len());
    let mut items: Vec<ReconcileItem<T>> = Vec::with_capacity(files.len());
    for f in files {
        let source_id = source_id_of(f.local_id());
        present.insert(source_id.clone());
        let event = if known.contains(&source_id) {
            ChangeEvent::Update {
                source_id: source_id.clone(),
                modified_at: f.modified_at(),
                new_content_hash: f.content_hash(),
            }
        } else {
            ChangeEvent::Add {
                source_id: source_id.clone(),
                modified_at: f.modified_at(),
            }
        };
        items.push(ReconcileItem {
            source_id,
            event,
            payload: Some(f),
        });
    }
    // Absence only means "deleted" when we're sure we saw the whole listing. A truncated pass hands
    // back Adds/Updates for what it did reach and leaves the known-but-unseen set alone to retry.
    if complete {
        for source_id in known {
            if !present.contains(&source_id) {
                items.push(ReconcileItem {
                    source_id: source_id.clone(),
                    event: ChangeEvent::Delete { source_id },
                    payload: None,
                });
            }
        }
    }
    items
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
    // Which document this location belongs to, and its ANCHOR id. Both matter: the anchor is what
    // `documents.content_hash` was derived from, so re-deriving it from the queried id would give a
    // sibling location its own identity and break the UNIQUE it shares with the anchor (#710).
    //
    // If this source was promoted to a full local import, it is no longer index-only and has no
    // locations. Do NOT re-embed (that would clobber the imported content with an index-only
    // summary). Just advance its tracked pointer so the next sync sees no change — the local copy is
    // the user's snapshot; they re-import to pull a fresh version. Checked before the (expensive)
    // split/embed below.
    let (doc_id, anchor_id) = {
        let conn = state.conn()?;
        let found: Option<(i64, Option<String>)> = conn
            .query_row(
                "SELECT d.id, d.source_id FROM document_locations l \
                 JOIN documents d ON d.id = l.document_id \
                 WHERE l.source_id = ?1 AND d.source_type = 'index_only'",
                params![source_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match found {
            Some((id, anchor)) => (id, anchor.unwrap_or_else(|| source_id.to_string())),
            None => {
                conn.execute(
                    "UPDATE documents SET source_content_hash = ?2, source_modified_at = ?3 \
                     WHERE source_id = ?1",
                    params![source_id, new_content_hash, input.source_modified_at],
                )?;
                return Ok(());
            }
        }
    };
    let content_hash = pointer_content_hash(&anchor_id, body);
    let chunks = ingest::split_document(gateway, body, &input.title, &content_hash)?;
    let texts = ingest::leaf_embed_texts(&chunks);
    let embeddings = gateway.embed_documents(&texts)?;
    ingest::check_embeddings(&embeddings, texts.len(), gateway.embedder().dimension)?;
    let summary = summarize(body);

    let mut conn = state.conn()?;
    let tx = conn.transaction()?;
    ingest::replace_chunks(&tx, doc_id, &chunks, &embeddings, true, Some(&summary))?;
    let now = ingest::iso_now(&tx)?;
    // The source facts ride along. This statement used to update the hash, summary and title and
    // silently drop the author, last editor, created date, size and parent folder it had just been
    // handed in `input` — so the columns #704 added were correct at first sight and then frozen
    // through every subsequent edit of the file. COALESCE for the same reason as
    // `refresh_source_facts`: a provider that has stopped reporting a fact must not erase it.
    //
    // The POINTER columns are deliberately absent here (#710): `source_state`,
    // `source_content_hash` and `source_modified_at` describe one LOCATION, and
    // `locations::record` below owns them along with the reachability rollup. One writer for the
    // mirror is the only thing keeping the two from drifting.
    tx.execute(
        "UPDATE documents SET content_hash = ?2, stored_summary = ?3, title = ?4, \
                source_author             = COALESCE(?5, source_author), \
                source_last_modified_by   = COALESCE(?6, source_last_modified_by), \
                source_created_at         = COALESCE(?7, source_created_at), \
                source_size_bytes         = COALESCE(?8, source_size_bytes), \
                source_parent_folder_id   = COALESCE(?9, source_parent_folder_id), \
                source_parent_folder_name = COALESCE(?10, source_parent_folder_name), \
                pm_refreshed_at           = ?11 \
         WHERE id = ?1",
        params![
            doc_id,
            content_hash,
            summary,
            input.title,
            input.source_author,
            input.source_last_modified_by,
            input.source_created_at,
            input.source_size_bytes,
            input.source_parent_folder_id,
            input.source_parent_folder_name,
            now,
        ],
    )?;
    // A location PM has just re-read is a location PM can reach, so this is also what clears a
    // stale `source_missing` — the same thing the old `source_state = 'ok'` in the statement above
    // did, now said about the place rather than the document.
    crate::locations::record(
        &tx,
        doc_id,
        &crate::locations::Location {
            source_id: source_id.to_string(),
            state: SourceState::Ok,
            external_ref: input.external_ref.clone(),
            source_modified_at: input.source_modified_at.clone(),
            source_content_hash: Some(new_content_hash.to_string()),
            source_parent_folder_id: input.source_parent_folder_id.clone(),
            source_parent_folder_name: input.source_parent_folder_name.clone(),
            anchor: anchor_id == source_id,
        },
    )?;
    tx.commit()?;
    Ok(())
}

/// Force a fresh full-body re-embed of one index-only item — the reader's on-demand "Re-index".
/// Unlike the change-driven [`react`]/[`apply_actions`] path this ignores the content-hash guard: the
/// caller already holds the current live body and wants the stored chunk offsets rebuilt against it
/// (e.g. after a rebuild-from-manifest left them indexing the ~500-char summary). The source's tracked
/// content hash is preserved — the source itself did not change, only PM's stored map of it.
pub fn reindex_pointer(
    state: &AppState,
    gateway: &ModelGateway<'_>,
    input: &PointerInput,
) -> Result<()> {
    let source_hash = input
        .source_content_hash
        .clone()
        .unwrap_or_else(|| ingest::hex_digest(input.body.trim().as_bytes()));
    reembed_item(state, gateway, &input.source_id, &source_hash, input)
}

/// Set ONE LOCATION's reachability, and re-derive its document's (#710).
///
/// The change since v54 is what happens to a document with more than one location: this moves the
/// place the connector is talking about, and `documents.source_state` becomes the rollup across all
/// of them — so a file deleted from one Drive account but still sitting in a tracked folder stays
/// readable, from the copy that is still there.
fn set_item_state(conn: &Connection, source_id: &str, state: SourceState) -> Result<()> {
    crate::locations::set_state(conn, source_id, state)?;
    Ok(())
}

/// Set the reachability of every LOCATION of a source — the source-failure fan-out. Matches an exact
/// `source_id`, or any id namespaced as `<source>:<localid>` (the convention a connector follows so
/// an account's items move together). Never deletes: a failed source must not read as a deletion —
/// and since v54 it no longer reads as one at the *document* either, when another copy is fine.
fn set_source_state(conn: &Connection, source: &str, state: SourceState) -> Result<()> {
    crate::locations::set_source_state(conn, source, state)?;
    Ok(())
}

/// Update one LOCATION's external ref (a rename/move kept the stable id, so its classification +
/// chunks are untouched). The document's own `external_ref` follows only when the moved location is
/// the anchor — a sibling moving must not send the reader somewhere the document isn't.
fn set_external_ref(conn: &Connection, source_id: &str, external_ref: Option<&str>) -> Result<()> {
    crate::locations::set_external_ref(conn, source_id, external_ref)?;
    Ok(())
}

/// Perform the [`Action`]s [`react`] produced against the live store. `fetched` is the connector-
/// supplied current content for the item an `IngestNew`/`ReEmbed` concerns (the dev affordance passes a
/// pasted body). State/pointer transitions are short synchronous DB writes; a re-embed runs the sidecar
/// OFF the DB lock, exactly like ingest. Returns whether the mirror changed (`dirtied`) so the caller
/// can rewrite the portable manifest on its own batched cadence — this fn never writes the manifest,
/// which lets a bulk sync flush once per `MANIFEST_FLUSH_EVERY` items instead of once per item (O(n²)).
pub fn apply_actions(
    state: &AppState,
    gateway: &ModelGateway<'_>,
    actions: &[Action],
    fetched: Option<&PointerInput>,
) -> Result<Applied> {
    let mut applied = Applied::default();
    let changed = &mut applied.dirtied;
    for action in actions {
        match action {
            Action::IngestNew { source_id } => {
                let input = require_fetched(fetched, source_id)?.clone();
                let registered = register_pointer(state, gateway, input)?;
                // ONLY a genuinely new row counts as an arrival. The promote short-circuit returns an
                // existing document, and every other arm below mutates a row that already existed —
                // that distinction is the whole reason a caller can announce these without
                // re-announcing files the user has already seen.
                if registered.created {
                    applied.landed.push(registered.document);
                }
                *changed = true;
            }
            Action::ReEmbed {
                source_id,
                new_content_hash,
            } => {
                let input = require_fetched(fetched, source_id)?;
                reembed_item(state, gateway, source_id, new_content_hash, input)?;
                *changed = true;
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
                *changed = true;
            }
            Action::SetSourceState {
                source,
                state: new_state,
            } => {
                let conn = state.conn()?;
                set_source_state(&conn, source, *new_state)?;
                *changed = true;
            }
            Action::UpdatePointer {
                source_id,
                external_ref,
            } => {
                let conn = state.conn()?;
                set_external_ref(&conn, source_id, external_ref.as_deref())?;
                *changed = true;
            }
            Action::Noop => {}
        }
    }
    Ok(applied)
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
            summary_indexed: false,
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
    fn same_hash_upgrades_a_summary_indexed_item_to_full_body() {
        // A rebuild-from-manifest leaves an item's chunks indexing only its ~500-char summary
        // (summary_indexed = true). The next sync sees the SAME source hash — normally a no-op — but we
        // must still re-fetch the full body and re-embed, so it returns ReEmbed rather than Noop.
        let mut summary_only = st(SourceState::Ok, Some("h1"));
        summary_only.summary_indexed = true;
        assert_eq!(
            react(update(Some("h1")), Some(&summary_only)),
            vec![Action::ReEmbed {
                source_id: "s1".into(),
                new_content_hash: "h1".into()
            }]
        );
        // Once it is full-body indexed (summary_indexed = false), the same unchanged hash no-ops again.
        assert_eq!(
            react(update(Some("h1")), Some(&st(SourceState::Ok, Some("h1")))),
            vec![Action::Noop]
        );
    }

    #[test]
    fn summary_indexed_flag_only_fires_on_a_truncated_summary_index() {
        let body = "a very long body that certainly exceeds the summary length ".repeat(20);
        let summary = summarize(&body); // ends with the truncation ellipsis
        assert!(summary.ends_with('…'));
        // content_hash built from the SUMMARY (a rebuild-from-manifest restore) → flagged.
        let summary_hash = pointer_content_hash("s1", summary.trim());
        assert!(summary_indexed_flag("s1", &summary_hash, Some(&summary)));
        // content_hash built from the FULL body (a fresh/upgraded index) → not flagged.
        let body_hash = pointer_content_hash("s1", body.trim());
        assert!(!summary_indexed_flag("s1", &body_hash, Some(&summary)));
        // A body that fits the summary (no ellipsis) is stored in full → never flagged, even though its
        // summary equals its body and hashes identically.
        let short = "short body";
        let short_summary = summarize(short);
        assert!(!short_summary.ends_with('…'));
        let short_hash = pointer_content_hash("s1", short_summary.trim());
        assert!(!summary_indexed_flag(
            "s1",
            &short_hash,
            Some(&short_summary)
        ));
        // No summary at all → never flagged.
        assert!(!summary_indexed_flag("s1", &summary_hash, None));
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
                linked_projects: Vec::new(),
                tags: vec!["plan".into()],
                importance: Some("high".into()),
                reviewed: true,
                last_activity: Some("2026-06-26T00:00:00Z".into()),
                external_ref: Some("https://drive/42".into()),
                source_modified_at: Some("2026-06-25T00:00:00Z".into()),
                source_content_hash: Some("deadbeef".into()),
                source_state: "ok".into(),
                stored_summary: Some("A short readable summary.".into()),
                locations: Vec::new(),
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
            linked_projects: Vec::new(),
            tags: vec![],
            importance: None,
            reviewed: false,
            last_activity: None,
            external_ref: None,
            source_modified_at: None,
            source_content_hash: None,
            source_state: "ok".into(),
            stored_summary: Some("s".into()),
            locations: Vec::new(),
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
        // ...and its anchor location, which is what v54's backfill gave every existing row and what
        // `register_pointer` records for every new one. Every per-item read and every state write
        // goes through the location now, so a document without one is invisible to its connector.
        conn.execute(
            "INSERT INTO document_locations(document_id, source_id, source_state) \
             VALUES (?1, ?2, 'ok')",
            params![conn.last_insert_rowid(), source_id],
        )
        .unwrap();
    }

    /// A full local import (e.g. a promoted spreadsheet) that owns `source_id` with a non-index-only
    /// type — the DB half of the F-21 post-promote crash window.
    fn insert_promoted(conn: &Connection, source_id: &str) {
        conn.execute(
            "INSERT INTO documents(vault_path, title, content_hash, source_type, source_id, project) \
             VALUES (?1, 'T', ?2, ?3, ?4, 'Unsorted')",
            params![
                format!("vault/{source_id}.md"),
                format!("h-{source_id}"),
                ingest::SOURCE_TYPE_SPREADSHEET,
                source_id
            ],
        )
        .unwrap();
    }

    /// Build a vault whose manifest holds "s1" (mirrored in the DB) and "s2" (file ONLY — the
    /// classified-but-awaiting-a-Rebuild case [`merged_manifest`]'s union exists to protect).
    /// Returns the temp dir, its connection, and the old/new ciphers spanning a passphrase change.
    fn rekey_fixture() -> (
        tempfile::TempDir,
        Connection,
        ManifestCipher,
        ManifestCipher,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        insert_index_only(&conn, "s1", "Taxes");

        let old = ManifestCipher::from_master(VAULT_ID, &MASTER);
        write_manifest(
            dir.path(),
            &old,
            &Manifest {
                schema: MANIFEST_SCHEMA,
                items: vec![item("s1", "Taxes"), item("s2", "Project Falcon")],
            },
        )
        .unwrap();

        // A passphrase change re-derives the master, so the subkey — and with it the file's
        // readability — moves.
        let new = ManifestCipher::from_master(VAULT_ID, &[9u8; 32]);
        assert!(
            read_manifest(dir.path(), &new).is_err(),
            "precondition: a rekeyed cipher must not read the old file",
        );
        (dir, conn, old, new)
    }

    /// The fix (#517): the migration re-encrypts the manifest under the new master, so the boot-time
    /// heal never runs and every classification survives a passphrase change.
    #[test]
    fn a_rekey_reencrypts_the_manifest_so_no_classification_is_lost() {
        let (dir, conn, old, new) = rekey_fixture();

        assert!(
            reencrypt_manifest(dir.path(), &old, &new).unwrap(),
            "the manifest was unreadable under the new key, so it must have been rewritten",
        );
        reconcile_on_open(&conn, dir.path(), &new).unwrap();

        let after = read_manifest(dir.path(), &new).unwrap().expect("manifest");
        let ids: Vec<&str> = after.items.iter().map(|i| i.source_id.as_str()).collect();
        assert!(ids.contains(&"s1"), "the mirrored item survives");
        assert!(
            ids.contains(&"s2"),
            "a classification the file held but the DB didn't must survive a rekey; got {ids:?}",
        );
    }

    /// Why the re-encrypt above is REQUIRED rather than a tidy-up: the heal on its own cannot
    /// preserve a file-only item, because the union that would have preserved it works by reading
    /// the very file it cannot decrypt. This pins the hazard, so if the re-encrypt is ever removed
    /// the test above fails and this one explains what was lost.
    #[test]
    fn the_heal_alone_cannot_preserve_a_file_only_item() {
        let (dir, conn, _old, new) = rekey_fixture();

        // No re-encrypt — straight to the heal, as it behaved before #517.
        reconcile_on_open(&conn, dir.path(), &new).unwrap();

        let healed = read_manifest(dir.path(), &new).unwrap().expect("manifest");
        let ids: Vec<&str> = healed.items.iter().map(|i| i.source_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["s1"],
            "the heal can only rebuild what the DB mirror holds — the file-only item is lost",
        );
    }

    /// Idempotent: re-running an interrupted migration must not rewrite an already-converted file,
    /// and a file readable under the new key is left exactly as it is.
    #[test]
    fn reencrypting_an_already_converted_manifest_is_a_no_op() {
        let (dir, _conn, old, new) = rekey_fixture();

        assert!(reencrypt_manifest(dir.path(), &old, &new).unwrap());
        let after_first = std::fs::read(manifest_path(dir.path())).unwrap();
        assert!(
            !reencrypt_manifest(dir.path(), &old, &new).unwrap(),
            "a manifest already readable under the new key must not be rewritten",
        );
        assert_eq!(
            after_first,
            std::fs::read(manifest_path(dir.path())).unwrap(),
            "the bytes must be untouched by the second pass",
        );

        // A vault with no manifest at all is also a no-op (never mints an empty file).
        let empty = tempfile::tempdir().unwrap();
        assert!(!reencrypt_manifest(empty.path(), &old, &new).unwrap());
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
    fn rekey_carries_a_renamed_source_and_its_filing_into_the_manifest() {
        // A shared-with-me row re-keyed from the My-Drive namespace to the account-independent one.
        // The DB update alone leaves the OLD id in the file, where `merged_manifest`'s mirror-∪-file
        // union keeps it forever — and the next Rebuild restores it as a SECOND document beside the
        // re-keyed one, indistinguishable from a real duplicate.
        let dir = tempfile::tempdir().unwrap();
        let cipher = ManifestCipher::from_master(VAULT_ID, &MASTER);
        write_manifest(
            dir.path(),
            &cipher,
            &Manifest {
                schema: MANIFEST_SCHEMA,
                items: vec![item("gdrive:a@b.com:F1", "Taxes"), item("other", "Archive")],
            },
        )
        .unwrap();

        let n = rekey_sources(
            dir.path(),
            &cipher,
            &[("gdrive:a@b.com:F1".into(), "gdrive:swm:R1:F1".into())],
        )
        .unwrap();
        assert_eq!(n, 1);

        let m = read_manifest(dir.path(), &cipher).unwrap().unwrap();
        let ids: Vec<&str> = m.items.iter().map(|i| i.source_id.as_str()).collect();
        assert!(
            !ids.contains(&"gdrive:a@b.com:F1"),
            "the old id must be gone"
        );
        assert_eq!(ids.iter().filter(|i| **i == "gdrive:swm:R1:F1").count(), 1);
        // The filing travelled with the rename — a plain `forget_source` would have thrown it away.
        let moved = m
            .items
            .iter()
            .find(|i| i.source_id == "gdrive:swm:R1:F1")
            .unwrap();
        assert_eq!(moved.project, "Taxes");
        // Untouched neighbours stay untouched.
        assert!(ids.contains(&"other"));

        // Idempotent: re-running writes nothing, so a retried sync can't corrupt the file.
        assert_eq!(
            rekey_sources(
                dir.path(),
                &cipher,
                &[("gdrive:a@b.com:F1".into(), "gdrive:swm:R1:F1".into())]
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn rekey_onto_an_id_the_file_already_holds_drops_the_stale_entry() {
        // Both ids present (a partly-applied adoption): renaming would DUPLICATE the new id, so the
        // stale old entry is dropped instead and the existing item is left as it is.
        let dir = tempfile::tempdir().unwrap();
        let cipher = ManifestCipher::from_master(VAULT_ID, &MASTER);
        write_manifest(
            dir.path(),
            &cipher,
            &Manifest {
                schema: MANIFEST_SCHEMA,
                items: vec![item("old", "Taxes"), item("new", "Archive")],
            },
        )
        .unwrap();

        assert_eq!(
            rekey_sources(dir.path(), &cipher, &[("old".into(), "new".into())]).unwrap(),
            1
        );
        let m = read_manifest(dir.path(), &cipher).unwrap().unwrap();
        let ids: Vec<&str> = m.items.iter().map(|i| i.source_id.as_str()).collect();
        assert_eq!(ids, vec!["new"]);
        assert_eq!(m.items[0].project, "Archive");
    }

    #[test]
    fn a_manifest_known_to_be_stale_is_repaired_before_it_is_applied() {
        // The F-20 heal only ever noticed ids the file was MISSING. A push that failed AFTER its rows
        // committed leaves a file that is complete but OLDER — and reconcile then applied it, silently
        // reverting the user's filing to a previous value with nothing in the UI to see.
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        let cipher = ManifestCipher::from_master(VAULT_ID, &MASTER);

        // The file holds the OLD filing; the DB has the newer one the user just chose.
        write_manifest(
            dir.path(),
            &cipher,
            &Manifest {
                schema: MANIFEST_SCHEMA,
                items: vec![item("s1", "Unsorted"), item("awaiting", "Archive")],
            },
        )
        .unwrap();
        insert_index_only(&conn, "s1", "Taxes");
        mark_manifest_stale(&conn);

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
            "a manifest known to be behind the mirror must not revert the DB"
        );
        // The file was repaired, not merely ignored — and the file-only item awaiting a Rebuild
        // survived the repair, which is the property `merged_manifest`'s union exists to guarantee.
        let m = read_manifest(dir.path(), &cipher).unwrap().unwrap();
        assert_eq!(
            m.items
                .iter()
                .find(|i| i.source_id == "s1")
                .map(|i| i.project.as_str()),
            Some("Taxes")
        );
        assert!(m.items.iter().any(|i| i.source_id == "awaiting"));
        // The successful write cleared the flag, so the next boot trusts the file again.
        assert!(!manifest_is_stale(&conn));
    }

    #[test]
    fn a_manifest_not_known_to_be_stale_still_wins_on_reconcile() {
        // The contract is unchanged in the normal case: with no recorded lost push, the file remains
        // the portable truth for classification and still applies onto the DB.
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
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
        insert_index_only(&conn, "s1", "Unsorted");

        reconcile_on_open(&conn, dir.path(), &cipher).unwrap();

        let project: String = conn
            .query_row(
                "SELECT project FROM documents WHERE source_id = 's1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(project, "Taxes");
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

    #[test]
    fn reconcile_self_heals_a_mirror_row_missing_from_the_file() {
        // F-20: `register_pointer` commits the DB row before it writes the manifest, so a crash in that
        // window leaves an index-only row in the mirror but absent from the file. `reconcile_on_open`
        // must union it back (mirror→file), or a later Rebuild-from-manifest drops the item entirely.
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        let cipher = ManifestCipher::from_master(VAULT_ID, &MASTER);

        // The file has only s2 (awaiting Rebuild); the DB mirror additionally holds the orphaned s1.
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

        reconcile_on_open(&conn, dir.path(), &cipher).unwrap();

        let ids: std::collections::HashSet<String> = read_manifest(dir.path(), &cipher)
            .unwrap()
            .unwrap()
            .items
            .into_iter()
            .map(|i| i.source_id)
            .collect();
        assert!(
            ids.contains("s1"),
            "the orphaned mirror row is healed into the file"
        );
        assert!(
            ids.contains("s2"),
            "the awaiting-Rebuild file item is preserved, not dropped"
        );
    }

    #[test]
    fn reconcile_does_not_rewrite_a_complete_manifest() {
        // The self-heal is gated: when the file already lists every mirror id, reconcile must not
        // rewrite the manifest. Each write re-nonces the ciphertext, so byte-identical bytes prove no
        // write happened (a normal boot must not churn the file or a backup diff every session).
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        let cipher = ManifestCipher::from_master(VAULT_ID, &MASTER);

        insert_index_only(&conn, "s1", "Unsorted");
        write_manifest(
            dir.path(),
            &cipher,
            &Manifest {
                schema: MANIFEST_SCHEMA,
                items: vec![item("s1", "Unsorted")],
            },
        )
        .unwrap();
        let before = std::fs::read(manifest_path(dir.path())).unwrap();

        reconcile_on_open(&conn, dir.path(), &cipher).unwrap();

        let after = std::fs::read(manifest_path(dir.path())).unwrap();
        assert_eq!(
            before, after,
            "a complete manifest is not needlessly rewritten"
        );
    }

    #[test]
    fn restore_skips_and_heals_a_promoted_source() {
        // F-21: after a promote a non-index-only document owns the source_id, but a crash between the
        // promote's commit and `forget_source` can leave the stale id in the manifest. On Rebuild the
        // vault walk re-creates the promoted doc first, so re-inserting an index-only row would collide
        // on the `source_id` UNIQUE index and fail forever. The guard must skip AND strip the entry.
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        let cipher = ManifestCipher::from_master(VAULT_ID, &MASTER);

        // The manifest still lists s1; the DB has s1 as a PROMOTED (non-index-only) document.
        write_manifest(
            dir.path(),
            &cipher,
            &Manifest {
                schema: MANIFEST_SCHEMA,
                items: vec![item("s1", "Taxes"), item("s2", "Archive")],
            },
        )
        .unwrap();
        insert_promoted(&conn, "s1");

        assert!(
            heal_if_promoted(&conn, dir.path(), &cipher, "s1").unwrap(),
            "a promoted source is detected and skipped"
        );
        let ids: Vec<String> = read_manifest(dir.path(), &cipher)
            .unwrap()
            .unwrap()
            .items
            .into_iter()
            .map(|i| i.source_id)
            .collect();
        assert_eq!(
            ids,
            vec!["s2".to_string()],
            "the stale promoted entry is stripped; other items stay"
        );

        // A genuine index-only source (no promoted owner) is NOT treated as promoted — restore proceeds.
        insert_index_only(&conn, "s2", "Archive");
        assert!(
            !heal_if_promoted(&conn, dir.path(), &cipher, "s2").unwrap(),
            "an index-only source is not mistaken for a promoted one"
        );
    }

    // --- folder-scoped reconcile planner (pure) ---

    struct FakeFile {
        id: String,
        hash: Option<String>,
    }

    impl EnumeratedFile for FakeFile {
        fn local_id(&self) -> &str {
            &self.id
        }
        fn modified_at(&self) -> Option<String> {
            Some("2026-06-26T00:00:00Z".into())
        }
        fn content_hash(&self) -> Option<String> {
            self.hash.clone()
        }
    }

    fn ef(id: &str, hash: &str) -> FakeFile {
        FakeFile {
            id: id.into(),
            hash: Some(hash.into()),
        }
    }

    #[test]
    fn reconcile_enumeration_classifies_present_new_and_absent() {
        // Two items are known-healthy; the live enumeration returns one of them (edited) plus a
        // brand-new one, and drops the other. `source_id_of` namespaces the provider-local id.
        let known: std::collections::HashSet<String> = ["acc:a".to_string(), "acc:b".to_string()]
            .into_iter()
            .collect();
        let files = vec![ef("a", "h-a2"), ef("c", "h-c1")];

        // A COMPLETE enumeration: absence is trustworthy, so the missing known item is deleted.
        let plan = reconcile_enumeration(files, known, true, |id| format!("acc:{id}"));

        // Present files first, in enumeration order, then the single absent-known deletion.
        assert_eq!(plan.len(), 3);

        // "a" was known → Update carrying its new hash; the payload rides along for the body fetch.
        assert_eq!(plan[0].source_id, "acc:a");
        assert_eq!(
            plan[0].event,
            ChangeEvent::Update {
                source_id: "acc:a".into(),
                modified_at: Some("2026-06-26T00:00:00Z".into()),
                new_content_hash: Some("h-a2".into()),
            }
        );
        assert!(plan[0].payload.is_some());

        // "c" was new → Add.
        assert_eq!(plan[1].source_id, "acc:c");
        assert_eq!(
            plan[1].event,
            ChangeEvent::Add {
                source_id: "acc:c".into(),
                modified_at: Some("2026-06-26T00:00:00Z".into()),
            }
        );
        assert!(plan[1].payload.is_some());

        // "b" vanished from the enumeration → soft Delete, with no payload to fetch.
        assert_eq!(plan[2].source_id, "acc:b");
        assert_eq!(
            plan[2].event,
            ChangeEvent::Delete {
                source_id: "acc:b".into()
            }
        );
        assert!(plan[2].payload.is_none());
    }

    #[test]
    fn reconcile_enumeration_skips_deletions_when_truncated() {
        // F-30: the same scenario, but the enumeration was cut short by the page/folder guard
        // (`complete = false`). "b" is absent only because we never reached it — deleting it would
        // wipe a still-present file. So NO Delete is emitted; only the positively-observed Add/Update
        // for the files we did see survive, and "b" is left untouched to retry next pass.
        let known: std::collections::HashSet<String> = ["acc:a".to_string(), "acc:b".to_string()]
            .into_iter()
            .collect();
        let files = vec![ef("a", "h-a2"), ef("c", "h-c1")];

        let plan = reconcile_enumeration(files, known, false, |id| format!("acc:{id}"));

        assert_eq!(plan.len(), 2, "a truncated pass emits no deletions");
        assert!(
            plan.iter()
                .all(|i| !matches!(i.event, ChangeEvent::Delete { .. })),
            "no Delete may be inferred from a truncated enumeration"
        );
        // The present files are still classified exactly as before (Update for known, Add for new).
        assert_eq!(plan[0].source_id, "acc:a");
        assert!(matches!(plan[0].event, ChangeEvent::Update { .. }));
        assert_eq!(plan[1].source_id, "acc:c");
        assert!(matches!(plan[1].event, ChangeEvent::Add { .. }));
    }

    #[test]
    fn reconcile_enumeration_empty_truncation_deletes_nothing() {
        // The worst case F-30 guards: the page/folder guard tripped on the very first page, so the
        // enumeration is EMPTY. Every known item is "absent" — but a complete=false pass must delete
        // none of them (a complete=true empty pass would, correctly, delete them all).
        let known: std::collections::HashSet<String> = [
            "acc:a".to_string(),
            "acc:b".to_string(),
            "acc:c".to_string(),
        ]
        .into_iter()
        .collect();

        let truncated = reconcile_enumeration(Vec::<FakeFile>::new(), known.clone(), false, |id| {
            format!("acc:{id}")
        });
        assert!(
            truncated.is_empty(),
            "an empty truncated enumeration must not delete the whole known set"
        );

        let complete = reconcile_enumeration(Vec::<FakeFile>::new(), known, true, |id| {
            format!("acc:{id}")
        });
        assert_eq!(
            complete.len(),
            3,
            "an empty COMPLETE enumeration means everything really is gone → 3 deletions"
        );
        assert!(complete
            .iter()
            .all(|i| matches!(i.event, ChangeEvent::Delete { .. })));
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

    /// A manifest item with only the fields these tests care about.
    fn manifest_item(source_id: &str, project: &str) -> ManifestItem {
        ManifestItem {
            source_id: source_id.into(),
            title: "T".into(),
            project: project.into(),
            linked_projects: Vec::new(),
            tags: Vec::new(),
            importance: None,
            reviewed: false,
            last_activity: None,
            external_ref: None,
            source_modified_at: None,
            source_content_hash: None,
            source_state: "ok".into(),
            stored_summary: Some("s".into()),
            locations: Vec::new(),
        }
    }

    #[test]
    fn a_manifest_naming_an_unknown_project_reports_the_mint() {
        // F-04. The reconcile resolves each item's project with create_if_new, so a manifest naming
        // a project the mirror lacks MINTS an entity right here — but this pass runs at boot AFTER
        // the entity-rules reconcile, which treats the rules FILE as truth. Unless the caller is
        // told to push the file, next boot rolls the mint back, this pass mints it again, and the
        // loop never ends — leaving any project-scoped preference on that entity dormant forever.
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        insert_index_only(&conn, "s1", "Unsorted");

        let manifest = Manifest {
            items: vec![manifest_item("s1", "Taxes")],
            ..Default::default()
        };
        assert!(
            apply_classification(&conn, &manifest).unwrap(),
            "a project the mirror has never seen is a mint the rules file must learn about"
        );
        // And the mint really happened — the report isn't just a flag.
        assert!(crate::entities::resolve_project(&conn, "Taxes", false)
            .unwrap()
            .is_some());
    }

    #[test]
    fn a_manifest_of_known_projects_reports_no_mint() {
        // The overwhelmingly common case: items land as the seeded 'Unsorted'. Reporting a mint
        // here would rewrite the encrypted rules file on every single boot for nothing.
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        insert_index_only(&conn, "s1", "Unsorted");
        crate::entities::resolve_project(&conn, "Taxes", true).unwrap();

        let manifest = Manifest {
            items: vec![manifest_item("s1", "Taxes")],
            ..Default::default()
        };
        assert!(
            !apply_classification(&conn, &manifest).unwrap(),
            "an already-known project is not a mint"
        );
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

    // --- source-fact freshness (#708) ---

    /// The columns `refresh_source_facts` touches, and nothing else.
    fn facts_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE documents (
                 id INTEGER PRIMARY KEY, source_type TEXT, source_id TEXT,
                 source_author TEXT, source_last_modified_by TEXT, source_created_at TEXT,
                 source_size_bytes INTEGER, source_modified_at TEXT,
                 source_parent_folder_id TEXT, source_parent_folder_name TEXT,
                 pm_refreshed_at TEXT
             );
             CREATE TABLE document_locations (
                 id INTEGER PRIMARY KEY, document_id INTEGER, source_id TEXT UNIQUE,
                 source_state TEXT, external_ref TEXT, source_modified_at TEXT,
                 source_content_hash TEXT, source_parent_folder_id TEXT,
                 source_parent_folder_name TEXT, first_seen_at TEXT
             );
             INSERT INTO documents(source_type, source_id, source_author, source_last_modified_by,
                 source_created_at, source_size_bytes, source_modified_at,
                 source_parent_folder_id, source_parent_folder_name, pm_refreshed_at)
             VALUES ('index_only','s1','Ada Lovelace','Grace Hopper','2026-01-01T00:00:00Z',
                     1024,'2026-05-01T00:00:00Z','fid-1','Invoices', NULL);
             INSERT INTO document_locations(document_id, source_id, source_state,
                 source_modified_at, source_parent_folder_id, source_parent_folder_name)
             VALUES (1,'s1','ok','2026-05-01T00:00:00Z','fid-1','Invoices');",
        )
        .unwrap();
        conn
    }

    fn stored(conn: &Connection) -> (Option<String>, Option<i64>, Option<String>, Option<String>) {
        conn.query_row(
            "SELECT source_author, source_size_bytes, source_parent_folder_name, pm_refreshed_at              FROM documents WHERE source_id = 's1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap()
    }

    fn as_stored() -> SourceFacts {
        SourceFacts {
            author: Some("Ada Lovelace".into()),
            last_modified_by: Some("Grace Hopper".into()),
            created_at: Some("2026-01-01T00:00:00Z".into()),
            size_bytes: Some(1024),
            modified_at: Some("2026-05-01T00:00:00Z".into()),
            parent_folder_id: Some("fid-1".into()),
            parent_folder_name: Some("Invoices".into()),
        }
    }

    #[test]
    fn an_item_that_has_not_changed_writes_nothing_at_all() {
        // This runs for every item of every account on every pass, including the fifteen-minute
        // poll. On a settled library it must not dirty a single page — so "nothing to say" has to
        // mean zero rows matched, not an UPDATE that happens to assign the same values back.
        let conn = facts_db();
        assert!(!refresh_source_facts(&conn, "s1", &as_stored(), "2026-08-02T10:00:00Z").unwrap());
        let (_, _, _, refreshed) = stored(&conn);
        assert_eq!(refreshed, None, "an idle pass leaves no stamp");
    }

    #[test]
    fn a_file_that_was_resized_is_updated_and_stamped() {
        let conn = facts_db();
        let facts = SourceFacts {
            size_bytes: Some(2048),
            ..as_stored()
        };
        assert!(refresh_source_facts(&conn, "s1", &facts, "2026-08-02T10:00:00Z").unwrap());
        let (_, size, _, refreshed) = stored(&conn);
        assert_eq!(size, Some(2048));
        assert_eq!(refreshed.as_deref(), Some("2026-08-02T10:00:00Z"));
    }

    #[test]
    fn a_fact_the_provider_stops_reporting_is_kept_not_erased() {
        // Drive genuinely stops sending `owners` once a file moves into a shared drive, and a
        // Google-native document has no `size` at all. A wholesale assignment would quietly erase an
        // author PM had already been told — "Unknown" must mean never told, not forgotten.
        let conn = facts_db();
        let facts = SourceFacts {
            author: None,
            size_bytes: Some(4096),
            ..as_stored()
        };
        assert!(refresh_source_facts(&conn, "s1", &facts, "2026-08-02T10:00:00Z").unwrap());
        let (author, size, _, _) = stored(&conn);
        assert_eq!(
            author.as_deref(),
            Some("Ada Lovelace"),
            "silence erases nothing"
        );
        assert_eq!(size, Some(4096), "while what it DID say still lands");
    }

    #[test]
    fn an_all_empty_refresh_never_reaches_the_database() {
        let conn = facts_db();
        assert!(!refresh_source_facts(&conn, "s1", &SourceFacts::default(), "t").unwrap());
        let (author, size, _, refreshed) = stored(&conn);
        assert_eq!(author.as_deref(), Some("Ada Lovelace"));
        assert_eq!(size, Some(1024));
        assert_eq!(refreshed, None);
    }

    #[test]
    fn a_moved_file_updates_the_folder_it_now_sits_in() {
        let conn = facts_db();
        let facts = SourceFacts {
            parent_folder_id: Some("fid-2".into()),
            parent_folder_name: Some("Archive 2026".into()),
            ..as_stored()
        };
        assert!(refresh_source_facts(&conn, "s1", &facts, "2026-08-02T10:00:00Z").unwrap());
        let (_, _, folder, _) = stored(&conn);
        assert_eq!(folder.as_deref(), Some("Archive 2026"));
        assert_eq!(
            stored_parent_folder_id(&conn, "s1").unwrap().as_deref(),
            Some("fid-2"),
            "and the id the next pass compares against moves with it"
        );
    }

    #[test]
    fn a_size_beyond_the_float_range_round_trips() {
        // Drive sends `size` as a decimal STRING precisely because it can exceed 2^53; the column is
        // INTEGER and the parse is i64, so nothing in the path may narrow it.
        let conn = facts_db();
        let huge = 9_007_199_254_740_993i64; // 2^53 + 1
        let facts = SourceFacts {
            size_bytes: Some(huge),
            ..as_stored()
        };
        assert!(refresh_source_facts(&conn, "s1", &facts, "t").unwrap());
        assert_eq!(stored(&conn).1, Some(huge));
    }
}
