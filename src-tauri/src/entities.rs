// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Canonical-entity resolution (spec §8.5, Stage 3). The fix for project-name variant
//! drift: today the *name is the identity* (`documents.project` is free text), so "PM",
//! "Personal Manager" and "Atlas - PM" are three co-equal projects that keep reappearing.
//! Here identity is separated from name — an `entities` row is the stable identity and
//! `entity_aliases` maps every known name string (the canonical included, as a self-alias)
//! to exactly one entity, so a review correction becomes a *forward-going rule*, not a
//! one-off row-patch.
//!
//! **Source of truth = a portable, always-encrypted rules file** at the data-home root
//! (next to `pm.sqlite`). The `entities` / `entity_aliases` tables are its queryable
//! **mirror**, rebuilt from the file at session open — so a vault copied to another device,
//! or one whose mirror was dropped by a future schema change, comes back from the file. The
//! integer ids are an index detail and are NOT stored in the file; the mirror reassigns them
//! on rebuild and re-points `documents.entity_id` by resolving each document's canonical-name
//! cache through the rebuilt aliases (the rebuild-painful invariant, handled in one place).
//!
//! The rules file is encrypted with the **Markdown-at-rest subkey** (XChaCha20-Poly1305 via
//! [`crate::vault::crypto`]) — the same primitive the vault uses, NOT a second mechanism — but
//! ALWAYS, even for device vaults whose Markdown is plaintext: an alias map of projects (and
//! later people) is more revealing than any single document, so it gets parity with the
//! always-encrypted SQLCipher store.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::{Error, Result};
use crate::vault::crypto;

/// The only entity type populated in PR 1 (the `person`/`thing` seam is banked in the schema).
pub const TYPE_PROJECT: &str = "project";

/// Current rules-file schema version.
const RULES_SCHEMA: u32 = 1;
/// Filename of the encrypted rules file, at the data-home root next to `pm.sqlite`.
pub const RULES_FILENAME: &str = "entities.pmrules";
/// AAD stem binding the rules ciphertext to its logical identity (mirrors the Markdown AAD).
const RULES_AAD_STEM: &str = "entities";

// --- mirror types -----------------------------------------------------------

/// One entity with its aliases, as exposed to the command surface / Teach tab (PR 2).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entity {
    pub id: i64,
    #[serde(rename = "type")]
    pub kind: String,
    pub canonical_name: String,
    /// All known aliases for this entity, the canonical self-alias included.
    pub aliases: Vec<String>,
}

/// The portable rules-file shape (the encrypted source of truth). Integer ids are an index
/// detail and deliberately absent — the mirror reassigns them on rebuild.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Rules {
    pub schema: u32,
    pub entities: Vec<RuleEntity>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuleEntity {
    #[serde(rename = "type")]
    pub kind: String,
    pub canonical_name: String,
    pub aliases: Vec<String>,
}

// --- deterministic resolution (the heart of the fix) ------------------------

/// Resolve a name to a project entity id via the alias table (exact, trimmed match). With
/// `create_if_new`, a name that matches nothing creates a new project entity (canonical +
/// self-alias) and returns it; without, returns `None`. Deterministic — the same name always
/// resolves the same way, which is what makes "only invent a new project if none fit" exact
/// rather than a soft LLM guess.
pub fn resolve_project(conn: &Connection, name: &str, create_if_new: bool) -> Result<Option<i64>> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(None);
    }
    if let Some(id) = lookup_alias(conn, name)? {
        return Ok(Some(id));
    }
    if !create_if_new {
        return Ok(None);
    }
    Ok(Some(create_project(conn, name)?))
}

/// The project entity an alias resolves to, or `None`. Exact trimmed match (case-sensitive —
/// the variant problem is distinct strings, not case, and case-folding could collapse genuinely
/// distinct names).
fn lookup_alias(conn: &Connection, alias: &str) -> Result<Option<i64>> {
    conn.query_row(
        "SELECT ea.entity_id FROM entity_aliases ea \
         JOIN entities e ON e.id = ea.entity_id \
         WHERE e.type = 'project' AND ea.alias = ?1",
        params![alias.trim()],
        |r| r.get::<_, i64>(0),
    )
    .optional()
    .map_err(Error::from)
}

