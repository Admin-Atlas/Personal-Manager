// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Where installed mods live on disk, and the only code that writes, moves or deletes there (Mods
//! spec §1.3, §6). The `mods` row is the truth; these bytes are what it points at.
//!
//! ```text
//! data_dir/mods/.staging/<random>/                                  a package being written
//! data_dir/mods/<vault_id>/<store_id>/<id>/<version>/               an installed version
//! data_dir/mods/<vault_id>/<store_id>/.quarantine/<stamp>-<name>/   a folder no row named, 30 days
//! ```
//!
//! - **Per store**, not only per vault, because each ledger lives in its own store and one machine
//!   can hold two stores of one vault: a restore keeps the vault's id, and the restored copy can run
//!   beside the vault it came from. The store id is a random name kept in that store's `settings`
//!   (`ledger::STORE_ID_KEY`), and a restore clears it, so a copy never sweeps, prunes or uninstalls
//!   the files of the store it was made from. Nothing here changes a folder of any store but the
//!   [`Layout`]'s, except [`remove_stores`] for a store PM has deleted; another store of the vault
//!   is only read, by [`recover`], which copies an exact copy of a row's files from it.
//! - **Staging sits beside the install folders**, on the same filesystem, so the final rename can't
//!   fail with EXDEV (the OS temp dir is tmpfs on Fedora) and a large file never sits in RAM.
//! - **Nothing exists until the first install** (the lean rule): a sweep that finds no `mods/`
//!   folder does nothing and creates nothing.
//! - **Every name is checked before it reaches a path:** the id is a [`ModId`], the version a
//!   [`VersionName`], the vault id passes [`check_vault_id`], the store id [`check_store_id`], and a
//!   package's file paths passed `package::check_path` in the verifier.
//! - **Deletion only on provable absence** (INVARIANTS.md I-30): the sweep acts only on a positive
//!   listing of the `mods` table, a folder no row names (a mod's or a version's) is quarantined
//!   rather than deleted, a version the ledger names again is put back or copied in ([`recover`]),
//!   and an entry whose type can't be read is left alone. Only an update's own prune deletes at
//!   once, after its row has committed.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use sha2::{Digest, Sha256};

use super::ids::{self, ModId};
use super::package::VerifiedFile;
use super::runtime::ServedFile;

/// `data_dir/mods`.
pub const MODS_DIR: &str = "mods";
/// `data_dir/mods/.staging`, shared by every vault (it only ever holds a package in flight).
pub const STAGING_DIR: &str = ".staging";
/// `data_dir/mods/<vault_id>/<store_id>/.quarantine`.
pub const QUARANTINE_DIR: &str = ".quarantine";
/// How long a quarantined folder is kept before the boot sweep deletes it (spec §1.3).
pub const QUARANTINE_DAYS: i64 = 30;

/// A quarantine stamp, `YYYYMMDDTHHMMSSZ` (UTC), at the front of every quarantined name.
const STAMP_LEN: usize = 16;
/// The suffix that keeps two quarantines of one name within a second apart: `~1` to `~99`.
const MAX_SUFFIX: usize = 99;
const SUFFIX_LEN: usize = 3;
/// Random bytes in a staging folder's name, and in a store id (so both are 16 lowercase hex).
const STAGING_NAME_BYTES: usize = 8;
const STORE_ID_LEN: usize = 2 * STAGING_NAME_BYTES;
/// A version folder's name, at most. The verifier caps a version at 64 too.
const MAX_VERSION_LEN: usize = 64;
/// A vault id as a folder name. PM's are UUIDs (36).
const MAX_VAULT_ID_LEN: usize = 64;

/// Windows' MAX_PATH (260) less its terminating NUL. PM's own file calls go through Rust's std, which
/// switches to a `\\?\` path by itself past this, but Explorer, the NSIS uninstaller's `RMDir /r`
/// and most tools that aren't long-path aware stop here, and a mod folder they can't remove would
/// outlive "remove PM completely". So a package whose paths could pass it is refused (Windows only).
pub const WINDOWS_FILE_LIMIT: usize = 259;
/// CreateDirectoryW's limit for a folder: MAX_PATH less 12 (room for an 8.3 name), less the NUL.
pub const WINDOWS_DIR_LIMIT: usize = 247;

/// Why a name can't become a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathRefusal {
    /// The vault id isn't one PM would make (lowercase letters, digits and hyphens).
    VaultId,
    /// The store id isn't one PM would make (16 lowercase hex).
    StoreId,
    /// The version isn't a canonical semver in lowercase ASCII.
    Version(String),
    /// On Windows, a file or folder of this package would pass the path limit.
    TooLong { longest: usize, limit: usize },
}

impl std::fmt::Display for PathRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VaultId => f.write_str("this vault's id can't name a folder for its mods"),
            Self::StoreId => f.write_str("this store's mods folder name is damaged"),
            Self::Version(v) => write!(
                f,
                "its version {v:?} can't name a folder (lowercase letters, digits, dots and hyphens only)"
            ),
            Self::TooLong { longest, limit } => write!(
                f,
                "its file names are too long for Windows here: one would be {longest} characters, \
                 and Windows allows {limit}"
            ),
        }
    }
}

/// A vault id as a folder name: what PM makes (a lowercase UUID), or near enough to it, and never a
/// Windows device name. The id comes from `vault-meta.json`, which sits in a folder other accounts
/// can write for a shared vault, so it is checked like anything else from outside.
pub fn check_vault_id(vault_id: &str) -> Result<(), PathRefusal> {
    let b = vault_id.as_bytes();
    let shaped = !b.is_empty()
        && b.len() <= MAX_VAULT_ID_LEN
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-');
    if shaped && !ids::is_device_name(vault_id) {
        Ok(())
    } else {
        Err(PathRefusal::VaultId)
    }
}

/// A store id as a folder name: exactly what [`new_store_id`] makes. It is read from the store,
/// which a hand edit could change, so it is checked like the vault id.
pub fn check_store_id(store_id: &str) -> Result<(), PathRefusal> {
    let b = store_id.as_bytes();
    if b.len() == STORE_ID_LEN
        && b.iter()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
    {
        Ok(())
    } else {
        Err(PathRefusal::StoreId)
    }
}

/// A fresh store id: random, so no two stores (a restored copy and its original included) share one.
pub fn new_store_id() -> io::Result<String> {
    random_name()
}

/// A fresh install id (`mods.install_id`, v60): the same shape, so a fresh install of an id never
/// takes the one an earlier install of it had.
pub fn new_install_id() -> io::Result<String> {
    random_name()
}

/// A mod version as a folder name: the canonical semver the verifier already demands, in lowercase
/// ASCII only, so two versions can't differ only by case and share a folder on NTFS or APFS
/// (`1.0.0-RC1` and `1.0.0-rc1` are two versions to semver). Starts with a digit, so it is never
/// `.`, `..` or a device name; has no `/`, `\` or `:`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct VersionName(String);

impl VersionName {
    pub fn new(version: &semver::Version) -> Result<Self, PathRefusal> {
        Self::parse(&version.to_string())
    }

