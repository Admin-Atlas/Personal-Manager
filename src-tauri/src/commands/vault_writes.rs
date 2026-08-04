// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The atomic "DB transaction ⊕ vault files ⊕ rules mirror" tail every vault-mutating pass
//! rides.
//!
//! The invariant its tests pin: a failure on the files-already-rewritten side must roll the
//! FILES back too, not just the database. A committed DB with reverted files (or the
//! reverse) leaves the mirror and the truth disagreeing, and nothing reports it until a
//! Rebuild quietly resurrects the old data. One tail, so there is one place to get it right.

use rusqlite::{params, Connection, OptionalExtension};
use tauri::{AppHandle, Manager};

use crate::blocking::spawn_blocking_result;
use crate::error::Result;
use crate::ingest;
use crate::{entities, index_only, vault, AppState};

/// Commit a pass that rewrote vault files inside `tx`, or roll BOTH halves back — the DB *and*
/// every file the pass touched.
///
/// Shared because getting this wrong is silent: a committed DB with reverted files (or the reverse)
/// leaves the mirror and the truth disagreeing, and nothing reports it until a Rebuild quietly
/// resurrects the old data. It went wrong exactly that way — `commit_review` hand-rolled this tail
/// and wrote the rules file with a bare `?`, so a disk-full or AV-locked vault rolled the database
/// back while leaving a whole review pass rewritten on disk, which the next Rebuild then adopted.
/// One tail, so there is one place to get it right.
///
/// `rules` is `Some((vault_root, cipher))` for the passes that also mutate the entity mirror. The
/// rules file is written from the still-uncommitted mirror BEFORE `tx.commit()` deliberately — a
/// captured rule must be exactly as durable as the commit that created it — and is put back if the
/// commit then fails. Reading the mirror is inside the same guarded region as writing it: both sit
/// on the file-already-rewritten side of the commit, so both owe the same rollback.
///
/// Takes `tx` BY VALUE on purpose: `set_document_metadata` reads `conn` after the tail, which only
/// compiles because consuming the transaction releases the borrow. `&mut Transaction` breaks it.
///
/// Deliberately does NOT cover after-commit work (`spawn_entity_mutation`'s `unlink` /
/// `forget_sources`). Those are irreversible and wait for a durable commit by decision — see
/// [`MutationFiles`] — and folding an irreversible step into a helper named for rollback is how the
/// original bug happened.
pub(super) fn finish_vault_transaction<T>(
    tx: rusqlite::Transaction<'_>,
    written: Vec<(std::path::PathBuf, Vec<u8>)>,
    rules: Option<(&std::path::Path, &entities::RulesCipher)>,
    result: Result<T>,
) -> Result<T> {
    let applied = match result {
        Ok(applied) => applied,
        Err(e) => {
            drop(tx); // roll back the DB side
            ingest::restore_vault_files(written);
            return Err(e);
        }
    };
    let prior_rules = match rules {
        Some((vault_root, cipher)) => {
            match entities::rules_from_mirror(&tx)
                .and_then(|r| entities::write_rules_file(vault_root, cipher, &r))
            {
                Ok(prior) => Some((vault_root, prior)),
                Err(e) => {
                    drop(tx);
                    ingest::restore_vault_files(written);
                    return Err(e);
                }
            }
        }
        None => None,
    };
    match tx.commit() {
        Ok(()) => Ok(applied),
        Err(e) => {
            if let Some((vault_root, prior)) = prior_rules {
                entities::restore_rules_file(vault_root, &prior);
            }
            ingest::restore_vault_files(written);
            Err(e.into())
        }
    }
}

/// What a mirror mutation did to the filesystem: files it REWROTE (snapshotted so a failed commit
/// can put them back), and files it wants UNLINKED — but only once the commit is durable.
///
/// The two halves are deliberately asymmetric, and it is the same asymmetry `chat.rs` reasons
/// about: a rewrite can be undone from its snapshot, so it happens before the commit and is
/// restored on failure; a delete cannot be undone, so it waits until the DB is committed. Ordering
/// it the other way round would let a failed commit leave the database pointing at truth that no
/// longer exists on disk. A leftover file is harmless and self-healing; a dangling row is not.
#[derive(Default)]
pub(super) struct MutationFiles {
    pub(super) written: Vec<(std::path::PathBuf, Vec<u8>)>,
    pub(super) unlink: Vec<std::path::PathBuf>,
    /// Index-only `source_id`s whose `.pmindex` manifest entries should be forgotten. Same
    /// after-commit rule as `unlink`, and for the same reason: #574 originally dropped these from
    /// the manifest *inside* the transaction, so a failed commit would have left the manifest
    /// missing entries for documents that still existed — un-restorable by a rebuild-from-manifest
    /// until the next connector sync happened to re-add them.
    pub(super) forget_sources: Vec<String>,
}

