// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pre-flight validation and effective-access probes for a shared-vault move (the
//! verify-then-commit half of the ACL-lockout fix).
//!
//! Windows evaluates a folder's DACL at handle-OPEN time, so the only honest way to know
//! a lockdown left the CURRENT process able to use a folder is to open fresh handles into
//! it and see. Two layers use that fact:
//!
//!  1. [`preflight_share_target`] runs BEFORE the migration mutates anything — it rejects
//!     locations a shared vault can't live on (network/removable-non-NTFS, spec §498) and
//!     dress-rehearses the exact `icacls` lockdown in a disposable subfolder, so a machine
//!     whose policy would strand the owner fails with nothing lost.
//!  2. [`probe_vault_access`] runs INSIDE the move, after the lockdown is applied to the
//!     real target but BEFORE the pointer commits — the last checkpoint at which an abort
//!     is free (source intact, pointer unmoved).
//!
//! The probe file/dirs are created and deleted; nothing here persists. Pure helpers
//! ([`classify_path`], [`judge_volume`]) are split out so the policy unit-tests without a
//! filesystem or the Windows API.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result, VaultFault, VaultFaultCode};

/// The distinct handle-open a probe attempts, named so an abort error points at the exact
/// operation that was refused rather than a bare "os error 5".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeStep {
    /// Create/write/read/delete a probe file in the vault root.
    CreateInRoot,
    /// The same, in the `vault/` Markdown subfolder.
    CreateInMarkdown,
    /// Read `vault-meta.json`.
    ReadMeta,
    /// Open `pm.sqlite` for read+write (no truncate — a DACL check, not a write).
    OpenDbReadWrite,
}

impl ProbeStep {
    fn describe(self) -> &'static str {
        match self {
            ProbeStep::CreateInRoot => "write to the vault folder",
            ProbeStep::CreateInMarkdown => "write to the vault's notes folder",
            ProbeStep::ReadMeta => "read the vault's settings",
            ProbeStep::OpenDbReadWrite => "open the vault's database",
        }
    }
}

/// One failed effective-access check: which handle-open failed, where, and the OS error.
#[derive(Debug)]
pub struct ProbeFailure {
    pub step: ProbeStep,
    pub path: PathBuf,
    pub source: std::io::Error,
}

impl From<ProbeFailure> for Error {
    fn from(f: ProbeFailure) -> Self {
        Error::Vault(VaultFault {
            code: VaultFaultCode::Denied,
            op: "verify access to the vault folder".into(),
            path: Some(f.path.display().to_string()),
            message: format!(
                "PM couldn't {} at {} ({}) — the folder's permissions would leave this \
                 account unable to use the vault there.",
                f.step.describe(),
                f.path.display(),
                f.source
            ),
        })
    }
}

const PROBE_FILENAME: &str = ".pm-access-probe";

/// Create, write, read back, and delete a probe file in `dir` — the exact shape of PM's
/// own future writes there. `dir` must already exist. Any step failing yields the
/// `ProbeFailure` with `step`.
fn probe_one(dir: &Path, step: ProbeStep) -> std::result::Result<(), ProbeFailure> {
    let probe = dir.join(PROBE_FILENAME);
    let fail = |e: std::io::Error| ProbeFailure {
        step,
        path: dir.to_path_buf(),
        source: e,
    };
    std::fs::write(&probe, b"pm-access-probe").map_err(fail)?;
    let read_back = std::fs::read(&probe).map_err(fail);
    // Always try to clean up, even if the read failed.
    let _ = std::fs::remove_file(&probe);
    read_back?;
    Ok(())
}

/// Write-probe a single directory (create/write/read/delete a probe file) — the cheap
/// "can this process still open handles here?" check the migration runs on the near-empty
/// destination right after the lockdown, before copying a possibly-large vault into it.
pub fn probe_dir_access(dir: &Path) -> std::result::Result<(), ProbeFailure> {
    probe_one(dir, ProbeStep::CreateInRoot)
}

/// Read-access probe: `fs::read` on a file, tolerating "not there yet" (the probe is about
/// ACCESS, not existence — a fresh share may not have every file at probe time).
fn probe_read(path: &Path, step: ProbeStep) -> std::result::Result<(), ProbeFailure> {
    match std::fs::read(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ProbeFailure {
            step,
            path: path.to_path_buf(),
            source: e,
        }),
    }
}

