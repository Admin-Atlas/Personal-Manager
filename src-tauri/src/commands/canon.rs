// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Canonical names: entities and their aliases, projects, and the merge/delete passes that
//! re-file everything pointing at one.

use rusqlite::{params, Connection, OptionalExtension};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

use crate::error::{Error, Result};
use crate::ingest;
use crate::llm_gateway::{self, Role};
use crate::projects::{self, ProjectOverview, ProjectProposalEvent};
use crate::{chat, clock, db, entities, index_only, openrouter, vault, AppState};

use super::shared::resolve_zone;
use super::spend::log_background_usage;
use super::vault_writes::rewrite_documents;
use super::vault_writes::rewrite_entity_documents;
use super::vault_writes::spawn_entity_mutation;

// --- canonical-entity management (the Teach-tab backend; §1.3) ---

/// Every project entity with its aliases — the Teach tab's list (PR 2). Read-only.
#[tauri::command]
pub fn list_entities(
    state: State<'_, AppState>,
    kind: Option<String>,
) -> Result<Vec<entities::Entity>> {
    let conn = state.conn()?;
    entities::list_entities(&conn, kind.as_deref().unwrap_or(entities::TYPE_PROJECT))
}

/// Record a forward-going alias for a project entity. Rejected (not silently folded) if the alias
/// already belongs to another project — that's a merge.
#[tauri::command]
pub async fn add_entity_alias(app: AppHandle, entity_id: i64, alias: String) -> Result<()> {
    spawn_entity_mutation(
        app,
        move |tx, _vault, _cipher, _vault_root, _manifest_cipher, _files| match entities::add_alias(
            tx, entity_id, &alias,
        )? {
            entities::AddAlias::Conflict(_) => Err(Error::Other(format!(
                "\"{}\" already belongs to another project; merge them instead",
                alias.trim()
            ))),
            _ => Ok(()),
        },
    )
    .await
}

/// Remove an alias from a project entity — undo a name/merge decision from the Teach tab. Wrapped in
/// the entity-mutation write path so `entities.pmrules` is persisted (and rolls back on failure). Any
/// documents still literally filed under the removed name are re-homed to a fresh standalone entity by
/// `entities::remove_alias`; the documents' name is unchanged (only the backing entity moves), so no
/// vault frontmatter rewrite is needed.
#[tauri::command]
pub async fn remove_entity_alias(app: AppHandle, entity_id: i64, alias: String) -> Result<()> {
    spawn_entity_mutation(
        app,
        move |tx, _vault, _cipher, _vault_root, _manifest_cipher, _files| {
            entities::remove_alias(tx, entity_id, &alias)?;
            Ok(())
        },
    )
    .await
}

/// Rewrite every vault file the rename/merge of a project touched — its own documents AND the ones
/// that merely LINKED to it (#275).
///
/// The second population is the one that is easy to miss: those documents are homed in some other
/// project, so no `entity_id` query in the rename/merge path reaches them. Their front-matter still
/// names the old project in `linked_projects:`, and the next Rebuild would read that back and
/// re-mint the project that was just renamed away or folded in.
///
/// `members` must be captured BEFORE `rename_project_satellites` re-keys the tag, since that is what
/// moves the join rows. They are rewritten with `None` — keep each where it is — because only their
/// membership changed, not their home.
#[allow(clippy::too_many_arguments)]
fn rewrite_after_project_rekey(
    tx: &Connection,
    vault: &std::path::Path,
    cipher: &vault::MarkdownCipher,
    vault_root: &std::path::Path,
    manifest_cipher: &index_only::ManifestCipher,
    entity_id: i64,
    canonical: &str,
    members: &[i64],
    out: &mut Vec<(std::path::PathBuf, Vec<u8>)>,
) -> Result<()> {
    rewrite_entity_documents(
        tx,
        vault,
        cipher,
        vault_root,
        manifest_cipher,
        entity_id,
        canonical,
        out,
    )?;
    let elsewhere: Vec<i64> = {
        let mut stmt = tx.prepare("SELECT 1 FROM documents WHERE id = ?1 AND entity_id IS ?2")?;
        let mut ids = Vec::new();
        for id in members {
            let homed_here = stmt
                .query_row(params![id, entity_id], |_| Ok(()))
                .optional()?
                .is_some();
            if !homed_here {
                ids.push(*id);
            }
        }
        ids
    };
    rewrite_documents(
        tx,
        vault,
        cipher,
        vault_root,
        manifest_cipher,
        &elsewhere,
        None,
        out,
    )
}