/// Create a new project entity with `name` as canonical + self-alias, returning its id. Reuses
/// an existing entity with that canonical (idempotent under a race / a missing self-alias).
fn create_project(conn: &Connection, name: &str) -> Result<i64> {
    let name = name.trim();
    if let Some(id) = conn
        .query_row(
            "SELECT id FROM entities WHERE type = 'project' AND canonical_name = ?1",
            params![name],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
    {
        ensure_alias_row(conn, id, name)?;
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO entities(type, canonical_name) VALUES ('project', ?1)",
        params![name],
    )?;
    let id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO entity_aliases(entity_id, alias) VALUES (?1, ?2)",
        params![id, name],
    )?;
    Ok(id)
}

/// Add `alias` for `entity_id` if it is free (no-op if already this entity's; never steals one
/// owned by another entity). `INSERT OR IGNORE` would silently swallow a steal, so we look first.
fn ensure_alias_row(conn: &Connection, entity_id: i64, alias: &str) -> Result<()> {
    let alias = alias.trim();
    match lookup_alias(conn, alias)? {
        Some(owner) if owner == entity_id => Ok(()),
        Some(_) => Ok(()), // owned elsewhere — leave it; resolution stays deterministic
        None => {
            conn.execute(
                "INSERT INTO entity_aliases(entity_id, alias) VALUES (?1, ?2)",
                params![entity_id, alias],
            )?;
            Ok(())
        }
    }
}

/// Outcome of an [`add_alias`] call, so a caller can react to a clash rather than fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddAlias {
    /// A new alias row was created.
    Added,
    /// The alias already belonged to this entity (no change).
    Existed,
    /// The alias is owned by another entity (`other`) — capturing it would be a *merge*, not an
    /// alias. The caller surfaces it for an explicit merge; it is NOT silently folded (§1.5).
    Conflict(i64),
}

/// Record `alias` as a forward-going rule for `entity_id`. The merge guard: an alias already
/// owned by another entity returns [`AddAlias::Conflict`] instead of being stolen.
pub fn add_alias(conn: &Connection, entity_id: i64, alias: &str) -> Result<AddAlias> {
    let alias = alias.trim();
    if alias.is_empty() {
        return Ok(AddAlias::Existed);
    }
    match lookup_alias(conn, alias)? {
        Some(owner) if owner == entity_id => Ok(AddAlias::Existed),
        Some(other) => Ok(AddAlias::Conflict(other)),
        None => {
            conn.execute(
                "INSERT INTO entity_aliases(entity_id, alias) VALUES (?1, ?2)",
                params![entity_id, alias],
            )?;
            touch_entity(conn, entity_id)?;
            Ok(AddAlias::Added)
        }
    }
}

/// Point one document at `entity_id` — the *reassignment* case (a misfile), distinct from a
/// *merge*. Mirror-only; the caller rewrites the document's canonical-name cache + vault
/// frontmatter to match.
pub fn reassign_document(conn: &Connection, doc_id: i64, entity_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE documents SET entity_id = ?2 WHERE id = ?1",
        params![doc_id, entity_id],
    )?;
    Ok(())
}

