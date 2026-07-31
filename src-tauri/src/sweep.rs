// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! **ONE-TIME CLEANUP — DELETE THIS MODULE IN THE RELEASE AFTER THE ONE THAT SHIPS IT.**
//! Tracked as the follow-up card to #651. Everything wired to it (the `orphan_*` commands, their
//! `ipc.ts` wrappers, `OrphanSweepBanner`, and the `orphan_sweep_dismissed` setting) goes with it.
//!
//! ## What it cleans up
//!
//! Before #620, `delete_document` decided what to unlink with `source_type != "vault"`. A photo's
//! source type is `photo` and a spreadsheet's is `spreadsheet`, so both were classified as connector
//! pointers: PM dropped the database rows and left the encrypted Markdown in the vault. A photo saved
//! with "keep a copy" also left its original in `photos/`, which nothing has ever deleted.
//!
//! #620 fixed the behaviour. It could not clean up what earlier versions had already left, and no
//! walk anywhere deletes a vault file that has no row. The leftovers are invisible — no view, no
//! search, no count — but **the vault file is the truth a Rebuild reads**, so a photo the user
//! deliberately deleted returns as a document on the next Rebuild.
//!
//! ## Why the decision is a pure function
//!
//! This is the most destructive operation in the app: it deletes user files that PM has no record
//! of, which means nothing else can cross-check it afterwards. The dangerous case is not a bug in
//! the diff — it is that **a missing row is also what a half-migrated, mid-restore or simply
//! unopened database looks like**, and the sweep cannot tell those apart from the file alone. So
//! every refusal lives here, in one function, tested in isolation, and a refused plan returns **no
//! orphans at all** rather than a list a caller might still act on.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use rusqlite::Connection;
use tauri::State;

use crate::error::{Error, Result};
use crate::{db, settings, AppState};

/// Set once the user has approved or dismissed the sweep, so the banner never returns.
const DISMISSED_KEY: &str = "orphan_sweep_dismissed";

/// Why the sweep will not offer itself. Serialised to the UI, which explains rather than retries.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SweepRefusal {
    /// A rebuild or ingest is running: it writes vault files and rows in either order, so a file
    /// whose row is not written *yet* is indistinguishable from a leftover.
    Indexing,
    /// A connector sync is running, for the same reason.
    Syncing,
    /// The walk could not read part of the vault. "We never saw it" must never be read as "the user
    /// deleted it" — the same guard [`crate::ingest::may_reap`] applies in the mirror direction.
    IncompleteWalk,
    /// The vault holds files but PM knows of no documents at all. That is not a vault full of
    /// leftovers; it is a database that has not loaded — mid-restore, or opened against the wrong
    /// store. Sweeping on that picture would delete the whole library.
    NoDocuments,
    /// Nearly everything in the vault looks orphaned. Same reasoning as [`Self::NoDocuments`], for
    /// the partially-populated case a bare emptiness check cannot catch.
    ImplausibleShare { orphans: usize, files: usize },
}

/// What else is writing to the vault or the index right now.
#[derive(Debug, Clone, Copy, Default)]
pub struct Busy {
    pub indexing: bool,
    pub syncing: bool,
}

/// The sweep's decision. `orphans` is empty whenever `refusal` is set — deliberately, so a caller
/// that forgets to check cannot delete anything.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SweepPlan {
    pub orphans: Vec<String>,
    pub refusal: Option<SweepRefusal>,
}

impl SweepPlan {
    fn refused(refusal: SweepRefusal) -> Self {
        Self {
            orphans: Vec::new(),
            refusal: Some(refusal),
        }
    }
}

/// Above this share of the vault, treat "orphaned" as a database problem rather than a cleanup.
const IMPLAUSIBLE_SHARE: f64 = 0.9;

/// …but only once there are enough files for the share to mean anything. A three-file vault with two
/// leftovers is 67% and perfectly ordinary.
const IMPLAUSIBLE_MIN_FILES: usize = 10;

/// Which vault files have no row behind them, or why PM refuses to say.
///
/// `files` are vault-relative paths exactly as enumerated from disk; `known` are the paths the
/// database holds (`documents.vault_path` ∪ `chat_sessions.vault_path` ∪ `photos.vault_path`).
/// Both use the same `/`-separated form, which is what makes the comparison meaningful — a bare
/// filename on one side and `chats/name` on the other would make every chat look orphaned.
pub fn plan_sweep(
    files: &[String],
    known: &BTreeSet<String>,
    enumeration_complete: bool,
    busy: Busy,
) -> SweepPlan {
    if busy.indexing {
        return SweepPlan::refused(SweepRefusal::Indexing);
    }
    if busy.syncing {
        return SweepPlan::refused(SweepRefusal::Syncing);
    }
    if !enumeration_complete {
        return SweepPlan::refused(SweepRefusal::IncompleteWalk);
    }
    if known.is_empty() && !files.is_empty() {
        return SweepPlan::refused(SweepRefusal::NoDocuments);
    }

    let orphans: Vec<String> = files
        .iter()
        .filter(|rel| !known.contains(*rel))
        .cloned()
        .collect();

    if files.len() >= IMPLAUSIBLE_MIN_FILES
        && orphans.len() as f64 > files.len() as f64 * IMPLAUSIBLE_SHARE
    {
        return SweepPlan::refused(SweepRefusal::ImplausibleShare {
            orphans: orphans.len(),
            files: files.len(),
        });
    }

    SweepPlan {
        orphans,
        refusal: None,
    }
}

