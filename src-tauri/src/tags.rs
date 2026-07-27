// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The tag registry (#275) — one table for both kinds of tag, and the join that binds a document
//! to them.
//!
//! The organising idea is Bobby's: **every project IS a tag**. A document belonging to several
//! projects then falls out of ordinary multi-tagging instead of needing its own many-to-many
//! machinery, and one `@tag` grammar can later reach both (#276). The `kind` column keeps the two
//! populations apart where they genuinely differ:
//!
//! - `kind = 'project'` rows mirror a real project. Their names keep the user's **verbatim
//!   casing**, because `projects.name` is a primary key and `entities.canonical_name` is the alias
//!   key — force-lowercasing here would collide with both. Only these rows touch the entity/alias
//!   space.
//! - `kind = 'group'` rows are the free-form lowercase labels the tag editor writes. Their TRUTH is
//!   still `documents.tags` — the JSON blob the vault's `tags:` line round-trips — and these rows
//!   are the queryable index over it, backfilled by v47 and kept in step by
//!   `write_document_truth`. They exist because `@tag` (#276) has to answer "which documents carry
//!   this tag" as a JOIN, to intersect with a chunk allow-set; the blob cannot answer that without
//!   scanning every row.
//!
//! Matching is case-insensitive, via a stored `norm` column rather than `COLLATE NOCASE` — the
//! same shape `preferences` already uses, and for the same reason recorded there: SQLite's
//! `lower()` is ASCII-only with no ICU, so the normalisation has to be visible and identical on
//! both sides rather than hidden in a collation.
//!
//! **Membership keys on tag id, never on the name string.** Re-keying a name across a dozen tables
//! is exactly the failure the entity layer was built to end; a rename here moves one row.
//!
//! The join is a *derived* index over what the vault already says (`project:` +
//! `linked_projects:` front-matter, or the index-only manifest). [`crate::ingest`]'s
//! `write_document_truth` is the only writer, which is what keeps the two from drifting — see
//! INVARIANTS I-02.

use rusqlite::{params, Connection};
use std::collections::HashMap;

use crate::error::Result;

/// A tag that mirrors a real project. Only these reach the entity/alias space.
pub const KIND_PROJECT: &str = "project";
/// A free-form label — what the tag editor writes. Populated from `documents.tags` by v47, and kept
/// in step by [`crate::ingest::write_document_truth`].
pub const KIND_GROUP: &str = "group";

/// A subquery naming every `(document_id, name, norm)` project membership in the store.
///
/// It is the UNION of two things, and the asymmetry is deliberate: a document's HOME comes straight
/// from `documents.project`, and only its EXTRA memberships come from the join.
///
/// The join is a derived index, and derived state can be behind — a store part-way through a
/// migration, one restored from a backup taken before a Rebuild, a code path that writes the column
/// without going through [`crate::ingest::write_document_truth`]. If the roster, the file lists and
/// the retrieval scope all read the join alone, any of those makes a project's own documents
/// silently vanish from the project. Deriving the home from the column it is stored in means the
/// join can only ever ADD memberships, never authorise the ones that were always there.
///
/// The `NOT EXISTS` guard keeps a document from appearing twice when the join already holds its
/// home under different casing — which would otherwise inflate its project's file count.
pub const MEMBERSHIPS_SQL: &str = "SELECT d.id AS document_id, d.project AS name, \
            lower(trim(d.project)) AS norm \
     FROM documents d \
     WHERE trim(COALESCE(d.project,'')) <> '' \
       AND NOT EXISTS (SELECT 1 FROM document_tags dt JOIN tags t ON t.id = dt.tag_id \
                       WHERE dt.document_id = d.id AND t.kind = 'project' \
                         AND t.norm = lower(trim(d.project))) \
     UNION ALL \
     SELECT dt.document_id, t.name, t.norm \
     FROM document_tags dt JOIN tags t ON t.id = dt.tag_id AND t.kind = 'project'";

/// The matching key for a tag name: trimmed and ASCII-lowercased.
///
/// `to_ascii_lowercase`, NOT `to_lowercase` — this has to agree byte-for-byte with the SQL
/// `lower(trim(...))` the v46 backfill computes, and SQLite's `lower()` is ASCII-only with no ICU.
/// Rust's Unicode-aware version would fold "ÉTÉ" to "été" where SQLite leaves it alone, so one
/// project would be looked up under a key the migration never wrote, mint a second tag row, and
/// split its memberships across both — with the unique index none the wiser, because the two norms
/// genuinely differ. `preferences.rs` learned this and says so at its own normalisation.
///
/// Deliberately NOT applied to the stored `name`: a project displays as the user typed it
/// ("Atlas, Inc.") and matches however they type it next time.
pub fn normalize(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

/// Find-or-create the tag row for `(kind, name)`, returning its id.
///
/// Find-first, so a project that already exists under different casing keeps its established
/// display name rather than being renamed by whoever typed it most recently. Renaming a project is
/// a deliberate act with its own command; it should not be a side effect of tagging.
pub fn intern(conn: &Connection, kind: &str, name: &str) -> Result<i64> {
    let display = name.trim();
    let norm = normalize(display);
    if norm.is_empty() {
        return Err(crate::error::Error::Other("a tag needs a name".into()));
    }
    if let Some(id) = lookup(conn, kind, &norm)? {
        return Ok(id);
    }
    conn.execute(
        "INSERT OR IGNORE INTO tags (kind, name, norm) VALUES (?1, ?2, ?3)",
        params![kind, display, norm],
    )?;
    // Re-read rather than trusting `last_insert_rowid`: the OR IGNORE above may have been a no-op
    // if a concurrent path interned the same name first, and returning 0 there would bind the
    // document to nothing.
    lookup(conn, kind, &norm)?
        .ok_or_else(|| crate::error::Error::Other("could not intern tag".into()))
}

fn lookup(conn: &Connection, kind: &str, norm: &str) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM tags WHERE kind = ?1 AND norm = ?2",
            params![kind, norm],
            |r| r.get(0),
        )
        .ok())
}