/// The full owner probe against a populated vault root: write-probe the root and its
/// `vault/` subdir, read `vault-meta.json`, and open `pm.sqlite` read+write (no truncate).
/// Run after the lockdown, before the pointer commits — the honest "can this process still
/// use the folder?" test that a stripped-inheritance DACL would fail.
pub fn probe_vault_access(root: &Path) -> std::result::Result<(), ProbeFailure> {
    probe_one(root, ProbeStep::CreateInRoot)?;
    let markdown = root.join("vault");
    if markdown.is_dir() {
        probe_one(&markdown, ProbeStep::CreateInMarkdown)?;
    }
    probe_read(&root.join(super::META_FILENAME), ProbeStep::ReadMeta)?;
    let db = root.join("pm.sqlite");
    if db.exists() {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&db)
            .map_err(|e| ProbeFailure {
                step: ProbeStep::OpenDbReadWrite,
                path: db.clone(),
                source: e,
            })?;
    }
    Ok(())
}

/// The disposable rehearsal subfolder name (self-healing: swept before each rehearsal).
const REHEARSAL_DIR: &str = ".pm-preflight";

/// Dress-rehearse the exact lockdown a share will apply, inside a throwaway subfolder of
/// the real target (with a nested child, to prove inheritable ACEs reach descendants):
/// `restrict_to_owner`, probe both levels, then reset + delete. Any leftover from a crashed
/// run is swept first. Nothing about the real vault is touched.
pub fn rehearse_lockdown(target: &Path) -> Result<()> {
    let dir = target.join(REHEARSAL_DIR);
    // Self-heal a crashed prior rehearsal before reusing the path.
    let _ = super::acl::reset_inheritance(&dir);
    let _ = std::fs::remove_dir_all(&dir);

    let child = dir.join("child");
    std::fs::create_dir_all(&child)
        .map_err(crate::error::io_at("prepare the vault folder", &child))?;

    // Run the rehearsal, capturing the first failure so cleanup always happens.
    let outcome = (|| -> Result<()> {
        super::acl::restrict_to_owner(&dir, &[])?;
        probe_one(&dir, ProbeStep::CreateInRoot)?;
        probe_one(&child, ProbeStep::CreateInMarkdown)?;
        Ok(())
    })();

    // Cleanup: reset the DACL so we can delete what we just locked down, then remove it.
    let _ = super::acl::reset_inheritance(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    outcome
}

/// How a target path is shaped, for the local-only volume policy (spec §498).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathClass {
    /// A UNC path (`\\server\share\…` or `\\?\UNC\…`) — a network location, rejected.
    Unc,
    /// A drive-letter path (`C:\…`) — the normal local case; check its volume.
    Drive,
    /// Anything else (relative, or a verbatim drive `\\?\C:\…`) — checked by volume too.
    Other,
}

/// Pure prefix classification — no filesystem access, so it unit-tests. Verbatim-UNC
/// (`\\?\UNC\…`) and plain UNC both classify as [`PathClass::Unc`].
pub fn classify_path(path: &Path) -> PathClass {
    let s = path.to_string_lossy();
    let s = s.as_ref();
    if s.starts_with(r"\\?\UNC\") || s.starts_with(r"\\.\UNC\") {
        return PathClass::Unc;
    }
    if let Some(rest) = s.strip_prefix(r"\\") {
        // `\\server\share` is UNC; `\\?\C:\` (verbatim drive) is not.
        if !rest.starts_with(r"?\") && !rest.starts_with(r".\") {
            return PathClass::Unc;
        }
    }
    // A drive-letter path: second char ':' with an ASCII-alpha first char.
    let mut chars = s.chars();
    if let (Some(a), Some(b)) = (chars.next(), chars.next()) {
        if a.is_ascii_alphabetic() && b == ':' {
            return PathClass::Drive;
        }
    }
    PathClass::Other
}

/// The verdict on a target volume — either usable, or rejected with a user-facing reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VolumeVerdict {
    Ok,
    Reject(String),
}

// Windows `GetDriveTypeW` return values (kept as our own constants so `judge_volume` is
// pure and testable without the `windows` crate).
const DRIVE_UNKNOWN: u32 = 0;
const DRIVE_NO_ROOT_DIR: u32 = 1;
const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOTE: u32 = 4;
const DRIVE_CDROM: u32 = 5;
const DRIVE_RAMDISK: u32 = 6;
/// `FILE_PERSISTENT_ACLS` in the volume's filesystem flags — set on NTFS/ReFS, clear on
/// FAT32/exFAT. Its absence means the folder can't carry the per-account ACLs a shared
/// vault relies on.
const FILE_PERSISTENT_ACLS: u32 = 0x0000_0008;