/// Every vault-relative path the database accounts for.
///
/// All three tables, because a file is a leftover only if NOTHING points at it. `chat_sessions` is
/// listed alongside `documents` even though a chat also has a document row: if the two ever
/// disagree, the union keeps the file and the sweep does nothing, which is the direction to be
/// wrong in. Index-only rows hold an `idx://…` sentinel that matches no real path — harmless here,
/// since they can only ever fail to match a file that does not exist.
fn known_paths(conn: &Connection) -> Result<BTreeSet<String>> {
    let mut known = BTreeSet::new();
    for sql in [
        "SELECT vault_path FROM documents",
        "SELECT vault_path FROM chat_sessions WHERE vault_path IS NOT NULL",
        "SELECT vault_path FROM photos WHERE vault_path IS NOT NULL",
    ] {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        for path in rows {
            known.insert(path?);
        }
    }
    Ok(known)
}

/// Resolve a vault-relative path under the vault root, refusing anything that could escape it.
///
/// The membership check in [`delete_orphan_files`] already makes traversal unreachable — a `..` path
/// cannot come out of a `read_dir` walk — but a delete path should carry its own guard rather than
/// inherit safety from a caller two functions away.
fn safe_join(vault: &Path, rel: &str) -> Result<PathBuf> {
    let candidate = PathBuf::from(rel);
    if candidate
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(Error::Other(format!(
            "refusing to act on a vault path that is not a plain relative name: {rel}"
        )));
    }
    Ok(vault.join(candidate))
}

/// Walk the vault, diff it against the database, and decide. Shared by the scan and the delete, so
/// the delete can never act on a picture older than itself.
fn current_plan(state: &AppState) -> Result<SweepPlan> {
    let (vault_dir, _cipher) = state.markdown_io()?;

    let known = {
        let conn = state.conn()?;
        // Nothing to offer someone still setting a vault up (Bobby's requirement), and a vault
        // created now cannot hold a pre-#620 leftover anyway. Also covers the dismissal: once the
        // user has approved or dismissed once, the banner is done for good.
        if !db::get_bool(&conn, settings::ONBOARDING_DONE_KEY, false)?
            || db::get_bool(&conn, DISMISSED_KEY, false)?
        {
            return Ok(SweepPlan {
                orphans: Vec::new(),
                refusal: None,
            });
        }
        known_paths(&conn)?
        // The guard drops here: the walk below must not hold the database lock (it is
        // non-reentrant, and a vault walk is slow).
    };

    let (markdown, markdown_complete) = crate::ingest::walk_vault_markdown(&vault_dir)?;
    let (photos, photos_complete) = crate::ingest::walk_vault_photos(&vault_dir)?;
    let mut files: Vec<String> = markdown
        .into_iter()
        .chain(photos)
        .map(|file| file.rel)
        .collect();
    files.sort();

    Ok(plan_sweep(
        &files,
        &known,
        markdown_complete && photos_complete,
        Busy {
            indexing: state.rebuild_running(),
            syncing: state.sync_active(),
        },
    ))
}

/// What the sweep would delete, or why it will not say. Read-only.
#[tauri::command]
pub fn scan_orphan_files(state: State<'_, AppState>) -> Result<SweepPlan> {
    current_plan(&state)
}