/// Replace a document's whole project membership set — the home plus its other projects — in one
/// pass.
///
/// Called only from `ingest::write_document_truth`, right after the vault (or manifest) truth is
/// written, so the join can never claim something the vault doesn't say. Delete-then-insert rather
/// than a diff: the set is tiny, and a diff has failure modes ("removed everywhere but one place")
/// that a replace does not.
///
/// The home is interned unconditionally, so a project that exists ONLY as somewhere documents are
/// filed still has a registry row for the pickers to offer.
pub fn set_document_projects(
    conn: &Connection,
    doc_id: i64,
    home: &str,
    linked: &[String],
) -> Result<()> {
    conn.execute(
        "DELETE FROM document_tags WHERE document_id = ?1 AND tag_id IN \
         (SELECT id FROM tags WHERE kind = 'project')",
        params![doc_id],
    )?;

    let mut seen: Vec<String> = Vec::new();
    for name in std::iter::once(home).chain(linked.iter().map(String::as_str)) {
        let norm = normalize(name);
        if norm.is_empty() || seen.contains(&norm) {
            continue;
        }
        seen.push(norm);
        let tag_id = intern(conn, KIND_PROJECT, name)?;
        conn.execute(
            "INSERT OR IGNORE INTO document_tags (document_id, tag_id) VALUES (?1, ?2)",
            params![doc_id, tag_id],
        )?;
    }
    gc_orphan_project_tags(conn)
}

/// Drop project tags that nothing refers to any more.
///
/// Without this the registry only grows: unlinking the last document from a project would leave the
/// name in every picker forever, and a typo made once would be offered as a real project for the
/// life of the store. A row survives if it still has a membership OR the project has a triage row
/// of its own — a project with a deadline and no documents yet is real and must stay offerable.
fn gc_orphan_project_tags(conn: &Connection) -> Result<()> {
    conn.execute(
        "DELETE FROM tags WHERE kind = 'project' \
           AND NOT EXISTS (SELECT 1 FROM document_tags dt WHERE dt.tag_id = tags.id) \
           AND NOT EXISTS (SELECT 1 FROM projects p WHERE lower(trim(p.name)) = tags.norm)",
        [],
    )?;
    Ok(())
}

/// Replace a document's group-tag memberships.
///
/// The counterpart to [`set_document_projects`], called from the same seam and for the same reason:
/// `documents.tags` stays the truth, and this keeps the queryable index over it from drifting.
pub fn set_document_group_tags(conn: &Connection, doc_id: i64, tags: &[String]) -> Result<()> {
    conn.execute(
        "DELETE FROM document_tags WHERE document_id = ?1 AND tag_id IN \
         (SELECT id FROM tags WHERE kind = 'group')",
        params![doc_id],
    )?;
    let mut seen: Vec<String> = Vec::new();
    for name in tags {
        let norm = normalize(name);
        if norm.is_empty() || seen.contains(&norm) {
            continue;
        }
        seen.push(norm);
        let tag_id = intern(conn, KIND_GROUP, name)?;
        conn.execute(
            "INSERT OR IGNORE INTO document_tags (document_id, tag_id) VALUES (?1, ?2)",
            params![doc_id, tag_id],
        )?;
    }
    // A label that just lost its last document should stop being offered. Unlike a project, a group
    // tag has no triage row that could keep it alive, so "no memberships" is the whole test.
    conn.execute(
        "DELETE FROM tags WHERE kind = 'group' \
           AND NOT EXISTS (SELECT 1 FROM document_tags dt WHERE dt.tag_id = tags.id)",
        [],
    )?;
    Ok(())
}

/// One tag as the pickers and the `@` autocomplete see it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TagSummary {
    pub name: String,
    /// `"project"` or `"group"`. The two behave differently when pinned, and the UI says which.
    pub kind: String,
    /// How many documents carry it. The autocomplete orders by this, so tags someone actually uses
    /// come before ones they typed once.
    pub documents: i64,
}