/// Rename a canonical project — a one-row identity update plus a frontmatter/cache rewrite of its
/// documents to the new canonical name (the payoff of identity-not-name).
#[tauri::command]
pub async fn rename_entity(app: AppHandle, entity_id: i64, new_name: String) -> Result<()> {
    spawn_entity_mutation(
        app,
        move |tx, vault, cipher, vault_root, manifest_cipher, files| {
            // Capture the old canonical BEFORE the rename so we can re-key the name-keyed project
            // satellites (triage, milestones, activity, chats) onto the new name — otherwise the
            // renamed project silently loses all of them (F-05). Runs before the document rewrite,
            // whose truth-writer would otherwise lazily upsert a bare new-name projects row.
            let old = entities::canonical_name(tx, entity_id)?;
            // Captured before the satellites (and with them the project tag) are re-keyed.
            let members = crate::tags::documents_tagged(tx, &old)?;
            let canonical = entities::rename_entity(tx, entity_id, &new_name)?;
            projects::rename_project_satellites(tx, &old, &canonical)?;
            rewrite_after_project_rekey(
                tx,
                vault,
                cipher,
                vault_root,
                manifest_cipher,
                entity_id,
                &canonical,
                &members,
                &mut files.written,
            )
        },
    )
    .await
}

/// What a project merge will move — the honest, computed preview behind the type-to-confirm
/// ceremony (#279). The counts are cheap because they are exactly the predicates the merge
/// itself runs: documents move by `entity_id`, milestones and chats by project *name*.
///
/// `files` deliberately EXCLUDES chat documents. A chat is a `documents` row too, so counting
/// the table raw would report every chat twice — once as a file and once as a chat — in the
/// one sentence the user reads before an irreversible action.
#[derive(serde::Serialize)]
pub struct MergePreview {
    pub files: i64,
    pub milestones: i64,
    pub chats: i64,
    /// The target's canonical name, resolved through the alias table. This is what the source's
    /// documents end up filed under, so it is also the string the user must type to confirm —
    /// typing the alias they happened to click would confirm a name that never appears again.
    pub into_canonical: String,
}

/// Resolve the two project names a merge names, applying the same guards `merge_projects` will,
/// so the UI can refuse an impossible merge before the ceremony rather than after it.
fn resolve_merge_pair(conn: &Connection, from: &str, into: &str) -> Result<(i64, i64, String)> {
    let (from, into) = (from.trim(), into.trim());
    if from.is_empty() || into.is_empty() {
        return Err(Error::Other("both projects must be named".into()));
    }
    let from_id = entities::resolve_project(conn, from, false)?
        .ok_or_else(|| Error::Other(format!("no project named \"{from}\"")))?;
    let into_id = entities::resolve_project(conn, into, false)?
        .ok_or_else(|| Error::Other(format!("no project named \"{into}\"")))?;
    if from_id == into_id {
        return Err(Error::Other(
            "that is the same project — pick a different one to merge into".into(),
        ));
    }
    // Mirror `entities::merge_entities`' guard here rather than letting the merge fail after the
    // user has typed the confirmation: Unsorted is the inbox, and merging FROM it would sweep
    // every unreviewed document into another project.
    if entities::resolve_project(conn, "Unsorted", false)? == Some(from_id) {
        return Err(Error::Other(
            "Unsorted is PM's inbox and can't be merged into another project".into(),
        ));
    }
    let into_canonical = entities::canonical_name(conn, into_id)?;
    Ok((from_id, into_id, into_canonical))
}

/// What a project holds, counted from the rows an operation will actually touch: documents by
/// `entity_id`, milestones and chats by project *name*. Shared by the merge and delete previews so
/// the two can never quote different numbers for the same project.
///
/// `files` EXCLUDES chat documents — a chat is a `documents` row too, so a raw count would report
/// every chat twice in the one sentence a user reads before an irreversible action.
fn project_content_counts(
    conn: &Connection,
    entity_id: i64,
    canonical: &str,
) -> Result<(i64, i64, i64)> {
    let files: i64 = conn.query_row(
        "SELECT COUNT(*) FROM documents \
         WHERE entity_id = ?1 AND COALESCE(source_type,'') <> 'chat'",
        params![entity_id],
        |r| r.get(0),
    )?;
    let chats: i64 = conn.query_row(
        "SELECT COUNT(*) FROM conversations WHERE project = ?1",
        params![canonical],
        |r| r.get(0),
    )?;
    let milestones: i64 = conn.query_row(
        "SELECT COUNT(*) FROM project_milestones WHERE project_name = ?1",
        params![canonical],
        |r| r.get(0),
    )?;
    Ok((files, chats, milestones))
}

