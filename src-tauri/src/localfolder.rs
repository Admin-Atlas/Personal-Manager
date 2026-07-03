// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Local-folder indexing (Stage 3, board card 6 / spec §8.1) — PM's third index-only source, a
//! sibling of the Google Drive ([`crate::drive`]) and OneDrive ([`crate::onedrive`]) connectors but
//! reading from the local filesystem rather than a cloud API. Builds on the same index-only
//! foundation ([`crate::index_only`]): this module supplies the per-folder change DETECTION (a
//! filtered directory walk + mtime→hash reconcile) and the on-disk body fetch; the foundation owns
//! the source-agnostic SEMANTICS (the [`react`](crate::index_only::react) reducer, `register_pointer`,
//! `apply_actions`, the encrypted manifest, the soft reachability states).
//!
//! **Observe, never write.** A tracked folder's real owner is the user (and whatever tools edit it);
//! PM only observes it. So — like the cloud connectors — no vault lock is taken here: a local file is
//! indexed-only (a metadata row + an embedding + a pointer: the file's OS id, path, mtime, content
//! hash), never a copy of the bytes. The body is read + converted live on demand.
//!
//! **Stable-id keying, not path.** Each file's item id is `local:<folderKey>:<fileId>` where `fileId`
//! is the file's OS-stable identity ([`file_identity`], NTFS FileId / inode). That is what lets a
//! rename or a folder reorganisation keep the item + its project/tags — a naive path key would read a
//! move as delete-plus-add and silently strip the classification (see [`crate::index_only`]). On a
//! filesystem that gives no stable id we fall back to a path digest; there a move degrades to a soft
//! delete + re-add, which the reducer keeps non-destructive.
//!
//! **This first card is reconcile-on-demand.** A "Sync now" walks the folder and diffs it against the
//! known-healthy set: a new file → `Add`, a touched file → mtime→hash → `Update`, a vanished file →
//! soft `Delete`, a moved file (same OS id, new path) → `Rename`, an unreadable root → `SourceFailure`
//! (→ `unreachable`, never a mass deletion). The live `notify` watcher is the next card and reuses
//! every decision here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::error::Result;
use crate::index_only;
use crate::ingest;

/// The `connector_sources.provider` / `.service` for a tracked local folder (the pair the v14 schema
/// reserved for exactly this — free TEXT, so no constraint change).
pub const PROVIDER: &str = "local";
pub const SERVICE: &str = "folder";

/// Directory names never descended into during a walk — version-control metadata, dependency/build
/// caches, and virtualenvs: high-volume, machine-generated, and never what a user means to index.
/// Dotfile directories are skipped generically on top of this (see [`is_ignored_dir`]).
const IGNORE_DIRS: &[&str] = &[
    ".git",
    ".svn",
    ".hg",
    "node_modules",
    "__pycache__",
    ".venv",
    "venv",
    "target",
    ".gradle",
    ".idea",
    ".vscode",
];

/// Upper bound on a single file's size we'll attempt to index. A file over this is skipped at the walk
/// (not pointed at) — indexing means reading + converting the whole body, and a giant binary is both
/// unlikely to be a document and expensive to churn. Generous for real documents.
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Bounds mirroring the drag-drop walk ([`crate::ingest`]) so a deep or symlink-looped tree can't
/// recurse without end.
const MAX_WALK_DEPTH: usize = 32;
const MAX_COLLECTED_FILES: usize = 100_000;

// --- namespacing -----------------------------------------------------------

/// A stable, short key for a tracked folder: a digest of its **canonical absolute path**. Re-adding
/// the same folder (even via a differently-spelled path) reuses the same key, so its already-indexed
/// items are recognised rather than duplicated. Canonicalisation also fixes the on-disk letter case
/// on Windows/macOS; an un-canonicalisable path (an unmounted root) still yields a stable key.
pub fn folder_key(root: &Path) -> String {
    let norm = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    ingest::hex_digest(norm.to_string_lossy().as_bytes())[..16].to_string()
}