/// Every tag in the registry with its kind and use count, most-used first.
///
/// Project rows count through [`MEMBERSHIPS_SQL`], so a project whose documents are all merely
/// linked to it still counts honestly; group rows count through the join, which for them is the
/// whole story.
pub fn list_all(conn: &Connection) -> Result<Vec<TagSummary>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT t.name, t.kind, \
                CASE WHEN t.kind = 'project' \
                     THEN (SELECT COUNT(*) FROM ({MEMBERSHIPS_SQL}) m WHERE m.norm = t.norm) \
                     ELSE (SELECT COUNT(*) FROM document_tags dt WHERE dt.tag_id = t.id) END \
         FROM tags t ORDER BY 3 DESC, t.norm"
    ))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(TagSummary {
                name: r.get(0)?,
                kind: r.get(1)?,
                documents: r.get(2)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// How many established labels the filing prompt is shown. Enough that a real vocabulary is
/// visible, few enough that the list stays a hint rather than a menu the model works through — and
/// bounded, because unlike projects the label space has no natural ceiling.
const PROMPT_TAG_LIMIT: usize = 60;

/// The free-form labels already in use, most-used first, for the filing prompt.
///
/// Tags only earn their keep by GROUPING things (Bobby, 2026-07-27). The prompt has always named
/// the existing projects and asked the model to prefer one; it said nothing at all about existing
/// tags, so every batch invented its own vocabulary and `tax` / `taxes` / `taxation` accumulated
/// side by side. Naming them is the cheap half of the fix — the half that stops new drift.
///
/// Only `kind = 'group'`: a project is reachable as a project, and offering project names here
/// would invite the model to duplicate a document's filing as a label.
///
/// The order is deterministic (count, then normalised name) because this list goes in the CACHED
/// system prefix (#509) — a set that reshuffled between calls in a run would silently cost the
/// prompt cache on every document.
pub fn common_group_tags(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT t.name FROM tags t \
         WHERE t.kind = 'group' \
         ORDER BY (SELECT COUNT(*) FROM document_tags dt WHERE dt.tag_id = t.id) DESC, t.norm \
         LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![PROMPT_TAG_LIMIT as i64], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// The `@mentions` in a message, in the order written, deduplicated on the normalised form.
///
/// A bare mention ends at whitespace — a greedy rule would swallow the rest of the sentence — and
/// trailing sentence punctuation is not part of the name. It must START a word, so
/// `bob@example.com` mentions nothing.
///
/// A name with a space in it is quoted: `@"Atlas, Inc."`. Without that form the projects #275 went
/// out of its way to allow would be the exact ones that could never be pinned, which is not a
/// limitation worth shipping.
///
/// **Code is skipped** — fenced blocks and inline spans. Someone pasting a diff, a log or a commit
/// trailer is quoting, not addressing, and a line-initial `@Something` in pasted code that happened
/// to collide with a tag name would otherwise widen their retrieval scope with nothing to show for
/// it. `chatMarkdown.ts` skips the same two constructs, for the same reason.
///
/// This returns CANDIDATES. Nothing is a tag until [`resolve_mentions`] finds it in the registry,
/// so a stray `@` in prose widens no one's retrieval scope.
pub fn parse_mentions(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for span in prose_spans(text) {
        collect_mentions(span, &mut out);
    }
    out
}

/// The parts of `text` that are prose rather than code: everything outside a fenced block and
/// outside a `backtick` span, in order.
fn prose_spans(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        // Inline code: keep the even-indexed runs, i.e. the text OUTSIDE the backtick spans.
        for (i, piece) in line.split('`').enumerate() {
            if i % 2 == 0 {
                out.push(piece);
            }
        }
    }
    out
}

fn collect_mentions(text: &str, out: &mut Vec<String>) {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] != '@' {
            i += 1;
            continue;
        }
        if i > 0 && !chars[i - 1].is_whitespace() && chars[i - 1] != '(' {
            i += 1;
            continue;
        }
        let (name, next) = read_mention(&chars, i);
        if !name.is_empty() && !out.iter().any(|p| normalize(p) == normalize(&name)) {
            out.push(name);
        }
        i = next.max(i + 1);
    }
}

/// Read the mention starting at the `@` in `chars[at]`, returning its name and the index just past
/// it. Shared by the parser and the stripper so the two can never disagree about where a mention
/// ends — which would leave a fragment of a pin in the text that reaches the embedder.
fn read_mention(chars: &[char], at: usize) -> (String, usize) {
    let start = at + 1;
    if chars.get(start) == Some(&'"') {
        let open = start + 1;
        let mut end = open;
        while end < chars.len() && chars[end] != '"' {
            end += 1;
        }
        // An unclosed quote is a quote the user is still typing, not a mention.
        if end >= chars.len() {
            return (String::new(), start);
        }
        return (chars[open..end].iter().collect(), end + 1);
    }
    let mut end = start;
    while end < chars.len() && !chars[end].is_whitespace() {
        end += 1;
    }
    let raw: String = chars[start..end].iter().collect();
    let trimmed = raw.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '"']);
    let kept = trimmed.chars().count();
    (trimmed.to_string(), start + kept)
}

/// One resolved `@mention`: a specific row in the registry, not merely a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedTag {
    pub id: i64,
    /// The canonical spelling, so the highlight and the widening agree on one form.
    pub name: String,
    pub kind: String,
}