    pub fn parse(s: &str) -> Result<Self, PathRefusal> {
        let ok = s.len() <= MAX_VERSION_LEN
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
            && semver::Version::parse(s).is_ok_and(|v| v.build.is_empty() && v.to_string() == s);
        if ok {
            Ok(Self(s.to_owned()))
        } else {
            Err(PathRefusal::Version(s.chars().take(80).collect()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One store's mod folders. The only way to name a path under `data_dir/mods`.
#[derive(Debug, Clone)]
pub struct Layout {
    root: PathBuf,
    vault_id: String,
    store_id: String,
    vault: PathBuf,
    store: PathBuf,
}

impl Layout {
    pub fn new(data_dir: &Path, vault_id: &str, store_id: &str) -> Result<Self, PathRefusal> {
        check_vault_id(vault_id)?;
        check_store_id(store_id)?;
        let root = data_dir.join(MODS_DIR);
        let vault = root.join(vault_id);
        Ok(Self {
            store: vault.join(store_id),
            vault,
            root,
            vault_id: vault_id.to_owned(),
            store_id: store_id.to_owned(),
        })
    }

    pub fn vault_id(&self) -> &str {
        &self.vault_id
    }

    pub fn store_id(&self) -> &str {
        &self.store_id
    }

    /// `data_dir/mods`.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn staging(&self) -> PathBuf {
        self.root.join(STAGING_DIR)
    }

    /// `data_dir/mods/<vault_id>`, which holds every store of the vault on this machine.
    pub fn vault_dir(&self) -> &Path {
        &self.vault
    }

    /// `data_dir/mods/<vault_id>/<store_id>`, this store's own.
    pub fn store_dir(&self) -> &Path {
        &self.store
    }

    pub fn quarantine(&self) -> PathBuf {
        self.store.join(QUARANTINE_DIR)
    }

    pub fn mod_dir(&self, id: &ModId) -> PathBuf {
        self.store.join(id.as_str())
    }

    pub fn version_dir(&self, id: &ModId, version: &VersionName) -> PathBuf {
        self.mod_dir(id).join(version.as_str())
    }
}

/// A path's length as Windows counts it (UTF-16 units).
pub fn path_units(path: &Path) -> usize {
    path.as_os_str().to_string_lossy().encode_utf16().count()
}

/// The longest folder and file a package's files can make under a store folder `vault_units` long
/// ([`Layout::store_dir`]), in UTF-16 units. Measured on the quarantine form, the longest a file
/// ever has: `<store>\.quarantine\<stamp>-<id>~NN\<version>\<path>`. The installed form is 32
/// characters shorter (no `.quarantine\<stamp>-` or `~NN`), and staging's `.staging\<16 hex>\` (26
/// past `mods\`) is shorter than the quarantine form's at least 61 (the vault id, the store id and
/// the quarantine's own). The worst case a valid package can reach, a 159-character id with a
/// 64-character version and a 200-character path, is 458 characters past the store folder, so only
/// a short id and short paths fit within [`WINDOWS_FILE_LIMIT`].
pub fn longest_paths<'a>(
    vault_units: usize,
    id: &ModId,
    version: &VersionName,
    files: impl IntoIterator<Item = &'a str>,
) -> (usize, usize) {
    let version_dir = vault_units
        + 1
        + QUARANTINE_DIR.len()
        + 1
        + STAMP_LEN
        + 1
        + id.as_str().len()
        + SUFFIX_LEN
        + 1
        + version.as_str().len();
    let mut longest_dir = version_dir;
    let mut longest_file = version_dir;
    for path in files {
        // Package paths are ASCII (`package::check_path`), so bytes are UTF-16 units.
        longest_file = longest_file.max(version_dir + 1 + path.len());
        if let Some((parent, _)) = path.rsplit_once('/') {
            longest_dir = longest_dir.max(version_dir + 1 + parent.len());
        }
    }
    (longest_dir, longest_file)
}

/// Whether a package fits Windows' path limits under a vault folder `vault_units` long.
pub fn check_windows_paths<'a>(
    vault_units: usize,
    id: &ModId,
    version: &VersionName,
    files: impl IntoIterator<Item = &'a str>,
) -> Result<(), PathRefusal> {
    let (dir, file) = longest_paths(vault_units, id, version, files);
    if file > WINDOWS_FILE_LIMIT {
        return Err(PathRefusal::TooLong {
            longest: file,
            limit: WINDOWS_FILE_LIMIT,
        });
    }
    if dir > WINDOWS_DIR_LIMIT {
        return Err(PathRefusal::TooLong {
            longest: dir,
            limit: WINDOWS_DIR_LIMIT,
        });
    }
    Ok(())
}

// ---- Failure injection (tests only) ------------------------------------------------------------

/// The steps a test can make fail, to prove each one leaves nothing behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Write,
    SyncFile,
    SyncDir,
    Rename,
    Commit,
}

#[cfg(test)]
thread_local! {
    /// `(step, n)`: the `n`th time `step` is reached on this thread (from 0), it fails.
    static FAIL_AT: std::cell::Cell<Option<(Step, usize)>> = const { std::cell::Cell::new(None) };
    static SEEN: std::cell::RefCell<Vec<Step>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Make the `n`th time `step` is reached on this thread fail, and forget earlier steps.
#[cfg(test)]
pub fn fail_at(step: Option<(Step, usize)>) {
    FAIL_AT.with(|f| f.set(step));
    SEEN.with(|s| s.borrow_mut().clear());
}

/// A point where a test may inject a failure. Free outside tests.
pub fn failpoint(step: Step) -> io::Result<()> {
    #[cfg(test)]
    {
        let reached = SEEN.with(|s| {
            let mut s = s.borrow_mut();
            s.push(step);
            s.iter().filter(|x| **x == step).count() - 1
        });
        if FAIL_AT.with(|f| f.get()) == Some((step, reached)) {
            return Err(io::Error::other(format!("injected failure at {step:?}")));
        }
    }
    #[cfg(not(test))]
    let _ = step;
    Ok(())
}

// ---- Writing a package -------------------------------------------------------------------------

fn random_name() -> io::Result<String> {
    let mut buf = [0u8; STAGING_NAME_BYTES];
    getrandom::fill(&mut buf).map_err(|e| io::Error::other(format!("no randomness: {e}")))?;
    Ok(hex::encode(buf))
}

/// Make a folder that must not exist yet. `create_dir` never follows a link at the last component.
fn make_dir(path: &Path) -> io::Result<()> {
    fs::create_dir(path)
}

/// Write a file that must not exist yet, and flush it to disk. `create_new` (O_EXCL) fails on
/// anything already there, a dangling link included, so no link is ever followed.
fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    failpoint(Step::Write)?;
    file.write_all(bytes)?;
    failpoint(Step::SyncFile)?;
    file.sync_all()
}

/// Flush a folder's entries, so a rename or a new file survives a crash as well as its bytes.
fn sync_dir(path: &Path) -> io::Result<()> {
    failpoint(Step::SyncDir)?;
    #[cfg(unix)]
    fs::File::open(path)?.sync_all()?;
    // Windows can't open a folder through std (it needs FILE_FLAG_BACKUP_SEMANTICS), and NTFS
    // journals its metadata itself.
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Whether anything is at `path`, without following a link. Only `NotFound` is "no".
fn present(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// A package written into its own staging folder, fsynced. Removed when dropped, unless placed.
#[derive(Debug)]
pub struct Staged {
    dir: Option<PathBuf>,
}

impl Drop for Staged {
    fn drop(&mut self) {
        if let Some(dir) = self.dir.take() {
            let _ = fs::remove_dir_all(dir);
        }
    }
}

/// Write exactly `files` (the verified bytes, never re-read from the zip) into a fresh folder under
/// `.staging`, flushing every file and then every folder, deepest first. Creates `mods/` and
/// `.staging` on the first install.
pub fn stage(layout: &Layout, files: &BTreeMap<String, VerifiedFile>) -> io::Result<Staged> {
    let staging = layout.staging();
    fs::create_dir_all(&staging)?;
    let dir = staging.join(random_name()?);
    make_dir(&dir)?;
    let staged = Staged {
        dir: Some(dir.clone()),
    };
    let mut dirs = BTreeSet::new();
    for (path, file) in files {
        let mut at = dir.clone();
        let mut parts = path.split('/').peekable();
        while let Some(part) = parts.next() {
            at.push(part);
            if parts.peek().is_some() {
                if dirs.insert(at.clone()) {
                    make_dir(&at)?;
                }
            } else {
                write_new(&at, &file.bytes)?;
            }
        }
    }
    // A child sorts after its parent, so in reverse every folder is flushed before the one holding it.
    for d in dirs.iter().rev() {
        sync_dir(d)?;
    }
    sync_dir(&dir)?;
    sync_dir(&staging)?;
    Ok(staged)
}

/// A staged package renamed into its version folder. Rolled back when dropped, unless kept: moved
/// back into staging and removed, the folder it replaced (a repair's) put back, and a mod folder
/// this install made removed again.
#[derive(Debug)]
pub struct Placed {
    dir: PathBuf,
    staging: PathBuf,
    /// Where the folder that stood at `dir` was quarantined, to be put back on a rollback.
    replaced: Option<PathBuf>,
    made_mod_dir: bool,
    keep: bool,
}

impl Placed {
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The row has committed: the files stay.
    pub fn keep(mut self) {
        self.keep = true;
    }
}

impl Drop for Placed {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        // Back into staging first, a rename, so the version folder is free at once and anything the
        // removal can't finish is swept at the next boot rather than left looking installed.
        let back = random_name().map(|n| self.staging.join(n));
        match back {
            Ok(back) if fs::rename(&self.dir, &back).is_ok() => {
                let _ = fs::remove_dir_all(&back);
            }
            _ => {
                let _ = fs::remove_dir_all(&self.dir);
            }
        }
        if let Some(replaced) = self.replaced.take() {
            let _ = fs::rename(&replaced, &self.dir);
            if let Some(entry) = replaced.parent() {
                let _ = fs::remove_dir(entry);
            }
        }
        if self.made_mod_dir {
            if let Some(mod_dir) = self.dir.parent() {
                let _ = fs::remove_dir(mod_dir);
            }
        }
    }
}

/// Rename a staged package into `mods/<vault_id>/<store_id>/<id>/<version>/`. Whatever already
/// stands there is a folder no row names (a crashed install's, or an update's version that never
/// committed) or the broken copy a repair replaces: either way it is quarantined, never merged into
/// or deleted. Only a repair's (`repair`: the row names this very folder) is put back if the install
/// then fails; any other has no row and stays in quarantine.
pub fn place(
    layout: &Layout,
    mut staged: Staged,
    id: &ModId,
    version: &VersionName,
    repair: bool,
    now: DateTime<Utc>,
) -> io::Result<Placed> {
    let mod_dir = layout.mod_dir(id);
    let made_mod_dir = !present(&mod_dir)?;
    fs::create_dir_all(&mod_dir)?;
    if !fs::symlink_metadata(&mod_dir)?.is_dir() {
        return Err(io::Error::other("the mod's folder isn't a folder"));
    }
    let target = layout.version_dir(id, version);
    let replaced = if present(&target)? {
        let moved = quarantine(layout, &target, id.as_str(), Some(version.as_str()), now)?;
        repair.then_some(moved)
    } else {
        None
    };
    let src = staged
        .dir
        .clone()
        .ok_or_else(|| io::Error::other("staged twice"))?;
    let renamed = failpoint(Step::Rename).and_then(|()| fs::rename(&src, &target));
    if let Err(e) = renamed {
        if let Some(replaced) = &replaced {
            let _ = fs::rename(replaced, &target);
            if let Some(entry) = replaced.parent() {
                let _ = fs::remove_dir(entry);
            }
        }
        if made_mod_dir {
            let _ = fs::remove_dir(&mod_dir);
        }
        return Err(e);
    }
    staged.dir = None;
    let placed = Placed {
        dir: target,
        staging: layout.staging(),
        replaced,
        made_mod_dir,
        keep: false,
    };
    sync_dir(&mod_dir)?;
    Ok(placed)
}

/// A quarantine stamp for `now`.
pub fn stamp(now: DateTime<Utc>) -> String {
    now.format("%Y%m%dT%H%M%SZ").to_string()
}

/// When a quarantined entry was quarantined, from its name. `None` for a name PM didn't make.
pub fn quarantined_at(name: &str) -> Option<DateTime<Utc>> {
    let b = name.as_bytes();
    if b.len() <= STAMP_LEN || b[8] != b'T' || b[15] != b'Z' || b[STAMP_LEN] != b'-' {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<u32> {
        let s = name.get(from..to)?;
        s.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| s.parse().ok())
            .flatten()
    };
    let date = NaiveDate::from_ymd_opt(num(0, 4)? as i32, num(4, 6)?, num(6, 8)?)?;
    let time = date.and_hms_opt(num(9, 11)?, num(11, 13)?, num(13, 15)?)?;
    Some(Utc.from_utc_datetime(&time))
}

/// Whether a quarantined entry has been kept its 30 days. A name PM didn't make, or a stamp in the
/// future (the clock went back), is kept: its age can't be proved.
pub fn quarantine_expired(name: &str, now: DateTime<Utc>) -> bool {
    quarantined_at(name)
        .is_some_and(|at| now >= at && now - at >= chrono::Duration::days(QUARANTINE_DAYS))
}

/// Move `from` into the vault's quarantine as `<stamp>-<name>` (or, with `nested`, as
/// `<stamp>-<name>/<nested>`, a version folder keeping its shape). Returns where it went.
fn quarantine(
    layout: &Layout,
    from: &Path,
    name: &str,
    nested: Option<&str>,
    now: DateTime<Utc>,
) -> io::Result<PathBuf> {
    let q = layout.quarantine();
    fs::create_dir_all(&q)?;
    let stamp = stamp(now);
    for n in 0..=MAX_SUFFIX {
        let entry = if n == 0 {
            q.join(format!("{stamp}-{name}"))
        } else {
            q.join(format!("{stamp}-{name}~{n}"))
        };
        // Checked first, because a rename onto an empty folder replaces it on Linux and macOS.
        // The ledger's lock keeps anything else in PM from racing for the name.
        if present(&entry)? {
            continue;
        }
        let dest = match nested {
            None => entry,
            Some(leaf) => {
                make_dir(&entry)?;
                entry.join(leaf)
            }
        };
        if let Err(e) = fs::rename(from, &dest) {
            if nested.is_some() {
                if let Some(entry) = dest.parent() {
                    let _ = fs::remove_dir(entry);
                }
            }
            return Err(e);
        }
        return Ok(dest);
    }
    Err(io::Error::other(
        "the quarantine has no free name this second",
    ))
}

/// Remove every version folder of `id` but `keep`, after an update has committed. Best-effort: what
/// can't be removed now (a file held open on Windows) is quarantined by the next boot sweep, and
/// removed 30 days after that.
pub fn prune_versions(layout: &Layout, id: &ModId, keep: &VersionName) -> Vec<String> {
    let mut pruned = Vec::new();
    let Ok(entries) = fs::read_dir(layout.mod_dir(id)) else {
        return pruned;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        if prune_version(&name, is_dir, Some(keep)) && fs::remove_dir_all(entry.path()).is_ok() {
            pruned.push(name);
        }
    }
    pruned
}

/// Remove a mod's whole folder (uninstall), then the vault's and the root's folders if that left
/// them empty, so the last uninstall leaves no `mods/` behind.
pub fn remove_mod(layout: &Layout, id: &ModId) -> io::Result<()> {
    match fs::remove_dir_all(layout.mod_dir(id)) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    tidy(layout);
    Ok(())
}

/// Remove the store's folder, the vault's, `.staging` and `mods/` where each is empty. `remove_dir`
/// refuses a folder with anything in it, so this can only ever remove nothing.
pub fn tidy(layout: &Layout) {
    let _ = fs::remove_dir(layout.store_dir());
    let _ = fs::remove_dir(layout.vault_dir());
    let _ = fs::remove_dir(layout.staging());
    let _ = fs::remove_dir(layout.root());
}

/// The folder "Remove PM data" removes for a vault (spec §6), every store of it on this machine
/// (the wipe removes every restored copy too), or `None` when the id can't name one.
pub fn vault_folder(data_dir: &Path, vault_id: &str) -> Option<PathBuf> {
    check_vault_id(vault_id)
        .ok()
        .map(|()| data_dir.join(MODS_DIR).join(vault_id))
}

/// Remove the mod folders of stores PM has deleted: every store folder of `vault_id` but `keep`
/// (the open store's, when it is that vault's), then the vault's folder and `mods/` if that left
/// them empty. For a pristine home a restore re-homed over, whose ledger went with it; the caller
/// holds the ledger lock. An id that can't name a folder removes nothing.
pub fn remove_stores(data_dir: &Path, vault_id: &str, keep: Option<&str>) -> io::Result<()> {
    let Some(folder) = vault_folder(data_dir, vault_id) else {
        return Ok(());
    };
    let entries = match fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries.flatten() {
        if keep.is_some_and(|k| entry.file_name() == k) {
            continue;
        }
        // Never through a link: `remove_dir_all` on a link removes the link, not what it names.
        let _ = match entry.file_type() {
            Ok(t) if t.is_dir() => fs::remove_dir_all(entry.path()),
            Ok(_) => fs::remove_file(entry.path()),
            Err(e) => Err(e),
        };
    }
    let _ = fs::remove_dir(&folder);
    let _ = fs::remove_dir(data_dir.join(MODS_DIR));
    Ok(())
}

// ---- The boot sweep ----------------------------------------------------------------------------

/// What the sweep does with an entry of the store's folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultEntry<'a> {
    /// The quarantine itself, aged separately.
    Quarantine,
    /// A folder no row names: quarantined.
    Unknown,
    /// A mod the ledger lists, with its current version folder (`None`: its row's version can't name
    /// one, so none of its folders is provably stale).
    Known(Option<&'a VersionName>),
    /// Not PM's: a file (PM only ever makes folders here, and Windows and macOS drop `desktop.ini`
    /// or `.DS_Store` into folders people open), a link, or a dot-name, which is never a mod id.
    Leave,
}

/// Decide an entry of the store's folder. Pure.
pub fn classify<'a>(
    name: &str,
    is_dir: bool,
    known: &'a BTreeMap<String, Option<VersionName>>,
) -> VaultEntry<'a> {
    if name == QUARANTINE_DIR {
        return VaultEntry::Quarantine;
    }
    if name.starts_with('.') || !is_dir {
        return VaultEntry::Leave;
    }
    match known.get(name) {
        Some(current) => VaultEntry::Known(current.as_ref()),
        None => VaultEntry::Unknown,
    }
}