/// The `connector_sources.id` for a tracked folder — `local:<folderKey>`. Also the prefix every one
/// of its files' item ids carry, so the foundation's `source_id LIKE 'local:<folderKey>:%'` fan-out
/// flips the whole folder to `unreachable` on a `SourceFailure` without touching other folders.
pub fn folder_source_id(folder_key: &str) -> String {
    format!("local:{folder_key}")
}

/// The stable index-only `source_id` for one file under a tracked folder: `local:<folderKey>:<fileId>`.
pub fn source_id_for(folder_key: &str, file_id: &str) -> String {
    format!("local:{folder_key}:{file_id}")
}

/// A file's OS-stable identity as a compact string — NTFS FileId on Windows, inode on Unix — so a
/// rename/move keeps the same item id. Falls back to a digest of the file's path relative to the
/// folder root when the filesystem gives no stable id (some network/removable volumes); there the item
/// is effectively path-keyed and a move reads as delete + re-add (non-destructive via the reducer's
/// soft state). The leading tag (`f`/`p`) records which so the two can never collide.
pub fn file_identity(abs: &Path, root: &Path) -> String {
    match file_id::get_file_id(abs) {
        Ok(id) => format!("f{}", encode_file_id(&id)),
        Err(_) => {
            let rel = abs.strip_prefix(root).unwrap_or(abs);
            format!(
                "p{}",
                &ingest::hex_digest(rel.to_string_lossy().as_bytes())[..24]
            )
        }
    }
}

/// Deterministic compact encoding of a [`file_id::FileId`], stable across renames on the same volume.
fn encode_file_id(id: &file_id::FileId) -> String {
    match id {
        file_id::FileId::Inode {
            device_id,
            inode_number,
        } => format!("i{device_id:x}-{inode_number:x}"),
        file_id::FileId::LowRes {
            volume_serial_number,
            file_index,
        } => format!("l{volume_serial_number:x}-{file_index:x}"),
        file_id::FileId::HighRes {
            volume_serial_number,
            file_id,
        } => format!("h{volume_serial_number:x}-{file_id:x}"),
    }
}

// --- the walk --------------------------------------------------------------

/// One file discovered by the walk: enough to key it, diff it, and (later) fetch its body. Cheap to
/// build — no bytes read here; the content hash is computed lazily only when the mtime says it might
/// have changed (see the reconcile driver).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalFile {
    /// `local:<folderKey>:<fileId>`.
    pub source_id: String,
    pub abs_path: PathBuf,
    /// The path relative to the folder root, shown as the item's external ref / review context.
    pub rel_path: String,
    pub size: u64,
}

/// Whether a directory name is one we never descend into (VCS/build/cache, or any dotfile dir).
fn is_ignored_dir(name: &str) -> bool {
    name.starts_with('.') || IGNORE_DIRS.contains(&name)
}

/// Whether a walked path is a file worth indexing: a supported extension, under the size cap, not
/// hidden. Pure over `(path, size, is_hidden)` so it's unit-testable without a filesystem.
pub fn should_index(path: &Path, size: u64, hidden: bool) -> bool {
    !hidden && size <= MAX_FILE_BYTES && ingest::is_supported_source(path)
}

/// Recursively collect the indexable files under `root`, keyed by OS file id. Skips ignored/hidden
/// directories, unsupported extensions, over-cap files, and (below the top level) directory symlinks —
/// the same cycle/scope guards the drag-drop walk uses. `root` itself must be a directory.
pub fn walk(root: &Path) -> Vec<LocalFile> {
    let key = folder_key(root);
    let mut out = Vec::new();
    walk_into(root, root, &key, &mut out, 0);
    out
}

