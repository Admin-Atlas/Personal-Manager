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
//! **Two ways in, one reconcile.** A "Sync now" walks the folder and diffs it against the
//! known-healthy set: a new file → `Add`, a touched file → mtime→hash → `Update`, a vanished file →
//! soft `Delete`, a moved file (same OS id, new path) → `Rename`, an unreadable root → `SourceFailure`
//! (→ `unreachable`, never a mass deletion). The live `notify` watcher ([`classify_event`] →
//! [`FsChange`], routed through [`folder_of`]) reduces a debounced filesystem event to the SAME
//! per-file decision — a save re-embeds within seconds without a full walk — and the watch set is kept
//! in step with the registry by [`diff_watch_set`]. Both paths converge on the shared `react`/
//! `apply_actions` semantics; nothing here duplicates them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use notify::event::{EventKind, ModifyKind, RenameMode};
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

// --- live watcher: reduce a debounced fs event to a per-file change (pure) ----

/// A filesystem change already reduced from a debounced [`notify`] event to what the reconcile acts
/// on. `Upsert` covers both a create and a modify (the reconcile decides Add vs re-embed vs touch from
/// the file's persisted state); a `Renamed` carries both endpoints so the processor can repoint the
/// item by its stable OS id (upsert `to`, then a remove of `from` that no-ops when the id was
/// preserved, or soft-deletes the orphan when the file was only path-keyed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FsChange {
    Upsert(PathBuf),
    Removed(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
}

/// Reduce one debounced notify event (its kind + paths) to zero or more [`FsChange`]s. **PURE** over
/// notify's own types, so the create/modify/delete/rename mapping is unit-testable without touching a
/// real filesystem. Access-only events and empty-path events reduce to nothing. An ambiguous kind
/// (a bare `Any`/`Other`, or a one-sided rename we can't place) degrades to an `Upsert`, which the
/// reconcile self-heals — a vanished path there simply finds nothing to index and the next sweep
/// catches the deletion.
pub fn classify_event(kind: &EventKind, paths: &[PathBuf]) -> Vec<FsChange> {
    match kind {
        EventKind::Create(_) => paths.iter().cloned().map(FsChange::Upsert).collect(),
        // The debouncer stitches a rename into one `Both` event carrying [from, to].
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) if paths.len() >= 2 => {
            vec![FsChange::Renamed {
                from: paths[0].clone(),
                to: paths[1].clone(),
            }]
        }
        // A half-rename that couldn't be paired: the `From` side is gone, the `To` side arrived.
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
            paths.iter().cloned().map(FsChange::Removed).collect()
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
            paths.iter().cloned().map(FsChange::Upsert).collect()
        }
        // Any other modify (data, metadata, an unplaceable name change) → re-check the path.
        EventKind::Modify(_) => paths.iter().cloned().map(FsChange::Upsert).collect(),
        EventKind::Remove(_) => paths.iter().cloned().map(FsChange::Removed).collect(),
        EventKind::Access(_) => Vec::new(),
        // A bare Any/Other with a path: treat conservatively as "something here changed".
        _ => paths.iter().cloned().map(FsChange::Upsert).collect(),
    }
}

/// Which tracked folder a filesystem path belongs to: the watched root that is a prefix of `path`,
/// longest match winning so a nested tracked folder claims its own files. Pure; returns the matching
/// `(key, root)`. A path under no watched root (a stray event) yields `None`.
pub fn folder_of(path: &Path, roots: &[(String, PathBuf)]) -> Option<(String, PathBuf)> {
    roots
        .iter()
        .filter(|(_, root)| path.starts_with(root))
        .max_by_key(|(_, root)| root.components().count())
        .cloned()
}

/// One tracked folder as the watcher's set-management sees it: its key, root, and whether the root is
/// currently a readable directory (an unmounted/deleted root reads `present = false`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchTarget {
    pub key: String,
    pub root: PathBuf,
    pub present: bool,
}

/// How the live watch set should change to match the registry. Pure so the churn logic is testable
/// without a real watcher: which roots to start watching, which to stop, and which tracked folders
/// have just gone absent (so the driver can fan their items out to `unreachable`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct WatchDiff {
    pub to_watch: Vec<WatchTarget>,
    pub to_unwatch: Vec<PathBuf>,
    /// Keys still registered but whose root is no longer present, while we were watching them — an
    /// unmount/removal to reconcile to `unreachable` (never a mass deletion).
    pub gone_absent: Vec<String>,
}