/// Delete the approved leftovers. Returns how many files actually went.
///
/// **The caller's list is a filter, never an authority.** The plan is recomputed here and only paths
/// that are still orphans in a fresh, unrefused plan are touched — so a list built before an ingest
/// started, or a webview that sends something of its own devising, cannot delete a live file. That
/// re-check is the whole safety story of this command, since PM has no record of these files and
/// nothing could detect the mistake afterwards.
#[tauri::command]
pub fn delete_orphan_files(state: State<'_, AppState>, paths: Vec<String>) -> Result<usize> {
    let fresh = current_plan(&state)?;
    if let Some(refusal) = fresh.refusal {
        return Err(Error::Other(format!(
            "the vault changed since it was scanned, so nothing was deleted ({refusal:?})"
        )));
    }
    let deletable: BTreeSet<String> = fresh.orphans.into_iter().collect();
    let (vault_dir, _cipher) = state.markdown_io()?;

    let mut removed = 0usize;
    for rel in paths {
        if !deletable.contains(&rel) {
            continue;
        }
        match std::fs::remove_file(safe_join(&vault_dir, &rel)?) {
            Ok(()) => removed += 1,
            // Already gone is the outcome we wanted; anything else is a real failure worth
            // surfacing rather than silently under-reporting.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }

    let conn = state.conn()?;
    db::set_bool(&conn, DISMISSED_KEY, true)?;
    Ok(removed)
}

/// "Not now" — the banner does not come back. Same key the delete stamps, so either answer ends it.
#[tauri::command]
pub fn dismiss_orphan_sweep(state: State<'_, AppState>) -> Result<()> {
    let conn = state.conn()?;
    db::set_bool(&conn, DISMISSED_KEY, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(paths: &[&str]) -> BTreeSet<String> {
        paths.iter().map(|s| s.to_string()).collect()
    }

    fn files(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn finds_only_the_files_no_row_points_at() {
        let plan = plan_sweep(
            &files(&["a.md.pmenc", "photos/old.png.pmenc", "chats/c.md.pmenc"]),
            &known(&["a.md.pmenc", "chats/c.md.pmenc"]),
            true,
            Busy::default(),
        );
        assert_eq!(plan.refusal, None);
        assert_eq!(plan.orphans, vec!["photos/old.png.pmenc"]);
    }

    #[test]
    fn a_clean_vault_yields_nothing_and_no_refusal() {
        let plan = plan_sweep(
            &files(&["a.md.pmenc"]),
            &known(&["a.md.pmenc"]),
            true,
            Busy::default(),
        );
        assert_eq!(plan.orphans, Vec::<String>::new());
        assert_eq!(plan.refusal, None);
    }

    #[test]
    fn paths_are_compared_whole_so_a_chat_is_not_an_orphan() {
        // The shape that would make this catastrophic: comparing bare filenames against the stored
        // `chats/…` paths. Every chat in the vault would come back as a leftover.
        let plan = plan_sweep(
            &files(&["chats/c.md.pmenc"]),
            &known(&["chats/c.md.pmenc"]),
            true,
            Busy::default(),
        );
        assert!(plan.orphans.is_empty());
    }

    #[test]
    fn an_incomplete_walk_refuses_rather_than_guessing() {
        // The mirror of `may_reap`: a dir entry we could not read means a file may exist that we
        // never enumerated. In this direction the danger is the opposite one — a row we did not see
        // makes a live file look abandoned.
        let plan = plan_sweep(&files(&["a.md.pmenc"]), &known(&[]), false, Busy::default());
        assert_eq!(plan.refusal, Some(SweepRefusal::IncompleteWalk));
        assert!(plan.orphans.is_empty(), "a refused plan offers nothing");
    }

    #[test]
    fn an_empty_database_over_a_full_vault_refuses() {
        // Mid-restore, or a store that never opened. Without this the sweep would offer to delete
        // the user's entire library, and every file in the list would look perfectly legitimate.
        let plan = plan_sweep(
            &files(&["a.md.pmenc", "b.md.pmenc"]),
            &known(&[]),
            true,
            Busy::default(),
        );
        assert_eq!(plan.refusal, Some(SweepRefusal::NoDocuments));
        assert!(plan.orphans.is_empty());
    }

    #[test]
    fn an_empty_vault_with_an_empty_database_is_not_a_refusal() {
        // A brand-new vault: nothing on disk, nothing in the database, nothing to say. This is also
        // what makes the banner invisible during onboarding without any version bookkeeping.
        let plan = plan_sweep(&files(&[]), &known(&[]), true, Busy::default());
        assert_eq!(plan.refusal, None);
        assert!(plan.orphans.is_empty());
    }

    #[test]
    fn a_partially_populated_database_refuses_on_the_share() {
        // The case `NoDocuments` cannot catch: one row loaded, the rest not. 11 files, 10 orphaned.
        let mut on_disk: Vec<String> = (0..10).map(|i| format!("f{i}.md.pmenc")).collect();
        on_disk.push("real.md.pmenc".into());
        let plan = plan_sweep(&on_disk, &known(&["real.md.pmenc"]), true, Busy::default());
        assert_eq!(
            plan.refusal,
            Some(SweepRefusal::ImplausibleShare {
                orphans: 10,
                files: 11
            })
        );
        assert!(plan.orphans.is_empty());
    }

    #[test]
    fn a_small_vault_that_is_mostly_leftovers_is_still_swept() {
        // The share guard must not block the ordinary case it resembles: someone who ingested a
        // handful of photos on an old build and deleted them all. Two of three is 67%, and with
        // fewer than ten files the share means nothing anyway.
        let plan = plan_sweep(
            &files(&["a.md.pmenc", "photos/x.png.pmenc", "photos/y.png.pmenc"]),
            &known(&["a.md.pmenc"]),
            true,
            Busy::default(),
        );
        assert_eq!(plan.refusal, None);
        assert_eq!(plan.orphans.len(), 2);
    }

    #[test]
    fn indexing_and_syncing_each_refuse() {
        // Both write the vault file and the index row in separate steps, so a file whose row is not
        // written YET is indistinguishable from one whose row is gone.
        for (busy, expected) in [
            (
                Busy {
                    indexing: true,
                    syncing: false,
                },
                SweepRefusal::Indexing,
            ),
            (
                Busy {
                    indexing: false,
                    syncing: true,
                },
                SweepRefusal::Syncing,
            ),
        ] {
            let plan = plan_sweep(&files(&["a.md.pmenc"]), &known(&["a.md.pmenc"]), true, busy);
            assert_eq!(plan.refusal, Some(expected));
            assert!(plan.orphans.is_empty());
        }
    }
}