fn walk_into(root: &Path, path: &Path, key: &str, out: &mut Vec<LocalFile>, depth: usize) {
    if out.len() >= MAX_COLLECTED_FILES {
        return;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    // Below the top level, don't follow directory symlinks (cycle / escape guard); a symlinked file
    // still indexes, and the root the user explicitly picked is always honoured.
    if depth > 0 {
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            if meta.file_type().is_symlink() && !path.is_file() {
                return;
            }
        }
    }
    if path.is_dir() {
        if depth > 0 && is_ignored_dir(&name) {
            return;
        }
        if depth >= MAX_WALK_DEPTH {
            return;
        }
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                walk_into(root, &entry.path(), key, out, depth + 1);
                if out.len() >= MAX_COLLECTED_FILES {
                    break;
                }
            }
        }
        return;
    }
    if !path.is_file() {
        return;
    }
    let hidden = name.starts_with('.');
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if !should_index(path, size, hidden) {
        return;
    }
    let file_id = file_identity(path, root);
    let rel_path = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string();
    out.push(LocalFile {
        source_id: source_id_for(key, &file_id),
        abs_path: path.to_path_buf(),
        rel_path,
        size,
    });
}

// --- the mtime→hash reconcile decision (pure) ------------------------------

/// The persisted view of one already-indexed local item the reconcile needs: where it was, when, and
/// its stored content hash (so the reducer can decide re-embed vs no-op once we hash the file).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownItem {
    /// The last path we recorded (the item's `external_ref`) — a mismatch means a rename/move.
    pub external_ref: Option<String>,
    /// The last mtime we recorded — equal means "untouched", so we can skip the (costly) hash.
    pub modified_at: Option<String>,
    /// The last content hash we recorded — the reducer compares a fresh hash against this.
    pub content_hash: Option<String>,
    /// The item's `source_state` string (`ok` / `source_missing` / `unreachable`).
    pub source_state: String,
}

impl KnownItem {
    /// The reducer's view of this item ([`index_only::ItemState`]) for a `react` call on `source_id`.
    pub fn to_item_state(&self, source_id: &str) -> index_only::ItemState {
        index_only::ItemState {
            source_id: source_id.to_string(),
            source_modified_at: self.modified_at.clone(),
            source_content_hash: self.content_hash.clone(),
            source_state: index_only::SourceState::from_db(&self.source_state),
        }
    }
}

/// What a present file needs, decided purely from its persisted state, its current path, and its
/// current mtime — no IO. The driver performs the IO (hashing, body fetch) and the reducer decides the
/// final action from the hash. This is the local mtime→hash gate: only `content_maybe_changed` files
/// get read + hashed.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FilePlan {
    /// The item was `source_missing`/`unreachable` but is present again → mark it reachable first.
    pub came_back: bool,
    /// The path moved (same OS id) → update the stored external ref, keeping classification.
    pub renamed_to: Option<String>,
    /// The mtime differs from the stored one (or the item is new) → read + hash to decide a re-embed.
    pub content_maybe_changed: bool,
}

/// Decide what a present file needs. A never-seen file (`known` is `None`) is a pure content case (it
/// will `Add`/ingest); a known file is checked for having come back, moved, and been touched.
pub fn plan_file(
    known: Option<&KnownItem>,
    current_path: &str,
    current_modified_at: Option<&str>,
) -> FilePlan {
    match known {
        None => FilePlan {
            came_back: false,
            renamed_to: None,
            content_maybe_changed: true,
        },
        Some(k) => FilePlan {
            came_back: k.source_state != ingest::SOURCE_STATE_OK,
            renamed_to: (k.external_ref.as_deref() != Some(current_path))
                .then(|| current_path.to_string()),
            content_maybe_changed: k.modified_at.as_deref() != current_modified_at,
        },
    }
}

// --- registry (the connector_sources row per tracked folder) ---------------

/// A tracked local folder as the Settings UI lists it.
#[derive(Clone, Debug, Serialize)]
pub struct LocalFolder {
    /// The stable folder key (`connector_sources.id` is `local:<key>`).
    pub key: String,
    /// The absolute path being tracked.
    pub path: String,
    /// User-facing name (the folder's own name).
    pub label: String,
    /// `'ok' | 'unreachable' | 'error'`.
    pub state: String,
    pub last_synced_at: Option<String>,
    /// How many index-only documents this folder currently has.
    pub indexed: i64,
    /// Whether the path is currently a readable directory (a removed/unmounted root reads `false`).
    pub present: bool,
}

