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
use std::sync::atomic::Ordering;
use std::time::Duration;

use notify::event::{EventKind, ModifyKind, RenameMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::error::{Error, Result};
use crate::{connector_sync, db, index_only, ingest, AppState};

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

/// A path's `Normal` components, lower-cased on the case-insensitive desktops (Windows/macOS). Used to
/// compare a walked path against a stored exclude entry independently of separator style (`/` vs `\`)
/// and on-disk letter case — the walk yields real OS-case, OS-separator paths while excludes are stored
/// root-relative with `/` from the picker.
fn normalized_components(p: &Path) -> Vec<String> {
    p.components()
        .filter_map(|c| match c {
            std::path::Component::Normal(os) => {
                let s = os.to_string_lossy();
                Some(if cfg!(any(windows, target_os = "macos")) {
                    s.to_lowercase()
                } else {
                    s.into_owned()
                })
            }
            _ => None,
        })
        .collect()
}

/// Whether every component of a root-relative path is an ordinary name — no `..`, no absolute/root or
/// drive prefix. Guards the picker/exclude boundary so a crafted or malformed path can't escape the
/// tracked root (`..`) or be silently collapsed by [`normalized_components`] (which drops `..`) into a
/// prefix that prunes the wrong folder.
fn is_safe_rel(rel: &Path) -> bool {
    rel.components()
        .all(|c| matches!(c, std::path::Component::Normal(_)))
}

/// Whether a root-relative path is at or below any excluded subfolder — i.e. one of `exclude` is a
/// leading component-prefix of `rel`. Excludes hold root-relative paths (as the picker emits them);
/// matching is component-wise via [`normalized_components`] so separators and case never trip it.
/// Malformed exclude entries (containing `..`, absolute) are ignored rather than collapsed. An empty
/// exclude list (the common case) short-circuits.
fn is_excluded(rel: &Path, exclude: &[String]) -> bool {
    if exclude.is_empty() {
        return false;
    }
    let rel_comps = normalized_components(rel);
    exclude.iter().any(|ex| {
        let ex_path = Path::new(ex);
        if !is_safe_rel(ex_path) {
            return false;
        }
        let ex_comps = normalized_components(ex_path);
        !ex_comps.is_empty()
            && rel_comps.len() >= ex_comps.len()
            && rel_comps[..ex_comps.len()] == ex_comps[..]
    })
}

/// Whether a walked path is a file worth indexing: a supported extension, under the size cap, not
/// hidden. Pure over `(path, size, is_hidden)` so it's unit-testable without a filesystem.
pub fn should_index(path: &Path, size: u64, hidden: bool) -> bool {
    !hidden && size <= MAX_FILE_BYTES && ingest::is_supported_source(path)
}

/// Recursively collect the indexable files under `root`, keyed by OS file id. Skips ignored/hidden
/// directories, any `exclude`d subfolder (and its whole subtree), unsupported extensions, over-cap
/// files, and (below the top level) directory symlinks — the same cycle/scope guards the drag-drop
/// walk uses. `root` itself must be a directory. `exclude` holds root-relative subfolder paths.
/// Returns the collected files and whether the walk was TRUNCATED at `MAX_COLLECTED_FILES` — an
/// incomplete enumeration, on which the caller must NOT infer deletions from a file's absence.
pub fn walk(root: &Path, exclude: &[String]) -> (Vec<LocalFile>, bool) {
    let key = folder_key(root);
    let mut out = Vec::new();
    let mut truncated = false;
    walk_into(root, root, &key, exclude, &mut out, 0, &mut truncated);
    (out, truncated)
}

fn walk_into(
    root: &Path,
    path: &Path,
    key: &str,
    exclude: &[String],
    out: &mut Vec<LocalFile>,
    depth: usize,
    truncated: &mut bool,
) {
    if out.len() >= MAX_COLLECTED_FILES {
        *truncated = true;
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
        // Prune an excluded subfolder (and its whole subtree) — the user chose to skip it. Checked at the
        // directory so nothing beneath is ever read; the watcher applies the same gate (see
        // `upsert_local_path`) so a create inside it can't sneak back in.
        if depth > 0 {
            if let Ok(rel) = path.strip_prefix(root) {
                if is_excluded(rel, exclude) {
                    return;
                }
            }
        }
        if depth >= MAX_WALK_DEPTH {
            return;
        }
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                walk_into(root, &entry.path(), key, exclude, out, depth + 1, truncated);
                if out.len() >= MAX_COLLECTED_FILES {
                    *truncated = true;
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
    // L-2: skip a symlinked file whose target resolves outside the tracked folder root.
    if ingest::symlink_escapes_root(path, root) {
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

/// One tracked folder as the live watcher resolves an event path against it: its key, canonical root,
/// and the subfolders excluded from indexing. Carries `exclude` so the watcher can apply the SAME
/// directory pruning the periodic walk does — otherwise a create inside an excluded folder would index
/// and the next walk (which skips it) would soft-delete it, the `ok`↔`source_missing` thrash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalRoot {
    pub key: String,
    pub root: PathBuf,
    pub exclude: Vec<String>,
}

/// Which tracked folder a filesystem path belongs to: the watched root that is a prefix of `path`,
/// longest match winning so a nested tracked folder claims its own files. Pure; returns the matching
/// [`LocalRoot`]. A path under no watched root (a stray event) yields `None`.
pub fn folder_of(path: &Path, roots: &[LocalRoot]) -> Option<LocalRoot> {
    roots
        .iter()
        .filter(|r| path.starts_with(&r.root))
        .max_by_key(|r| r.root.components().count())
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
    /// Root-relative subfolders excluded from indexing (empty = index the whole folder).
    pub exclude: Vec<String>,
}

/// What a tracked local folder indexes, persisted as JSON in `connector_sources.folder_ids`: the
/// canonical root plus the subfolders to skip. Older rows stored the bare root path there (no exclude
/// concept); [`parse_scope`] reads both shapes, so this is additive with no migration.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalScope {
    /// The canonical absolute root path.
    pub root: String,
    /// Root-relative subfolders excluded from indexing (each skips that folder and its subtree).
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// Read a folder's stored scope from the `folder_ids` cell, tolerating the two shapes: the current
/// JSON object, or a legacy bare root path (which reads as that root with no excludes). An absolute
/// path never starts with `{`, so the leading brace cleanly distinguishes them.
fn parse_scope(raw: &str) -> LocalScope {
    let t = raw.trim();
    if t.starts_with('{') {
        if let Ok(s) = serde_json::from_str::<LocalScope>(t) {
            return s;
        }
    }
    LocalScope {
        root: raw.to_string(),
        exclude: Vec::new(),
    }
}

/// Register a folder to track (or reactivate one already tracked), returning its stable key. The
/// scope (root path + any excludes) is stored as JSON in `folder_ids`. Re-adding the same folder
/// reuses the row and clears any prior failure state, **preserving its existing scope** (the canonical
/// root can't change for a given key, and its excludes should survive a reconnect). No documents are
/// touched here — indexing happens on the following sync.
pub fn add_folder(conn: &Connection, root: &Path) -> Result<String> {
    let key = folder_key(root);
    let id = folder_source_id(&key);
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let path = canonical.to_string_lossy().to_string();
    let label = canonical
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.clone());
    let scope_json = serde_json::to_string(&LocalScope {
        root: path,
        exclude: Vec::new(),
    })
    .map_err(|e| Error::Other(format!("encode local scope: {e}")))?;
    conn.execute(
        "INSERT INTO connector_sources (id, provider, service, label, mode, folder_ids, state) \
         VALUES (?1, ?2, ?3, ?4, 'index_only', ?5, 'ok') \
         ON CONFLICT(id) DO UPDATE SET label = excluded.label, state = 'ok'",
        params![id, PROVIDER, SERVICE, label, scope_json],
    )?;
    Ok(key)
}

/// A folder's stored scope (root + excludes), or `None` if the key isn't registered.
pub fn get_scope(conn: &Connection, key: &str) -> Result<Option<LocalScope>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT folder_ids FROM connector_sources WHERE id = ?1",
            params![folder_source_id(key)],
            |r| r.get(0),
        )
        .optional()
        .map(Option::flatten)?;
    Ok(raw.map(|s| parse_scope(&s)))
}

/// Persist a folder's excluded subfolders, keeping its stored root. The UI follows this with a sync to
/// apply the change (soft-remove now-excluded files, re-index any un-excluded ones). A no-op if the key
/// isn't registered.
pub fn set_excludes(conn: &Connection, key: &str, exclude: &[String]) -> Result<()> {
    let Some(mut scope) = get_scope(conn, key)? else {
        return Ok(());
    };
    scope.exclude = exclude.to_vec();
    let json = serde_json::to_string(&scope)
        .map_err(|e| Error::Other(format!("encode local scope: {e}")))?;
    conn.execute(
        "UPDATE connector_sources SET folder_ids = ?2 WHERE id = ?1",
        params![folder_source_id(key), json],
    )?;
    Ok(())
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
    for (id, label, raw, state, last_synced_at, indexed) in rows {
        let scope = parse_scope(raw.as_deref().unwrap_or_default());
        let present = Path::new(&scope.root).is_dir();
        let key = id.strip_prefix("local:").unwrap_or(&id).to_string();
        out.push(LocalFolder {
            key,
            path: scope.root,
            label,
            state,
            last_synced_at,
            indexed,
            present,
            exclude: scope.exclude,
        });
    }
    Ok(out)
}

/// The absolute root path of a tracked folder, or `None` if the key isn't registered.
pub fn folder_root(conn: &Connection, key: &str) -> Result<Option<PathBuf>> {
    Ok(get_scope(conn, key)?.map(|s| PathBuf::from(s.root)))
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

/// Finalize a folder pass honoring failures: a clean pass records the time + state 'ok' (via
/// [`finalize_sync`]); a pass with ANY failed item is stamped 'error' so the failure isn't hidden
/// behind a misleading 'ok' — the item retries next sync (mirrors the cloud connectors, F-29).
pub fn finalize_or_flag(conn: &Connection, key: &str, failed: bool) -> Result<()> {
    if !failed {
        return finalize_sync(conn, key);
    }
    conn.execute(
        "UPDATE connector_sources SET state = 'error' WHERE id = ?1",
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
/// per-event lookup (the on-demand walk loads the whole set up front instead). `None` = never
/// indexed. The shared foundation lookup, kept to live index-only rows (like OneDrive; only Drive
/// widens to promoted rows).
pub fn known_item(conn: &Connection, source_id: &str) -> Result<Option<KnownItem>> {
    Ok(
        index_only::read_raw_item_state(conn, source_id, /* include_promoted */ false)?.map(
            |raw| KnownItem {
                external_ref: raw.external_ref,
                modified_at: raw.source_modified_at,
                content_hash: raw.source_content_hash,
                source_state: raw.source_state,
            },
        ),
    )
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
        .map(|(id, raw)| {
            let key = id.strip_prefix("local:").unwrap_or(&id).to_string();
            let root = PathBuf::from(parse_scope(raw.as_deref().unwrap_or_default()).root);
            let present = root.is_dir();
            WatchTarget { key, root, present }
        })
        .collect())
}

/// Every tracked folder as a [`LocalRoot`] (key, canonical root, exclude list) — the live watcher's
/// per-event resolution set. Mirrors [`watch_targets`] but carries excludes, so a create/modify event
/// can be pruned exactly as the periodic walk prunes it.
pub fn tracked_roots(conn: &Connection) -> Result<Vec<LocalRoot>> {
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
        .map(|(id, raw)| {
            let key = id.strip_prefix("local:").unwrap_or(&id).to_string();
            let scope = parse_scope(raw.as_deref().unwrap_or_default());
            LocalRoot {
                key,
                root: PathBuf::from(scope.root),
                exclude: scope.exclude,
            }
        })
        .collect())
}

/// One immediate subfolder for the local folder picker: its root-relative path (what an exclude
/// stores) and its own name (what the tree shows).
#[derive(Clone, Debug, Serialize)]
pub struct LocalSubfolder {
    pub rel: String,
    pub name: String,
}

/// The immediate child subfolders of `rel` (root-relative, `/`-joined, empty = the root) inside the
/// tracked folder `root` — one lazy level of the local folder picker. Skips the same ignored dirs the
/// walk does (VCS/build/cache, dotfiles) and directory symlinks, so the tree shows only what could
/// actually be indexed. Each entry's `rel` is what an exclude stores.
pub fn list_subfolders(root: &Path, rel: &str) -> Result<Vec<LocalSubfolder>> {
    // Never let a `..`/absolute `rel` escape the tracked root — `PathBuf::join` doesn't normalise it,
    // so the OS would resolve it at read time and enumerate directories outside the folder.
    if !rel.is_empty() && !is_safe_rel(Path::new(rel)) {
        return Err(Error::Other("invalid subfolder path".into()));
    }
    let base = if rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    };
    let mut out: Vec<LocalSubfolder> = Vec::new();
    let entries = std::fs::read_dir(&base)
        .map_err(|e| Error::Other(format!("read {}: {e}", base.display())))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        // A directory, but not a symlinked one (the walk won't descend those below the root).
        if !meta.is_dir() || meta.file_type().is_symlink() {
            continue;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if is_ignored_dir(&name) {
            continue;
        }
        let child_rel = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        out.push(LocalSubfolder {
            rel: child_rel,
            name,
        });
    }
    out.sort_by_key(|s| s.name.to_lowercase());
    Ok(out)
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

// ===== Local-folder indexing (board card 6) =====
//
// A third index-only source (siblings: Drive, OneDrive), reading from the filesystem. This first card
// reconciles a tracked folder on demand — the live `notify` watcher is the next card. The engine
// mirrors `drive_sync_core`: detached + single-flight + crash-resumable, broadcasting progress on
// `local://sync` (the same `counted`/`item`/`finished` shape) so the UI reuses `IngestProgress`. All
// change SEMANTICS are the shared foundation's (`index_only::react`/`apply_actions`); this only
// supplies DETECTION (walk + mtime→hash) and the on-disk body fetch.

/// Settings key marking a local-folder sync started but not cleanly finished (the crash-resume marker,
/// sibling of [`crate::cloud_sync::DRIVE_SYNC_PENDING_KEY`]).
pub(crate) const LOCAL_SYNC_PENDING_KEY: &str = "local_sync_pending";

/// Apply `f` to the local-folder sync snapshot, best-effort (a poisoned lock is skipped).
fn with_local_snap(app: &AppHandle, f: impl FnOnce(&mut crate::LocalFolderSyncState)) {
    let state = app.state::<AppState>();
    // Bind the guard to a named local before `if let` so the lock `Result` temporary (which borrows
    // `state`) drops before `state` does — the E0597 pitfall the Drive snapshot helper documents.
    let guard = state.local_sync.lock();
    if let Ok(mut snap) = guard {
        f(&mut snap);
    }
}

/// Update the snapshot + broadcast a `local://sync` progress event globally (the local mirror of the
/// cloud engines' `emit_drive_progress` / `emit_onedrive_progress`).
fn emit_local_progress(app: &AppHandle, ev: LocalSyncEvent) {
    with_local_snap(app, |snap| match &ev {
        LocalSyncEvent::Counted { total } => {
            snap.total = Some(*total);
            snap.processed = 0;
        }
        LocalSyncEvent::Item {
            processed, total, ..
        } => {
            snap.processed = *processed;
            snap.total = Some(*total);
        }
        LocalSyncEvent::Finished { report } => {
            snap.last_report = Some(report.clone());
        }
    });
    let _ = app.emit("local://sync", ev);
}

/// True if the running local-folder sync has been asked to stop.
fn local_sync_cancelled(app: &AppHandle) -> bool {
    app.state::<AppState>()
        .local_sync_cancel
        .load(Ordering::SeqCst)
}

/// Record a local file that couldn't be indexed, up to the shared report cap.
fn record_local_issue(
    issues: &mut Vec<LocalSyncIssue>,
    truncated: &mut bool,
    name: &str,
    reason: &str,
) {
    if issues.len() < connector_sync::MAX_REPORT_ISSUES {
        issues.push(LocalSyncIssue {
            name: name.to_string(),
            reason: reason.to_string(),
        });
    } else {
        *truncated = true;
    }
}

/// The sync engine behind [`crate::commands::sync_local_folder`] and
/// [`crate::commands::resume_local_folder_sync`]. Single-flight (a
/// request arriving mid-sync folds into one follow-up all-folders pass), detached (progress via the
/// global `local://sync` event + [`crate::LocalFolderSyncState`]), and crash-resumable (a marker
/// persisted while running, cleared on the clean exit). Returns the number of items touched.
pub(crate) async fn local_sync_core(app: &AppHandle, folder: Option<String>) -> Result<usize> {
    let st: &AppState = app.state::<AppState>().inner();
    connector_sync::run_detached_sync(
        st,
        &st.local_sync,
        &st.local_sync_cancel,
        LOCAL_SYNC_PENDING_KEY,
        folder,
        |target| run_local_sync(app, target),
    )
    .await
}

/// One folder's gathered work: the walk result + the known-item set, or a `missing` root to fan out.
struct FolderWork {
    key: String,
    root: std::path::PathBuf,
    files: Vec<LocalFile>,
    known: std::collections::HashMap<String, KnownItem>,
    missing: bool,
    /// The walk hit `MAX_COLLECTED_FILES` — an incomplete enumeration; don't infer deletions from it.
    truncated: bool,
}

/// One sync pass: walk each folder off the lock, then reconcile it against the known set. Split out so
/// [`local_sync_core`] can run it again (the follow-up sweep) and own the running/marker lifecycle.
async fn run_local_sync(app: &AppHandle, folder: Option<String>) -> Result<usize> {
    // The sidecar converts each file's body to Markdown — ensure it once up front. It's blocking
    // (a first run installs the Python venv + deps), so run it on the blocking pool rather than
    // the async runtime so a first-sync-after-install can't pin a tokio worker (F-41).
    {
        let app = app.clone();
        tokio::task::spawn_blocking(move || app.state::<AppState>().sidecar.ensure_installed())
            .await
            .map_err(|e| Error::Other(format!("sidecar install task panicked: {e}")))??;
    }

    let keys: Vec<String> = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        match folder {
            Some(k) => vec![k],
            None => folder_keys(&conn)?,
        }
    };

    // Phase 1 — gather. Read each folder's root + known set (short locks), then walk off the lock (the
    // walk stats + opens files for their OS id, so it must not hold the DB).
    let mut work: Vec<FolderWork> = Vec::new();
    for key in keys {
        let scope = {
            let state = app.state::<AppState>();
            let conn = state.conn()?;
            get_scope(&conn, &key)?
        };
        let Some(scope) = scope else {
            continue; // the registry row was removed mid-run
        };
        let root = PathBuf::from(&scope.root);
        let known = {
            let state = app.state::<AppState>();
            let conn = state.conn()?;
            known_items(&conn, &key)?
        };
        if !root.is_dir() {
            work.push(FolderWork {
                key,
                root,
                files: Vec::new(),
                known,
                missing: true,
                truncated: false,
            });
            continue;
        }
        let root2 = root.clone();
        let exclude = scope.exclude.clone();
        let (files, truncated) = tokio::task::spawn_blocking(move || walk(&root2, &exclude))
            .await
            .map_err(|e| Error::Other(format!("local walk task panicked: {e}")))?;
        work.push(FolderWork {
            key,
            root,
            files,
            known,
            missing: false,
            truncated,
        });
    }

    // Total = present files + known-but-absent (deletions); a missing folder is one fan-out step.
    let total: usize = work
        .iter()
        .map(|w| {
            if w.missing {
                return 1;
            }
            let present: std::collections::HashSet<&String> =
                w.files.iter().map(|f| &f.source_id).collect();
            let deletions = w.known.keys().filter(|k| !present.contains(k)).count();
            w.files.len() + deletions
        })
        .sum();
    emit_local_progress(app, LocalSyncEvent::Counted { total });

    // Phase 2 — reconcile.
    let (mut indexed, mut updated, mut removed, mut skipped, mut failed) = (0usize, 0, 0, 0, 0);
    let mut processed = 0usize;
    let mut issues: Vec<LocalSyncIssue> = Vec::new();
    let mut issues_truncated = false;
    let mut cancelled = false;
    let mut last_err: Option<Error> = None;
    // Batch the manifest rewrite across the whole walk: each reconcile only commits DB rows, and the
    // encrypted manifest is flushed every MANIFEST_FLUSH_EVERY items + once after the loop, not per file
    // (which was O(n²) over a pass). A mid-walk bail is caught by the flusher's Drop.
    let mut manifest_flush = connector_sync::ManifestFlusher::new(app);

    'folders: for w in &work {
        if local_sync_cancelled(app) {
            cancelled = true;
            break 'folders;
        }

        // Unreadable root → fan every item of this folder out to `unreachable` (never a mass deletion).
        if w.missing {
            let actions = index_only::react(
                index_only::ChangeEvent::SourceFailure {
                    source: folder_source_id(&w.key),
                },
                None,
            );
            manifest_flush.note(apply_local_actions_off_lock(app, actions, None).await?)?;
            {
                let state = app.state::<AppState>();
                let conn = state.conn()?;
                set_state(&conn, &w.key, "unreachable")?;
            }
            processed += 1;
            emit_local_progress(
                app,
                LocalSyncEvent::Item {
                    processed,
                    total,
                    name: w.root.to_string_lossy().to_string(),
                },
            );
            continue;
        }

        // Per-folder failure gate (mirrors the cloud connectors, F-29): any failed item flags this
        // folder 'error' at finalize instead of a misleading 'ok', so the item retries next sync.
        let mut folder_failed = false;
        // A truncated walk (hit the file cap) is an INCOMPLETE enumeration: surface it and, below, skip
        // inferring deletions from absence so past-cap files aren't soft-deleted every sync.
        if w.truncated {
            record_local_issue(
                &mut issues,
                &mut issues_truncated,
                &w.root.to_string_lossy(),
                "This folder has more files than one sync can list at once; nothing was removed, and \
                 the rest is picked up on the next sync.",
            );
        }

        let present: std::collections::HashSet<String> =
            w.files.iter().map(|f| f.source_id.clone()).collect();

        // Present files: came-back → rename → mtime→hash content, each via the shared reducer path
        // (identical to what the live watcher runs per event — see [`reconcile_present_file`]).
        for f in &w.files {
            if local_sync_cancelled(app) {
                cancelled = true;
                break 'folders;
            }
            let name = f.rel_path.clone();
            let known = w.known.get(&f.source_id).cloned();
            match reconcile_present_file(app, f, known.as_ref(), &mut manifest_flush).await? {
                PresentOutcome::Indexed => indexed += 1,
                PresentOutcome::Updated => updated += 1,
                PresentOutcome::NoChange => {}
                PresentOutcome::NoText => {
                    skipped += 1;
                    record_local_issue(
                        &mut issues,
                        &mut issues_truncated,
                        &name,
                        "No extractable text (unsupported or empty file)",
                    );
                }
                PresentOutcome::Failed(reason) => {
                    failed += 1;
                    folder_failed = true;
                    record_local_issue(&mut issues, &mut issues_truncated, &name, &reason);
                    last_err = Some(Error::Other(reason));
                }
            }

            processed += 1;
            emit_local_progress(
                app,
                LocalSyncEvent::Item {
                    processed,
                    total,
                    name,
                },
            );

            // Gentle mode: breathe between files (re-read the setting each item, off any await).
            let pause_ms = {
                let state = app.state::<AppState>();
                let conn = state.conn()?;
                db::indexing_pause_ms(&conn)
            };
            if pause_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(pause_ms)).await;
            }
        }

        // Deletions: known items no longer present → soft `source_missing` (kept findable). SKIPPED when
        // the walk was truncated — an incomplete enumeration must not read a past-cap file as deleted.
        if !w.truncated {
            for (source_id, item) in &w.known {
                if present.contains(source_id) {
                    continue;
                }
                if local_sync_cancelled(app) {
                    cancelled = true;
                    break 'folders;
                }
                reconcile_deleted_item(app, source_id, item, &mut manifest_flush).await?;
                removed += 1;
                processed += 1;
                emit_local_progress(
                    app,
                    LocalSyncEvent::Item {
                        processed,
                        total,
                        name: source_id.clone(),
                    },
                );
            }
        }

        // Finalize, honoring any failure: a clean pass records the time + 'ok'; a pass with any failed
        // item is flagged 'error' so it isn't hidden behind a misleading 'ok' (F-29).
        {
            let state = app.state::<AppState>();
            let conn = state.conn()?;
            finalize_or_flag(&conn, &w.key, folder_failed)?;
        }
    }

    // Persist the tail of the batched manifest — reached on a normal finish AND a Stop that broke the
    // loop. A hard `?` bail earlier is covered by the flusher's Drop (bounded to < MANIFEST_FLUSH_EVERY).
    manifest_flush.flush()?;

    let report = LocalSyncReport {
        indexed,
        updated,
        removed,
        skipped,
        failed,
        cancelled,
        issues,
        issues_truncated,
    };
    emit_local_progress(app, LocalSyncEvent::Finished { report });

    if !cancelled {
        if let Some(e) = last_err {
            return Err(e);
        }
    }
    Ok(indexed + updated + removed)
}