/// Count what merging `from` into `into` would move. Read-only; safe to call on every keystroke
/// of the target picker.
#[tauri::command]
pub fn merge_project_preview(
    state: State<'_, AppState>,
    from: String,
    into: String,
) -> Result<MergePreview> {
    let conn = state.conn()?;
    let (from_id, _, into_canonical) = resolve_merge_pair(&conn, &from, &into)?;
    let from_canonical = entities::canonical_name(&conn, from_id)?;
    let (files, chats, milestones) = project_content_counts(&conn, from_id, &from_canonical)?;
    Ok(MergePreview {
        files,
        milestones,
        chats,
        into_canonical,
    })
}

/// Fold one project into another **by name** — the project-level *Merge into* (#279), and the
/// replacement for the `parent` field #278 retired.
///
/// This is deliberately a thin resolver over [`merge_entities`] rather than a second merge
/// implementation. A project's identity IS its entity, so "merge Landing Page Redesign into
/// Marketing" and "merge these two name variants" are the same operation reached from two
/// surfaces; duplicating the engine would mean two places to keep the satellite re-keying,
/// the alias fold and the vault rewrite correct.
#[tauri::command]
pub async fn merge_projects(app: AppHandle, from: String, into: String) -> Result<()> {
    // Resolve OUTSIDE the mutation so a bad pair fails fast with a clear message, then re-resolve
    // inside the transaction (below) — ids can't be trusted across the lock boundary.
    {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        resolve_merge_pair(&conn, &from, &into)?;
    }
    spawn_entity_mutation(
        app,
        move |tx, vault, cipher, vault_root, manifest_cipher, files| {
            let (from_id, into_id, _) = resolve_merge_pair(tx, &from, &into)?;
            // Identical ordering to `merge_entities`: capture the folded name BEFORE the entity
            // row dies, fold the entity, then re-key the name-keyed satellites onto the survivor.
            let old = entities::canonical_name(tx, from_id)?;
            // Captured before the satellites (and with them the project tag) are folded.
            let members = crate::tags::documents_tagged(tx, &old)?;
            entities::merge_entities(tx, from_id, into_id)?;
            let canonical = entities::canonical_name(tx, into_id)?;
            projects::rename_project_satellites(tx, &old, &canonical)?;
            rewrite_after_project_rekey(
                tx,
                vault,
                cipher,
                vault_root,
                manifest_cipher,
                into_id,
                &canonical,
                &members,
                &mut files.written,
            )
        },
    )
    .await
}

// --- deleting a project (#573) --------------------------------------------------------------
//
// Deliberately built on the merge machinery rather than beside it: a delete IS a disposal of a
// project's contents, and a merge is the special case where every disposition points at one target.
// `resolve_*`, `project_content_counts`, `rename_project_satellites`, `rewrite_documents` and
// `spawn_entity_mutation` are all shared, so the FK ordering, the rules-file durability and the
// delete-after-commit rule are each defined exactly once.

/// The always-present inbox. Documents re-homed by a delete land here, and it can never itself be
/// the project being deleted.
const UNSORTED: &str = "Unsorted";

/// Where a deleted project's non-chat documents go.
#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileDisposition {
    /// Re-file to `Unsorted`, keeping the files and their index.
    Unsorted,
    /// Destroy them: index rows AND the vault Markdown. For an index-only (cloud) document there is
    /// no vault file and the remote is never touched — only PM's pointer + manifest entry go.
    Delete,
}

/// Where a deleted project's chats go.
#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatDisposition {
    /// Un-scope them: the conversation survives as a general chat.
    Global,
    /// Destroy them, through the same cascade the per-chat delete uses.
    Delete,
}

/// What happens to the project's NAME once its contents are dealt with.
#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NameDisposition {
    /// The entity and all its aliases die, so the name is free to use again — a future document
    /// naming it mints a fresh project. Matches what removing an alias chip in Teach does.
    Free,
    /// The name lives on as an alias of `Unsorted`, so anything later referring to it files to the
    /// inbox instead of silently recreating the project. This is literally "merge into Unsorted".
    Unsorted,
}

