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
use std::time::{Duration, Instant};

use notify::event::{EventKind, ModifyKind, RenameMode};
use notify_debouncer_full::{new_debouncer_opt, DebounceEventResult, RecommendedCache};
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

/// A path's `Normal` components, lower-cased. Used to compare a walked path against a stored exclude
/// entry independently of separator style (`/` vs `\`) and on-disk letter case — the walk yields real
/// OS-case, OS-separator paths while excludes are stored root-relative with `/` from the picker. The
/// lower-casing is unconditional (not gated on the case-insensitive desktops) so the match honours the
/// documented "case never trips it" contract on every platform; the result is used only for this
/// comparison, never to store or display a path, so the real on-disk case is preserved everywhere.
fn normalized_components(p: &Path) -> Vec<String> {
    p.components()
        .filter_map(|c| match c {
            std::path::Component::Normal(os) => Some(os.to_string_lossy().to_lowercase()),
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

/// Whether a failed directory read leaves files PM never enumerated — in which case the walk is
/// INCOMPLETE and the caller must not infer deletions from absence (INVARIANTS I-09.3).
///
/// Fail-closed by construction: only a `NotFound` BELOW the root is a provable absence (the directory
/// was deleted between the `is_dir` check and the open — a real delete the sweep should act on).
/// Everything else counts as unseen. The test is inverted deliberately: an allow-list of "unreadable"
/// kinds would miss the ones that matter most, because Windows maps a dropped share
/// (ERROR_BAD_NETPATH, ERROR_NETNAME_DELETED) to an uncategorised kind, not `PermissionDenied`.
///
/// At the ROOT (`depth == 0`) even `NotFound` counts: a tracked folder that vanished mid-walk is
/// [`run_local_sync`]'s `missing` branch (one `SourceFailure` → state `unreachable`) and must never
/// arrive as a mass deletion of every item under it.
pub(crate) fn read_failure_hides_files(kind: std::io::ErrorKind, depth: usize) -> bool {
    depth == 0 || kind != std::io::ErrorKind::NotFound
}

/// Recursively collect the indexable files under `root`, keyed by OS file id. Skips ignored/hidden
/// directories, any `exclude`d subfolder (and its whole subtree), unsupported extensions, over-cap
/// files, and (below the top level) directory symlinks — the same cycle/scope guards the drag-drop
/// walk uses. `root` itself must be a directory. `exclude` holds root-relative subfolder paths.
/// Returns the collected files and whether the walk did NOT provably see everything: it hit the
/// `MAX_COLLECTED_FILES` cap, or a directory/entry it could not read (see
/// [`read_failure_hides_files`]). Either way the enumeration is incomplete and the caller must NOT
/// infer deletions from a file's absence. A deliberate prune (an ignored dir, an exclude, the depth
/// cap) is NOT incompleteness — the watcher applies the same gates, so nothing beneath is ever
/// indexed and there is nothing to soft-delete.
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
    // still indexes, and the root the user explicitly picked is always honoured. The stat result is
    // kept rather than discarded: a FAILED stat is the only evidence we have that a listed entry
    // exists but could not be seen (the non-dir/non-file arm below).
    let link_meta = if depth > 0 {
        let meta = std::fs::symlink_metadata(path);
        if let Ok(m) = &meta {
            if m.file_type().is_symlink() && !path.is_file() {
                return;
            }
        }
        Some(meta)
    } else {
        None
    };
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
        match std::fs::read_dir(path) {
            Ok(entries) => {
                for entry in entries {
                    // A dir entry we could not read is a file we never enumerated — fail closed, the
                    // rule the vault walk applies.
                    let Ok(entry) = entry else {
                        *truncated = true;
                        continue;
                    };
                    walk_into(root, &entry.path(), key, exclude, out, depth + 1, truncated);
                    if out.len() >= MAX_COLLECTED_FILES {
                        *truncated = true;
                        break;
                    }
                }
            }
            // A directory PM cannot OPEN is not an empty directory: everything beneath it would look
            // absent and the caller would soft-delete the subtree. The trade this accepts is that a
            // PERMANENTLY unreadable subtree (a system junction, a placeholder folder that denies
            // enumeration) permanently withholds this folder's absence-inferred deletion sweep — I-09.3
            // mandates it, and the live watcher still reconciles real single-file deletes, so the sweep
            // is a backstop rather than the only path. Don't "fix" this back.
            Err(e) => {
                if read_failure_hides_files(e.kind(), depth) {
                    *truncated = true;
                }
            }
        }
        return;
    }
    if !path.is_file() {
        // `read_dir` listed this entry, so something IS here. If we could not stat it (a dropped
        // share, a locked file) it is neither a dir nor a file to us — an item we could not SEE, so
        // the same fail-closed rule applies. A broken symlink still stats fine via `symlink_metadata`
        // and so raises no false incompleteness.
        if let Some(Err(e)) = &link_meta {
            if read_failure_hides_files(e.kind(), depth) {
                *truncated = true;
            }
        }
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
    /// Whether this item's chunks were built from its ~500-char summary (see
    /// [`index_only::ItemState::summary_indexed`]) — carried into the reducer so an unchanged local
    /// file still upgrades to the full body after a rebuild-from-manifest restore.
    pub summary_indexed: bool,
}

impl KnownItem {
    /// The reducer's view of this item ([`index_only::ItemState`]) for a `react` call on `source_id`.
    pub fn to_item_state(&self, source_id: &str) -> index_only::ItemState {
        index_only::ItemState {
            source_id: source_id.to_string(),
            source_modified_at: self.modified_at.clone(),
            source_content_hash: self.content_hash.clone(),
            source_state: index_only::SourceState::from_db(&self.source_state),
            summary_indexed: self.summary_indexed,
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

/// Whether a stored `external_ref` (an item's absolute path) lies under `dir`. Component-wise, so
/// `<root>/notes-old/x.md` is NOT under `<root>/notes` — the Rust confirmation of the SQL prefix
/// [`items_under_dir`] uses.
pub fn ref_under_dir(external_ref: &str, dir: &Path) -> bool {
    Path::new(external_ref).starts_with(dir)
}

/// Where a stored ref lands when an ancestor directory is renamed `from` → `to`. The file's OS id is
/// untouched by a directory rename, so this is a pure repoint (same item, new path), not a delete +
/// add. `None` when the ref isn't under `from` at all.
pub fn repoint_ref(external_ref: &str, from: &Path, to: &Path) -> Option<PathBuf> {
    Path::new(external_ref)
        .strip_prefix(from)
        .ok()
        .map(|rest| to.join(rest))
}

/// A directory fan-out target, which must be STRICTLY BELOW a tracked root. The root itself is never
/// fanned out: an unmounted/removed root is the manager's `gone_absent` → `unreachable` path (see
/// [`diff_watch_set`] and `run_local_sync`'s missing branch), and per-item `source_missing` there would
/// be exactly the mass deletion that branch exists to prevent.
pub fn fanout_dir<'a>(dir: &'a Path, root: &Path) -> Option<&'a Path> {
    (dir.starts_with(root) && dir != root).then_some(dir)
}

/// The walk's DIRECTORY gates for a root-relative file path: within the depth cap, no ignored
/// ancestor, not inside an excluded subtree. Extracted from [`upsert_local_path`] (which now calls it)
/// so the watcher's directory endpoints apply exactly the gates [`walk_into`] applies — that
/// equivalence is the only thing preventing an `ok`↔`source_missing` thrash between the two paths.
pub fn walk_admits_file(rel: &Path, exclude: &[String]) -> bool {
    let comps: Vec<_> = rel.components().collect();
    let dir_count = comps.len().saturating_sub(1); // components minus the file name
    comps.len() <= MAX_WALK_DEPTH
        && !comps.iter().take(dir_count).any(|c| {
            matches!(c, std::path::Component::Normal(os) if is_ignored_dir(&os.to_string_lossy()))
        })
        && !is_excluded(rel, exclude)
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
        "SELECT source_id, external_ref, source_modified_at, source_content_hash, source_state, \
                content_hash, stored_summary \
         FROM documents WHERE source_type = 'index_only' AND source_id LIKE ?1 || ':%'",
    )?;
    let rows = stmt
        .query_map(params![id], |r| {
            let sid: String = r.get(0)?;
            let content_hash: String = r.get(5)?;
            let stored_summary: Option<String> = r.get(6)?;
            let summary_indexed =
                index_only::summary_indexed_flag(&sid, &content_hash, stored_summary.as_deref());
            Ok((
                sid,
                KnownItem {
                    external_ref: r.get::<_, Option<String>>(1)?,
                    modified_at: r.get::<_, Option<String>>(2)?,
                    content_hash: r.get::<_, Option<String>>(3)?,
                    source_state: r.get::<_, String>(4)?,
                    summary_indexed,
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
                summary_indexed: raw.summary_indexed,
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
        "SELECT source_id, external_ref, source_modified_at, source_content_hash, source_state, \
                content_hash, stored_summary \
         FROM documents \
         WHERE source_type = 'index_only' AND source_id LIKE ?1 || ':%' AND external_ref = ?2",
        params![id, abs_path],
        |r| {
            let sid: String = r.get(0)?;
            let content_hash: String = r.get(5)?;
            let stored_summary: Option<String> = r.get(6)?;
            let summary_indexed =
                index_only::summary_indexed_flag(&sid, &content_hash, stored_summary.as_deref());
            Ok((
                sid,
                KnownItem {
                    external_ref: r.get(1)?,
                    modified_at: r.get(2)?,
                    content_hash: r.get(3)?,
                    source_state: r.get(4)?,
                    summary_indexed,
                },
            ))
        },
    )
    .optional()
    .map_err(Into::into)
}

/// Every indexed item whose stored path lies under `dir`, for the watcher's directory endpoints (a
/// renamed or deleted subfolder, which no single `external_ref` can match). Returns
/// `(source_id, external_ref, persisted state)` per row.
///
/// The prefix compare is `substr(external_ref, 1, n) = ?`, NOT `LIKE`: real paths contain `%` and `_`,
/// which a `LIKE` pattern would read as wildcards and match a sibling folder with them. The prefix
/// carries a trailing separator so `<root>/notes` can't claim `<root>/notesX`, and every row is
/// confirmed component-wise with [`ref_under_dir`] before it is acted on. Scoped to the folder's id
/// prefix so two tracked folders can't cross-match.
pub fn items_under_dir(
    conn: &Connection,
    key: &str,
    dir: &Path,
) -> Result<Vec<(String, String, KnownItem)>> {
    let id = folder_source_id(key);
    let prefix = format!("{}{}", dir.display(), std::path::MAIN_SEPARATOR);
    // SQLite's `substr` counts CHARACTERS on a TEXT value, so the bound length must too.
    let prefix_len = prefix.chars().count() as i64;
    let mut stmt = conn.prepare(
        "SELECT source_id, external_ref, source_modified_at, source_content_hash, source_state, \
                content_hash, stored_summary \
         FROM documents \
         WHERE source_type = 'index_only' AND source_id LIKE ?1 || ':%' \
           AND substr(external_ref, 1, ?3) = ?2",
    )?;
    let rows = stmt
        .query_map(params![id, prefix, prefix_len], |r| {
            let sid: String = r.get(0)?;
            let external_ref: String = r.get(1)?;
            let content_hash: String = r.get(5)?;
            let stored_summary: Option<String> = r.get(6)?;
            let summary_indexed =
                index_only::summary_indexed_flag(&sid, &content_hash, stored_summary.as_deref());
            Ok((
                sid,
                external_ref.clone(),
                KnownItem {
                    external_ref: Some(external_ref),
                    modified_at: r.get::<_, Option<String>>(2)?,
                    content_hash: r.get::<_, Option<String>>(3)?,
                    source_state: r.get::<_, String>(4)?,
                    summary_indexed,
                },
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows
        .into_iter()
        .filter(|(_, external_ref, _)| ref_under_dir(external_ref, dir))
        .collect())
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
    /// The total this PASS will work through, and which folder it is for (`None` = every folder).
    /// A run can span several passes — see [`crate::connector_sync::SyncQueue`] — so this is also how
    /// the UI learns a folder it is showing as "Queued" has come up.
    Counted {
        total: usize,
        target: Option<String>,
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
        LocalSyncEvent::Counted { total, .. } => {
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

/// Fold one pass's report into the run's running total, without emitting. The local mirror of
/// [`crate::cloud_sync`]'s accumulator, and for the same reason: a run is one or more passes, and
/// reporting only the last would end a run that indexed 50 files by announcing "0 indexed".
fn accumulate_local_pass(app: &AppHandle, report: LocalSyncReport) {
    with_local_snap(app, |snap| match snap.last_report.as_mut() {
        None => snap.last_report = Some(report),
        Some(run) => merge_local_pass_into_run(run, report),
    });
}

/// Add one pass's counts to the run's — the local mirror of `cloud_sync::merge_pass_into_run`, with
/// the same rules (`cancelled` is the last pass's, the issue list stays capped, truncation sticky).
fn merge_local_pass_into_run(run: &mut LocalSyncReport, pass: LocalSyncReport) {
    run.indexed += pass.indexed;
    run.updated += pass.updated;
    run.removed += pass.removed;
    run.skipped += pass.skipped;
    run.failed += pass.failed;
    run.cancelled = pass.cancelled;
    let room = connector_sync::MAX_REPORT_ISSUES.saturating_sub(run.issues.len());
    if pass.issues.len() > room {
        run.issues_truncated = true;
    }
    run.issues.extend(pass.issues.into_iter().take(room));
    run.issues_truncated |= pass.issues_truncated;
}

/// Emit the run's single terminal event from the accumulated totals. Called once by
/// [`connector_sync::run_detached_sync`] after the final pass, on every exit path.
fn emit_local_run_finished(app: &AppHandle) {
    let report = {
        // Bind the state to a named local so its temporary outlives the guard borrowed from it
        // (the E0716 trap the Drive snapshot helper documents).
        let state = app.state::<AppState>();
        let guard = state.local_sync.lock();
        guard.ok().and_then(|snap| snap.last_report.clone())
    };
    let _ = app.emit(
        "local://sync",
        LocalSyncEvent::Finished {
            report: report.unwrap_or_default(),
        },
    );
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
        // The local connector covers the same ground whichever request a pass is answering, so
        // `req.rerun` is unused here (Drive's Shared-with-me widening is the only reader).
        |req: connector_sync::PassRequest| run_local_sync(app, req.target),
        || emit_local_run_finished(app),
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
    /// The walk did not provably see everything — it hit `MAX_COLLECTED_FILES`, or a directory/entry
    /// it could not read. An incomplete enumeration; don't infer deletions from it.
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
        match &folder {
            Some(k) => vec![k.clone()],
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
    emit_local_progress(
        app,
        LocalSyncEvent::Counted {
            total,
            target: folder,
        },
    );

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
        // A truncated walk (the file cap, or a directory/entry it could not read) is an INCOMPLETE
        // enumeration: surface it and, below, skip inferring deletions from absence so unseen files
        // aren't soft-deleted every sync. The note stays cause-agnostic because the user's next step is
        // the same for all of them: wait a sync. Pushed directly, NOT through the capped
        // `record_local_issue`: there's at most one of these per folder and it's the only report-side
        // signal that a deletion sweep was withheld, so it must never be starved by a full per-file
        // issues list (the cloud engine pushes its twin the same way).
        if w.truncated {
            issues.push(LocalSyncIssue {
                name: w.root.to_string_lossy().to_string(),
                reason: "Some of this folder couldn't be listed this sync (too many files, or a \
                         subfolder PM couldn't read). Nothing was removed — PM tries again on the \
                         next sync."
                    .to_string(),
            });
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
        // the walk was truncated — an incomplete enumeration must not read a file it never saw as
        // deleted, whether it sat past the cap or inside a subfolder that wouldn't open.
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
    // End of a PASS, not the run — accumulate; `run_detached_sync` announces the run once.
    accumulate_local_pass(app, report);

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
    // Content unchanged (same mtime) and not new → normally the state work above (if any) is all
    // there was. EXCEPT an item whose stored chunks were built from its ~500-char summary (a
    // rebuild-from-manifest restore) must still be upgraded to the full body — so fall through to
    // hash + re-embed even on a stable mtime; the reducer turns the unchanged-hash Update into a
    // ReEmbed, and once it is full-body indexed the flag self-clears so later walks no-op again.
    let summary_only = known.is_some_and(|k| k.summary_indexed);
    if !plan.content_maybe_changed && !summary_only {
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
//
// Watching a folder can simply FAIL, and not transiently: on Linux a recursive watch costs one
// inotify watch per directory out of a machine-wide budget shared with every other running app.
// Both the manager (a root that won't register) and the processor (a watcher-level error) respond to
// failure by reconciling, and both are on hot paths — so both are rate-limited. Without that, a
// single unwatchable folder re-registers, fails, and re-triggers a full reconcile on every tick,
// forever. Backing off is safe because the watcher is an optimisation, not the only path: the
// connector poll reconciles every tracked folder on its own timer regardless.

/// Debounce window: wait this long after the last fs event on a path before emitting, so a
/// multi-write save settles into one change (and the file is stable to hash).
const LOCAL_WATCH_DEBOUNCE_SECS: u64 = 2;
/// How often the manager reconciles its watch set with the tracked-folder registry and re-checks
/// whether the vault is unlocked. Short enough that a just-added folder starts being watched promptly.
const LOCAL_WATCH_TICK_SECS: u64 = 5;
/// How long to leave a root alone after its watch registration failed, before trying again. Without
/// a penalty the manager re-proposes the failed root on every tick, and since a newly-watched folder
/// triggers a catch-up reconcile, one unwatchable folder becomes a permanent sync storm. The usual
/// cause on Linux is `fs.inotify.max_user_watches` — a machine-wide budget shared with every other
/// app, which PM cannot raise and which will not clear in five seconds.
const LOCAL_WATCH_RETRY_SECS: u64 = 300;
/// Minimum gap between watcher-error self-heal sweeps. A watcher-level error can recur on every
/// batch (an inotify queue that keeps overflowing), and each sweep walks every tracked folder — with
/// no floor the errors and the sweeps feed each other.
const LOCAL_HEAL_MIN_GAP_SECS: u64 = 60;

/// When the last watcher-error self-heal sweep fired. Watcher-local bookkeeping with no reader
/// outside this module, so it stays out of `AppState` (and out of the database).
static LAST_WATCH_HEAL: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);

/// Whether a backoff has expired and the guarded work may run: `None` means it has never tripped, so
/// the first attempt always passes. Pure — takes the elapsed time rather than reading a clock — so
/// both watcher backoffs are testable without a real watcher.
fn backoff_elapsed(since: Option<Duration>, min_gap_secs: u64) -> bool {
    since.is_none_or(|d| d.as_secs() >= min_gap_secs)
}

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
        // `follow_symlinks(false)`: the periodic walk refuses to descend a symlink, so a watcher that
        // followed them would register (and spend inotify watches on) trees the walk will never index
        // — including, for a link that points at or above its own root, an unbounded one.
        let mut debouncer = match new_debouncer_opt::<_, notify::RecommendedWatcher, _>(
            Duration::from_secs(LOCAL_WATCH_DEBOUNCE_SECS),
            None,
            handler,
            RecommendedCache::new(),
            notify::Config::default().with_follow_symlinks(false),
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
        // key -> when its watch registration last failed, so a root PM cannot watch is retried on a
        // slow cadence instead of every tick. Cleared alongside `watched` when the vault locks.
        let mut failed: std::collections::HashMap<String, Instant> =
            std::collections::HashMap::new();
        let mut was_ready = false;
        loop {
            tokio::time::sleep(Duration::from_secs(LOCAL_WATCH_TICK_SECS)).await;
            manage_local_watches(
                &manager_app,
                &mut debouncer,
                &mut watched,
                &mut failed,
                &mut was_ready,
            );
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
    failed: &mut std::collections::HashMap<String, Instant>,
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
            failed.clear();
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
    // Drop the retry penalty for anything that left the registry or whose root went away, so a folder
    // that is removed and re-added — or a drive that comes back — is tried again at once.
    failed.retain(|key, _| targets.iter().any(|t| t.key == *key && t.present));

    let diff = diff_watch_set(watched, &targets);
    let now = Instant::now();
    // Count roots that actually STARTED being watched, not roots we merely intended to watch: the
    // catch-up reconcile below hangs off this, and a root that fails to register has not caught up
    // on anything.
    let mut started = 0usize;
    for t in &diff.to_watch {
        let since_failure = failed.get(&t.key).map(|s| now.duration_since(*s));
        if !backoff_elapsed(since_failure, LOCAL_WATCH_RETRY_SECS) {
            continue;
        }
        match debouncer.watch(&t.root, notify::RecursiveMode::Recursive) {
            Ok(()) => {
                watched.insert(t.key.clone(), t.root.clone());
                if failed.remove(&t.key).is_some() {
                    eprintln!("local watcher: watching {} again", t.root.display());
                }
                started += 1;
            }
            Err(e) => {
                // A recursive add registers directory by directory and gives up on the first failure,
                // keeping everything it already registered. Those watches are live but unreachable to
                // us, so hand them back before we forget the root.
                let _ = debouncer.unwatch(&t.root);
                // Unwatching a root drops every watch beneath it, including those of a tracked folder
                // nested inside — forget those too so the next tick re-registers them.
                watched.retain(|_, r| !r.starts_with(&t.root));
                if failed.insert(t.key.clone(), now).is_none() {
                    eprintln!(
                        "local watcher: could not watch {} ({e}) — retrying in {LOCAL_WATCH_RETRY_SECS}s. \
                         On Linux this is usually the machine-wide fs.inotify.max_user_watches limit. \
                         The folder is still reconciled by the periodic sync.",
                        t.root.display()
                    );
                }
            }
        }
    }
    for root in &diff.to_unwatch {
        let _ = debouncer.unwatch(root);
        // Same caveat as above: dropping a root's watches drops every nested folder's watches with it.
        watched.retain(|_, r| !r.starts_with(root));
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
    if !*was_ready || started > 0 {
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
    //
    // Rate-limited, because "an error burst folds into one pass" only holds while the errors stop: a
    // watcher wedged at the inotify limit keeps erroring, and single-flight does not damp that — a
    // busy run sets the rerun flag and the guard immediately runs another pass.
    let events = match res {
        Ok(events) => events,
        Err(_) => {
            let due = {
                let mut last = LAST_WATCH_HEAL
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let now = Instant::now();
                let due =
                    backoff_elapsed(last.map(|t| now.duration_since(t)), LOCAL_HEAL_MIN_GAP_SECS);
                if due {
                    *last = Some(now);
                }
                due
            };
            if due {
                let app2 = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = local_sync_core(&app2, None).await;
                });
            }
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
///
/// Both endpoints of a rename/removal can be a DIRECTORY, and no single item's `external_ref` (always
/// a file's absolute path) can ever match one — so those two arms fan out over the stored refs beneath
/// the directory ([`rename_local_dir`] / [`remove_local_dir`]). `Upsert` deliberately does NOT: notify's
/// Windows backend reports directory LAST_WRITE constantly, and a scoped fan-out per event would be a
/// hot-path regression. A folder moved *into* a tracked root therefore still waits for the periodic
/// walk (a copy emits per-file creates, so only same-filesystem moves are affected).
async fn handle_fs_change(app: &AppHandle, change: FsChange, roots: &[LocalRoot]) -> Result<bool> {
    match change {
        FsChange::Upsert(path) => upsert_local_path(app, &path, roots).await,
        FsChange::Removed(path) => {
            // Try the exact ref first (cheap, and the overwhelmingly common case). A gone path can't be
            // stat'd, so a directory is only recognisable as a prefix of the refs we stored.
            if remove_local_path(app, &path, roots).await? {
                Ok(true)
            } else {
                remove_local_dir(app, &path, roots).await
            }
        }
        FsChange::Renamed { from, to } => {
            if to.is_dir() {
                return rename_local_dir(app, &from, &to, roots).await;
            }
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
        if !walk_admits_file(rel, &exclude) {
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

/// Reconcile a renamed/moved DIRECTORY: repoint every stored ref beneath it, then soft-delete whatever
/// didn't make the move. A directory rename leaves each file's OS id untouched, so this is a pure
/// repoint — the items keep their project, tags and embeddings, exactly as the periodic walk would
/// resolve them (it just wouldn't run for up to 15 minutes).
///
/// A subtree that moved BETWEEN tracked roots, or one whose source wasn't strictly below its root, is
/// handed to [`remove_local_dir`] instead: re-keying every item into the other folder's
/// `local:<key>:` namespace is the walk's job, and it does it correctly.
async fn rename_local_dir(
    app: &AppHandle,
    from: &Path,
    to: &Path,
    roots: &[LocalRoot],
) -> Result<bool> {
    let Some(LocalRoot { key, root, exclude }) = folder_of(to, roots) else {
        return Ok(false);
    };
    let from_key = folder_of(from, roots).map(|f| f.key);
    if from_key.as_deref() != Some(key.as_str()) || fanout_dir(from, &root).is_none() {
        return remove_local_dir(app, from, roots).await;
    }
    // Collect under a guard that DROPS before any await (the DB mutex is not reentrant, and
    // `reconcile_present_file` takes its own short locks).
    let items = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        items_under_dir(&conn, &key, from)?
    };
    let mut flusher = connector_sync::ManifestFlusher::new(app);
    let mut changed = false;
    for (source_id, external_ref, known) in items {
        let Some(new_path) = repoint_ref(&external_ref, from, to) else {
            continue;
        };
        if !new_path.is_file() {
            continue; // gone, not moved — the removal pass below owns it
        }
        let Ok(rel) = new_path.strip_prefix(&root) else {
            continue;
        };
        if !walk_admits_file(rel, &exclude) {
            continue; // renamed into .archive/node_modules/an excluded subtree — the walk skips it too
        }
        let file = LocalFile {
            source_id: source_id_for(&key, &file_identity(&new_path, &root)),
            abs_path: new_path.clone(),
            rel_path: rel.to_string_lossy().to_string(),
            size: std::fs::metadata(&new_path).map(|m| m.len()).unwrap_or(0),
        };
        // A volume with no stable file id keys items by PATH, so the move produced a NEW id: hand the
        // reconcile no prior state and let it ingest a fresh item (the old one is soft-deleted below).
        let known = (file.source_id == source_id).then_some(known);
        reconcile_present_file(app, &file, known.as_ref(), &mut flusher).await?;
        changed = true; // a repoint IS a row change even when the content outcome is NoChange
    }
    flusher.flush()?;
    // Ordering is load-bearing: this re-reads the refs under `from` AFTER the repoint loop, so an item
    // that moved no longer matches and only genuine leftovers are soft-deleted.
    let removed = remove_local_dir(app, from, roots).await?;
    Ok(changed || removed)
}

/// Reconcile a removed DIRECTORY: soft-delete every indexed item beneath it whose file is really gone.
/// Deletion is decided per item by a `Path::exists()` check, never by absence from an enumeration, so a
/// truncated walk can't leak into it.
///
/// The tracked ROOT is never fanned out (see [`fanout_dir`]), and neither is anything beneath a root
/// that has itself gone away: an unmounted or deleted root is the manager's `unreachable` path, and
/// per-item `source_missing` there would be the mass deletion that path exists to prevent.
async fn remove_local_dir(app: &AppHandle, dir: &Path, roots: &[LocalRoot]) -> Result<bool> {
    let Some(LocalRoot { key, root, .. }) = folder_of(dir, roots) else {
        return Ok(false);
    };
    if fanout_dir(dir, &root).is_none() || !root.is_dir() {
        return Ok(false);
    }
    // Same guard discipline as the rename fan-out: collect, drop, then await.
    let items = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        items_under_dir(&conn, &key, dir)?
    };
    let mut flusher = connector_sync::ManifestFlusher::new(app);
    let mut changed = false;
    for (source_id, external_ref, known) in items {
        if Path::new(&external_ref).exists() {
            continue; // still on disk → this was a rename of an ancestor, not a deletion
        }
        reconcile_deleted_item(app, &source_id, &known, &mut flusher).await?;
        changed = true;
    }
    flusher.flush()?;
    Ok(changed)
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
            summary_indexed: false,
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
            summary_indexed: false,
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

        let (files, truncated) = walk(root, &[]);
        let mut rels: Vec<String> = files
            .into_iter()
            .map(|f| f.rel_path.replace('\\', "/"))
            .collect();
        rels.sort();
        assert_eq!(
            rels,
            vec!["a.md".to_string(), "photo.png".into(), "sub/b.txt".into()]
        );
        // A healthy walk is COMPLETE: nothing here was unreadable, so the caller may infer deletions
        // from absence. A flag that fired on a normal tree would suppress every deletion forever.
        assert!(!truncated, "a healthy walk is complete");
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
        let (files, truncated) = walk(root, &["Archive".to_string()]);
        let mut rels: Vec<String> = files
            .into_iter()
            .map(|f| f.rel_path.replace('\\', "/"))
            .collect();
        rels.sort();
        assert_eq!(rels, vec!["Work/plan.md".to_string(), "keep.md".into()]);
        // A DELIBERATE prune is not incompleteness: the watcher applies the same gate, so nothing under
        // an exclude is ever indexed and there is nothing to soft-delete. Flagging it would withhold
        // this folder's deletion sweep for as long as the exclude exists.
        assert!(!truncated, "a deliberate prune is not incompleteness");

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
    fn unreadable_dir_is_incomplete_but_a_deleted_one_is_not() {
        use std::io::ErrorKind;
        // A subtree we were refused is a subtree we never enumerated → the picture is incomplete.
        assert!(read_failure_hides_files(ErrorKind::PermissionDenied, 1));
        assert!(
            read_failure_hides_files(ErrorKind::Other, 1),
            "Windows reports a dropped share as an uncategorised errno, not PermissionDenied"
        );
        // Below the root, a vanished directory is a REAL absence — the sweep must still reap it, or a
        // deleted folder's items would stay 'ok' forever.
        assert!(
            !read_failure_hides_files(ErrorKind::NotFound, 1),
            "a directory deleted mid-walk is a REAL delete — the sweep must still reap it"
        );
        // At the root it is not: a tracked folder that vanished is the missing/unreachable branch.
        assert!(
            read_failure_hides_files(ErrorKind::NotFound, 0),
            "the tracked root vanishing is `missing`/unreachable, never a mass deletion"
        );
    }

    #[cfg(unix)]
    #[test]
    fn walk_flags_an_unreadable_subdirectory() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("keep.md"), b"x").unwrap();
        let locked = root.join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::write(locked.join("inside.md"), b"x").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read_dir(&locked).is_ok() {
            // Running as root ignores the mode bits — skip rather than flake.
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
            return;
        }

        let (files, truncated) = walk(root, &[]);
        // Restore before the tempdir drops, so its recursive cleanup can succeed.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();

        assert!(
            truncated,
            "a directory that would not open leaves files unseen — the walk is incomplete"
        );
        let rels: Vec<String> = files
            .iter()
            .map(|f| f.rel_path.replace('\\', "/"))
            .collect();
        assert!(
            rels.contains(&"keep.md".to_string()),
            "readable files still index"
        );
        assert!(
            !rels.iter().any(|r| r.contains("inside.md")),
            "nothing under the locked directory was enumerated"
        );
    }

    #[test]
    fn repoint_ref_maps_a_subtree_and_ignores_a_sibling_prefix() {
        assert_eq!(
            repoint_ref(
                "/root/notes/a.md",
                Path::new("/root/notes"),
                Path::new("/root/archive")
            ),
            Some(PathBuf::from("/root/archive/a.md"))
        );
        // Nested refs keep their whole tail.
        assert_eq!(
            repoint_ref(
                "/root/notes/2026/q1/a.md",
                Path::new("/root/notes"),
                Path::new("/root/archive")
            ),
            Some(PathBuf::from("/root/archive/2026/q1/a.md"))
        );
        // A sibling that merely shares a string prefix is NOT under the renamed directory.
        assert_eq!(
            repoint_ref(
                "/root/notes-old/a.md",
                Path::new("/root/notes"),
                Path::new("/root/archive")
            ),
            None
        );
    }

    #[test]
    fn ref_under_dir_is_component_wise_not_string_prefix() {
        assert!(ref_under_dir("/root/notes/a.md", Path::new("/root/notes")));
        assert!(ref_under_dir(
            "/root/notes/deep/a.md",
            Path::new("/root/notes")
        ));
        // `notesX` starts with `notes` as a STRING but is a different directory.
        assert!(!ref_under_dir(
            "/root/notesX/a.md",
            Path::new("/root/notes")
        ));
        assert!(!ref_under_dir("/root/other/a.md", Path::new("/root/notes")));
    }

    #[test]
    fn fanout_dir_never_targets_the_tracked_root() {
        // The one-way guard: an unmounted/removed ROOT must stay the manager's `unreachable` path, never
        // a per-item `source_missing` fan-out (which would be a mass deletion over the user's files).
        assert!(fanout_dir(Path::new("/root"), Path::new("/root")).is_none());
        assert!(fanout_dir(Path::new("/root/sub"), Path::new("/root")).is_some());
        assert!(fanout_dir(Path::new("/root/sub/deep"), Path::new("/root")).is_some());
        // A path outside the root isn't this folder's business at all.
        assert!(fanout_dir(Path::new("/other/sub"), Path::new("/root")).is_none());
    }

    #[test]
    fn walk_admits_file_matches_the_walks_directory_gates() {
        let none: &[String] = &[];
        assert!(walk_admits_file(Path::new("a/b/c.md"), none));
        assert!(walk_admits_file(Path::new("c.md"), none));
        // The walk never descends into these, so the watcher must never index into them either.
        assert!(!walk_admits_file(Path::new("node_modules/x.md"), none));
        assert!(!walk_admits_file(Path::new(".git/x.md"), none));
        assert!(!walk_admits_file(
            Path::new("a/node_modules/deep/x.md"),
            none
        ));
        // An excluded subtree is pruned at the directory, whatever the depth beneath it.
        let exclude = &["skipme".to_string()];
        assert!(!walk_admits_file(Path::new("skipme/deep/x.md"), exclude));
        assert!(walk_admits_file(Path::new("keepme/deep/x.md"), exclude));
        // The depth cap counts components: a file exactly at the cap is admitted, one past it is not
        // (the walk stops descending at `depth >= MAX_WALK_DEPTH`, so its children reach the cap).
        let at_cap: PathBuf = (0..MAX_WALK_DEPTH).map(|i| format!("d{i}")).collect();
        assert!(walk_admits_file(&at_cap, none));
        assert!(!walk_admits_file(&at_cap.join("past.md"), none));
    }

    #[test]
    fn items_under_dir_finds_only_the_named_subtree() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE documents (
                 id INTEGER PRIMARY KEY, source_type TEXT, source_id TEXT, external_ref TEXT,
                 source_modified_at TEXT, source_content_hash TEXT, source_state TEXT,
                 content_hash TEXT, stored_summary TEXT
             );",
        )
        .unwrap();
        let root = PathBuf::from("/vault/docs");
        let notes = root.join("notes");
        let add = |sid: &str, path: &Path| {
            conn.execute(
                "INSERT INTO documents(source_type,source_id,external_ref,source_modified_at,\
                     source_content_hash,source_state,content_hash,stored_summary) \
                 VALUES ('index_only',?1,?2,'t0','h0','ok','h0',NULL)",
                params![sid, path.to_string_lossy()],
            )
            .unwrap();
        };
        add("local:k1:f1", &notes.join("a.md"));
        // A file name full of LIKE metacharacters still comes back — the prefix compare is `substr`.
        add("local:k1:f2", &notes.join("100%_done.md"));
        add("local:k1:f3", &notes.join("deep").join("c.md"));
        add("local:k1:f4", &root.join("notesX").join("b.md")); // sibling prefix, not a child
        add("local:k2:f5", &notes.join("other.md")); // another tracked folder's namespace

        let mut ids: Vec<String> = items_under_dir(&conn, "k1", &notes)
            .unwrap()
            .into_iter()
            .map(|(sid, _, _)| sid)
            .collect();
        ids.sort();
        assert_eq!(
            ids,
            vec![
                "local:k1:f1".to_string(),
                "local:k1:f2".into(),
                "local:k1:f3".into()
            ]
        );

        // The discriminating case for `substr` vs `LIKE`: the metacharacters in the DIRECTORY name. A
        // `LIKE '<root>/q_1/%'` pattern would read `_` as "any character" and drag `qX1` in with it.
        let tricky = root.join("q_1");
        add("local:k1:f6", &tricky.join("z.md"));
        add("local:k1:f7", &root.join("qX1").join("z.md"));
        let ids: Vec<String> = items_under_dir(&conn, "k1", &tricky)
            .unwrap()
            .into_iter()
            .map(|(sid, _, _)| sid)
            .collect();
        assert_eq!(ids, vec!["local:k1:f6".to_string()]);

        // The rows carry the persisted state the reconcile needs, not just an id.
        let (_, external_ref, known) = items_under_dir(&conn, "k1", &tricky)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(external_ref, tricky.join("z.md").to_string_lossy());
        assert_eq!(known.source_state, "ok");
        assert_eq!(known.content_hash.as_deref(), Some("h0"));
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

    #[test]
    fn backoff_lets_the_first_attempt_through_then_holds_it_off() {
        assert!(
            backoff_elapsed(None, LOCAL_WATCH_RETRY_SECS),
            "a root that has never failed is watched immediately"
        );
        assert!(
            !backoff_elapsed(Some(Duration::from_secs(0)), LOCAL_WATCH_RETRY_SECS),
            "the tick right after a failure does not retry — this is the sync storm"
        );
        assert!(
            !backoff_elapsed(
                Some(Duration::from_secs(LOCAL_WATCH_RETRY_SECS - 1)),
                LOCAL_WATCH_RETRY_SECS
            ),
            "still held off one second short of the window"
        );
        assert!(
            backoff_elapsed(
                Some(Duration::from_secs(LOCAL_WATCH_RETRY_SECS)),
                LOCAL_WATCH_RETRY_SECS
            ),
            "retried once the window has fully elapsed"
        );
    }

    #[test]
    fn backoff_window_is_long_enough_to_break_the_tick_loop() {
        // The storm exists because the manager re-proposes a failed root every tick. Whatever the
        // window is, it has to be longer than a tick or the penalty changes nothing.
        const {
            assert!(
                LOCAL_WATCH_RETRY_SECS > LOCAL_WATCH_TICK_SECS,
                "a retry window inside one tick is no backoff at all"
            );
        }
        assert!(
            !backoff_elapsed(
                Some(Duration::from_secs(LOCAL_WATCH_TICK_SECS)),
                LOCAL_WATCH_RETRY_SECS
            ),
            "one tick after the failure the root is still skipped"
        );
        // The self-heal sweep walks every tracked folder, so its floor must clear the debouncer's
        // own cadence (tick_rate defaults to timeout/4) by a wide margin.
        const {
            assert!(
                LOCAL_HEAL_MIN_GAP_SECS > LOCAL_WATCH_DEBOUNCE_SECS,
                "a heal floor inside the debounce window would not damp an error burst"
            );
        }
    }
}