/// Register a folder to track (or reactivate one already tracked), returning its stable key. The
/// absolute path is stored in `folder_ids` (local rows have no folder-scope concept, so the column
/// carries the root path); re-adding the same folder reuses the row and clears any prior failure
/// state. No documents are touched here — indexing happens on the following sync.
pub fn add_folder(conn: &Connection, root: &Path) -> Result<String> {
    let key = folder_key(root);
    let id = folder_source_id(&key);
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let path = canonical.to_string_lossy().to_string();
    let label = canonical
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.clone());
    conn.execute(
        "INSERT INTO connector_sources (id, provider, service, label, mode, folder_ids, state) \
         VALUES (?1, ?2, ?3, ?4, 'index_only', ?5, 'ok') \
         ON CONFLICT(id) DO UPDATE SET label = excluded.label, folder_ids = excluded.folder_ids, \
                                        state = 'ok'",
        params![id, PROVIDER, SERVICE, label, path],
    )?;
    Ok(key)
}

/// Every tracked local folder, with its live document count and whether its path is currently present.
pub fn list_folders(conn: &Connection) -> Result<Vec<LocalFolder>> {
    let mut stmt = conn.prepare(
        "SELECT id, label, folder_ids, state, last_synced_at, \
                (SELECT COUNT(*) FROM documents d \
                   WHERE d.source_type = 'index_only' AND d.source_id LIKE cs.id || ':%') \
         FROM connector_sources cs \
         WHERE provider = ?1 AND service = ?2 ORDER BY created_at",
    )?;
    let rows = stmt
        .query_map(params![PROVIDER, SERVICE], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut out = Vec::with_capacity(rows.len());
    for (id, label, path, state, last_synced_at, indexed) in rows {
        let path = path.unwrap_or_default();
        let present = Path::new(&path).is_dir();
        let key = id.strip_prefix("local:").unwrap_or(&id).to_string();
        out.push(LocalFolder {
            key,
            path,
            label,
            state,
            last_synced_at,
            indexed,
            present,
        });
    }
    Ok(out)
}

/// The absolute root path of a tracked folder, or `None` if the key isn't registered.
pub fn folder_root(conn: &Connection, key: &str) -> Result<Option<PathBuf>> {
    let path: Option<String> = conn
        .query_row(
            "SELECT folder_ids FROM connector_sources WHERE id = ?1",
            params![folder_source_id(key)],
            |r| r.get(0),
        )
        .optional()
        .map(Option::flatten)?;
    Ok(path.map(PathBuf::from))
}

/// The folder keys of every tracked local folder (for an all-folders sync).
pub fn folder_keys(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM connector_sources WHERE provider = ?1 AND service = ?2 ORDER BY created_at",
    )?;
    let keys = stmt
        .query_map(params![PROVIDER, SERVICE], |r| r.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .map(|id| id.strip_prefix("local:").unwrap_or(&id).to_string())
        .collect();
    Ok(keys)
}

/// Set a tracked folder's registry state (`ok` / `unreachable` / `error`).
pub fn set_state(conn: &Connection, key: &str, state: &str) -> Result<()> {
    conn.execute(
        "UPDATE connector_sources SET state = ?2 WHERE id = ?1",
        params![folder_source_id(key), state],
    )?;
    Ok(())
}

/// Stamp a healthy sync: record the time and clear any failure state.
pub fn finalize_sync(conn: &Connection, key: &str) -> Result<()> {
    conn.execute(
        "UPDATE connector_sources \
         SET last_synced_at = strftime('%Y-%m-%dT%H:%M:%fZ','now'), state = 'ok' WHERE id = ?1",
        params![folder_source_id(key)],
    )?;
    Ok(())
}

/// Stop tracking a folder: soft-flag its items `unreachable` (kept findable, never hard-deleted — the
/// summaries stay searchable offline) and drop the registry row. Mirrors a cloud `disconnect`.
pub fn remove_folder(conn: &Connection, key: &str) -> Result<()> {
    let id = folder_source_id(key);
    conn.execute(
        "UPDATE documents SET source_state = 'unreachable' \
         WHERE source_type = 'index_only' AND source_id LIKE ?1 || ':%'",
        params![id],
    )?;
    conn.execute("DELETE FROM connector_sources WHERE id = ?1", params![id])?;
    Ok(())
}

/// Load the persisted state of every already-indexed item under a tracked folder, keyed by source id,
/// so the reconcile can diff the walk against it (present-but-known, and known-but-absent = deleted).
pub fn known_items(conn: &Connection, key: &str) -> Result<HashMap<String, KnownItem>> {
    let id = folder_source_id(key);
    let mut stmt = conn.prepare(
        "SELECT source_id, external_ref, source_modified_at, source_content_hash, source_state \
         FROM documents WHERE source_type = 'index_only' AND source_id LIKE ?1 || ':%'",
    )?;
    let rows = stmt
        .query_map(params![id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                KnownItem {
                    external_ref: r.get::<_, Option<String>>(1)?,
                    modified_at: r.get::<_, Option<String>>(2)?,
                    content_hash: r.get::<_, Option<String>>(3)?,
                    source_state: r.get::<_, String>(4)?,
                },
            ))
        })?
        .collect::<std::result::Result<HashMap<_, _>, _>>()?;
    Ok(rows)
}