/// What deleting `project` would affect. Same counts as the merge preview (one shared query), plus
/// the canonical name the user must type to confirm.
#[derive(serde::Serialize)]
pub struct DeletePreview {
    pub files: i64,
    pub chats: i64,
    pub milestones: i64,
    pub canonical: String,
}

/// Resolve a project that is allowed to be deleted, applying the guards up front so an impossible
/// delete fails before the confirmation ceremony rather than during it.
fn resolve_deletable_project(conn: &Connection, project: &str) -> Result<(i64, String)> {
    let project = project.trim();
    if project.is_empty() {
        return Err(Error::Other("no project named".into()));
    }
    let id = entities::resolve_project(conn, project, false)?
        .ok_or_else(|| Error::Other(format!("no project named \"{project}\"")))?;
    // Same reasoning as the merge guard: Unsorted is the inbox every unfiled document lands in.
    // Deleting it would destroy or strand the entire unreviewed queue.
    if entities::resolve_project(conn, UNSORTED, false)? == Some(id) {
        return Err(Error::Other(
            "Unsorted is PM's inbox and can't be deleted".into(),
        ));
    }
    Ok((id, entities::canonical_name(conn, id)?))
}

/// Count what deleting `project` would affect. Read-only.
#[tauri::command]
pub fn delete_project_preview(
    state: State<'_, AppState>,
    project: String,
) -> Result<DeletePreview> {
    let conn = state.conn()?;
    let (id, canonical) = resolve_deletable_project(&conn, &project)?;
    let (files, chats, milestones) = project_content_counts(&conn, id, &canonical)?;
    Ok(DeletePreview {
        files,
        chats,
        milestones,
        canonical,
    })
}