/// The mention candidates that are REAL tags, resolved to specific registry rows.
///
/// Resolution is to ONE row, by id — never to a name both kinds happen to share. A project called
/// `Research` and a label called `research` are different things (that is what the per-kind unique
/// index is for), and widening by name would quietly pull in both: someone pinning a free-form
/// label would get an entire same-named project's documents, home and linked, with nothing
/// anywhere saying so. The card's own discipline — no silent cross-project retrieval — rules it out.
///
/// A collision resolves to the PROJECT: it is the heavier concept and the likelier intent when a
/// name is shared. The `kind` travels with the result, so a caller can say which was meant.
pub fn resolve_mentions(conn: &Connection, candidates: &[String]) -> Result<Vec<PinnedTag>> {
    let mut out: Vec<PinnedTag> = Vec::new();
    // Explicit CASE, not `ORDER BY kind`: 'group' sorts before 'project', so ordering on the column
    // alone would pick exactly the wrong row.
    let mut stmt = conn.prepare(
        "SELECT id, name, kind FROM tags WHERE norm = ?1 \
         ORDER BY CASE kind WHEN 'project' THEN 0 ELSE 1 END LIMIT 1",
    )?;
    for c in candidates {
        let norm = normalize(c);
        if norm.is_empty() {
            continue;
        }
        let found = stmt
            .query_row(params![norm], |r| {
                Ok(PinnedTag {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    kind: r.get(2)?,
                })
            })
            .ok();
        if let Some(tag) = found {
            if !out.iter().any(|t| t.id == tag.id) {
                out.push(tag);
            }
        }
    }
    Ok(out)
}