/// How reconciling one present local file resolved. Kept distinct from an `Err` (a fatal DB/embed
/// failure that aborts the pass): a per-file read/convert problem is a `Failed` the caller records as
/// an issue and moves past.
enum PresentOutcome {
    /// A never-seen file was ingested.
    Indexed,
    /// An existing item's body changed and was re-embedded (classification preserved).
    Updated,
    /// A state-only change (came back / renamed / a same-hash touch) or nothing to do — no re-embed.
    NoChange,
    /// The file rendered to no extractable text — kept out of the index; caller records an issue.
    NoText,
    /// The file couldn't be read/converted — caller records the reason + counts it failed.
    Failed(String),
}

/// Reconcile ONE present local file against its persisted state: came-back → rename → mtime→hash
/// content, each via the shared [`index_only::react`] reducer. The single source of the per-file
/// semantics — the on-demand walk calls it for every file it finds, the live watcher calls it for the
/// one file an fs event touched, so both behave identically. Performs its IO (stat/hash/convert) off
/// the DB lock. `known` is the item's persisted state (`None` = never indexed). A fatal DB/embed error
/// propagates as `Err`; a per-file read/convert failure returns `Ok(PresentOutcome::Failed)`.
async fn reconcile_present_file(
    app: &AppHandle,
    file: &LocalFile,
    known: Option<&KnownItem>,
    flusher: &mut connector_sync::ManifestFlusher,
) -> Result<PresentOutcome> {
    let path_str = file.abs_path.to_string_lossy().to_string();

    // Current mtime (formatted like ingest) + this item's persisted reducer state.
    let current_iso = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        ingest::iso_from_mtime(&conn, &file.abs_path).ok()
    };
    let plan = plan_file(known, &path_str, current_iso.as_deref());
    let cur_state = known.map(|k| k.to_item_state(&file.source_id));

    // It came back (was missing/unreachable) → mark reachable before anything else.
    if plan.came_back {
        let actions = index_only::react(
            index_only::ChangeEvent::Add {
                source_id: file.source_id.clone(),
                modified_at: current_iso.clone(),
            },
            cur_state.as_ref(),
        );
        flusher.note(apply_local_actions_off_lock(app, actions, None).await?)?;
    }
    // It moved (same OS id, new path) → update the stored ref, keeping classification.
    if let Some(newref) = &plan.renamed_to {
        let actions = index_only::react(
            index_only::ChangeEvent::Rename {
                source_id: file.source_id.clone(),
                new_external_ref: Some(newref.clone()),
            },
            cur_state.as_ref(),
        );
        flusher.note(apply_local_actions_off_lock(app, actions, None).await?)?;
    }
    // Content unchanged (same mtime) and not new → the state work above (if any) is all there was.
    if !plan.content_maybe_changed {
        return Ok(PresentOutcome::NoChange);
    }

    // The mtime moved (or the file is new) → hash off the lock → Add / ReEmbed / touch.
    let path = file.abs_path.clone();
    let hashed =
        tokio::task::spawn_blocking(move || std::fs::read(&path).map(|b| ingest::hex_digest(&b)))
            .await
            .map_err(|e| Error::Other(format!("local hash task panicked: {e}")))?;
    let hash = match hashed {
        Ok(h) => h,
        Err(e) => {
            return Ok(PresentOutcome::Failed(format!(
                "Couldn't read the file: {e}"
            )))
        }
    };
    let event = if known.is_none() {
        index_only::ChangeEvent::Add {
            source_id: file.source_id.clone(),
            modified_at: current_iso.clone(),
        }
    } else {
        index_only::ChangeEvent::Update {
            source_id: file.source_id.clone(),
            modified_at: current_iso.clone(),
            new_content_hash: Some(hash.clone()),
        }
    };
    let actions = index_only::react(event, cur_state.as_ref());
    let category = connector_sync::action_category(&actions);
    let needs_body = actions.iter().any(|a| {
        matches!(
            a,
            index_only::Action::IngestNew { .. } | index_only::Action::ReEmbed { .. }
        )
    });
    if needs_body {
        match build_local_pointer(app, file, &hash, current_iso.clone()).await {
            Ok(Some(input)) => {
                flusher.note(apply_local_actions_off_lock(app, actions, Some(input)).await?)?;
                Ok(match category {
                    connector_sync::ActionKind::Indexed => PresentOutcome::Indexed,
                    connector_sync::ActionKind::Updated => PresentOutcome::Updated,
                    _ => PresentOutcome::NoChange,
                })
            }
            Ok(None) => Ok(PresentOutcome::NoText),
            Err(e) => Ok(PresentOutcome::Failed(format!(
                "Couldn't read the file: {e}"
            ))),
        }
    } else {
        // Same hash — a touch, not an edit. Advance the stored mtime so the next sync doesn't re-hash.
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        touch_modified_at(&conn, &file.source_id, current_iso.as_deref())?;
        Ok(PresentOutcome::NoChange)
    }
}