/// Run a mirror mutation in a transaction, persist the encrypted rules file from the resulting
/// mirror (file-first, so a rule is as durable as the commit), then commit — restoring any
/// rewritten vault files + the rules file if the commit fails, and unlinking any files the mutation
/// asked to delete only once that commit succeeded. Off-runtime (file IO), like the review
/// commands. This is the single write path the Teach tab drives, identical to the inline review
/// correction — and now also the path project deletion (#573) rides, so the rules file, the
/// rollback and the delete-after-commit rule all stay defined in exactly one place.
pub(super) async fn spawn_entity_mutation<F>(app: AppHandle, work: F) -> Result<()>
where
    F: FnOnce(
            &Connection,
            &std::path::Path,
            &vault::MarkdownCipher,
            &std::path::Path,
            &index_only::ManifestCipher,
            &mut MutationFiles,
        ) -> Result<()>
        + Send
        + 'static,
{
    spawn_blocking_result("entity", move || -> Result<()> {
        let state = app.state::<AppState>();
        let (vault, cipher) = state.markdown_io()?;
        let (vault_root, rules_cipher) = state.rules_io()?;
        let (_, manifest_cipher) = state.manifest_io()?;
        let mut conn = state.conn()?;
        let tx = conn.transaction()?;

        // The ledger is owned HERE, not by the closure, and is passed in by reference: a mutation
        // that fails part-way has already rewritten vault files, and if its snapshots go down with
        // its own return value there is nothing left to restore from. That is what happened when a
        // project delete tripped the entity FK — the DB rolled back while every moved document's
        // front matter stayed rewritten, and the vault is what a Rebuild believes. `commit_review`
        // has always kept its snapshot list outside the fallible section for this reason; this is
        // the same shape, made the rule for every mutation that rides this path.
        let mut files = MutationFiles::default();
        let done = work(
            &tx,
            &vault,
            &cipher,
            &vault_root,
            &manifest_cipher,
            &mut files,
        );
        finish_vault_transaction(tx, files.written, Some((&vault_root, &rules_cipher)), done)?;
        // Committed: the deletions are now safe to make real. Best-effort by contract — see the
        // `MutationFiles` note; a file that outlives its row is reclaimed by the next Rebuild.
        for path in files.unlink {
            let _ = std::fs::remove_file(&path);
        }
        for source_id in files.forget_sources {
            let _ = index_only::forget_source(&vault_root, &manifest_cipher, &source_id);
        }
        Ok(())
    })
    .await
}