/// Rename a canonical project — a one-row update (plus a self-alias for the new name), the payoff
/// of identity-not-name. The caller repoints the denormalised `documents.project` cache + vault
/// frontmatter of the entity's documents to the new canonical.
pub fn rename_entity(conn: &Connection, entity_id: i64, new_canonical: &str) -> Result<String> {
    let new_canonical = new_canonical.trim();
    if new_canonical.is_empty() {
        return Err(Error::Other("a project name is required".into()));
    }
    // Refuse to collide with a different existing project's canonical (that's a merge).
    if let Some(other) = conn
        .query_row(
            "SELECT id FROM entities WHERE type = 'project' AND canonical_name = ?1",
            params![new_canonical],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
    {
        if other != entity_id {
            return Err(Error::Other(format!(
                "a project named \"{new_canonical}\" already exists — merge instead of renaming"
            )));
        }
    }
    conn.execute(
        "UPDATE entities SET canonical_name = ?2, \
         updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?1",
        params![entity_id, new_canonical],
    )?;
    ensure_alias_row(conn, entity_id, new_canonical)?;
    Ok(new_canonical.to_string())
}

/// Fold `from_id` into `into_id`: move its aliases, repoint its documents + triage rows, delete
/// the now-empty source. Mirror-only — the caller rewrites the moved documents' canonical-name
/// cache + vault frontmatter to the target's canonical. A no-op if `from == into`.
pub fn merge_entities(conn: &Connection, from_id: i64, into_id: i64) -> Result<()> {
    if from_id == into_id {
        return Ok(());
    }
    // Aliases are globally unique, so each belongs to exactly one entity — moving them never
    // collides. The canonical self-alias of the source comes along, becoming a plain alias of
    // the target (the variant now resolves to the canonical, so it never recurs).
    conn.execute(
        "UPDATE entity_aliases SET entity_id = ?2 WHERE entity_id = ?1",
        params![from_id, into_id],
    )?;
    conn.execute(
        "UPDATE documents SET entity_id = ?2 WHERE entity_id = ?1",
        params![from_id, into_id],
    )?;
    conn.execute(
        "UPDATE projects SET entity_id = ?2 WHERE entity_id = ?1",
        params![from_id, into_id],
    )?;
    conn.execute("DELETE FROM entities WHERE id = ?1", params![from_id])?;
    touch_entity(conn, into_id)?;
    Ok(())
}

/// Every entity of a type with its aliases, alphabetical by canonical name (the Teach tab's list).
pub fn list_entities(conn: &Connection, kind: &str) -> Result<Vec<Entity>> {
    let mut stmt = conn.prepare(
        "SELECT id, type, canonical_name FROM entities WHERE type = ?1 ORDER BY canonical_name",
    )?;
    let heads: Vec<(i64, String, String)> = stmt
        .query_map(params![kind], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<std::result::Result<_, _>>()?;
    drop(stmt);

    let mut out = Vec::with_capacity(heads.len());
    for (id, kind, canonical_name) in heads {
        out.push(Entity {
            id,
            kind,
            canonical_name,
            aliases: aliases_of(conn, id)?,
        });
    }
    Ok(out)
}

/// The aliases of one entity, alphabetical.
fn aliases_of(conn: &Connection, entity_id: i64) -> Result<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT alias FROM entity_aliases WHERE entity_id = ?1 ORDER BY alias")?;
    let rows = stmt
        .query_map(params![entity_id], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// The canonical name of an entity id.
pub fn canonical_name(conn: &Connection, entity_id: i64) -> Result<String> {
    conn.query_row(
        "SELECT canonical_name FROM entities WHERE id = ?1",
        params![entity_id],
        |r| r.get(0),
    )
    .map_err(Error::from)
}

/// Canonical project names only (one per entity) — the list handed to the proposal prompt, so the
/// model is never offered variants to choose between.
pub fn canonical_project_names(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT canonical_name FROM entities WHERE type = 'project' ORDER BY canonical_name",
    )?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Map a model-proposed string to the canonical name it resolves to (read-only — no entity is
/// created for an un-confirmed proposal). An unknown string is returned trimmed and unchanged: a
/// genuinely new project the user can confirm, at which point the commit creates the entity.
pub fn resolve_to_canonical(conn: &Connection, name: &str) -> Result<String> {
    match resolve_project(conn, name, false)? {
        Some(id) => canonical_name(conn, id),
        None => Ok(name.trim().to_string()),
    }
}

fn touch_entity(conn: &Connection, entity_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE entities SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?1",
        params![entity_id],
    )?;
    Ok(())
}

// --- the encrypted rules file (the portable source of truth) ----------------

/// Always-on encryption for the rules file, reusing the Markdown-at-rest crypto primitive
/// (XChaCha20-Poly1305 under the Markdown subkey). Distinct from a device vault's plaintext
/// `MarkdownCipher`: the subkey is derived even for device vaults, so the rules file is ALWAYS
/// ciphertext — parity with the always-encrypted store.
#[derive(Clone)]
pub struct RulesCipher {
    vault_id: String,
    subkey: Zeroizing<[u8; 32]>,
}

impl RulesCipher {
    /// Build from the vault id + the resolved 32-byte master (the same input the Markdown cipher
    /// uses), deriving the Markdown subkey regardless of the vault's Markdown policy.
    pub fn from_master(vault_id: &str, master: &[u8; 32]) -> Self {
        Self {
            vault_id: vault_id.to_string(),
            subkey: crate::vault::markdown_subkey(master),
        }
    }

    fn encrypt(&self, rules: &Rules) -> Result<Vec<u8>> {
        let json =
            serde_json::to_vec(rules).map_err(|e| Error::Other(format!("encode rules: {e}")))?;
        crypto::encrypt(&json, &self.subkey, &self.vault_id, RULES_AAD_STEM)
    }

    fn decrypt(&self, bytes: &[u8]) -> Result<Rules> {
        let plain = crypto::decrypt(bytes, &self.subkey, &self.vault_id, RULES_AAD_STEM)?;
        serde_json::from_slice(&plain).map_err(|e| Error::Other(format!("decode rules: {e}")))
    }
}

/// Path to the rules file at the vault root (next to `pm.sqlite`, one level up from `vault/`).
pub fn rules_path(vault_root: &Path) -> PathBuf {
    vault_root.join(RULES_FILENAME)
}

/// Read + decrypt the rules file, or `None` if it doesn't exist yet. A decrypt failure (e.g. the
/// vault key rotated without the file being re-encrypted) surfaces as an error so the caller can
/// self-heal from the mirror.
pub fn read_rules_file(vault_root: &Path, cipher: &RulesCipher) -> Result<Option<Rules>> {
    match std::fs::read(rules_path(vault_root)) {
        Ok(bytes) => Ok(Some(cipher.decrypt(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Write the rules file atomically (temp + rename), returning the prior raw bytes (empty if none)
/// so a caller can restore it should a surrounding DB transaction fail to commit.
pub fn write_rules_file(vault_root: &Path, cipher: &RulesCipher, rules: &Rules) -> Result<Vec<u8>> {
    let path = rules_path(vault_root);
    let prior = std::fs::read(&path).unwrap_or_default();
    let bytes = cipher.encrypt(rules)?;
    let tmp = path.with_extension("pmrules.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(prior)
}

/// Restore the rules file to prior bytes (or remove it if there were none) — the file half of
/// rolling back an abandoned mutation, mirroring [`crate::ingest::restore_vault_files`].
pub fn restore_rules_file(vault_root: &Path, prior: &[u8]) {
    let path = rules_path(vault_root);
    if prior.is_empty() {
        let _ = std::fs::remove_file(&path);
    } else {
        let _ = std::fs::write(&path, prior);
    }
}

// --- mirror <-> file reconciliation -----------------------------------------

/// Serialize the whole mirror (entities + their aliases) into the portable rules shape.
pub fn rules_from_mirror(conn: &Connection) -> Result<Rules> {
    let mut stmt = conn
        .prepare("SELECT id, type, canonical_name FROM entities ORDER BY type, canonical_name")?;
    let heads: Vec<(i64, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<std::result::Result<_, _>>()?;
    drop(stmt);

    let mut entities = Vec::with_capacity(heads.len());
    for (id, kind, canonical_name) in heads {
        entities.push(RuleEntity {
            kind,
            canonical_name,
            aliases: aliases_of(conn, id)?,
        });
    }
    Ok(Rules {
        schema: RULES_SCHEMA,
        entities,
    })
}

/// Whether the mirror already equals `rules` exactly — lets the boot reconcile skip a churny
/// rebuild (which would reassign ids + re-point documents needlessly) when nothing changed.
fn mirror_matches(conn: &Connection, rules: &Rules) -> Result<bool> {
    Ok(&rules_from_mirror(conn)? == rules)
}

/// Rebuild the mirror (entities + entity_aliases) from the rules file, then re-point
/// `documents.entity_id` + `projects.entity_id` by resolving their canonical-name caches through
/// the rebuilt aliases. Ids are reassigned (an index detail), which is exactly why the document
/// pointers are re-resolved here rather than carried (invariant: entity_id is reassigned on
/// rebuild by resolving the frontmatter/cache name through the rules). Idempotent.
pub fn rebuild_mirror_from_rules(conn: &Connection, rules: &Rules) -> Result<()> {
    // Drop the pointers first so deleting the entities they reference doesn't trip the FK.
    conn.execute("UPDATE documents SET entity_id = NULL", [])?;
    conn.execute("UPDATE projects SET entity_id = NULL", [])?;
    conn.execute("DELETE FROM entity_aliases", [])?;
    conn.execute("DELETE FROM entities", [])?;

    for e in &rules.entities {
        conn.execute(
            "INSERT INTO entities(type, canonical_name) VALUES (?1, ?2)",
            params![e.kind, e.canonical_name],
        )?;
        let id = conn.last_insert_rowid();
        for alias in &e.aliases {
            ensure_alias_row(conn, id, alias)?;
        }
        // Guarantee the canonical is always a resolvable self-alias, even if a file omitted it.
        ensure_alias_row(conn, id, &e.canonical_name)?;
    }

    conn.execute(
        "UPDATE documents SET entity_id = (SELECT ea.entity_id FROM entity_aliases ea \
         JOIN entities e ON e.id = ea.entity_id \
         WHERE e.type = 'project' AND ea.alias = documents.project)",
        [],
    )?;
    conn.execute(
        "UPDATE projects SET entity_id = (SELECT ea.entity_id FROM entity_aliases ea \
         JOIN entities e ON e.id = ea.entity_id \
         WHERE e.type = 'project' AND ea.alias = projects.name)",
        [],
    )?;
    Ok(())
}

/// Reconcile the encrypted rules file with the DB mirror at session open. The file is the
/// portable source of truth: when present it rebuilds the mirror (so a vault copied to another
/// device, or one whose mirror was wiped, returns from the file). When ABSENT — first run after
/// the v10 backfill — or UNDECRYPTABLE — a key rotation that didn't re-encrypt it — the file is
/// (re)written from the mirror (the one bootstrap direction; the mirror survives a SQLCipher
/// rekey intact).
pub fn reconcile_on_open(conn: &Connection, vault_root: &Path, cipher: &RulesCipher) -> Result<()> {
    match read_rules_file(vault_root, cipher) {
        Ok(Some(rules)) => {
            if !mirror_matches(conn, &rules)? {
                let tx = conn.unchecked_transaction()?;
                rebuild_mirror_from_rules(&tx, &rules)?;
                tx.commit()?;
            }
            Ok(())
        }
        Ok(None) => {
            write_rules_file(vault_root, cipher, &rules_from_mirror(conn)?)?;
            Ok(())
        }
        Err(e) => {
            // A corrupt or post-rotation-undecryptable file lands here. The mirror is the live
            // truth, so heal by rewriting the file under the current key rather than failing boot.
            eprintln!("entities: rules file unreadable ({e}); rewriting it from the DB mirror");
            write_rules_file(vault_root, cipher, &rules_from_mirror(conn)?)?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    /// A store with the full schema (incl. the v10 entity tables) and one document.
    fn store_with_doc(project: &str) -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), KEY).unwrap();
        conn.execute(
            "INSERT INTO documents(vault_path, title, content_hash, project) \
             VALUES ('a.md', 'A', 'h-a', ?1)",
            params![project],
        )
        .unwrap();
        let doc_id = conn.last_insert_rowid();
        // Mirror the v10 backfill for this hand-inserted row (the migration ran on an empty store).
        // Capture the doc id *before* resolve_project — it does its own INSERTs, so reading
        // last_insert_rowid afterwards would point at an alias row, not the document.
        let id = resolve_project(&conn, project, true).unwrap().unwrap();
        reassign_document(&conn, doc_id, id).unwrap();
        (dir, conn)
    }

    #[test]
    fn resolve_is_deterministic_and_creates_only_when_asked() {
        let (_d, conn) = store_with_doc("PM");
        let pm = resolve_project(&conn, "PM", false).unwrap().unwrap();
        // A trimmed match resolves to the same entity; whitespace is not a new project.
        assert_eq!(resolve_project(&conn, "  PM  ", false).unwrap(), Some(pm));
        // An unknown name does not resolve without create.
        assert_eq!(resolve_project(&conn, "Research", false).unwrap(), None);
        // ...and creates exactly one entity with create.
        let research = resolve_project(&conn, "Research", true).unwrap().unwrap();
        assert_ne!(pm, research);
        assert_eq!(
            resolve_project(&conn, "Research", false).unwrap(),
            Some(research)
        );
    }

    #[test]
    fn alias_makes_a_variant_resolve_to_canonical() {
        let (_d, conn) = store_with_doc("PM");
        let pm = resolve_project(&conn, "PM", false).unwrap().unwrap();
        // Record the rule "Atlas - PM → PM", then a proposal of the variant resolves to PM.
        assert_eq!(add_alias(&conn, pm, "Atlas - PM").unwrap(), AddAlias::Added);
        assert_eq!(
            resolve_project(&conn, "Atlas - PM", false).unwrap(),
            Some(pm)
        );
        assert_eq!(resolve_to_canonical(&conn, "Atlas - PM").unwrap(), "PM");
        // Re-adding is a no-op; an alias owned elsewhere is a conflict, never stolen.
        assert_eq!(
            add_alias(&conn, pm, "Atlas - PM").unwrap(),
            AddAlias::Existed
        );
        let research = resolve_project(&conn, "Research", true).unwrap().unwrap();
        assert_eq!(
            add_alias(&conn, research, "Atlas - PM").unwrap(),
            AddAlias::Conflict(pm)
        );
    }

    #[test]
    fn merge_folds_aliases_and_repoints_documents() {
        let (_d, conn) = store_with_doc("Atlas - PM");
        let variant = resolve_project(&conn, "Atlas - PM", false)
            .unwrap()
            .unwrap();
        let pm = resolve_project(&conn, "PM", true).unwrap().unwrap();
        // The doc starts on the variant; merge it into PM.
        merge_entities(&conn, variant, pm).unwrap();
        // The variant entity is gone, its alias now resolves to PM, and the doc points at PM.
        assert_eq!(
            resolve_project(&conn, "Atlas - PM", false).unwrap(),
            Some(pm)
        );
        let doc_entity: i64 = conn
            .query_row(
                "SELECT entity_id FROM documents WHERE vault_path='a.md'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(doc_entity, pm);
        let remaining: i64 = conn
            .query_row(
                "SELECT count(*) FROM entities WHERE id=?1",
                params![variant],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn rename_is_one_row_and_blocks_collisions() {
        let (_d, conn) = store_with_doc("PM");
        let pm = resolve_project(&conn, "PM", false).unwrap().unwrap();
        let _research = resolve_project(&conn, "Research", true).unwrap().unwrap();
        rename_entity(&conn, pm, "Personal Manager").unwrap();
        assert_eq!(canonical_name(&conn, pm).unwrap(), "Personal Manager");
        // The new canonical resolves; renaming onto another project's canonical is refused.
        assert_eq!(
            resolve_project(&conn, "Personal Manager", false).unwrap(),
            Some(pm)
        );
        assert!(rename_entity(&conn, pm, "Research").is_err());
    }

    #[test]
    fn mirror_round_trips_through_rules() {
        let (_d, conn) = store_with_doc("PM");
        let pm = resolve_project(&conn, "PM", false).unwrap().unwrap();
        add_alias(&conn, pm, "Atlas - PM").unwrap();
        resolve_project(&conn, "Research", true).unwrap();

        // Snapshot → rebuild from it reproduces an identical mirror and keeps the document linked.
        let rules = rules_from_mirror(&conn).unwrap();
        rebuild_mirror_from_rules(&conn, &rules).unwrap();
        assert!(mirror_matches(&conn, &rules).unwrap());
        // The document's entity_id was reassigned by resolving its canonical cache (id may differ).
        let pm2 = resolve_project(&conn, "PM", false).unwrap().unwrap();
        let doc_entity: i64 = conn
            .query_row(
                "SELECT entity_id FROM documents WHERE vault_path='a.md'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(doc_entity, pm2);
        // The alias survived the round-trip.
        assert_eq!(
            resolve_project(&conn, "Atlas - PM", false).unwrap(),
            Some(pm2)
        );
    }

    #[test]
    fn rules_file_is_encrypted_at_rest_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let master = [7u8; 32];
        let cipher = RulesCipher::from_master("vault-xyz", &master);
        let rules = Rules {
            schema: RULES_SCHEMA,
            entities: vec![RuleEntity {
                kind: TYPE_PROJECT.into(),
                canonical_name: "Medical".into(),
                aliases: vec!["Medical".into(), "Health".into()],
            }],
        };
        let prior = write_rules_file(dir.path(), &cipher, &rules).unwrap();
        assert!(prior.is_empty(), "no prior file existed");

        // On disk it is a PMVAULT1 container, and the revealing names are NOT plaintext.
        let raw = std::fs::read(rules_path(dir.path())).unwrap();
        assert!(crypto::is_encrypted(&raw), "rules file must be ciphertext");
        assert!(
            !raw.windows(7).any(|w| w == b"Medical"),
            "a project name must not appear in plaintext on disk"
        );
        // It decrypts back to the same rules; the wrong vault id fails authentication.
        assert_eq!(read_rules_file(dir.path(), &cipher).unwrap(), Some(rules));
        let other = RulesCipher::from_master("vault-other", &master);
        assert!(read_rules_file(dir.path(), &other).is_err());
        // No file yet → None.
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(read_rules_file(empty.path(), &cipher).unwrap(), None);
    }

    #[test]
    fn reconcile_writes_then_rebuilds_from_the_file() {
        let (dir, conn) = store_with_doc("PM");
        let cipher = RulesCipher::from_master("vault-xyz", &[9u8; 32]);
        resolve_project(&conn, "Research", true).unwrap();

        // First open: no file yet → it is written from the mirror.
        assert_eq!(read_rules_file(dir.path(), &cipher).unwrap(), None);
        reconcile_on_open(&conn, dir.path(), &cipher).unwrap();
        let on_disk = read_rules_file(dir.path(), &cipher).unwrap().unwrap();
        assert_eq!(on_disk, rules_from_mirror(&conn).unwrap());

        // Simulate a wiped mirror (a future drop-recreate); reconcile restores it from the file.
        conn.execute("UPDATE documents SET entity_id = NULL", [])
            .unwrap();
        conn.execute("DELETE FROM entity_aliases", []).unwrap();
        conn.execute("DELETE FROM entities", []).unwrap();
        reconcile_on_open(&conn, dir.path(), &cipher).unwrap();
        assert_eq!(resolve_to_canonical(&conn, "PM").unwrap(), "PM");
        let doc_entity: Option<i64> = conn
            .query_row(
                "SELECT entity_id FROM documents WHERE vault_path='a.md'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(doc_entity, resolve_project(&conn, "PM", false).unwrap());
    }
}