/// Pure volume policy (the `boot_meta_decision` pattern: raw observations in, verdict
/// out). Rejects network/CD/unknown drive types outright; for a usable drive type,
/// rejects a filesystem that can't hold ACLs (FAT32/exFAT) when the flags are known.
/// Unknown flags pass — the rehearsal + probe are the real gate; this is the fast fail.
pub fn judge_volume(class: PathClass, drive_type: u32, fs_flags: Option<u32>) -> VolumeVerdict {
    if class == PathClass::Unc {
        return VolumeVerdict::Reject(
            "a shared vault can't live on a network location — pick a folder on a local drive \
             (network shares aren't supported)"
                .into(),
        );
    }
    match drive_type {
        DRIVE_REMOTE => {
            return VolumeVerdict::Reject(
                "that folder is on a network drive, which shared vaults don't support — pick a \
                 folder on a local drive"
                    .into(),
            )
        }
        DRIVE_CDROM => return VolumeVerdict::Reject("that folder is on read-only media".into()),
        DRIVE_UNKNOWN | DRIVE_NO_ROOT_DIR => {
            return VolumeVerdict::Reject(
                "PM couldn't identify that drive — pick a folder on a local disk".into(),
            )
        }
        DRIVE_FIXED | DRIVE_REMOVABLE | DRIVE_RAMDISK => {}
        _ => {}
    }
    if let Some(flags) = fs_flags {
        if flags & FILE_PERSISTENT_ACLS == 0 {
            return VolumeVerdict::Reject(
                "that drive's filesystem (e.g. FAT32/exFAT) can't store the per-account \
                 permissions a shared vault needs — use an NTFS drive, or the suggested \
                 location"
                    .into(),
            );
        }
    }
    VolumeVerdict::Ok
}

/// Observe a path's volume (drive type + filesystem flags) via the Win32 API. The volume
/// root is the path's top ancestor (`C:\` for `C:\ProgramData\…`). Non-Windows returns
/// `None` — `judge_volume` then relies on the drive type alone (which is also `None` there,
/// so the POSIX rehearsal is the gate).
#[cfg(windows)]
fn observe_volume(path: &Path) -> Option<(u32, Option<u32>)> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumeInformationW};

    let root = path.ancestors().last()?; // e.g. "C:\"
    let mut wide: Vec<u16> = root.as_os_str().encode_wide().collect();
    if wide.last() != Some(&0) {
        wide.push(0);
    }
    let root_pcwstr = PCWSTR(wide.as_ptr());
    // SAFETY: `wide` is a valid, null-terminated UTF-16 buffer that outlives the calls.
    let drive_type = unsafe { GetDriveTypeW(root_pcwstr) };
    let mut fs_flags: u32 = 0;
    let flags =
        unsafe { GetVolumeInformationW(root_pcwstr, None, None, None, Some(&mut fs_flags), None) }
            .is_ok()
            .then_some(fs_flags);
    Some((drive_type, flags))
}

#[cfg(not(windows))]
fn observe_volume(_path: &Path) -> Option<(u32, Option<u32>)> {
    None
}