/// Remove the RESOLVED mentions from a message, leaving the rest of the words intact.
///
/// This is what reaches the embedder and the keyword index. Leaving `@marketing` in would make the
/// pin a relevance boost as well as a scope — the bare term is embedded, and `fts_query` splits on
/// non-alphanumerics so it is OR-ed into the MATCH too. Scope-not-boost is the settled decision: a
/// boost is worth adding only once the relevance-feedback corpus (#566) can calibrate one, and a
/// boost applied silently now would be impossible to separate from the scoping in that data.
///
/// Only resolved mentions go. An unrecognised `@word` is ordinary prose, and removing it would
/// change the question being asked. If stripping would leave nothing to search on, the original is
/// kept — `@marketing` alone still means "what is in Marketing".
pub fn strip_mentions(text: &str, pinned: &[PinnedTag]) -> String {
    if pinned.is_empty() {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '@' && (i == 0 || chars[i - 1].is_whitespace() || chars[i - 1] == '(') {
            let (name, next) = read_mention(&chars, i);
            if !name.is_empty()
                && pinned
                    .iter()
                    .any(|t| normalize(&t.name) == normalize(&name))
            {
                i = next;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    if out.trim().is_empty() {
        return text.to_string();
    }
    out
}

/// The chunk ids of every document carrying any of `names` — what a pinned tag contributes to a
/// retrieval allow-set.
///
/// Empty `names` yields an EMPTY set, never "everything". A widening that silently degraded to
/// no-filter would break the guarantee a project chat rests on: it retrieves only what it was told
/// to.
pub fn tag_chunk_ids(
    conn: &Connection,
    pinned: &[PinnedTag],
) -> Result<std::collections::HashSet<i64>> {
    let mut out = std::collections::HashSet::new();
    if pinned.is_empty() {
        return Ok(out);
    }
    // A project tag reaches documents through MEMBERSHIPS_SQL (home + links, so a document linked
    // into the project counts too); a group tag reaches them through the join alone. Both key on
    // the tag's ID, so pinning one of two same-named tags widens by exactly the row that resolved.
    let mut project_stmt = conn.prepare(&format!(
        "SELECT c.id FROM chunks c WHERE c.document_id IN ( \
             SELECT m.document_id FROM ({MEMBERSHIPS_SQL}) m \
             WHERE m.norm = (SELECT norm FROM tags WHERE id = ?1) \
         )"
    ))?;
    let mut group_stmt = conn.prepare(
        "SELECT c.id FROM chunks c \
         JOIN document_tags dt ON dt.document_id = c.document_id \
         WHERE dt.tag_id = ?1",
    )?;
    for tag in pinned {
        // The two statements' row iterators are distinct closure types, so each branch drains its
        // own rather than trying to unify them behind one binding.
        if tag.kind == KIND_PROJECT {
            for id in project_stmt.query_map(params![tag.id], |r| r.get::<_, i64>(0))? {
                out.insert(id?);
            }
        } else {
            for id in group_stmt.query_map(params![tag.id], |r| r.get::<_, i64>(0))? {
                out.insert(id?);
            }
        }
    }
    Ok(out)
}

/// A document's project memberships OTHER than `home`, in stable (name) order.
///
/// This is what the vault's `linked_projects:` key holds, so every rewrite path can re-derive the
/// list it must write without threading it through the whole call stack. Comparing on the
/// normalised form means a hand-edited file that repeats the home under different casing still
/// yields "no extras" rather than a document linked to itself.
pub fn linked_projects(conn: &Connection, doc_id: i64, home: &str) -> Result<Vec<String>> {
    let home_norm = normalize(home);
    let mut stmt = conn.prepare(&format!(
        "SELECT name FROM ({MEMBERSHIPS_SQL}) WHERE document_id = ?1 AND norm <> ?2 ORDER BY name"
    ))?;
    let rows = stmt
        .query_map(params![doc_id, home_norm], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Every document that carries the project tag `name` — home or linked.
///
/// The rename/merge/delete paths need this BEFORE they mutate the tag, because those documents'
/// vault files name the project in their `linked_projects:` line and must be rewritten. Miss them
/// and the next Rebuild reads the dead name straight back out of the vault and re-mints the
/// project that was just merged away.
pub fn documents_tagged(conn: &Connection, name: &str) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT DISTINCT document_id FROM ({MEMBERSHIPS_SQL}) WHERE norm = ?1 ORDER BY document_id"
    ))?;
    let rows = stmt
        .query_map(params![normalize(name)], |r| r.get::<_, i64>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Every document carrying the free-form label `name`, with its FULL current tag list.
///
/// The whole list, not just the match, because the callers (delete / rename a tag everywhere, #579)
/// have to hand `write_document_truth` the complete set the document should end up with — the truth
/// is `documents.tags`, and a partial write would drop everything it did not mention.
///
/// Ordered by id so a bulk rewrite touches vault files in a stable order, which makes a failure
/// midway reproducible rather than dependent on join order.
pub fn documents_with_group_tag(conn: &Connection, name: &str) -> Result<Vec<(i64, Vec<String>)>> {
    let mut stmt = conn.prepare(
        "SELECT d.id, d.tags FROM documents d \
         JOIN document_tags dt ON dt.document_id = d.id \
         JOIN tags t ON t.id = dt.tag_id AND t.kind = 'group' \
         WHERE t.norm = ?1 ORDER BY d.id",
    )?;
    let rows = stmt
        .query_map(params![normalize(name)], |r| {
            let json: String = r.get(1)?;
            Ok((
                r.get::<_, i64>(0)?,
                serde_json::from_str::<Vec<String>>(&json).unwrap_or_default(),
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Drop group-tag registry rows no document carries any more.
///
/// [`set_document_group_tags`] already runs exactly this after every write, so the normal paths need
/// no help: rewriting a document's tags heals any label that just lost its last document, anywhere
/// in the store. This exists for the one case that write cannot reach — deleting a tag that is
/// ALREADY carried by nothing, where there is no document to rewrite and so no write to piggyback
/// on. Idempotent, so calling it after a bulk rewrite that already pruned costs one no-op statement.
///
/// Group rows only. A project tag legitimately exists with no documents (it can be created as a
/// triage row before anything is filed into it), and `project_names` depends on that.
pub fn prune_orphan_group_tags(conn: &Connection) -> Result<usize> {
    let n = conn.execute(
        "DELETE FROM tags WHERE kind = 'group' \
           AND NOT EXISTS (SELECT 1 FROM document_tags dt WHERE dt.tag_id = tags.id)",
        [],
    )?;
    Ok(n)
}

/// Point the project tag `old` at `new`, folding into `new`'s existing tag if there is one.
///
/// The rename arm is a one-row update. The merge arm can't be: `document_tags` is keyed
/// `(document_id, tag_id)`, so a document already in BOTH projects would collide — hence
/// INSERT OR IGNORE the survivor's rows first, then drop the old tag and let the join cascade.
/// This mirrors `projects::rename_project_satellites`' treatment of `project_activity_daily`,
/// which has the same shape of problem for the same reason.
pub fn rename_project_tag(conn: &Connection, old: &str, new: &str) -> Result<()> {
    let (old_norm, new_norm) = (normalize(old), normalize(new));
    if old_norm.is_empty() || new_norm.is_empty() || old_norm == new_norm {
        return Ok(());
    }
    let Some(old_id) = lookup(conn, KIND_PROJECT, &old_norm)? else {
        return Ok(());
    };
    match lookup(conn, KIND_PROJECT, &new_norm)? {
        Some(new_id) => {
            conn.execute(
                "INSERT OR IGNORE INTO document_tags (document_id, tag_id) \
                 SELECT document_id, ?2 FROM document_tags WHERE tag_id = ?1",
                params![old_id, new_id],
            )?;
            conn.execute("DELETE FROM tags WHERE id = ?1", params![old_id])?;
        }
        None => {
            conn.execute(
                "UPDATE tags SET name = ?2, norm = ?3 WHERE id = ?1",
                params![old_id, new.trim(), new_norm],
            )?;
        }
    }
    Ok(())
}

/// Drop a project tag and, by cascade, every membership of it.
///
/// Only the registry side. Re-homing or deleting the documents themselves is the caller's
/// disposition to make (`delete_project`), and the vault rewrite that follows is what makes this
/// stick past the next Rebuild.
pub fn delete_project_tag(conn: &Connection, name: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM tags WHERE kind = 'project' AND norm = ?1",
        params![normalize(name)],
    )?;
    Ok(())
}

/// Every project-kind tag name, for the pickers. Ordered case-insensitively so a list mixing
/// "atlas" and "Atlas Ltd" reads the way a person would sort it.
pub fn project_names(conn: &Connection) -> Result<Vec<String>> {
    // Registry rows — which include every project that exists as a triage row only — plus,
    // defensively, any home project the join has not caught up with. A picker that cannot offer a
    // project the user is looking at is worse than one that offers a stale name.
    let mut stmt = conn.prepare(&format!(
        "SELECT name FROM ( \
             SELECT name, norm FROM tags WHERE kind = 'project' \
             UNION \
             SELECT name, norm FROM ({MEMBERSHIPS_SQL}) \
         ) GROUP BY norm ORDER BY norm, name"
    ))?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Every document's project memberships in one query, keyed by document id.
///
/// The list surfaces need this for hundreds of rows at a time; asking per document would be an
/// N+1 across the whole library. One scan of a small join beats that at every size.
pub fn all_project_memberships(conn: &Connection) -> Result<HashMap<i64, Vec<String>>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT document_id, name FROM ({MEMBERSHIPS_SQL}) ORDER BY document_id, name"
    ))?;
    let mut out: HashMap<i64, Vec<String>> = HashMap::new();
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (doc_id, name) = row?;
        out.entry(doc_id).or_default().push(name);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn store() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        (dir, conn)
    }

    fn doc(conn: &Connection, id: i64, project: &str) {
        conn.execute(
            "INSERT INTO documents (id, vault_path, title, content_hash, project) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                id,
                format!("d{id}.md"),
                format!("Doc {id}"),
                format!("h{id}"),
                project
            ],
        )
        .unwrap();
    }

    /// The group-tag backfill reads `documents.tags` — a JSON array in a TEXT column — with
    /// `json_each`. JSON1 is compiled into SQLite by default and rusqlite's bundled amalgamation
    /// keeps it, but "by default" is not a guarantee this repo can afford to assume: a migration
    /// that calls a missing function fails at OPEN, on the user's machine, with the store
    /// half-migrated. Pin it here so the build tells us instead.
    #[test]
    fn a_mention_must_begin_a_word_and_ends_at_whitespace() {
        assert_eq!(parse_mentions("ask @Marketing about it"), ["Marketing"]);
        // Sentence punctuation is not part of the name.
        assert_eq!(parse_mentions("see @Atlas, Inc. later"), ["Atlas"]);
        // A name with a space in it needs quoting — otherwise the projects #275 went out of its
        // way to allow would be exactly the ones that could never be pinned.
        assert_eq!(
            parse_mentions(r#"see @"Atlas, Inc." later"#),
            ["Atlas, Inc."]
        );
        // A quote the user is still typing is not a mention yet.
        assert!(parse_mentions(r#"see @"Atlas, Inc"#).is_empty());
        assert_eq!(parse_mentions("(@Ops) handled it"), ["Ops"]);
        // An address is not a mention.
        assert!(parse_mentions("mail bob@example.com").is_empty());
        // Deduplicated case-insensitively, first spelling kept.
        assert_eq!(parse_mentions("@ops and @OPS"), ["ops"]);
    }

    #[test]
    fn a_mention_inside_code_pins_nothing() {
        // Someone pasting a diff or a log is quoting, not addressing. Widening their retrieval
        // scope off a line of pasted code would be invisible and impossible to explain.
        let fenced = "before @Real after\n```\n@Fake decorator\n```\n@Second";
        assert_eq!(parse_mentions(fenced), ["Real", "Second"]);
        assert_eq!(parse_mentions("use `@Fake` here but @Real too"), ["Real"]);
    }

    #[test]
    fn a_name_shared_by_a_project_and_a_label_resolves_to_the_project_only() {
        // The failure this prevents: pinning a free-form label and silently receiving an entire
        // same-named PROJECT's documents, with nothing anywhere saying so.
        let (_dir, conn) = store();
        let project = intern(&conn, KIND_PROJECT, "Research").unwrap();
        let group = intern(&conn, KIND_GROUP, "research").unwrap();
        assert_ne!(project, group, "the kinds are separate namespaces");

        let pinned = resolve_mentions(&conn, &["research".to_string()]).unwrap();
        assert_eq!(pinned.len(), 1, "one mention resolves to exactly one row");
        assert_eq!(pinned[0].id, project);
        assert_eq!(pinned[0].kind, KIND_PROJECT);
        assert_eq!(
            pinned[0].name, "Research",
            "the canonical spelling comes back"
        );
    }

    #[test]
    fn an_unknown_mention_resolves_to_nothing() {
        let (_dir, conn) = store();
        intern(&conn, KIND_PROJECT, "Sales").unwrap();
        assert!(resolve_mentions(&conn, &["markting".to_string()])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn stripping_removes_the_pin_but_not_the_question() {
        let (_dir, conn) = store();
        intern(&conn, KIND_PROJECT, "Marketing").unwrap();
        let pinned = resolve_mentions(&conn, &parse_mentions("@Marketing what shipped?")).unwrap();

        // The pin has already chosen the corpus; leaving the word in would ALSO embed it and OR it
        // into the FTS match, quietly making a scope into a relevance boost.
        assert_eq!(
            strip_mentions("@Marketing what shipped?", &pinned),
            " what shipped?"
        );
        // An unresolved `@word` is ordinary prose and must survive — removing it would change the
        // question being asked.
        assert_eq!(
            strip_mentions("@nobody what shipped?", &pinned),
            "@nobody what shipped?"
        );
        // Stripping everything leaves nothing to search on, so the original stands.
        assert_eq!(strip_mentions("@Marketing", &pinned), "@Marketing");
    }

    #[test]
    fn a_pinned_group_tag_reaches_only_its_own_documents() {
        let (_dir, conn) = store();
        doc(&conn, 1, "Sales");
        doc(&conn, 2, "Research");
        // Doc 1 carries the LABEL "research"; doc 2 is homed in the PROJECT "Research".
        set_document_group_tags(&conn, 1, &["research".into()]).unwrap();
        set_document_projects(&conn, 2, "Research", &[]).unwrap();
        for (doc_id, chunk_id) in [(1i64, 11i64), (2, 22)] {
            conn.execute(
                "INSERT INTO chunks(id, document_id, ordinal, content, char_count)                  VALUES (?1, ?2, 0, 'x', 1)",
                params![chunk_id, doc_id],
            )
            .unwrap();
        }

        // Only the label exists under that exact name for a group pin...
        let group = vec![PinnedTag {
            id: lookup(&conn, KIND_GROUP, "research").unwrap().unwrap(),
            name: "research".into(),
            kind: KIND_GROUP.into(),
        }];
        assert_eq!(
            tag_chunk_ids(&conn, &group).unwrap(),
            std::collections::HashSet::from([11])
        );

        // ...and the project pin reaches the project's documents, not the label's.
        let project = vec![PinnedTag {
            id: lookup(&conn, KIND_PROJECT, "research").unwrap().unwrap(),
            name: "Research".into(),
            kind: KIND_PROJECT.into(),
        }];
        assert_eq!(
            tag_chunk_ids(&conn, &project).unwrap(),
            std::collections::HashSet::from([22])
        );
    }

    #[test]
    fn nothing_pinned_widens_nothing() {
        // A widening that degraded to "no filter" would break the guarantee a project chat rests
        // on, so the empty case must be an empty SET, never an absent one.
        let (_dir, conn) = store();
        doc(&conn, 1, "Sales");
        assert!(tag_chunk_ids(&conn, &[]).unwrap().is_empty());
    }

    #[test]
    fn a_group_tag_stops_being_offered_once_nothing_carries_it() {
        let (_dir, conn) = store();
        doc(&conn, 1, "Sales");
        set_document_group_tags(&conn, 1, &["draft".into()]).unwrap();
        assert!(list_all(&conn).unwrap().iter().any(|t| t.name == "draft"));
        set_document_group_tags(&conn, 1, &[]).unwrap();
        assert!(!list_all(&conn).unwrap().iter().any(|t| t.name == "draft"));
    }

    #[test]
    fn the_bundled_sqlite_provides_json1() {
        let (_dir, conn) = store();
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM json_each('[\"a\", \"b\", \"c\"]')",
                [],
                |r| r.get(0),
            )
            .expect("json_each must exist — the group-tag backfill is written in terms of it");
        assert_eq!(n, 3);
    }

    #[test]
    fn interning_is_case_insensitive_but_keeps_the_first_spelling() {
        let (_dir, conn) = store();
        let a = intern(&conn, KIND_PROJECT, "Atlas, Inc.").unwrap();
        let b = intern(&conn, KIND_PROJECT, "atlas, inc.").unwrap();
        assert_eq!(a, b, "the same project however it is typed");
        let name: String = conn
            .query_row("SELECT name FROM tags WHERE id = ?1", params![a], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            name, "Atlas, Inc.",
            "a later, differently-cased mention must not rename the project"
        );
    }

    #[test]
    fn memberships_replace_wholesale_and_exclude_the_home_from_linked() {
        let (_dir, conn) = store();
        doc(&conn, 1, "Sales");
        set_document_projects(&conn, 1, "Sales", &["Marketing".into()]).unwrap();
        assert_eq!(linked_projects(&conn, 1, "Sales").unwrap(), ["Marketing"]);

        // Re-filing to a project it was merely linked to flips which one is the home. The column
        // moves with it, exactly as `write_document_truth` moves both together — the home is read
        // from `documents.project`, so a test that changed only the join would be testing a state
        // production never produces.
        rehome(&conn, 1, "Marketing");
        set_document_projects(&conn, 1, "Marketing", &["Sales".into()]).unwrap();
        assert_eq!(linked_projects(&conn, 1, "Marketing").unwrap(), ["Sales"]);

        // And dropping the extra leaves only the home.
        set_document_projects(&conn, 1, "Marketing", &[]).unwrap();
        assert!(linked_projects(&conn, 1, "Marketing").unwrap().is_empty());
    }

    /// Move a document's HOME the way production does — the column and the join together.
    fn rehome(conn: &Connection, id: i64, project: &str) {
        conn.execute(
            "UPDATE documents SET project = ?2 WHERE id = ?1",
            params![id, project],
        )
        .unwrap();
    }

    #[test]
    fn a_hand_edited_file_repeating_the_home_does_not_link_a_document_to_itself() {
        let (_dir, conn) = store();
        doc(&conn, 1, "Sales");
        // Someone edits the vault file and writes the home into `linked_projects:` too.
        set_document_projects(&conn, 1, "Sales", &["sales".into(), "Ops".into()]).unwrap();
        assert_eq!(linked_projects(&conn, 1, "Sales").unwrap(), ["Ops"]);
    }

    #[test]
    fn renaming_a_project_tag_moves_the_memberships() {
        let (_dir, conn) = store();
        doc(&conn, 1, "Old");
        set_document_projects(&conn, 1, "Old", &[]).unwrap();
        rename_project_tag(&conn, "Old", "New").unwrap();
        // The caller re-homes the documents right after (`rewrite_documents`), which is what makes
        // the rename visible to the home-derived half of the membership set.
        rehome(&conn, 1, "New");
        assert_eq!(documents_tagged(&conn, "New").unwrap(), [1]);
        assert!(documents_tagged(&conn, "Old").unwrap().is_empty());
    }

    #[test]
    fn merging_into_a_project_a_document_is_already_in_does_not_collide() {
        let (_dir, conn) = store();
        doc(&conn, 1, "Keep");
        // Homed in Keep, also linked to Fold — the exact overlap a naive re-key would fail on.
        set_document_projects(&conn, 1, "Keep", &["Fold".into()]).unwrap();
        rename_project_tag(&conn, "Fold", "Keep").unwrap();
        assert_eq!(documents_tagged(&conn, "Keep").unwrap(), [1]);
        assert!(linked_projects(&conn, 1, "Keep").unwrap().is_empty());
    }

    /// The delete/rename paths hand `write_document_truth` the COMPLETE tag list a document should
    /// end up with, so they need the whole list — a partial one would drop every label it did not
    /// mention.
    #[test]
    fn documents_with_a_group_tag_come_back_with_their_whole_tag_list() {
        let (_dir, conn) = store();
        doc(&conn, 1, "Home");
        doc(&conn, 2, "Home");
        conn.execute(
            r#"UPDATE documents SET tags = '["tax","invoice"]' WHERE id = 1"#,
            [],
        )
        .unwrap();
        set_document_group_tags(&conn, 1, &["tax".into(), "invoice".into()]).unwrap();
        set_document_group_tags(&conn, 2, &["invoice".into()]).unwrap();

        let got = documents_with_group_tag(&conn, "tax").unwrap();
        assert_eq!(
            got,
            vec![(1, vec!["tax".to_string(), "invoice".to_string()])]
        );
        assert!(
            documents_with_group_tag(&conn, "nothing")
                .unwrap()
                .is_empty(),
            "a tag nobody carries matches nothing"
        );
    }

    /// A label that loses its last document must stop being offered, or it lingers in the `@` menu
    /// and in search matching nothing. `set_document_group_tags` already does this on every write —
    /// asserted here because `delete_tag` RELIES on it rather than pruning per document.
    #[test]
    fn a_label_that_loses_its_last_document_is_dropped_by_the_write_itself() {
        let (_dir, conn) = store();
        doc(&conn, 1, "Home");
        set_document_group_tags(&conn, 1, &["tax".into()]).unwrap();
        set_document_group_tags(&conn, 1, &[]).unwrap();

        let names: Vec<String> = list_all(&conn)
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert!(!names.contains(&"tax".to_string()));
        // ...so an explicit prune afterwards has nothing left to do.
        assert_eq!(prune_orphan_group_tags(&conn).unwrap(), 0);
    }

    /// The case the write path cannot reach: a label already carried by nothing, where there is no
    /// document to rewrite and so no write to piggyback the prune on. Deleting such a tag from the
    /// Teach list has to work anyway.
    #[test]
    fn pruning_clears_a_label_no_write_can_reach_but_spares_an_empty_project() {
        let (_dir, conn) = store();
        intern(&conn, KIND_GROUP, "orphan").unwrap();
        // An empty project exists legitimately — a triage row before anything is filed into it.
        intern(&conn, KIND_PROJECT, "Empty Project").unwrap();

        assert_eq!(prune_orphan_group_tags(&conn).unwrap(), 1);

        let names: Vec<String> = list_all(&conn)
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert!(
            !names.contains(&"orphan".to_string()),
            "orphan label pruned"
        );
        assert!(
            names.contains(&"Empty Project".to_string()),
            "an empty PROJECT must survive — project_names depends on it"
        );
    }

    #[test]
    fn deleting_a_project_tag_drops_its_memberships_but_leaves_the_others() {
        let (_dir, conn) = store();
        doc(&conn, 1, "Home");
        set_document_projects(&conn, 1, "Home", &["Doomed".into(), "Other".into()]).unwrap();
        delete_project_tag(&conn, "Doomed").unwrap();
        assert_eq!(linked_projects(&conn, 1, "Home").unwrap(), ["Other"]);
    }

    #[test]
    fn deleting_a_document_takes_its_memberships_with_it() {
        let (_dir, conn) = store();
        doc(&conn, 1, "Home");
        set_document_projects(&conn, 1, "Home", &["Other".into()]).unwrap();
        conn.execute("DELETE FROM documents WHERE id = 1", [])
            .unwrap();
        let left: i64 = conn
            .query_row("SELECT count(*) FROM document_tags", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0, "the join cascades from documents");
    }
}