/// Reconcile ONE deleted local item → soft `source_missing` (kept findable, never a hard drop), via
/// the shared reducer. Shared by the walk (a known id the walk no longer found) and the watcher (a
/// remove event resolved to its item by external ref).
async fn reconcile_deleted_item(
    app: &AppHandle,
    source_id: &str,
    known: &KnownItem,
    flusher: &mut connector_sync::ManifestFlusher,
) -> Result<()> {
    let cur_state = known.to_item_state(source_id);
    let actions = index_only::react(
        index_only::ChangeEvent::Delete {
            source_id: source_id.to_string(),
        },
        Some(&cur_state),
    );
    flusher.note(apply_local_actions_off_lock(app, actions, None).await?)
}

/// Read + convert a local file's body via the sidecar (off the DB lock), returning the foundation's
/// [`index_only::PointerInput`] — the external ref is the absolute path, the parent folder rides along
/// as review context. `Ok(None)` when the file renders to no indexable text (kept findable by title).
async fn build_local_pointer(
    app: &AppHandle,
    file: &LocalFile,
    hash: &str,
    modified_at: Option<String>,
) -> Result<Option<index_only::PointerInput>> {
    let path = file.abs_path.clone();
    let app2 = app.clone();
    let converted = tokio::task::spawn_blocking(move || {
        let state = app2.state::<AppState>();
        state.sidecar.convert(&path)
    })
    .await
    .map_err(|e| Error::Other(format!("local convert task panicked: {e}")))?;
    let (markdown, title) = converted?;
    let markdown = markdown.trim().to_string();
    if markdown.is_empty() {
        return Ok(None);
    }
    let title = if title.trim().is_empty() {
        file.abs_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| file.rel_path.clone())
    } else {
        title
    };
    let parent = file.abs_path.parent();
    Ok(Some(index_only::PointerInput {
        source_id: file.source_id.clone(),
        title,
        external_ref: Some(file.abs_path.to_string_lossy().to_string()),
        source_modified_at: modified_at,
        source_content_hash: Some(hash.to_string()),
        body: markdown,
        source_parent_folder_id: parent.map(|p| p.to_string_lossy().to_string()),
        source_parent_folder_name: parent
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string()),
    }))
}