/// Whether an entry of a known mod's folder is a stale version: a folder (never a link or a file)
/// that isn't the row's current version. Pure.
pub fn prune_version(name: &str, is_dir: bool, current: Option<&VersionName>) -> bool {
    is_dir && current.is_some_and(|c| c.as_str() != name)
}

/// What the sweep did, for the log.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SweepReport {
    /// Mod folders no row names (`<id>`), and version folders their row doesn't (`<id>/<version>`).
    pub quarantined: Vec<String>,
    pub expired: Vec<String>,
    pub staging_cleared: usize,
    /// Entries it should have acted on and couldn't (each is retried next boot).
    pub failed: Vec<String>,
}

impl SweepReport {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The boot sweep's moves (spec §1.3), from `known`: every `mods` row's id with its current version
/// folder, read from the table. The caller runs it only as the writer and only after that listing
/// read cleanly; it never runs on a guess. It deletes nothing: a mod folder no row names, and a
/// version folder its row doesn't, go to quarantine, so a sweep against a ledger that is briefly
/// older than its files ([`recover`]) loses nothing. Touches this store's folder only.
pub fn sweep(
    layout: &Layout,
    known: &BTreeMap<String, Option<VersionName>>,
    now: DateTime<Utc>,
) -> SweepReport {
    let mut report = SweepReport::default();
    let Ok(entries) = fs::read_dir(layout.store_dir()) else {
        // Missing (no mod was ever installed in this store) or unreadable: nothing is provable.
        return report;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        match classify(&name, kind.is_dir(), known) {
            VaultEntry::Quarantine | VaultEntry::Leave => {}
            VaultEntry::Unknown => match quarantine(layout, &entry.path(), &name, None, now) {
                Ok(_) => report.quarantined.push(name),
                Err(_) => report.failed.push(name),
            },
            VaultEntry::Known(current) => {
                quarantine_stale(layout, &entry.path(), &name, current, now, &mut report)
            }
        }
    }
    report
}

/// The boot sweep's deletions: whatever `.staging` holds (only ever an install in flight, and the
/// caller holds the ledger lock, so none is) and quarantine past its 30 days. Kept apart from
/// [`sweep`] so it can run off the async runtime: either can be a large tree.
pub fn expire(layout: &Layout, now: DateTime<Utc>) -> SweepReport {
    let mut report = SweepReport::default();
    clear_staging(layout, &mut report);
    expire_quarantine(layout, now, &mut report);
    report
}

fn clear_staging(layout: &Layout, report: &mut SweepReport) {
    let Ok(entries) = fs::read_dir(layout.staging()) else {
        return;
    };
    for entry in entries.flatten() {
        let removed = match entry.file_type() {
            Ok(t) if t.is_dir() => fs::remove_dir_all(entry.path()),
            Ok(_) => fs::remove_file(entry.path()),
            Err(e) => Err(e),
        };
        match removed {
            Ok(()) => report.staging_cleared += 1,
            Err(_) => report
                .failed
                .push(entry.file_name().to_string_lossy().into_owned()),
        }
    }
}

/// Quarantine a known mod's version folders its row doesn't name, each as `<stamp>-<id>/<version>`.
fn quarantine_stale(
    layout: &Layout,
    dir: &Path,
    id: &str,
    current: Option<&VersionName>,
    now: DateTime<Utc>,
    report: &mut SweepReport,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let (Ok(kind), Some(name)) = (
            entry.file_type(),
            entry.file_name().to_str().map(str::to_owned),
        ) else {
            continue;
        };
        if prune_version(&name, kind.is_dir(), current) {
            match quarantine(layout, &entry.path(), id, Some(&name), now) {
                Ok(_) => report.quarantined.push(format!("{id}/{name}")),
                Err(_) => report.failed.push(format!("{id}/{name}")),
            }
        }
    }
}