/// Rewrite every document currently pointing at `entity_id` so its vault frontmatter + `project`
/// cache show `canonical` (preserving tags/importance/reviewed/last_activity). The mirror pointer
/// is already set by the caller; this syncs the denormalised cache + vault. Appends the file
/// snapshots to `out` for rollback.
#[allow(clippy::too_many_arguments)]
pub(super) fn rewrite_entity_documents(
    tx: &Connection,
    vault: &std::path::Path,
    cipher: &vault::MarkdownCipher,
    vault_root: &std::path::Path,
    manifest_cipher: &index_only::ManifestCipher,
    entity_id: i64,
    canonical: &str,
    out: &mut Vec<(std::path::PathBuf, Vec<u8>)>,
) -> Result<()> {
    let mut stmt = tx.prepare("SELECT id FROM documents WHERE entity_id = ?1")?;
    let ids: Vec<i64> = stmt
        .query_map(params![entity_id], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    drop(stmt);
    rewrite_documents(
        tx,
        vault,
        cipher,
        vault_root,
        manifest_cipher,
        &ids,
        Some(canonical),
        out,
    )
}

/// The id-scoped half of [`rewrite_entity_documents`]: rewrite exactly these documents' frontmatter
/// + `project` cache to `canonical`, preserving tags/importance/reviewed/last_activity.
///
/// Split out for project deletion (#573), which re-homes only the documents it moved. Rewriting by
/// entity there would touch every document already sitting in Unsorted — correct but pointlessly
/// rewriting (and re-encrypting) a potentially large inbox to the name it already carries.
///
/// Snapshots are appended to `out` as each file is written, not returned in a batch at the end: a
/// rewrite that fails on document 5 of 10 has already replaced four vault files, and the caller
/// needs those four to roll back. Returning them only on success would discard exactly the ones a
/// failure has to undo.
#[allow(clippy::too_many_arguments)]
pub(super) fn rewrite_documents(
    tx: &Connection,
    vault: &std::path::Path,
    cipher: &vault::MarkdownCipher,
    vault_root: &std::path::Path,
    manifest_cipher: &index_only::ManifestCipher,
    ids: &[i64],
    canonical: Option<&str>,
    out: &mut Vec<(std::path::PathBuf, Vec<u8>)>,
) -> Result<()> {
    let mut rows: Vec<(i64, String, String, Option<String>, i64, String)> =
        Vec::with_capacity(ids.len());
    {
        let mut stmt = tx.prepare(
            "SELECT id, project, tags, importance, reviewed, COALESCE(last_activity, ingested_at) \
             FROM documents WHERE id = ?1",
        )?;
        for id in ids {
            let row = stmt
                .query_row(params![id], |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                })
                .optional()?;
            if let Some(row) = row {
                rows.push(row);
            }
        }
    }

    // Renaming or merging an entity rewrites every document linked to it, so this is a bulk loop and
    // gets the same treatment as `commit_review`: the manifest is regenerated whole from the mirror,
    // making a per-document push quadratic in library size. Flushed once after the loop (#722).
    let mut deferred_manifest = false;
    for (doc_id, project, tags_json, importance, reviewed, last_activity) in rows {
        let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
        // `None` = leave this document where it is. That is the case for a document merely LINKED
        // to the project being renamed/merged/deleted: its home is elsewhere and must not move, but
        // its vault file still names the old project in `linked_projects:`, so it has to be
        // rewritten or the next Rebuild reads the dead name straight back in and re-mints it.
        let home = canonical.unwrap_or(project.as_str());
        // Read AFTER the tag itself has been re-keyed or dropped, so this is already the new truth
        // for the JOIN half of the membership set. The `documents.project` half has NOT moved yet —
        // `write_document_truth` below is what moves it — so this relies on `linked_projects`
        // excluding the row's current home as well as the incoming one. Without that, a rename
        // emitted the OLD name here and wrote it straight back into every renamed document.
        let linked = crate::tags::linked_projects(tx, doc_id, home)?;
        let w = ingest::write_document_truth(
            tx,
            vault,
            cipher,
            doc_id,
            home,
            &linked,
            &tags,
            importance.as_deref(),
            reviewed != 0,
            &last_activity,
            vault_root,
            manifest_cipher,
            // Identity maintenance, not engagement: renaming/merging an entity rewrites every linked
            // doc, and logging one "filed" observation per doc would read as a burst of activity (B6-6).
            ingest::FilingActivity::Suppress,
            ingest::ManifestWrite::Batched,
        )?;
        deferred_manifest |= w.is_none();
        out.extend(w);
    }
    // Called more than once in a transaction by a caller that rewrites several id sets, which is
    // still correct — one push per call instead of one per document, and the last write subsumes the
    // rest because the manifest is regenerated whole.
    if deferred_manifest {
        out.push(ingest::flush_manifest_batch(
            tx,
            vault_root,
            manifest_cipher,
        )?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    // Only the rollback tests mint an error by hand; the module body itself no longer names `Error`
    // now that the `spawn_blocking` JoinError wrapper lives in `crate::blocking`.
    use crate::error::Error;

    // --- finish_vault_transaction: the one tail every vault-mutating pass rides ---
    //
    // These are the batch's only coverage of a rollback that spans BOTH halves. There was none
    // before, which is how `commit_review` came to write the rules file with a bare `?` — a
    // failure there rolled the DB back and left the vault rewritten, and the vault is what the next
    // Rebuild believes.

    const TAIL_DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    /// A store mid-pass: one document row, and its truth file already rewritten with the prior
    /// bytes snapshotted exactly the way every caller builds `written`.
    fn vault_tail_fixture() -> (
        tempfile::TempDir,
        Connection,
        Vec<(std::path::PathBuf, Vec<u8>)>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), TAIL_DB_KEY).unwrap();
        conn.execute(
            "INSERT INTO documents(id, vault_path, title, content_hash, project, tags, reviewed) \
             VALUES (1, 'doc.md', 'T', 'h', 'Unsorted', '[]', 0)",
            [],
        )
        .unwrap();
        let doc = dir.path().join("doc.md");
        std::fs::write(&doc, b"PRIOR").unwrap();
        let prior = std::fs::read(&doc).unwrap();
        std::fs::write(&doc, b"REWRITTEN").unwrap();
        (dir, conn, vec![(doc, prior)])
    }

    fn tail_project_of(conn: &Connection) -> String {
        conn.query_row("SELECT project FROM documents WHERE id = 1", [], |r| {
            r.get(0)
        })
        .unwrap()
    }

    fn tail_cipher() -> entities::RulesCipher {
        entities::RulesCipher::from_master("test-vault", &[7u8; 32])
    }

    #[test]
    fn a_failed_pass_rolls_back_the_db_and_puts_every_file_back() {
        let (_dir, mut conn, written) = vault_tail_fixture();
        let doc = written[0].0.clone();

        let tx = conn.transaction().unwrap();
        tx.execute("UPDATE documents SET project = 'Finances' WHERE id = 1", [])
            .unwrap();
        let work: Result<usize> = Err(Error::Other("the pass failed".into()));
        assert!(finish_vault_transaction(tx, written, None, work).is_err());

        assert_eq!(std::fs::read(&doc).unwrap(), b"PRIOR");
        assert_eq!(tail_project_of(&conn), "Unsorted");
    }

    #[test]
    fn a_clean_commit_keeps_the_rewrites_and_leaves_the_rules_file_in_place() {
        let (dir, mut conn, written) = vault_tail_fixture();
        let doc = written[0].0.clone();
        let cipher = tail_cipher();

        let tx = conn.transaction().unwrap();
        tx.execute("UPDATE documents SET project = 'Finances' WHERE id = 1", [])
            .unwrap();
        let applied =
            finish_vault_transaction(tx, written, Some((dir.path(), &cipher)), Ok(3usize)).unwrap();

        assert_eq!(applied, 3);
        assert_eq!(std::fs::read(&doc).unwrap(), b"REWRITTEN");
        assert_eq!(tail_project_of(&conn), "Finances");
        assert!(
            entities::rules_path(dir.path()).exists(),
            "a successful pass persists the rules file it wrote"
        );
    }

    /// THE regression net: a rules-file write that fails after the vault is already rewritten must
    /// take the files back with it. `commit_review` used a bare `?` here, so the DB rolled back
    /// while every rewritten document kept its new project/tags/`reviewed: true` — and because the
    /// vault is truth (I-02), the next Rebuild adopted a review pass the database had rejected. The
    /// user saw only "the commit failed".
    #[test]
    fn a_failed_rules_write_takes_the_vault_files_back_with_it() {
        let (dir, mut conn, written) = vault_tail_fixture();
        let doc = written[0].0.clone();
        let cipher = tail_cipher();

        // A vault root that is a FILE: `entities.pmrules` cannot be created inside it on any
        // platform, which is the portable stand-in for the real causes (disk full, AV lock,
        // permissions).
        let unwritable_root = dir.path().join("not-a-directory");
        std::fs::write(&unwritable_root, b"x").unwrap();

        let tx = conn.transaction().unwrap();
        tx.execute("UPDATE documents SET project = 'Finances' WHERE id = 1", [])
            .unwrap();
        let out =
            finish_vault_transaction(tx, written, Some((&unwritable_root, &cipher)), Ok(1usize));

        assert!(out.is_err(), "the write failure must surface");
        assert_eq!(
            std::fs::read(&doc).unwrap(),
            b"PRIOR",
            "the vault must not keep a pass the database rejected"
        );
        assert_eq!(tail_project_of(&conn), "Unsorted");
    }

    /// The same hole one step earlier, and it was present in all three hand-rolled tails: reading
    /// the mirror is on the files-already-rewritten side of the commit, so it owes the same
    /// rollback as writing it.
    #[test]
    fn a_failed_mirror_read_takes_the_vault_files_back_with_it() {
        let (dir, mut conn, written) = vault_tail_fixture();
        let doc = written[0].0.clone();
        let cipher = tail_cipher();

        let tx = conn.transaction().unwrap();
        tx.execute("UPDATE documents SET project = 'Finances' WHERE id = 1", [])
            .unwrap();
        // Transactional DDL: `rules_from_mirror` can no longer prepare its SELECT, and the drop
        // itself rolls back with everything else.
        tx.execute("DROP TABLE entities", []).unwrap();

        let out = finish_vault_transaction(tx, written, Some((dir.path(), &cipher)), Ok(1usize));

        assert!(out.is_err());
        assert_eq!(std::fs::read(&doc).unwrap(), b"PRIOR");
        assert_eq!(tail_project_of(&conn), "Unsorted");
        assert!(
            !entities::rules_path(dir.path()).exists(),
            "nothing should have been written before the read failed"
        );
    }
}