/// [`connector_sync::apply_connector_actions`] on a blocking thread, awaited — so the async driver
/// never runs the sidecar on the runtime. Local applies per file mid-pass, so it needs the async form
/// (the cloud engines spawn_blocking the shared apply inline). Returns whether the mirror changed, so
/// the caller can note it against its [`connector_sync::ManifestFlusher`] and defer the manifest write.
async fn apply_local_actions_off_lock(
    app: &AppHandle,
    actions: Vec<index_only::Action>,
    fetched: Option<index_only::PointerInput>,
) -> Result<bool> {
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || {
        connector_sync::apply_connector_actions(&app2, &actions, fetched)
    })
    .await
    .map_err(|e| Error::Other(format!("local apply task panicked: {e}")))?
}

// --- the live filesystem watcher (board card 6, PR2) -----------------------
//
// The on-demand sync above answers "reconcile this folder now"; the watcher answers "keep it
// reconciled". `notify-debouncer-full` coalesces the multi-write burst of a save into one settled
// event (and stitches rename From→To pairs), so a save re-embeds within seconds without a full walk.
// Every event is reduced to a per-file change (`classify_event` → `FsChange`), placed to
// its tracked folder (`folder_of`), and pushed through the SAME `reconcile_present_file` /
// `reconcile_deleted_item` the walk uses — the watcher adds detection, never new semantics.
//
// Two cooperating tasks: a MANAGER owns the debouncer and keeps its watch set in step with the
// registry (start newly-added folders, stop removed/unmounted ones, catch up after a lock→unlock),
// and a PROCESSOR drains the debounced events and applies them. No vault lock is taken — this is a
// read-only observer, like the cloud connectors.