/// Whether a quarantine entry's name is `<stamp>-<id>` or `<stamp>-<id>~N`, as [`quarantine`] names
/// a mod's folder or one of its versions.
fn quarantined_as(name: &str, id: &ModId) -> bool {
    if quarantined_at(name).is_none() {
        return false;
    }
    let Some(rest) = name[STAMP_LEN + 1..].strip_prefix(id.as_str()) else {
        return false;
    };
    rest.is_empty()
        || rest.strip_prefix('~').is_some_and(|n| {
            !n.is_empty() && n.len() < SUFFIX_LEN && n.bytes().all(|b| b.is_ascii_digit())
        })
}

/// The bytes of the file at `path` under `dir`, when it is a plain file of `want`'s size and sha256
/// reached through real folders only (no link at any step).
fn exact_file(dir: &Path, path: &str, want: &ServedFile) -> Option<Vec<u8>> {
    let mut at = dir.to_path_buf();
    let mut parts = path.split('/').peekable();
    while let Some(part) = parts.next() {
        at.push(part);
        let meta = fs::symlink_metadata(&at).ok()?;
        let last = parts.peek().is_none();
        if !last && !meta.is_dir() {
            return None;
        }
        if last && !(meta.is_file() && meta.len() == want.size) {
            return None;
        }
    }
    let bytes = fs::read(&at).ok()?;
    (<[u8; 32]>::from(Sha256::digest(&bytes)) == want.sha256).then_some(bytes)
}

/// Whether `dir` holds every one of `files` exactly ([`exact_file`]).
fn holds_exactly(dir: &Path, files: &[(String, ServedFile)]) -> bool {
    files
        .iter()
        .all(|(path, want)| exact_file(dir, path, want).is_some())
}

/// Every one of `files` from `dir`, each read once and checked ([`exact_file`]), or `None` when any
/// isn't exact. What it returns is what gets written, so a file changed after its check can't slip in.
fn exact_copy(
    dir: &Path,
    files: &[(String, ServedFile)],
) -> Option<BTreeMap<String, VerifiedFile>> {
    files
        .iter()
        .map(|(path, want)| {
            let bytes = exact_file(dir, path, want)?;
            Some((
                path.clone(),
                VerifiedFile {
                    sha256: want.sha256,
                    bytes,
                },
            ))
        })
        .collect()
}

fn real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

/// The entries of the quarantine `q` that hold a folder of `id` (`<stamp>-<id>[~N]`), real folders
/// only, newest first. None when there is no quarantine.
fn quarantined_names(q: &Path, id: &ModId) -> io::Result<Vec<String>> {
    let entries = match fs::read_dir(q) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .filter(|name| quarantined_as(name, id))
        .collect();
    // The stamp leads the name, so the newest sorts last.
    names.sort();
    names.reverse();
    Ok(names)
}

/// Bring back the files a row names when they are missing, from a copy PM already holds. First this
/// store's own quarantine ([`take_from_quarantine`]); then another store of the same vault on this
/// machine ([`copy_from_sibling`]). Returns where the files came from, or `None` when there is no
/// exact copy to take. The caller holds the ledger lock, on a blocking thread, since a copy reads,
/// writes and fsyncs whole packages (`ledger::Ledger::recover_missing`).
pub fn recover(
    layout: &Layout,
    id: &ModId,
    version: &VersionName,
    files: &[(String, ServedFile)],
    now: DateTime<Utc>,
) -> io::Result<Option<String>> {
    if let Some(name) = take_from_quarantine(layout, id, version, files, now)? {
        return Ok(Some(name));
    }
    copy_from_sibling(layout, id, version, files, now)
}

/// Put back a version folder that a sweep (or a placement) quarantined, when its row names it again:
/// a store that was briefly an older copy of itself (a sync glitch, a store file put back by hand)
/// swept files its own ledger still needed. Only a copy holding every file the row lists, each its
/// recorded size and sha256, is moved back, newest first; a partial folder at the target goes to
/// quarantine in its place. Returns the quarantine entry it came from. A candidate is hashed but
/// never copied, only renamed, so the startup load runs this inline (`ledger::Ledger::startup`); the
/// caller holds the ledger lock.
pub fn take_from_quarantine(
    layout: &Layout,
    id: &ModId,
    version: &VersionName,
    files: &[(String, ServedFile)],
    now: DateTime<Utc>,
) -> io::Result<Option<String>> {
    let Some((name, from)) = quarantined_names(&layout.quarantine(), id)?
        .into_iter()
        .find_map(|name| {
            let from = layout.quarantine().join(&name).join(version.as_str());
            (real_dir(&from) && holds_exactly(&from, files)).then_some((name, from))
        })
    else {
        return Ok(None);
    };
    let mod_dir = layout.mod_dir(id);
    fs::create_dir_all(&mod_dir)?;
    if !fs::symlink_metadata(&mod_dir)?.is_dir() {
        return Err(io::Error::other("the mod's folder isn't a folder"));
    }
    let target = layout.version_dir(id, version);
    if present(&target)? {
        quarantine(layout, &target, id.as_str(), Some(version.as_str()), now)?;
    }
    fs::rename(&from, &target)?;
    let _ = fs::remove_dir(layout.quarantine().join(&name));
    sync_dir(&mod_dir)?;
    sync_dir(&layout.quarantine())?;
    Ok(Some(name))
}