/// Record a file's new mtime without re-embedding — used when the mtime moved but the content hash
/// didn't (a touch, not an edit), so the next sync doesn't re-hash it needlessly.
pub fn touch_modified_at(
    conn: &Connection,
    source_id: &str,
    modified_at: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE documents SET source_modified_at = ?2 WHERE source_id = ?1 AND source_type = 'index_only'",
        params![source_id, modified_at],
    )?;
    Ok(())
}

// --- sync progress + report (mirrors the Drive/OneDrive shapes) ------------

/// One file the sync attempted but couldn't index (unsupported/empty, or a read error), for the report.
#[derive(Clone, Debug, Serialize)]
pub struct LocalSyncIssue {
    pub name: String,
    pub reason: String,
}

/// The outcome of a sync pass, shown in Settings and stashed in the live snapshot so a user returning
/// after it finished still sees the result.
#[derive(Clone, Debug, Serialize, Default)]
pub struct LocalSyncReport {
    pub indexed: usize,
    pub updated: usize,
    pub removed: usize,
    pub skipped: usize,
    pub failed: usize,
    /// The user pressed Stop — already-indexed files are kept; the rest were left for next time.
    pub cancelled: bool,
    pub issues: Vec<LocalSyncIssue>,
    /// True when more files couldn't be indexed than the capped `issues` list holds.
    pub issues_truncated: bool,
}