/// Fail-fast gate before a share migration mutates anything: reject a location a shared
/// vault can't live on (network / non-ACL filesystem), then dress-rehearse the real
/// lockdown in a throwaway subfolder. On success the target is proven usable; on failure
/// nothing has been touched. A `target` we create solely for the rehearsal and leave empty
/// is removed again, so a rejected pre-flight leaves no trace.
pub fn preflight_share_target(target: &Path) -> Result<()> {
    let class = classify_path(target);
    let (drive_type, fs_flags) = observe_volume(target).unwrap_or((DRIVE_UNKNOWN, None));
    // Off-Windows there's no drive type to judge; skip straight to the rehearsal (the POSIX
    // chmod-700 rehearsal is the real check there). On Windows, judge the volume first.
    if cfg!(windows) {
        if let VolumeVerdict::Reject(reason) = judge_volume(class, drive_type, fs_flags) {
            return Err(Error::Vault(VaultFault {
                code: VaultFaultCode::Other,
                op: "check the shared-vault location".into(),
                path: Some(target.display().to_string()),
                message: reason,
            }));
        }
    } else if class == PathClass::Unc {
        return Err(Error::Other(
            "a shared vault can't live on a network location — pick a folder on a local drive"
                .into(),
        ));
    }

    // The rehearsal needs the target to exist; note whether we created it so a rejected
    // pre-flight can remove an empty dir it made (leaving a user's existing folder alone).
    let created = !target.exists();
    std::fs::create_dir_all(target)
        .map_err(crate::error::io_at("prepare the vault folder", target))?;
    let outcome = rehearse_lockdown(target);
    if outcome.is_err() && created {
        // Only remove what we made, and only if it's still empty (the rehearsal cleaned up
        // after itself, so it should be).
        if let Ok(mut entries) = std::fs::read_dir(target) {
            if entries.next().is_none() {
                let _ = std::fs::remove_dir(target);
            }
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_path_distinguishes_unc_drive_and_other() {
        assert_eq!(
            classify_path(Path::new(r"\\server\share\vault")),
            PathClass::Unc
        );
        assert_eq!(
            classify_path(Path::new(r"\\?\UNC\server\share")),
            PathClass::Unc
        );
        assert_eq!(
            classify_path(Path::new(r"C:\ProgramData\PM")),
            PathClass::Drive
        );
        assert_eq!(
            classify_path(Path::new(r"\\?\C:\verbatim")),
            PathClass::Other
        );
        assert_eq!(classify_path(Path::new("relative/path")), PathClass::Other);
    }

    #[test]
    fn judge_volume_rejects_network_and_non_acl_filesystems() {
        // UNC is rejected whatever the (unknowable) drive type.
        assert!(matches!(
            judge_volume(PathClass::Unc, DRIVE_UNKNOWN, None),
            VolumeVerdict::Reject(_)
        ));
        // A network drive letter is rejected.
        assert!(matches!(
            judge_volume(PathClass::Drive, DRIVE_REMOTE, Some(FILE_PERSISTENT_ACLS)),
            VolumeVerdict::Reject(_)
        ));
        // A fixed NTFS drive (ACL flag set) is fine.
        assert_eq!(
            judge_volume(PathClass::Drive, DRIVE_FIXED, Some(FILE_PERSISTENT_ACLS)),
            VolumeVerdict::Ok
        );
        // A removable FAT32 stick (ACL flag clear) is rejected.
        assert!(matches!(
            judge_volume(PathClass::Drive, DRIVE_REMOVABLE, Some(0)),
            VolumeVerdict::Reject(_)
        ));
        // Removable NTFS is allowed (the unplugged case is already recoverable).
        assert_eq!(
            judge_volume(
                PathClass::Drive,
                DRIVE_REMOVABLE,
                Some(FILE_PERSISTENT_ACLS)
            ),
            VolumeVerdict::Ok
        );
        // Unknown flags pass — the rehearsal/probe is the real gate.
        assert_eq!(
            judge_volume(PathClass::Drive, DRIVE_FIXED, None),
            VolumeVerdict::Ok
        );
        // CD-ROM and unidentifiable drives are rejected.
        assert!(matches!(
            judge_volume(PathClass::Drive, DRIVE_CDROM, None),
            VolumeVerdict::Reject(_)
        ));
        assert!(matches!(
            judge_volume(PathClass::Other, DRIVE_NO_ROOT_DIR, None),
            VolumeVerdict::Reject(_)
        ));
    }

    #[test]
    fn probe_vault_access_passes_on_a_writable_tempdir() {
        // On a normal writable temp dir (any OS) the probe round-trips cleanly — the
        // happy path the migration relies on.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("vault")).unwrap();
        probe_vault_access(dir.path()).expect("probe should pass on a writable dir");
        // And it leaves no probe files behind.
        assert!(!dir.path().join(PROBE_FILENAME).exists());
        assert!(!dir.path().join("vault").join(PROBE_FILENAME).exists());
    }

    #[test]
    fn rehearse_lockdown_leaves_no_residue() {
        let dir = tempfile::tempdir().unwrap();
        // On platforms with a real lockdown this exercises icacls/setfacl; on macOS the
        // restrict_to_owner stub errors, which is a legitimate rehearsal failure — either
        // way, no rehearsal folder may survive.
        let _ = rehearse_lockdown(dir.path());
        assert!(!dir.path().join(REHEARSAL_DIR).exists());
    }
}