/// When this store holds no copy: an exact one in another store of the same vault on this machine,
/// such as the store a restored copy was made from (a restore gives the copy a store id of its own)
/// or the one "Start a new empty store" set aside. Its installed folder first, then its quarantine,
/// newest first; stores in name order. Copied in through staging and the normal placement, never
/// moved: the other store may be opened again, and its files are its own ledger's. Only the listed
/// files are copied, each written from the bytes that were checked. Returns
/// `<store_id>/<id>/<version>` or `<store_id>/.quarantine/<entry>`.
fn copy_from_sibling(
    layout: &Layout,
    id: &ModId,
    version: &VersionName,
    files: &[(String, ServedFile)],
    now: DateTime<Utc>,
) -> io::Result<Option<String>> {
    let entries = match fs::read_dir(layout.vault_dir()) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut stores: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .filter(|name| name != layout.store_id() && check_store_id(name).is_ok())
        .collect();
    stores.sort();
    for store in stores {
        let base = layout.vault_dir().join(&store);
        let mut candidates = Vec::new();
        let installed = base.join(id.as_str());
        if real_dir(&installed) {
            candidates.push((
                installed.join(version.as_str()),
                format!("{store}/{id}/{}", version.as_str()),
            ));
        }
        let q = base.join(QUARANTINE_DIR);
        if real_dir(&q) {
            for name in quarantined_names(&q, id).unwrap_or_default() {
                candidates.push((
                    q.join(&name).join(version.as_str()),
                    format!("{store}/{QUARANTINE_DIR}/{name}"),
                ));
            }
        }
        for (from, label) in candidates {
            let Some(copy) = real_dir(&from).then(|| exact_copy(&from, files)).flatten() else {
                continue;
            };
            place(layout, stage(layout, &copy)?, id, version, false, now)?.keep();
            return Ok(Some(label));
        }
    }
    Ok(None)
}