/// Delete a project, disposing of its contents as the user chose (#573).
///
/// **Milestones are always destroyed** — there is nowhere sensible to move a dated milestone whose
/// project no longer exists, so the UI warns instead of offering a choice.
///
/// Ordering is load-bearing throughout, and each step exists because of a specific way this goes
/// wrong; see the inline notes. The whole thing runs inside `spawn_entity_mutation`, which is not
/// optional: `reconcile_on_open` rebuilds the entity mirror from `entities.pmrules` whenever the two
/// disagree, so a delete that skipped the rules-file write would be **resurrected at the next
/// launch**.
#[tauri::command]
pub async fn delete_project(
    app: AppHandle,
    project: String,
    files: FileDisposition,
    chats: ChatDisposition,
    name: NameDisposition,
) -> Result<()> {
    {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        resolve_deletable_project(&conn, &project)?;
    }
    spawn_entity_mutation(
        app,
        move |tx, vault, cipher, vault_root, manifest_cipher, out| {
            let (entity_id, canonical) = resolve_deletable_project(tx, &project)?;
            let unsorted_id = entities::resolve_project(tx, UNSORTED, true)?
                .ok_or_else(|| Error::Other("could not resolve the Unsorted inbox".into()))?;
            // Documents that survive and move; rewritten to their new name after the moves, so the
            // vault frontmatter and the DB never disagree about where a file is filed.
            let mut moved: Vec<i64> = Vec::new();
            // Documents deleted outright, so the rewrite pass below can skip them.
            let mut deleted: Vec<i64> = Vec::new();
            // Every document carrying this project as a tag — home OR merely linked — captured NOW,
            // because step 3 drops the tag and takes the join rows with it. The linked-elsewhere
            // ones are invisible to every `entity_id` query in this function (their entity is their
            // own home project), and they are exactly the files that would otherwise keep the
            // deleted name in their front-matter.
            let linked_members = crate::tags::documents_tagged(tx, &canonical)?;

            // --- 1. CHATS ------------------------------------------------------------------
            //
            // A chat belongs to this project by EITHER of two independent identities, and reaching
            // it by only one strands it. `conversations.project` is the chat's SCOPE, set when the
            // chat is started inside a project. `documents.entity_id` is where its transcript is
            // FILED, which is what Review writes — a general chat is born unscoped and reviewable
            // (chat_index.rs), so filing it into a project moves the document and leaves the
            // conversation scope NULL, by design. Selecting on scope alone missed exactly those:
            // the transcript survived step 3, step 4 rewrote it under its own (just-deleted) home
            // and re-interned the tag, so the project came back — or, with the name freed, the
            // surviving `documents.entity_id` tripped the FK at the end and the whole delete
            // aborted with an opaque error.
            let conv_ids = chat::conversations_in_project(tx, &canonical, entity_id)?;
            match chats {
                ChatDisposition::Delete => {
                    for id in conv_ids {
                        // The same cascade the per-chat delete uses, minus its transaction. It
                        // deletes the chat's `documents` row too, so the rewrite pass below skips
                        // it — `rewrite_documents` looks each id up with `.optional()`.
                        if let Some(rel) = chat::delete_conversation_rows(tx, id)? {
                            out.unlink.push(vault.join(rel));
                        }
                    }
                }
                ChatDisposition::Global => {
                    // A general chat is one with no project (`chat.rs` derives scope from exactly
                    // this), so un-scoping is the whole move. By id, not by project name, so a chat
                    // reached only through its filed transcript is un-scoped too.
                    for id in &conv_ids {
                        tx.execute(
                            "UPDATE conversations SET project = NULL WHERE id = ?1",
                            params![id],
                        )?;
                    }
                    // A chat is also a `documents` row; it follows its conversation to the inbox.
                    let mut stmt = tx.prepare(
                        "SELECT id FROM documents WHERE entity_id = ?1 AND source_type = 'chat'",
                    )?;
                    let ids: Vec<i64> = stmt
                        .query_map(params![entity_id], |r| r.get(0))?
                        .collect::<std::result::Result<_, _>>()?;
                    drop(stmt);
                    for id in &ids {
                        tx.execute(
                            "UPDATE documents SET entity_id = ?2, project = ?3 WHERE id = ?1",
                            params![id, unsorted_id, UNSORTED],
                        )?;
                    }
                    moved.extend(ids);
                }
            }

            // --- 2. FILES (everything that isn't a chat) -----------------------------------
            type FileRow = (i64, Option<String>, Option<String>, Option<String>);
            let file_rows: Vec<FileRow> = {
                let mut stmt = tx.prepare(
                    "SELECT id, vault_path, source_type, source_id FROM documents \
                     WHERE entity_id = ?1 AND COALESCE(source_type,'') <> 'chat'",
                )?;
                let rows = stmt
                    .query_map(params![entity_id], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                    })?
                    .collect::<std::result::Result<Vec<FileRow>, _>>()?;
                rows
            };
            match files {
                FileDisposition::Unsorted => {
                    for (id, ..) in &file_rows {
                        tx.execute(
                            "UPDATE documents SET entity_id = ?2, project = ?3 WHERE id = ?1",
                            params![id, unsorted_id, UNSORTED],
                        )?;
                        moved.push(*id);
                    }
                }
                FileDisposition::Delete => {
                    for (id, vault_path, source_type, source_id) in &file_rows {
                        // An index-only document is a POINTER at someone else's file. Deleting it
                        // must remove PM's row + its manifest entry and nothing else — PM never
                        // deletes from Drive/OneDrive or a watched folder, and there is no vault
                        // file to unlink. Everything else PM holds the file for, including the
                        // saved original behind a photo — looked up per document (before the
                        // delete below cascades the `photos` row away) rather than joined into the
                        // query above, so the rule lives in one tested place. The extra indexed
                        // lookup is nothing beside the statements `delete_document` already runs
                        // for each of these.
                        if ingest::owns_a_vault_file(source_type.as_deref()) {
                            let photo_original = ingest::saved_photo_original(tx, *id)?;
                            for rel in [vault_path.clone(), photo_original].into_iter().flatten() {
                                if !rel.trim().is_empty() {
                                    out.unlink.push(vault.join(rel));
                                }
                            }
                        } else if let Some(sid) = source_id.as_deref().filter(|s| !s.is_empty()) {
                            // Queued, not applied here — the manifest must not lose an entry
                            // for a document whose row survives a failed commit.
                            out.forget_sources.push(sid.to_string());
                        }
                        ingest::delete_document(tx, *id)?;
                        deleted.push(*id);
                    }
                }
            }

            // --- 3. Satellites: milestones (+ their flags), activity, pinboard, triage row,
            //        and the project's own tag (which cascades every membership of it) ---------
            //
            // Ahead of the vault rewrites below, not after them as it used to be: the rewrites
            // re-derive each document's `linked_projects:` line FROM the membership join, so the
            // dying project's tag has to be gone by then or every rewritten file would name it
            // again — and the next Rebuild would read it back and re-mint the project just deleted.
            projects::delete_project_satellites(tx, &canonical)?;

            // --- 4. Rewrite the vault truth of everything the deletion touched ---------------
            //
            // Two populations, and they move differently. `moved` are documents HOMED here, re-homed
            // to Unsorted. `linked_elsewhere` are documents homed in another project that merely
            // carried this one as an extra membership: they stay where they are (hence `None`), but
            // their files still name the dead project and must be rewritten too.
            rewrite_documents(
                tx,
                vault,
                cipher,
                vault_root,
                manifest_cipher,
                &moved,
                Some(UNSORTED),
                &mut out.written,
            )?;
            let linked_elsewhere: Vec<i64> = linked_members
                .into_iter()
                .filter(|id| !moved.contains(id) && !deleted.contains(id))
                .collect();
            rewrite_documents(
                tx,
                vault,
                cipher,
                vault_root,
                manifest_cipher,
                &linked_elsewhere,
                None,
                &mut out.written,
            )?;

            // --- 5. Project-scoped preferences ---------------------------------------------
            // `preferences.entity_id` REFERENCES entities(id) ON DELETE CASCADE, and those records
            // live ONLY in the database — no vault copy, nothing to re-derive them from. Dropping
            // the entity below would silently destroy everything the user taught PM about this
            // project. Go dormant instead (the same move `rebuild_mirror_from_rules` makes), so
            // they stay listed in Teach.
            tx.execute(
                "UPDATE preferences SET entity_id = NULL WHERE entity_id = ?1",
                params![entity_id],
            )?;
            // `calendar_events.entity_id` is the third FK into `entities` and the only one with no
            // ON DELETE action left unhandled. Nothing writes it yet (v18 added it as the
            // correspondence slot), so this clears nothing today — but the `Free` arm below deletes
            // the entity row, and the day that column gains a writer it would abort the whole
            // delete with an opaque FK error. That is the exact failure the chat pointer above just
            // caused; one line closes it in advance rather than after a user hits it.
            tx.execute(
                "UPDATE calendar_events SET entity_id = NULL WHERE entity_id = ?1",
                params![entity_id],
            )?;

            // --- 6. The name ----------------------------------------------------------------
            match name {
                NameDisposition::Unsorted => {
                    // Literally a merge into the inbox: the aliases (including this project's own
                    // canonical) fold onto Unsorted, so the old name keeps resolving there forever.
                    entities::merge_entities(tx, entity_id, unsorted_id)?;
                }
                NameDisposition::Free => {
                    // Free the name. `documents.entity_id` REFERENCES entities(id) with NO ON
                    // DELETE action and `PRAGMA foreign_keys = ON`, so this DELETE fails outright
                    // while any row still points at the entity — which is exactly why it comes
                    // after the document and preference steps above rather than before them.
                    tx.execute(
                        "DELETE FROM entity_aliases WHERE entity_id = ?1",
                        params![entity_id],
                    )?;
                    tx.execute("DELETE FROM entities WHERE id = ?1", params![entity_id])?;
                }
            }
            Ok(())
        },
    )
    .await
}