/// Debounce window: wait this long after the last fs event on a path before emitting, so a
/// multi-write save settles into one change (and the file is stable to hash).
const LOCAL_WATCH_DEBOUNCE_SECS: u64 = 2;
/// How often the manager reconciles its watch set with the tracked-folder registry and re-checks
/// whether the vault is unlocked. Short enough that a just-added folder starts being watched promptly.
const LOCAL_WATCH_TICK_SECS: u64 = 5;

/// Start the live local-folder watcher: a debouncer feeding two background tasks for the life of the
/// app. A no-op in practice until the user tracks a folder (nothing to watch), and quiet whenever the
/// vault is locked (the manager drops its watches and the processor skips events; a lock→unlock
/// re-watches everything and runs a catch-up reconcile for anything changed while away).
pub fn spawn_local_watcher(app: AppHandle) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<DebounceEventResult>();

    // Manager: owns the debouncer, keeps the watch set matching the registry.
    let manager_app = app.clone();
    tauri::async_runtime::spawn(async move {
        let handler = move |res: DebounceEventResult| {
            // Forward to the processor; a full channel/closed receiver just means we're shutting down.
            let _ = tx.send(res);
        };
        let mut debouncer = match new_debouncer(
            Duration::from_secs(LOCAL_WATCH_DEBOUNCE_SECS),
            None,
            handler,
        ) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("local watcher: could not start the debouncer: {e}");
                return;
            }
        };
        // key -> watched root. Cleared whenever the vault locks; rebuilt on the next unlock.
        let mut watched: std::collections::HashMap<String, std::path::PathBuf> =
            std::collections::HashMap::new();
        let mut was_ready = false;
        loop {
            tokio::time::sleep(Duration::from_secs(LOCAL_WATCH_TICK_SECS)).await;
            manage_local_watches(&manager_app, &mut debouncer, &mut watched, &mut was_ready);
        }
    });

    // Processor: apply each debounced batch as it arrives (prompt, not tied to the manager's tick).
    tauri::async_runtime::spawn(async move {
        while let Some(res) = rx.recv().await {
            process_local_watch_batch(&app, res).await;
        }
    });
}