/// Diff the currently-watched folders (key → root) against the registry. A present, un-watched folder
/// is started; a folder that left the registry, or whose root vanished while watched, is stopped; a
/// still-registered folder whose root vanished under us is also reported as `gone_absent` so its items
/// go unreachable. Watching a root is what later triggers its catch-up sync (see the driver), so this
/// stays purely about the watch set.
pub fn diff_watch_set(watched: &HashMap<String, PathBuf>, registry: &[WatchTarget]) -> WatchDiff {
    let registered: HashMap<&String, &WatchTarget> = registry.iter().map(|t| (&t.key, t)).collect();
    let mut diff = WatchDiff::default();
    for t in registry {
        if t.present && !watched.contains_key(&t.key) {
            diff.to_watch.push(t.clone());
        }
    }
    for (key, root) in watched {
        match registered.get(key) {
            // Left the registry entirely (removed) → stop watching it.
            None => diff.to_unwatch.push(root.clone()),
            // Still registered but its root vanished under us → stop + flag it unreachable.
            Some(t) if !t.present => {
                diff.to_unwatch.push(root.clone());
                diff.gone_absent.push(key.clone());
            }
            Some(_) => {}
        }
    }
    diff
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

/// The persisted state of a single already-indexed item, keyed by its source id — the watcher's
/// per-event lookup (the on-demand walk loads the whole set up front instead). `None` = never indexed.
pub fn known_item(conn: &Connection, source_id: &str) -> Result<Option<KnownItem>> {
    conn.query_row(
        "SELECT external_ref, source_modified_at, source_content_hash, source_state \
         FROM documents WHERE source_id = ?1 AND source_type = 'index_only'",
        params![source_id],
        |r| {
            Ok(KnownItem {
                external_ref: r.get(0)?,
                modified_at: r.get(1)?,
                content_hash: r.get(2)?,
                source_state: r.get(3)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

/// Find the indexed item a now-vanished path belonged to, by its stored `external_ref` (the absolute
/// path). A deletion event can't re-derive the OS file id from a path that no longer exists, so the
/// watcher resolves the item this way instead. Scoped to the folder's id prefix so two folders can't
/// cross-match. Returns the item's source id + persisted state, or `None` if it was never indexed.
pub fn source_id_for_ref(
    conn: &Connection,
    key: &str,
    abs_path: &str,
) -> Result<Option<(String, KnownItem)>> {
    let id = folder_source_id(key);
    conn.query_row(
        "SELECT source_id, external_ref, source_modified_at, source_content_hash, source_state \
         FROM documents \
         WHERE source_type = 'index_only' AND source_id LIKE ?1 || ':%' AND external_ref = ?2",
        params![id, abs_path],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                KnownItem {
                    external_ref: r.get(1)?,
                    modified_at: r.get(2)?,
                    content_hash: r.get(3)?,
                    source_state: r.get(4)?,
                },
            ))
        },
    )
    .optional()
    .map_err(Into::into)
}

/// Every tracked folder as a lean [`WatchTarget`] (key, root, present) for the live watcher's set
/// management — no per-folder document count, unlike [`list_folders`], since the watcher polls this.
pub fn watch_targets(conn: &Connection) -> Result<Vec<WatchTarget>> {
    let mut stmt = conn.prepare(
        "SELECT id, folder_ids FROM connector_sources \
         WHERE provider = ?1 AND service = ?2 ORDER BY created_at",
    )?;
    let rows = stmt
        .query_map(params![PROVIDER, SERVICE], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows
        .into_iter()
        .map(|(id, path)| {
            let key = id.strip_prefix("local:").unwrap_or(&id).to_string();
            let root = PathBuf::from(path.unwrap_or_default());
            let present = root.is_dir();
            WatchTarget { key, root, present }
        })
        .collect())
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

    // --- live-watcher event translation (pure) ---

    use notify::event::{CreateKind, DataChange, MetadataKind, RemoveKind};

    #[test]
    fn classify_maps_create_modify_and_delete_to_a_per_file_change() {
        let p = PathBuf::from("/root/a.md");
        // A create and any modify both re-check the path (Add vs re-embed is the reconcile's call).
        assert_eq!(
            classify_event(
                &EventKind::Create(CreateKind::File),
                std::slice::from_ref(&p)
            ),
            vec![FsChange::Upsert(p.clone())]
        );
        assert_eq!(
            classify_event(
                &EventKind::Modify(ModifyKind::Data(DataChange::Content)),
                std::slice::from_ref(&p)
            ),
            vec![FsChange::Upsert(p.clone())]
        );
        assert_eq!(
            classify_event(
                &EventKind::Modify(ModifyKind::Metadata(MetadataKind::WriteTime)),
                std::slice::from_ref(&p)
            ),
            vec![FsChange::Upsert(p.clone())]
        );
        // A delete → a removal the processor soft-resolves by external ref.
        assert_eq!(
            classify_event(
                &EventKind::Remove(RemoveKind::File),
                std::slice::from_ref(&p)
            ),
            vec![FsChange::Removed(p.clone())]
        );
        // Access events carry no change.
        assert_eq!(
            classify_event(
                &EventKind::Access(notify::event::AccessKind::Read),
                std::slice::from_ref(&p)
            ),
            Vec::<FsChange>::new()
        );
    }

    #[test]
    fn classify_stitches_and_splits_renames() {
        let from = PathBuf::from("/root/old.md");
        let to = PathBuf::from("/root/new.md");
        // The debouncer's paired rename → one Renamed carrying both endpoints, from then to.
        assert_eq!(
            classify_event(
                &EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
                &[from.clone(), to.clone()]
            ),
            vec![FsChange::Renamed {
                from: from.clone(),
                to: to.clone(),
            }]
        );
        // An unpaired half-rename: the From side reads as a removal, the To side as an upsert.
        assert_eq!(
            classify_event(
                &EventKind::Modify(ModifyKind::Name(RenameMode::From)),
                std::slice::from_ref(&from)
            ),
            vec![FsChange::Removed(from)]
        );
        assert_eq!(
            classify_event(
                &EventKind::Modify(ModifyKind::Name(RenameMode::To)),
                std::slice::from_ref(&to)
            ),
            vec![FsChange::Upsert(to)]
        );
    }

    #[test]
    fn folder_of_picks_the_innermost_tracked_root() {
        let roots = vec![
            ("outer".to_string(), PathBuf::from("/home/docs")),
            ("inner".to_string(), PathBuf::from("/home/docs/work")),
        ];
        // A file under the nested root belongs to the nested folder (longest prefix wins).
        let (k, _) = folder_of(Path::new("/home/docs/work/plan.md"), &roots).unwrap();
        assert_eq!(k, "inner");
        // A file only under the outer root belongs to it.
        let (k, _) = folder_of(Path::new("/home/docs/notes.md"), &roots).unwrap();
        assert_eq!(k, "outer");
        // A path under no tracked root → no folder.
        assert!(folder_of(Path::new("/tmp/x.md"), &roots).is_none());
    }

    #[test]
    fn diff_watch_set_starts_stops_and_flags_unmounts() {
        let mut watched = HashMap::new();
        watched.insert("keep".to_string(), PathBuf::from("/a"));
        watched.insert("removed".to_string(), PathBuf::from("/b"));
        watched.insert("unmounted".to_string(), PathBuf::from("/c"));
        let registry = vec![
            WatchTarget {
                key: "keep".into(),
                root: "/a".into(),
                present: true,
            },
            // "removed" is absent from the registry (the user dropped it).
            WatchTarget {
                key: "unmounted".into(),
                root: "/c".into(),
                present: false, // still tracked, but its drive vanished under us
            },
            WatchTarget {
                key: "fresh".into(),
                root: "/d".into(),
                present: true, // newly added, not yet watched
            },
        ];
        let diff = diff_watch_set(&watched, &registry);
        assert_eq!(
            diff.to_watch,
            vec![WatchTarget {
                key: "fresh".into(),
                root: "/d".into(),
                present: true
            }],
            "a present, un-watched folder is started"
        );
        let mut unwatch = diff.to_unwatch.clone();
        unwatch.sort();
        assert_eq!(
            unwatch,
            vec![PathBuf::from("/b"), PathBuf::from("/c")],
            "the removed folder and the vanished-root folder are both stopped"
        );
        assert_eq!(
            diff.gone_absent,
            vec!["unmounted".to_string()],
            "only the still-registered vanished root is flagged for unreachable"
        );
    }
}