/// Merge `from_id` into `into_id`: fold aliases, repoint every document, rewrite their frontmatter
/// + cache to the target canonical, and delete the empty source — the headline action that fixes
/// the variant pain in one move and stops it recurring.
#[tauri::command]
pub async fn merge_entities(app: AppHandle, from_id: i64, into_id: i64) -> Result<()> {
    spawn_entity_mutation(
        app,
        move |tx, vault, cipher, vault_root, manifest_cipher, files| {
            // Capture the folded project's name BEFORE the merge deletes the source entity, then fold
            // its name-keyed satellites into the survivor's name (F-05). `rename_project_satellites`
            // keeps the survivor's own triage (INSERT OR IGNORE) and sums the daily rollup on collision.
            let old = entities::canonical_name(tx, from_id)?;
            // Captured before the satellites (and with them the project tag) are folded.
            let members = crate::tags::documents_tagged(tx, &old)?;
            entities::merge_entities(tx, from_id, into_id)?;
            let canonical = entities::canonical_name(tx, into_id)?;
            projects::rename_project_satellites(tx, &old, &canonical)?;
            rewrite_after_project_rekey(
                tx,
                vault,
                cipher,
                vault_root,
                manifest_cipher,
                into_id,
                &canonical,
                &members,
                &mut files.written,
            )
        },
    )
    .await
}