/// One manager tick: bring the live watch set in line with the tracked-folder registry. Synchronous
/// (all debouncer ops are non-async); the catch-up reconciles it triggers are fired detached so a big
/// sync never stalls the tick. Clears every watch when the vault is locked and rebuilds + catches up
/// on the next unlock.
fn manage_local_watches(
    app: &AppHandle,
    debouncer: &mut notify_debouncer_full::Debouncer<
        notify::RecommendedWatcher,
        notify_debouncer_full::RecommendedCache,
    >,
    watched: &mut std::collections::HashMap<String, std::path::PathBuf>,
    was_ready: &mut bool,
) {
    let ready = app.state::<AppState>().conn().is_ok();
    if !ready {
        // Vault locked → stop watching; a fresh unlock re-watches and re-syncs from scratch.
        if *was_ready {
            for root in watched.values() {
                let _ = debouncer.unwatch(root);
            }
            watched.clear();
            *was_ready = false;
        }
        return;
    }

    let targets = {
        let state = app.state::<AppState>();
        // Bind the lock Result to a named local before destructuring so its DbGuard temporary drops
        // before `state` does (the E0597 pitfall the Drive snapshot helper documents).
        let conn = state.conn();
        let Ok(conn) = conn else { return };
        watch_targets(&conn).unwrap_or_default()
    };
    let diff = diff_watch_set(watched, &targets);
    for t in &diff.to_watch {
        match debouncer.watch(&t.root, notify::RecursiveMode::Recursive) {
            Ok(()) => {
                watched.insert(t.key.clone(), t.root.clone());
            }
            Err(e) => eprintln!("local watcher: could not watch {}: {e}", t.root.display()),
        }
    }
    for root in &diff.to_unwatch {
        let _ = debouncer.unwatch(root);
        watched.retain(|_, r| r != root);
    }
    // A watched root vanished under us (unmount/removal) → fan its items out to `unreachable` via a
    // per-folder reconcile (the driver's missing-root branch), never a mass deletion.
    for key in &diff.gone_absent {
        let (app2, key2) = (app.clone(), key.clone());
        tauri::async_runtime::spawn(async move {
            let _ = local_sync_core(&app2, Some(key2)).await;
        });
    }
    // A fresh unlock, or any newly-watched folder → catch up on changes made while closed/locked/away.
    if !*was_ready || !diff.to_watch.is_empty() {
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            let _ = local_sync_core(&app2, None).await;
        });
    }
    *was_ready = true;
}