/// Progress broadcast on the global `local://sync` event, the same `counted`/`item`/`finished` shape
/// the cloud connectors use, so the UI reuses `IngestProgress` verbatim.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LocalSyncEvent {
    Counted {
        total: usize,
    },
    Item {
        processed: usize,
        total: usize,
        name: String,
    },
    Finished {
        report: LocalSyncReport,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_id_namespacing_round_trips_and_matches_the_fanout_shape() {
        let sid = source_id_for("abc123", "fh12-34");
        assert_eq!(sid, "local:abc123:fh12-34");
        // The fan-out matches `local:<key>:%`, so the folder prefix must be exactly this.
        assert!(sid.starts_with("local:abc123:"));
        assert_eq!(folder_source_id("abc123"), "local:abc123");
        // The item id is namespaced under its folder's source id, so a SourceFailure fan-out
        // (`source_id LIKE 'local:abc123:%'`) catches it.
        assert!(sid.starts_with(&format!("{}:", folder_source_id("abc123"))));
    }

    #[test]
    fn folder_key_is_stable_and_path_derived() {
        let dir = tempfile::tempdir().unwrap();
        let k1 = folder_key(dir.path());
        let k2 = folder_key(dir.path());
        assert_eq!(k1, k2, "same path → same key");
        assert_eq!(k1.len(), 16);
        let other = tempfile::tempdir().unwrap();
        assert_ne!(
            k1,
            folder_key(other.path()),
            "different paths → different keys"
        );
    }

    #[test]
    fn should_index_filters_by_type_size_and_hidden() {
        let doc = Path::new("notes.md");
        assert!(should_index(doc, 1_000, false));
        // Hidden file → skipped even if supported.
        assert!(!should_index(doc, 1_000, true));
        // Over the size cap → skipped.
        assert!(!should_index(doc, MAX_FILE_BYTES + 1, false));
        // Unsupported extension → skipped.
        assert!(!should_index(Path::new("archive.zip"), 10, false));
        assert!(!should_index(Path::new("binary"), 10, false));
        // Photos and spreadsheets are supported sources.
        assert!(should_index(Path::new("scan.png"), 10, false));
        assert!(should_index(Path::new("budget.xlsx"), 10, false));
    }

    #[test]
    fn is_ignored_dir_skips_vcs_caches_and_dotfiles() {
        assert!(is_ignored_dir(".git"));
        assert!(is_ignored_dir("node_modules"));
        assert!(is_ignored_dir(".anything")); // any dotfile dir
        assert!(!is_ignored_dir("src"));
        assert!(!is_ignored_dir("Documents"));
    }

    #[test]
    fn plan_file_new_file_is_a_content_case() {
        let plan = plan_file(None, "a/b.md", Some("2026-01-01T00:00:00.000Z"));
        assert!(plan.content_maybe_changed);
        assert!(!plan.came_back);
        assert_eq!(plan.renamed_to, None);
    }

    #[test]
    fn plan_file_untouched_known_file_is_a_noop() {
        let known = KnownItem {
            external_ref: Some("a/b.md".into()),
            modified_at: Some("2026-01-01T00:00:00.000Z".into()),
            content_hash: Some("deadbeef".into()),
            source_state: ingest::SOURCE_STATE_OK.into(),
        };
        let plan = plan_file(Some(&known), "a/b.md", Some("2026-01-01T00:00:00.000Z"));
        assert_eq!(
            plan,
            FilePlan::default(),
            "unchanged, same place, reachable → nothing"
        );
    }

    #[test]
    fn plan_file_detects_touch_move_and_return() {
        let known = KnownItem {
            external_ref: Some("old/name.md".into()),
            modified_at: Some("2026-01-01T00:00:00.000Z".into()),
            content_hash: Some("deadbeef".into()),
            source_state: ingest::SOURCE_STATE_MISSING.into(),
        };
        // Moved (new path), touched (new mtime), and previously missing → all three flags set.
        let plan = plan_file(
            Some(&known),
            "new/name.md",
            Some("2026-02-02T00:00:00.000Z"),
        );
        assert!(plan.came_back);
        assert_eq!(plan.renamed_to.as_deref(), Some("new/name.md"));
        assert!(plan.content_maybe_changed);
    }

    #[test]
    fn walk_collects_supported_files_and_skips_ignored_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("a.md"), b"hello").unwrap();
        std::fs::write(root.join("photo.png"), b"x").unwrap();
        std::fs::write(root.join("archive.zip"), b"x").unwrap(); // unsupported
        std::fs::write(root.join(".hidden.md"), b"x").unwrap(); // hidden
        let sub = root.join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("b.txt"), b"deep").unwrap();
        let git = root.join(".git");
        std::fs::create_dir(&git).unwrap();
        std::fs::write(git.join("config.txt"), b"vcs").unwrap(); // ignored dir

        let mut rels: Vec<String> = walk(root)
            .into_iter()
            .map(|f| f.rel_path.replace('\\', "/"))
            .collect();
        rels.sort();
        assert_eq!(
            rels,
            vec!["a.md".to_string(), "photo.png".into(), "sub/b.txt".into()]
        );
        // Every item is namespaced under this folder's key.
        let key = folder_key(root);
        assert!(walk(root)
            .iter()
            .all(|f| f.source_id.starts_with(&format!("local:{key}:"))));
    }
}