// --- personal assistant: projects & focus view (Step 5) ---

/// Every active project with its triage metadata and one derived status — the
/// focus view's data (spec §4.1).
#[tauri::command]
pub fn list_project_overviews(state: State<'_, AppState>) -> Result<Vec<ProjectOverview>> {
    let conn = state.conn()?;
    let today = clock::today_sql_in(resolve_zone(&conn));
    projects::list_overviews(&conn, &today)
}

/// Set (or update) a project's triage metadata — the user confirming/correcting an
/// AI proposal, or editing by hand in the focus/project view. Creates the row on
/// first set; blanks clear a field.
#[tauri::command]
pub fn set_project_metadata(
    state: State<'_, AppState>,
    name: String,
    deadline: Option<String>,
    size: Option<String>,
    blocked_by: Option<String>,
    // Manual priority override ("high"/"medium"/"low"); None / "auto" / blank = Auto (no tag).
    // Optional on the wire so an older caller that omits it still deserializes (serde → None).
    importance: Option<String>,
) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::Other("project name is empty".into()));
    }
    let conn = state.conn()?;
    projects::set_metadata(&conn, name, deadline, size, blocked_by, importance)
}

/// Propose triage metadata (size/blocked-by/deadline) for projects, on
/// demand — the AI-proposes-you-confirm half of the focus view, mirroring
/// `propose_metadata`. `names` limits it to specific projects (default: all).
/// Proposals stream over `on_event`; the user confirms via `set_project_metadata`.
/// Runs on the background API key; never holds the DB lock across a model call.
#[tauri::command]
pub async fn propose_project_metadata(
    app: AppHandle,
    names: Option<Vec<String>>,
    on_event: Channel<ProjectProposalEvent>,
) -> Result<()> {
    let Some(plan) = llm_gateway::resolve(&app, Role::Background)? else {
        return Err(Error::Other(llm_gateway::no_provider_message()));
    };

    // Bound the (untrusted webview) name list — one model call per name, so this
    // also caps runaway spend. Far above any real project count.
    const MAX_PROPOSE_NAMES: usize = 2_000;
    if names.as_ref().is_some_and(|n| n.len() > MAX_PROPOSE_NAMES) {
        return Err(Error::Other("too many projects selected at once".into()));
    }

    struct Target {
        name: String,
        samples: Vec<String>,
    }

    // Gather targets + their document samples + the full project list (for picking
    // a real parent/blocker) + models under a short lock, then drop it (rule #4).
    let (targets, all_projects) = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let all_projects: Vec<String> = db::distinct_projects(&conn)?;
        let target_names = match names {
            Some(n) if !n.is_empty() => n,
            _ => all_projects.clone(),
        };
        let mut targets = Vec::new();
        for name in target_names {
            let samples = projects::document_samples(&conn, &name)?;
            targets.push(Target { name, samples });
        }
        (targets, all_projects)
    };

    let mut proposed = 0;
    let mut usage_rows: Vec<(Option<String>, openrouter::Usage, llm_gateway::CallMeta)> =
        Vec::new();
    for t in targets {
        let others: Vec<String> = all_projects
            .iter()
            .filter(|p| **p != t.name)
            .cloned()
            .collect();
        let (proposal, usage_info) =
            projects::propose(&app, &plan, &t.name, &t.samples, &others).await;
        if let Some((usage, served, meta)) = usage_info {
            usage_rows.push((served, usage, meta));
        }
        let _ = on_event.send(ProjectProposalEvent::Proposed {
            project: t.name,
            proposal,
        });
        proposed += 1;
    }
    log_background_usage(&app, plan.models(), &usage_rows);
    let _ = on_event.send(ProjectProposalEvent::Finished { proposed });
    Ok(())
}

/// The guards on the project-level *Merge into* (#279). Every one of these refuses BEFORE the
/// user types a confirmation, which is the point: a merge that fails halfway through the
/// ceremony reads as a bug, and one of these cases (merging out of Unsorted) would sweep the
/// whole inbox into another project if it ever got through.
#[cfg(test)]
mod merge_project_tests {
    use super::resolve_merge_pair;
    use crate::entities;