/// Apply one debounced batch of filesystem events. Quiet while the vault is locked or the engine
/// isn't ready (a catch-up sweep heals on the next unlock). Emits `local://changed` once if anything
/// in the batch altered the index, so a UI can refresh.
async fn process_local_watch_batch(app: &AppHandle, res: DebounceEventResult) {
    let ready = {
        let state = app.state::<AppState>();
        state.conn().is_ok() && state.sidecar.is_ready()
    };
    if !ready {
        return;
    }
    // A watcher-level error (an inotify/FSEvents queue overflow, or a path briefly inaccessible) means
    // we may have MISSED changes in this batch — and nothing else reconciles folder CONTENT on a timer
    // (the manager only catches up on a fresh unlock or a newly-watched folder). So self-heal with a
    // detached full reconcile, exactly as the manager does on unlock: idempotent (walk + diff) and
    // single-flight, so an error burst folds into one follow-up pass instead of piling up syncs. An
    // unmount still surfaces separately in the manager as an absent root → `unreachable`.
    let events = match res {
        Ok(events) => events,
        Err(_) => {
            let app2 = app.clone();
            tauri::async_runtime::spawn(async move {
                let _ = local_sync_core(&app2, None).await;
            });
            return;
        }
    };
    // Snapshot the tracked roots once for this batch, to place each event's path to its folder (and
    // carry each folder's excludes so an event inside an excluded subtree is pruned like the walk does).
    let roots: Vec<LocalRoot> = {
        let state = app.state::<AppState>();
        let conn = state.conn();
        let Ok(conn) = conn else { return };
        tracked_roots(&conn).unwrap_or_default()
    };
    if roots.is_empty() {
        return;
    }

    let mut touched = false;
    for event in events {
        for change in classify_event(&event.event.kind, &event.event.paths) {
            match handle_fs_change(app, change, &roots).await {
                Ok(true) => touched = true,
                Ok(false) => {}
                Err(e) => eprintln!("local watcher: applying a change failed: {e}"),
            }
        }
    }
    if touched {
        let _ = app.emit("local://changed", ());
    }
}

/// Route one reduced filesystem change to the shared per-file reconcile. Returns whether the index
/// changed. A rename repoints by the stable OS id: upsert `to` (keeps the item when the id survived,
/// or ingests a fresh one when the file was only path-keyed), then a remove of `from` that no-ops on
/// the survived-id case and soft-deletes the orphan otherwise.
async fn handle_fs_change(app: &AppHandle, change: FsChange, roots: &[LocalRoot]) -> Result<bool> {
    match change {
        FsChange::Upsert(path) => upsert_local_path(app, &path, roots).await,
        FsChange::Removed(path) => remove_local_path(app, &path, roots).await,
        FsChange::Renamed { from, to } => {
            let upserted = upsert_local_path(app, &to, roots).await?;
            let removed = remove_local_path(app, &from, roots).await?;
            Ok(upserted || removed)
        }
    }
}

/// Reconcile a created/modified path: place it to its folder, key it by OS file id, and run the shared
/// present-file reconcile. Ignores directories and filtered files (their child file events carry the
/// real changes). Returns whether it ingested or re-embedded.
async fn upsert_local_path(
    app: &AppHandle,
    path: &std::path::Path,
    roots: &[LocalRoot],
) -> Result<bool> {
    let Some(LocalRoot { key, root, exclude }) = folder_of(path, roots) else {
        return Ok(false);
    };
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => return Ok(false), // vanished between the event and now — a remove will follow
    };
    if !meta.is_file() {
        return Ok(false);
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if !should_index(path, meta.len(), name.starts_with('.')) {
        return Ok(false);
    }
    // Match the periodic walk's DIRECTORY guards too (not just should_index's file guards): never index
    // a file the walk would skip — one inside an ignored dir (node_modules/.git/…), an excluded subtree,
    // or below the depth cap — or the next walk (which does skip it) soft-deletes it → ok↔source_missing
    // thrash.
    if let Ok(rel) = path.strip_prefix(&root) {
        let comps: Vec<_> = rel.components().collect();
        let dir_count = comps.len().saturating_sub(1); // components minus the file name
        if comps.len() > MAX_WALK_DEPTH
            || comps.iter().take(dir_count).any(|c| {
                matches!(c, std::path::Component::Normal(os) if is_ignored_dir(&os.to_string_lossy()))
            })
            || is_excluded(rel, &exclude)
        {
            return Ok(false);
        }
    }
    // L-2: skip a symlinked file whose target resolves outside the tracked folder root.
    if ingest::symlink_escapes_root(path, &root) {
        return Ok(false);
    }
    let file_id = file_identity(path, &root);
    let source_id = source_id_for(&key, &file_id);
    let rel_path = path
        .strip_prefix(&root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string();
    let file = LocalFile {
        source_id: source_id.clone(),
        abs_path: path.to_path_buf(),
        rel_path,
        size: meta.len(),
    };
    let known = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        known_item(&conn, &source_id)?
    };
    // A single watcher event: flush its manifest change immediately (no O(n²) loop to batch here).
    let mut manifest_flush = connector_sync::ManifestFlusher::new(app);
    let outcome = reconcile_present_file(app, &file, known.as_ref(), &mut manifest_flush).await?;
    manifest_flush.flush()?;
    Ok(matches!(
        outcome,
        PresentOutcome::Indexed | PresentOutcome::Updated
    ))
}