fn expire_quarantine(layout: &Layout, now: DateTime<Utc>, report: &mut SweepReport) {
    let Ok(entries) = fs::read_dir(layout.quarantine()) else {
        return;
    };
    for entry in entries.flatten() {
        let (Ok(kind), Some(name)) = (
            entry.file_type(),
            entry.file_name().to_str().map(str::to_owned),
        ) else {
            continue;
        };
        if !quarantine_expired(&name, now) {
            continue;
        }
        let removed = if kind.is_dir() {
            fs::remove_dir_all(entry.path())
        } else {
            fs::remove_file(entry.path())
        };
        match removed {
            Ok(()) => report.expired.push(name),
            Err(_) => report.failed.push(name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    const VAULT: &str = "11111111-2222-3333-4444-555555555555";
    const STORE: &str = "0123456789abcdef";

    fn id(s: &str) -> ModId {
        ModId::parse(s).unwrap()
    }

    fn ver(s: &str) -> VersionName {
        VersionName::parse(s).unwrap()
    }

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn files(list: &[(&str, &[u8])]) -> BTreeMap<String, VerifiedFile> {
        list.iter()
            .map(|(p, b)| {
                let f = VerifiedFile {
                    sha256: Sha256::digest(b).into(),
                    bytes: b.to_vec(),
                };
                (p.to_string(), f)
            })
            .collect()
    }

    /// Every file and folder under `dir`, relative, `/`-separated, sorted.
    fn tree(dir: &Path) -> Vec<String> {
        fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
            let Ok(rd) = fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                let rel = e
                    .path()
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let is_dir = e.file_type().unwrap().is_dir();
                out.push(if is_dir { format!("{rel}/") } else { rel });
                if is_dir {
                    walk(base, &e.path(), out);
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, dir, &mut out);
        out.sort();
        out
    }

    #[test]
    fn vault_ids_are_checked_before_they_name_a_folder() {
        for good in [VAULT, "vault-1", "a", "0f"] {
            assert_eq!(check_vault_id(good), Ok(()), "{good}");
        }
        for bad in [
            "",
            ".",
            "..",
            "../x",
            "a/b",
            "a\\b",
            "C:",
            "Vault",
            "-a",
            ".quarantine",
            ".staging",
            "con",
            "nul",
            "com1",
            "a b",
            "é",
            &"a".repeat(65),
        ] {
            assert_eq!(check_vault_id(bad), Err(PathRefusal::VaultId), "{bad:?}");
            assert!(Layout::new(Path::new("/d"), bad, STORE).is_err());
        }
    }

    /// The store id comes out of the store, so it is held to exactly the shape PM mints.
    #[test]
    fn store_ids_are_what_pm_mints_and_nothing_else() {
        for _ in 0..8 {
            let minted = new_store_id().unwrap();
            assert_eq!(check_store_id(&minted), Ok(()), "{minted}");
        }
        assert_ne!(new_store_id().unwrap(), new_store_id().unwrap());
        for bad in [
            "",
            "0123456789abcde",
            "0123456789abcdef0",
            "0123456789ABCDEF",
            "0123456789abcdeg",
            "../456789abcdef0",
            "0123456789abcd/f",
            ".quarantine12345",
        ] {
            assert_eq!(check_store_id(bad), Err(PathRefusal::StoreId), "{bad:?}");
            assert!(Layout::new(Path::new("/d"), VAULT, bad).is_err());
        }
    }

    #[test]
    fn versions_name_a_folder_only_in_lowercase_canonical_form() {
        for good in ["1.0.0", "0.1.0-beta.2", "10.20.30-rc-1", "1.0.0-0a"] {
            assert_eq!(ver(good).as_str(), good);
        }
        for bad in [
            "",
            "1.0",
            "1.0.0-RC1",
            "1.0.0-Beta",
            "1.0.0+build",
            "01.0.0",
            "1.0.0 ",
            "../1.0.0",
            "1.0.0/..",
            "1.0.0\\x",
            "1.0.0:x",
            "v1.0.0",
            "1.0.0-",
            &format!("1.0.0-{}", "a".repeat(60)),
        ] {
            assert!(VersionName::parse(bad).is_err(), "{bad:?}");
        }
        let built = semver::Version::parse("1.0.0+meta").unwrap();
        assert!(
            VersionName::new(&built).is_err(),
            "build metadata never names a folder"
        );
        let upper = semver::Version::parse("1.0.0-RC1").unwrap();
        assert_eq!(
            VersionName::new(&upper),
            Err(PathRefusal::Version("1.0.0-RC1".into()))
        );
    }

    #[test]
    fn the_layout_puts_each_thing_where_the_spec_says() {
        let l = Layout::new(Path::new("/data"), VAULT, STORE).unwrap();
        assert_eq!(l.root(), Path::new("/data/mods"));
        assert_eq!(l.staging(), Path::new("/data/mods/.staging"));
        assert_eq!(l.vault_dir(), Path::new(&format!("/data/mods/{VAULT}")));
        assert_eq!(
            l.store_dir(),
            Path::new(&format!("/data/mods/{VAULT}/{STORE}"))
        );
        assert_eq!(
            l.quarantine(),
            Path::new(&format!("/data/mods/{VAULT}/{STORE}/.quarantine"))
        );
        assert_eq!(
            l.version_dir(&id("atlas.map"), &ver("1.2.0")),
            Path::new(&format!("/data/mods/{VAULT}/{STORE}/atlas.map/1.2.0"))
        );
        assert_eq!(
            vault_folder(Path::new("/data"), VAULT).as_deref(),
            Some(l.vault_dir())
        );
        assert_eq!(vault_folder(Path::new("/data"), "../x"), None);
    }

    /// The pure length check, at its boundaries, and the measured worst case.
    #[test]
    fn the_windows_path_check_counts_the_longest_form_a_file_takes() {
        let m = id("atlas.map");
        let v = ver("1.0.0");
        // The version folder's quarantine form under a vault folder of length 0: 1 + 11 + 1 + 16 +
        // 1 + 9 + 3 + 1 + 5 = 48.
        let (dir, file) = longest_paths(0, &m, &v, ["manifest.json"]);
        assert_eq!((dir, file), (48, 48 + 1 + 13));
        let (dir, file) = longest_paths(10, &m, &v, ["app/js/x.js", "LICENCE.txt"]);
        assert_eq!(dir, 10 + 48 + 1 + "app/js".len());
        assert_eq!(file, 10 + 48 + 1 + "app/js/x.js".len());

        // Exactly at the file limit fits; one more doesn't.
        let fill = |extra: usize| "a".repeat(WINDOWS_FILE_LIMIT - 48 - 1 - 4 + extra);
        let at_limit = format!("app/{}", fill(0));
        assert_eq!(check_windows_paths(0, &m, &v, [at_limit.as_str()]), Ok(()));
        let over = format!("app/{}", fill(1));
        assert_eq!(
            check_windows_paths(0, &m, &v, [over.as_str()]),
            Err(PathRefusal::TooLong {
                longest: WINDOWS_FILE_LIMIT + 1,
                limit: WINDOWS_FILE_LIMIT
            })
        );
        // A deep folder can pass the folder limit while its file still fits the file limit.
        let deep = format!("app/{}/x", "d".repeat(WINDOWS_DIR_LIMIT - 48 - 1 - 4 + 1));
        assert!(matches!(
            check_windows_paths(0, &m, &v, [deep.as_str()]),
            Err(PathRefusal::TooLong {
                limit: WINDOWS_DIR_LIMIT,
                ..
            })
        ));

        // The measured worst case: the longest id, version and path the verifier lets through.
        let long_id = id(&format!(
            "{}.{}.{}.{}.{}",
            "a".repeat(31),
            "b".repeat(31),
            "c".repeat(31),
            "d".repeat(31),
            "e".repeat(31)
        ));
        assert_eq!(long_id.as_str().len(), 159);
        let long_ver = ver(&format!("1.0.0-{}", "a".repeat(58)));
        let long_path = format!("app/{}", "p".repeat(196));
        let (_, worst) = longest_paths(0, &long_id, &long_ver, [long_path.as_str()]);
        assert_eq!(worst, 458);
        assert!(check_windows_paths(0, &long_id, &long_ver, [long_path.as_str()]).is_err());
        // A typical mod under a typical profile (`C:\Users\<name>\AppData\Local\Personal Manager\
        // mods\<uuid>\<store id>`, 105 characters for a 6-letter name) fits with room to spare.
        let typical = r"C:\Users\bobbyy\AppData\Local\Personal Manager\mods\11111111-2222-3333-4444-555555555555\0123456789abcdef";
        assert_eq!(path_units(Path::new(typical)), 105);
        assert_eq!(
            check_windows_paths(
                105,
                &m,
                &v,
                ["app/assets/index-0123abcd.js", "manifest.json"]
            ),
            Ok(())
        );
    }

    #[test]
    fn quarantine_stamps_round_trip_and_age_by_their_name() {
        let now = at("2026-11-05T18:18:31Z");
        assert_eq!(stamp(now), "20261105T181831Z");
        let name = format!("{}-atlas.map", stamp(now));
        assert_eq!(quarantined_at(&name), Some(now));
        assert_eq!(quarantined_at(&format!("{name}~3")), Some(now));
        assert!(!quarantine_expired(
            &name,
            now + chrono::Duration::days(30) - chrono::Duration::seconds(1)
        ));
        assert!(quarantine_expired(&name, now + chrono::Duration::days(30)));
        // A stamp in the future (the clock went back) can't prove an age.
        assert!(!quarantine_expired(
            &name,
            now - chrono::Duration::days(400)
        ));
        for theirs in [
            "atlas.map",
            "20261105T181831Z",
            "20261105T181831Zatlas",
            "20261305T181831Z-x",
            "2026110xT181831Z-x",
            "20261105-181831Z-x",
            "notes.txt",
        ] {
            assert_eq!(quarantined_at(theirs), None, "{theirs}");
            assert!(!quarantine_expired(
                theirs,
                now + chrono::Duration::days(9999)
            ));
        }
    }

    #[test]
    fn entries_are_classified_from_the_positive_listing_only() {
        let mut known = BTreeMap::new();
        known.insert("atlas.map".to_owned(), Some(ver("1.1.0")));
        known.insert("acme.broken".to_owned(), None);
        assert_eq!(
            classify(".quarantine", true, &known),
            VaultEntry::Quarantine
        );
        assert_eq!(classify(".DS_Store", false, &known), VaultEntry::Leave);
        assert_eq!(classify(".hidden", true, &known), VaultEntry::Leave);
        assert_eq!(classify("desktop.ini", false, &known), VaultEntry::Leave);
        assert_eq!(
            classify("atlas.map", false, &known),
            VaultEntry::Leave,
            "a link or file is never descended into"
        );
        assert_eq!(classify("acme.gone", true, &known), VaultEntry::Unknown);
        assert_eq!(
            classify("atlas.map", true, &known),
            VaultEntry::Known(Some(&ver("1.1.0")))
        );
        assert_eq!(
            classify("acme.broken", true, &known),
            VaultEntry::Known(None)
        );

        assert!(prune_version("1.0.0", true, Some(&ver("1.1.0"))));
        assert!(!prune_version("1.1.0", true, Some(&ver("1.1.0"))));
        assert!(
            !prune_version("1.0.0", false, Some(&ver("1.1.0"))),
            "a file or link stays"
        );
        assert!(
            !prune_version("1.0.0", true, None),
            "an unknown current version proves nothing stale"
        );
    }

    #[test]
    fn staging_writes_exactly_the_verified_bytes_and_placing_renames_them_in() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path(), VAULT, STORE).unwrap();
        assert!(
            !d.path().join(MODS_DIR).exists(),
            "nothing exists before the first install"
        );
        let fs_ = files(&[
            ("manifest.json", b"{}"),
            ("app/index.html", b"<p>"),
            ("app/js/a.js", b"1"),
        ]);
        let staged = stage(&l, &fs_).unwrap();
        let placed = place(
            &l,
            staged,
            &id("atlas.map"),
            &ver("1.0.0"),
            false,
            Utc::now(),
        )
        .unwrap();
        let dir = placed.dir().to_path_buf();
        placed.keep();
        assert_eq!(fs::read(dir.join("app/js/a.js")).unwrap(), b"1");
        assert_eq!(
            tree(&l.store_dir().join("atlas.map")),
            [
                "1.0.0/",
                "1.0.0/app/",
                "1.0.0/app/index.html",
                "1.0.0/app/js/",
                "1.0.0/app/js/a.js",
                "1.0.0/manifest.json"
            ]
        );
        assert!(
            tree(&l.staging()).is_empty(),
            "the staging folder was renamed away"
        );
    }

    #[cfg(unix)]
    #[test]
    fn writing_never_follows_a_link() {
        let d = tempfile::tempdir().unwrap();
        let outside = d.path().join("outside.txt");
        fs::write(&outside, b"mine").unwrap();
        let link = d.path().join("link");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert!(write_new(&link, b"theirs").is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"mine");
        let dangling = d.path().join("dangling");
        std::os::unix::fs::symlink(d.path().join("nowhere"), &dangling).unwrap();
        assert!(write_new(&dangling, b"x").is_err());
        assert!(!d.path().join("nowhere").exists());
        let dir_link = d.path().join("dir-link");
        std::os::unix::fs::symlink(d.path(), &dir_link).unwrap();
        assert!(make_dir(&dir_link).is_err());
    }

    /// Each write step's failure leaves nothing under `mods/` but empty folders the next sweep or
    /// install reuses, and no staging folder.
    #[test]
    fn a_failed_write_leaves_no_staging_folder() {
        for step in [
            (Step::Write, 1),
            (Step::SyncFile, 0),
            (Step::SyncDir, 0),
            (Step::SyncDir, 2),
        ] {
            let d = tempfile::tempdir().unwrap();
            let l = Layout::new(d.path(), VAULT, STORE).unwrap();
            fail_at(Some(step));
            let got = stage(&l, &files(&[("app/a/x.js", b"1"), ("app/b.js", b"2")]));
            fail_at(None);
            assert!(got.is_err(), "{step:?} fails the stage");
            assert!(
                tree(&l.staging()).is_empty(),
                "{step:?} left {:?}",
                tree(&l.staging())
            );
            assert!(!l.store_dir().exists());
        }
    }

    #[test]
    fn a_failed_rename_puts_back_what_it_moved_aside() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path(), VAULT, STORE).unwrap();
        let (m, v) = (id("atlas.map"), ver("1.0.0"));
        // A fresh install: the mod folder it made goes again.
        let staged = stage(&l, &files(&[("app/x.js", b"new")])).unwrap();
        fail_at(Some((Step::Rename, 0)));
        assert!(place(&l, staged, &m, &v, false, Utc::now()).is_err());
        fail_at(None);
        assert!(!l.mod_dir(&m).exists());
        assert!(tree(&l.staging()).is_empty());
        // A repair: the old folder goes back where it was.
        fs::create_dir_all(l.version_dir(&m, &v).join("app")).unwrap();
        fs::write(l.version_dir(&m, &v).join("app/x.js"), b"old").unwrap();
        let staged = stage(&l, &files(&[("app/x.js", b"new")])).unwrap();
        fail_at(Some((Step::Rename, 0)));
        assert!(place(&l, staged, &m, &v, true, Utc::now()).is_err());
        fail_at(None);
        assert_eq!(
            fs::read(l.version_dir(&m, &v).join("app/x.js")).unwrap(),
            b"old"
        );
        assert!(
            tree(&l.quarantine()).is_empty(),
            "{:?}",
            tree(&l.quarantine())
        );
    }

    #[test]
    fn a_placement_not_kept_rolls_back_into_nothing() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path(), VAULT, STORE).unwrap();
        let (m, v) = (id("atlas.map"), ver("1.0.0"));
        // A sync failure after the rename is a rollback too.
        let staged = stage(&l, &files(&[("app/x.js", b"new")])).unwrap();
        fail_at(Some((Step::SyncDir, 0)));
        assert!(place(&l, staged, &m, &v, false, Utc::now()).is_err());
        fail_at(None);
        assert!(!l.mod_dir(&m).exists());
        // A dropped placement (the row didn't commit) over a repair restores the old folder.
        fs::create_dir_all(l.version_dir(&m, &v)).unwrap();
        fs::write(l.version_dir(&m, &v).join("old.txt"), b"old").unwrap();
        let staged = stage(&l, &files(&[("app/x.js", b"new")])).unwrap();
        let placed = place(&l, staged, &m, &v, true, Utc::now()).unwrap();
        assert!(l.version_dir(&m, &v).join("app/x.js").exists());
        drop(placed);
        assert_eq!(tree(&l.mod_dir(&m)), ["1.0.0/", "1.0.0/old.txt"]);
        assert!(tree(&l.quarantine()).is_empty());
        assert!(tree(&l.staging()).is_empty());
        // Over a folder no row names (not a repair), it stays in quarantine: the rollback leaves
        // nothing at the version path for a row to be missing.
        let staged = stage(&l, &files(&[("app/x.js", b"new")])).unwrap();
        let placed = place(&l, staged, &m, &v, false, at("2026-11-05T10:00:00Z")).unwrap();
        drop(placed);
        assert!(!l.mod_dir(&m).join("1.0.0").exists());
        assert_eq!(
            tree(&l.quarantine()),
            [
                "20261105T100000Z-atlas.map/",
                "20261105T100000Z-atlas.map/1.0.0/",
                "20261105T100000Z-atlas.map/1.0.0/old.txt"
            ]
        );
    }

    #[test]
    fn a_folder_already_at_the_target_is_quarantined_not_merged() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path(), VAULT, STORE).unwrap();
        let (m, v) = (id("atlas.map"), ver("1.0.0"));
        fs::create_dir_all(l.version_dir(&m, &v)).unwrap();
        fs::write(l.version_dir(&m, &v).join("stray.js"), b"?").unwrap();
        let now = at("2026-11-05T10:00:00Z");
        let placed = place(
            &l,
            stage(&l, &files(&[("app/x.js", b"1")])).unwrap(),
            &m,
            &v,
            false,
            now,
        )
        .unwrap();
        placed.keep();
        assert_eq!(
            tree(&l.mod_dir(&m)),
            ["1.0.0/", "1.0.0/app/", "1.0.0/app/x.js"]
        );
        assert_eq!(
            tree(&l.quarantine()),
            [
                "20261105T100000Z-atlas.map/",
                "20261105T100000Z-atlas.map/1.0.0/",
                "20261105T100000Z-atlas.map/1.0.0/stray.js"
            ]
        );
    }

    #[test]
    fn the_sweep_quarantines_and_its_expiry_clears_staging_and_old_quarantine() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path(), VAULT, STORE).unwrap();
        let other = Layout::new(d.path(), "another-vault", STORE).unwrap();
        let sibling = Layout::new(d.path(), VAULT, "fedcba9876543210").unwrap();
        let mk = |p: PathBuf| {
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, b"x").unwrap();
        };
        mk(l.store_dir().join("atlas.map/1.0.0/app/x.js"));
        mk(l.store_dir().join("atlas.map/1.1.0/app/x.js"));
        mk(l.store_dir().join("atlas.map/notes.txt"));
        mk(l.store_dir().join("acme.broken/0.1.0/app/x.js"));
        mk(l.store_dir().join("acme.gone/2.0.0/app/x.js"));
        mk(l.store_dir().join("desktop.ini"));
        mk(l.store_dir().join(".DS_Store"));
        mk(l.quarantine().join("20261001T000000Z-acme.old/1.0.0/x"));
        mk(l.quarantine().join("20261020T000000Z-acme.young/1.0.0/x"));
        mk(l.quarantine().join("someone-elses"));
        mk(l.staging().join("0011223344556677/app/x.js"));
        mk(other.store_dir().join("acme.gone/2.0.0/app/x.js"));
        mk(sibling.store_dir().join("acme.gone/2.0.0/app/x.js"));
        mk(sibling.store_dir().join("atlas.map/1.0.0/app/x.js"));
        let mut known = BTreeMap::new();
        known.insert("atlas.map".to_owned(), Some(ver("1.1.0")));
        known.insert("acme.broken".to_owned(), None);

        let now = at("2026-11-05T12:00:00Z");
        let mut report = sweep(&l, &known, now);
        report.quarantined.sort();
        assert_eq!(report.quarantined, ["acme.gone", "atlas.map/1.0.0"]);
        assert!(report.expired.is_empty() && report.staging_cleared == 0);
        assert!(report.failed.is_empty());
        let report = expire(&l, now);
        assert_eq!(report.expired, ["20261001T000000Z-acme.old"]);
        assert_eq!(report.staging_cleared, 1);
        assert!(report.quarantined.is_empty() && report.failed.is_empty());
        assert_eq!(
            tree(l.store_dir()),
            [
                ".DS_Store",
                ".quarantine/",
                ".quarantine/20261020T000000Z-acme.young/",
                ".quarantine/20261020T000000Z-acme.young/1.0.0/",
                ".quarantine/20261020T000000Z-acme.young/1.0.0/x",
                ".quarantine/20261105T120000Z-acme.gone/",
                ".quarantine/20261105T120000Z-acme.gone/2.0.0/",
                ".quarantine/20261105T120000Z-acme.gone/2.0.0/app/",
                ".quarantine/20261105T120000Z-acme.gone/2.0.0/app/x.js",
                // A version its row doesn't name is moved aside too, never deleted on sight.
                ".quarantine/20261105T120000Z-atlas.map/",
                ".quarantine/20261105T120000Z-atlas.map/1.0.0/",
                ".quarantine/20261105T120000Z-atlas.map/1.0.0/app/",
                ".quarantine/20261105T120000Z-atlas.map/1.0.0/app/x.js",
                ".quarantine/someone-elses",
                "acme.broken/",
                "acme.broken/0.1.0/",
                "acme.broken/0.1.0/app/",
                "acme.broken/0.1.0/app/x.js",
                "atlas.map/",
                "atlas.map/1.1.0/",
                "atlas.map/1.1.0/app/",
                "atlas.map/1.1.0/app/x.js",
                "atlas.map/notes.txt",
                "desktop.ini",
            ]
        );
        assert!(tree(&l.staging()).is_empty());
        assert_eq!(
            tree(other.store_dir()).len(),
            4,
            "another vault's folder is never touched"
        );
        assert_eq!(
            tree(sibling.store_dir()).len(),
            8,
            "nor another store's of this vault (a restored copy beside its original)"
        );
        // Quarantining one name twice in a second keeps both.
        mk(l.store_dir().join("acme.gone/2.0.1/x"));
        assert_eq!(sweep(&l, &known, now).quarantined, ["acme.gone"]);
        assert!(l
            .quarantine()
            .join("20261105T120000Z-acme.gone~1/2.0.1/x")
            .exists());
    }

    #[cfg(unix)]
    #[test]
    fn the_sweep_never_follows_a_link() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path(), VAULT, STORE).unwrap();
        let outside = d.path().join("outside");
        fs::create_dir_all(outside.join("9.9.9")).unwrap();
        fs::create_dir_all(l.store_dir()).unwrap();
        std::os::unix::fs::symlink(&outside, l.store_dir().join("atlas.map")).unwrap();
        std::os::unix::fs::symlink(&outside, l.store_dir().join("acme.gone")).unwrap();
        let mut known = BTreeMap::new();
        known.insert("atlas.map".to_owned(), Some(ver("1.0.0")));
        let report = sweep(&l, &known, Utc::now());
        assert!(report.is_empty(), "{report:?}");
        assert!(outside.join("9.9.9").exists());
    }

    #[test]
    fn a_sweep_with_no_mods_folder_creates_nothing() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path(), VAULT, STORE).unwrap();
        assert!(sweep(&l, &BTreeMap::new(), Utc::now()).is_empty());
        assert!(expire(&l, Utc::now()).is_empty());
        assert!(!d.path().join(MODS_DIR).exists());
    }

    /// The files a row lists, as `recover` checks them.
    fn served(list: &[(&str, &[u8])]) -> Vec<(String, ServedFile)> {
        list.iter()
            .map(|(p, b)| {
                let f = ServedFile {
                    sha256: Sha256::digest(b).into(),
                    size: b.len() as u64,
                };
                (p.to_string(), f)
            })
            .collect()
    }

    /// A quarantined version comes back only when it is exactly what the row lists, newest first,
    /// from this mod's entries only, and a partial folder at the target goes aside in its place.
    #[test]
    fn recovery_puts_back_only_an_exact_copy_of_the_rows_files() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path(), VAULT, STORE).unwrap();
        let (m, v) = (id("atlas.map"), ver("1.0.0"));
        let want = served(&[("manifest.json", b"{}"), ("app/main.js", b"map")]);
        let put = |entry: &str, main: &[u8]| {
            let dir = l.quarantine().join(entry).join("1.0.0");
            fs::create_dir_all(dir.join("app")).unwrap();
            fs::write(dir.join("manifest.json"), b"{}").unwrap();
            fs::write(dir.join("app/main.js"), main).unwrap();
        };
        let now = at("2026-11-05T12:00:00Z");
        assert_eq!(
            recover(&l, &m, &v, &want, now).unwrap(),
            None,
            "no quarantine"
        );
        // Same size, other bytes; another mod's entry; another version; a name PM didn't make.
        put("20261101T000000Z-atlas.map", b"pam");
        put("20261102T000000Z-atlas.mapper", b"map");
        put("20261102T000000Z-xatlas.map", b"map");
        put("someone-elses-atlas.map", b"map");
        fs::create_dir_all(l.quarantine().join("20261103T000000Z-atlas.map/0.9.0")).unwrap();
        assert_eq!(recover(&l, &m, &v, &want, now).unwrap(), None);
        // Two good copies: the newer one comes back, the older stays; a partial folder at the
        // target is moved aside first.
        put("20261102T000000Z-atlas.map~1", b"map");
        put("20261104T000000Z-atlas.map", b"map");
        fs::create_dir_all(l.version_dir(&m, &v)).unwrap();
        fs::write(l.version_dir(&m, &v).join("manifest.json"), b"{}").unwrap();
        assert_eq!(
            recover(&l, &m, &v, &want, now).unwrap().as_deref(),
            Some("20261104T000000Z-atlas.map")
        );
        assert_eq!(
            tree(&l.version_dir(&m, &v)),
            ["app/", "app/main.js", "manifest.json"]
        );
        assert_eq!(
            fs::read(l.version_dir(&m, &v).join("app/main.js")).unwrap(),
            b"map"
        );
        let q = tree(&l.quarantine());
        assert!(
            !q.iter()
                .any(|p| p.starts_with("20261104T000000Z-atlas.map/")),
            "{q:?}"
        );
        assert!(q.contains(&"20261102T000000Z-atlas.map~1/1.0.0/app/main.js".to_owned()));
        assert!(
            q.contains(&"20261105T120000Z-atlas.map/1.0.0/manifest.json".to_owned()),
            "{q:?}"
        );
        assert!(q.contains(&"20261101T000000Z-atlas.map/1.0.0/app/main.js".to_owned()));
    }

    #[cfg(unix)]
    #[test]
    fn recovery_never_takes_a_copy_through_a_link() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path(), VAULT, STORE).unwrap();
        let (m, v) = (id("atlas.map"), ver("1.0.0"));
        let want = served(&[("app/main.js", b"map")]);
        let outside = d.path().join("outside");
        fs::create_dir_all(outside.join("1.0.0/app")).unwrap();
        fs::write(outside.join("1.0.0/app/main.js"), b"map").unwrap();
        fs::create_dir_all(l.quarantine().join("20261101T000000Z-atlas.map/1.0.0")).unwrap();
        std::os::unix::fs::symlink(
            outside.join("1.0.0/app"),
            l.quarantine().join("20261101T000000Z-atlas.map/1.0.0/app"),
        )
        .unwrap();
        std::os::unix::fs::symlink(&outside, l.quarantine().join("20261102T000000Z-atlas.map"))
            .unwrap();
        let now = at("2026-11-05T12:00:00Z");
        assert_eq!(recover(&l, &m, &v, &want, now).unwrap(), None);
        assert!(!l.mod_dir(&m).exists());
        assert!(outside.join("1.0.0/app/main.js").exists());
    }

    /// With no copy of its own, a store copies an exact one from another store of its vault (a
    /// restored copy from the store it was made from): that store's installed folder first, then its
    /// quarantine, newest first. Never another vault's, never a folder PM didn't name, never an
    /// inexact copy, and the source is left exactly as it was.
    #[test]
    fn recovery_copies_an_exact_copy_from_another_store_of_the_vault() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path(), VAULT, STORE).unwrap();
        let (m, v) = (id("atlas.map"), ver("1.0.0"));
        let want = served(&[("manifest.json", b"{}"), ("app/main.js", b"map")]);
        let put = |dir: &Path, main: &[u8]| {
            fs::create_dir_all(dir.join("app")).unwrap();
            fs::write(dir.join("manifest.json"), b"{}").unwrap();
            fs::write(dir.join("app/main.js"), main).unwrap();
        };
        let now = at("2026-11-05T12:00:00Z");
        let vault = l.vault_dir().to_path_buf();
        put(&vault.join("not-a-store-id/atlas.map/1.0.0"), b"map");
        put(
            &d.path()
                .join("mods/another-vault/1111111111111111/atlas.map/1.0.0"),
            b"map",
        );
        put(&vault.join("1111111111111111/atlas.map/1.0.0"), b"pam");
        put(&vault.join("1111111111111111/atlas.map/0.9.0"), b"map");
        #[cfg(unix)]
        {
            let elsewhere = d.path().join("elsewhere");
            put(&elsewhere.join("atlas.map/1.0.0"), b"map");
            std::os::unix::fs::symlink(&elsewhere, vault.join("0000000000000000")).unwrap();
        }
        assert_eq!(recover(&l, &m, &v, &want, now).unwrap(), None);
        assert!(!l.version_dir(&m, &v).exists());

        // Another store's quarantine: the newest exact copy.
        let sibling = vault.join("2222222222222222");
        put(
            &sibling.join(".quarantine/20261101T000000Z-atlas.map/1.0.0"),
            b"map",
        );
        put(
            &sibling.join(".quarantine/20261102T000000Z-atlas.map/1.0.0"),
            b"map",
        );
        let before = tree(&sibling);
        assert_eq!(
            recover(&l, &m, &v, &want, now).unwrap().as_deref(),
            Some("2222222222222222/.quarantine/20261102T000000Z-atlas.map")
        );
        assert_eq!(
            tree(&l.version_dir(&m, &v)),
            ["app/", "app/main.js", "manifest.json"]
        );
        assert_eq!(tree(&sibling), before, "copied, never moved");
        assert!(fs::read_dir(l.staging()).unwrap().next().is_none());

        // Its installed folder comes before its quarantine, and a partial folder at the target is
        // moved into this store's quarantine first.
        put(&sibling.join("atlas.map/1.0.0"), b"map");
        fs::remove_file(l.version_dir(&m, &v).join("app/main.js")).unwrap();
        assert_eq!(
            recover(&l, &m, &v, &want, now).unwrap().as_deref(),
            Some("2222222222222222/atlas.map/1.0.0")
        );
        assert_eq!(
            fs::read(l.version_dir(&m, &v).join("app/main.js")).unwrap(),
            b"map"
        );
        assert_eq!(
            tree(&l.quarantine()),
            [
                "20261105T120000Z-atlas.map/",
                "20261105T120000Z-atlas.map/1.0.0/",
                "20261105T120000Z-atlas.map/1.0.0/app/",
                "20261105T120000Z-atlas.map/1.0.0/manifest.json"
            ]
        );
        // This store's own quarantine is looked in first, and takes its copy back by a rename.
        fs::remove_file(l.version_dir(&m, &v).join("app/main.js")).unwrap();
        put(
            &l.quarantine().join("20261104T000000Z-atlas.map/1.0.0"),
            b"map",
        );
        assert_eq!(
            recover(&l, &m, &v, &want, now).unwrap().as_deref(),
            Some("20261104T000000Z-atlas.map")
        );
    }

    /// A deleted store's folders go, the open store's and other vaults' stay.
    #[test]
    fn removing_deleted_stores_keeps_the_open_one() {
        let d = tempfile::tempdir().unwrap();
        let open = Layout::new(d.path(), VAULT, STORE).unwrap();
        let gone = Layout::new(d.path(), VAULT, "fedcba9876543210").unwrap();
        let other = Layout::new(d.path(), "another-vault", STORE).unwrap();
        for l in [&open, &gone, &other] {
            fs::create_dir_all(l.version_dir(&id("atlas.map"), &ver("1.0.0"))).unwrap();
        }
        remove_stores(d.path(), "../..", None).unwrap();
        remove_stores(d.path(), VAULT, Some(STORE)).unwrap();
        assert!(!gone.store_dir().exists());
        assert!(open.store_dir().exists() && other.store_dir().exists());
        remove_stores(d.path(), "another-vault", None).unwrap();
        assert!(!other.vault_dir().exists());
        assert!(open.store_dir().exists());
        remove_stores(d.path(), VAULT, None).unwrap();
        assert!(
            !d.path().join(MODS_DIR).exists(),
            "and tidies an emptied mods folder"
        );
        remove_stores(d.path(), VAULT, None).unwrap();
    }

    #[test]
    fn pruning_and_removal_keep_to_their_mod_and_tidy_empty_folders() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path(), VAULT, STORE).unwrap();
        let m = id("atlas.map");
        for v in ["1.0.0", "1.1.0", "1.2.0"] {
            fs::create_dir_all(l.version_dir(&m, &ver(v))).unwrap();
        }
        fs::create_dir_all(l.mod_dir(&id("acme.tool")).join("0.1.0")).unwrap();
        let mut pruned = prune_versions(&l, &m, &ver("1.2.0"));
        pruned.sort();
        assert_eq!(pruned, ["1.0.0", "1.1.0"]);
        assert_eq!(tree(&l.mod_dir(&m)), ["1.2.0/"]);
        remove_mod(&l, &m).unwrap();
        assert!(!l.mod_dir(&m).exists());
        assert!(l.store_dir().exists(), "another mod is still installed");
        remove_mod(&l, &id("acme.tool")).unwrap();
        assert!(
            !d.path().join(MODS_DIR).exists(),
            "the last uninstall leaves no mods folder"
        );
        remove_mod(&l, &m).unwrap();
    }
}