    const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn conn_with_projects(names: &[&str]) -> rusqlite::Connection {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("t.sqlite"), DB_KEY).unwrap();
        // Leak the tempdir: the Connection must outlive it, and these are short-lived tests.
        std::mem::forget(dir);
        for n in names {
            entities::resolve_project(&conn, n, true).unwrap();
        }
        conn
    }

    #[test]
    fn resolves_a_real_pair_and_reports_the_target_canonical() {
        let conn = conn_with_projects(&["Landing Page Redesign", "Marketing"]);
        let (from, into, canonical) =
            resolve_merge_pair(&conn, "Landing Page Redesign", "Marketing").unwrap();
        assert_ne!(from, into);
        // The canonical is what the documents end up filed under — and so what the user types.
        assert_eq!(canonical, "Marketing");
    }

    #[test]
    fn refuses_merging_a_project_into_itself() {
        let conn = conn_with_projects(&["Atlas"]);
        let err = resolve_merge_pair(&conn, "Atlas", "Atlas").unwrap_err();
        assert!(
            err.to_string().contains("same project"),
            "unexpected error: {err}"
        );
    }

    /// An ALIAS of the target resolves to the same entity, so this is the self-merge case wearing
    /// a different name — and the one a user is most likely to reach by accident.
    #[test]
    fn refuses_a_self_merge_reached_through_an_alias() {
        let conn = conn_with_projects(&["Personal Manager"]);
        let id = entities::resolve_project(&conn, "Personal Manager", false)
            .unwrap()
            .unwrap();
        entities::add_alias(&conn, id, "PM").unwrap();
        let err = resolve_merge_pair(&conn, "PM", "Personal Manager").unwrap_err();
        assert!(
            err.to_string().contains("same project"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn refuses_merging_out_of_the_unsorted_inbox() {
        let conn = conn_with_projects(&["Unsorted", "Marketing"]);
        let err = resolve_merge_pair(&conn, "Unsorted", "Marketing").unwrap_err();
        assert!(err.to_string().contains("inbox"), "unexpected error: {err}");
        // Merging INTO Unsorted stays allowed — a deliberate "these belong back in the inbox".
        assert!(resolve_merge_pair(&conn, "Marketing", "Unsorted").is_ok());
    }

    #[test]
    fn refuses_an_unknown_or_blank_project() {
        let conn = conn_with_projects(&["Marketing"]);
        assert!(resolve_merge_pair(&conn, "Ghost", "Marketing").is_err());
        assert!(resolve_merge_pair(&conn, "Marketing", "Ghost").is_err());
        assert!(resolve_merge_pair(&conn, "   ", "Marketing").is_err());
    }

    // --- delete guards (#573) ---------------------------------------------------

    #[test]
    fn delete_resolves_a_real_project_to_its_canonical() {
        let conn = conn_with_projects(&["Marketing"]);
        let (id, canonical) = super::resolve_deletable_project(&conn, "Marketing").unwrap();
        assert!(id > 0);
        assert_eq!(canonical, "Marketing");
    }

    /// Reached through an alias, the canonical is what comes back — which is what the dialog asks
    /// the user to type, so confirming against the clicked label would be confirming the wrong name.
    #[test]
    fn delete_through_an_alias_reports_the_canonical_name() {
        let conn = conn_with_projects(&["Personal Manager"]);
        let id = entities::resolve_project(&conn, "Personal Manager", false)
            .unwrap()
            .unwrap();
        entities::add_alias(&conn, id, "PM").unwrap();
        let (resolved, canonical) = super::resolve_deletable_project(&conn, "PM").unwrap();
        assert_eq!(resolved, id);
        assert_eq!(canonical, "Personal Manager");
    }

    /// Deleting the inbox would destroy or strand every unreviewed document in it.
    #[test]
    fn refuses_to_delete_the_unsorted_inbox() {
        let conn = conn_with_projects(&["Unsorted", "Marketing"]);
        let err = super::resolve_deletable_project(&conn, "Unsorted").unwrap_err();
        assert!(err.to_string().contains("inbox"), "unexpected error: {err}");
        assert!(super::resolve_deletable_project(&conn, "Marketing").is_ok());
    }

    #[test]
    fn refuses_to_delete_an_unknown_or_blank_project() {
        let conn = conn_with_projects(&["Marketing"]);
        assert!(super::resolve_deletable_project(&conn, "Ghost").is_err());
        assert!(super::resolve_deletable_project(&conn, "   ").is_err());
    }
}