/// Reconcile a removed path → soft-delete the item that pointed at it (by stored external ref, since a
/// gone path can't re-derive its OS id). A path we never indexed is a clean no-op.
async fn remove_local_path(
    app: &AppHandle,
    path: &std::path::Path,
    roots: &[LocalRoot],
) -> Result<bool> {
    let Some(LocalRoot { key, .. }) = folder_of(path, roots) else {
        return Ok(false);
    };
    let abs = path.to_string_lossy().to_string();
    let found = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        source_id_for_ref(&conn, &key, &abs)?
    };
    let Some((source_id, known)) = found else {
        return Ok(false);
    };
    // A single watcher event: flush its manifest change immediately (no O(n²) loop to batch here).
    let mut manifest_flush = connector_sync::ManifestFlusher::new(app);
    reconcile_deleted_item(app, &source_id, &known, &mut manifest_flush).await?;
    manifest_flush.flush()?;
    Ok(true)
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

        let mut rels: Vec<String> = walk(root, &[])
            .0
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
        assert!(walk(root, &[])
            .0
            .iter()
            .all(|f| f.source_id.starts_with(&format!("local:{key}:"))));
    }

    #[test]
    fn walk_prunes_excluded_subfolders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("keep.md"), b"x").unwrap();
        let archive = root.join("Archive");
        std::fs::create_dir(&archive).unwrap();
        std::fs::write(archive.join("old.md"), b"x").unwrap();
        let deep = archive.join("2020");
        std::fs::create_dir(&deep).unwrap();
        std::fs::write(deep.join("older.md"), b"x").unwrap();
        let work = root.join("Work");
        std::fs::create_dir(&work).unwrap();
        std::fs::write(work.join("plan.md"), b"x").unwrap();

        // Excluding "Archive" drops it and its whole subtree, and nothing else.
        let mut rels: Vec<String> = walk(root, &["Archive".to_string()])
            .0
            .into_iter()
            .map(|f| f.rel_path.replace('\\', "/"))
            .collect();
        rels.sort();
        assert_eq!(rels, vec!["Work/plan.md".to_string(), "keep.md".into()]);

        // A nested exclude prunes just that branch; the case/separator style don't matter.
        let mut rels: Vec<String> = walk(root, &["archive/2020".to_string()])
            .0
            .into_iter()
            .map(|f| f.rel_path.replace('\\', "/"))
            .collect();
        rels.sort();
        assert_eq!(
            rels,
            vec![
                "Archive/old.md".to_string(),
                "Work/plan.md".into(),
                "keep.md".into()
            ]
        );
    }

    #[test]
    fn list_subfolders_rejects_parent_dir_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("sub")).unwrap();
        // A legit relative level works.
        assert!(list_subfolders(root, "").is_ok());
        assert!(list_subfolders(root, "sub").is_ok());
        // A `..` escape is refused rather than enumerating outside the root.
        assert!(list_subfolders(root, "..").is_err());
        assert!(list_subfolders(root, "../..").is_err());
        assert!(list_subfolders(root, "sub/../..").is_err());
    }

    #[test]
    fn is_excluded_ignores_malformed_parent_dir_entries() {
        // A `..`-bearing exclude must NOT collapse to "Work" and prune a top-level Work folder.
        assert!(!is_excluded(Path::new("Work"), &["../Work".to_string()]));
        // A well-formed exclude still matches (itself and descendants), case/separator-insensitively.
        assert!(is_excluded(Path::new("Archive"), &["archive".to_string()]));
        assert!(is_excluded(
            Path::new("Archive/2020/x"),
            &["Archive/2020".to_string()]
        ));
        assert!(!is_excluded(Path::new("Work"), &["Archive".to_string()]));
    }

    #[test]
    fn parse_scope_reads_json_and_legacy_bare_path() {
        // Legacy rows stored a bare path with no exclude concept.
        let legacy = parse_scope("/home/docs");
        assert_eq!(legacy.root, "/home/docs");
        assert!(legacy.exclude.is_empty());
        // The current JSON shape round-trips root + excludes.
        let json = parse_scope(r#"{"root":"/home/docs","exclude":["Archive","Work/tmp"]}"#);
        assert_eq!(json.root, "/home/docs");
        assert_eq!(json.exclude, vec!["Archive".to_string(), "Work/tmp".into()]);
        // A JSON object without the key defaults excludes to empty.
        let no_excl = parse_scope(r#"{"root":"/x"}"#);
        assert_eq!(no_excl.root, "/x");
        assert!(no_excl.exclude.is_empty());
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
        let mk = |key: &str, root: &str| LocalRoot {
            key: key.to_string(),
            root: PathBuf::from(root),
            exclude: Vec::new(),
        };
        let roots = vec![mk("outer", "/home/docs"), mk("inner", "/home/docs/work")];
        // A file under the nested root belongs to the nested folder (longest prefix wins).
        assert_eq!(
            folder_of(Path::new("/home/docs/work/plan.md"), &roots)
                .unwrap()
                .key,
            "inner"
        );
        // A file only under the outer root belongs to it.
        assert_eq!(
            folder_of(Path::new("/home/docs/notes.md"), &roots)
                .unwrap()
                .key,
            "outer"
        );
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
