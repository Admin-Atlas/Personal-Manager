// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared-folder access control (spec §5). When a vault lives in a shared location
//! (e.g. `C:\ProgramData\org.itsatlas.pm\<vault>`), file-system ACLs stop *other*
//! standard accounts on the machine from reading the raw files. This is defence in
//! depth: the real protection is Markdown-at-rest encryption ([`super::crypto`]) plus
//! the SQLCipher DB key, so a failure here is a warning, never fatal — a shared vault
//! is still safe, just not additionally OS-isolated.
//!
//! Windows uses `icacls`: strip inherited ACEs and grant Full, inheritable access to
//! the current user, the Administrators group, and any explicitly linked accounts.
//! Linux uses plain POSIX permissions plus ACLs: `chmod 700` on the vault root denies
//! traversal to every other account (children are unreachable regardless of their own
//! modes), and `setfacl` re-admits explicitly linked accounts. macOS is a flagged stub
//! (shared vaults are weaker there; the UI says so).
//!
//! As of the verify-then-commit change (the ACL-lockout fix), the share migration runs
//! the owner lockdown BEFORE it commits the move and probes effective access afterwards
//! ([`super::preflight`]), so a lockdown that would strand the owner aborts the move with
//! nothing lost — instead of the old post-commit warning that let a stripped-inheritance
//! DACL brick the vault silently. The Windows `run_icacls` therefore fails loud now (no
//! `/C`), and [`reset_inheritance`] / [`verify_grant`] back the abort and repair paths.

/// The outcome of reading back whether a principal's grant landed on a folder — see
/// [`platform::verify_grant`]. Not an effective-access proof (can't run as the other
/// account); `Inconclusive` never blocks a link that may well have worked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantCheck {
    /// The principal has an explicit ACE on the folder.
    Granted,
    /// The readback ran cleanly but found no ACE for the principal.
    NotFound,
    /// The readback itself couldn't be trusted (spawn failure, unexpected output).
    Inconclusive(String),
}

/// Whether this platform actually applies a shared-folder lockdown — true on Windows
/// (icacls) and Linux (chmod 700 + setfacl), false on macOS (a flagged stub that always
/// errors). The migration gates FATAL lockdown-before-commit on this: on macOS the
/// lockdown stays best-effort (there's nothing to enforce), so a share never fails there
/// for want of an ACL primitive that doesn't exist.
pub const fn lockdown_supported() -> bool {
    cfg!(any(windows, target_os = "linux"))
}

// --- Windows: icacls --------------------------------------------------------------

#[cfg(windows)]
mod platform {
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::Command;

    use crate::error::{Error, Result};

    /// The Administrators group's well-known SID — always granted so an administrator
    /// can still back up or recover the vault folder.
    const ADMINISTRATORS_SID: &str = "S-1-5-32-544";
    /// Suppress the console window that would flash when a GUI app spawns a child.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// Format one `icacls /grant:r` principal with full, inheritable access. A SID is
    /// addressed with the `*S-1-...` form; anything else is treated as an account name.
    /// `(OI)(CI)F` = object-inherit + container-inherit + Full control.
    fn grant_arg(principal: &str) -> String {
        let p = principal.trim();
        if p.starts_with("S-1-") {
            format!("*{p}:(OI)(CI)F")
        } else {
            format!("{p}:(OI)(CI)F")
        }
    }

    /// Parse `icacls`'s English trailer `"Successfully processed N files; Failed processing M files."`
    /// and return M. `None` when the line isn't present (localized Windows, or output shape drift) —
    /// the caller keeps the process exit code as the primary success signal, so this is belt-and-braces
    /// against an `icacls` that strips inheritance, hits a per-file error, yet still exits 0. Pure.
    fn icacls_failed_count(output: &str) -> Option<u64> {
        let line = output.lines().find(|l| l.contains("Failed processing"))?;
        let after = line.split("Failed processing").nth(1)?;
        let digits: String = after
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits.parse().ok()
    }

    /// Pull the current user's SID out of `whoami /user /fo csv /nh` output, which is a
    /// single line like `"DOMAIN\user","S-1-5-21-..."`. The SID is the last field.
    fn parse_sid_from_csv(output: &str) -> Option<String> {
        let line = output.lines().find(|l| l.contains("S-1-"))?;
        let sid = line
            .rsplit(',')
            .next()?
            .trim()
            .trim_matches('"')
            .to_string();
        sid.starts_with("S-1-").then_some(sid)
    }

    /// Format one `icacls /remove:g` principal — the removal counterpart of
    /// [`grant_arg`] (SIDs use the `*S-1-...` form; no rights suffix on removal).
    fn remove_arg(principal: &str) -> String {
        let p = principal.trim();
        if p.starts_with("S-1-") {
            format!("*{p}")
        } else {
            p.to_string()
        }
    }

    /// Resolve the current user's SID (so the lockdown grants *this* account, by a
    /// stable identifier rather than a localizable name). Public so the local-accounts
    /// picker can mark which enumerated account is the caller.
    pub fn current_user_sid() -> Result<String> {
        let out = Command::new("whoami")
            .args(["/user", "/fo", "csv", "/nh"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| Error::Other(format!("could not run whoami: {e}")))?;
        if !out.status.success() {
            return Err(Error::Other("whoami /user failed".into()));
        }
        parse_sid_from_csv(&String::from_utf8_lossy(&out.stdout))
            .ok_or_else(|| Error::Other("could not determine the current user's SID".into()))
    }

    /// Run an `icacls` invocation against `dir`, returning a friendly error on failure.
    /// `recurse` adds `/T` (apply to existing children). We deliberately DROP `/C`
    /// (continue-past-errors): a lockdown or grant that hits a per-file failure must FAIL
    /// LOUD, not exit 0 having stripped inheritance without landing the owner grant — the
    /// exact silent partial that let the owner lock themselves out. Two guards: the process
    /// exit code (locale-independent, primary) AND a parse of the English "Failed
    /// processing N files" trailer (belt-and-braces for an icacls that still exits 0). The
    /// full trimmed stdout+stderr rides along so the caller's error names the real cause.
    fn run_icacls(dir: &Path, args: &[String], recurse: bool) -> Result<()> {
        let mut cmd = Command::new("icacls");
        cmd.arg(dir).args(args);
        if recurse {
            cmd.arg("/T");
        }
        let out = cmd
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| Error::Other(format!("could not run icacls: {e}")))?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let failed = icacls_failed_count(&stdout).unwrap_or(0);
        if !out.status.success() || failed > 0 {
            let detail = format!("{stdout}\n{stderr}");
            return Err(Error::Other(format!(
                "icacls failed: {}",
                crate::error::truncate_detail(detail.trim())
            )));
        }
        Ok(())
    }

    /// Lock a shared vault folder down to its owner: remove inherited ACEs, then grant
    /// Full inheritable access to the current user, Administrators, and any
    /// `extra_principals` (account names or `S-1-...` SIDs) to link from the start.
    pub fn restrict_to_owner(dir: &Path, extra_principals: &[String]) -> Result<()> {
        let me = current_user_sid()?;
        let mut args = vec![
            "/inheritance:r".to_string(),
            "/grant:r".to_string(),
            grant_arg(&me),
            grant_arg(ADMINISTRATORS_SID),
        ];
        for p in extra_principals {
            if !p.trim().is_empty() {
                args.push(grant_arg(p));
            }
        }
        run_icacls(dir, &args, true)
    }

    /// Undo a botched lockdown: reset the folder's DACL to inherit from its parent
    /// (`/reset /T`), used by the migration's abort path and the owner-side repair. A
    /// folder's OS OWNER keeps implicit `WRITE_DAC`, so the account that created it can
    /// always run this even after `restrict_to_owner` stripped its explicit access — the
    /// property that makes recovery from a self-lockout possible without elevation.
    pub fn reset_inheritance(dir: &Path) -> Result<()> {
        run_icacls(dir, &["/reset".to_string()], true)
    }

    /// Additively grant one more account (the Settings "link a second account" field)
    /// Full inheritable access, without disturbing the existing ACEs.
    pub fn grant_access(dir: &Path, principal: &str) -> Result<()> {
        if principal.trim().is_empty() {
            return Err(Error::Other("an account name or SID is required".into()));
        }
        run_icacls(dir, &["/grant:r".to_string(), grant_arg(principal)], true)
    }

    /// Whether a principal holds an explicit ACE on `dir` after a grant — a locale-safe
    /// readback via `icacls <dir> /findsid <principal>` (no `/T`; the container line is
    /// what we check). NOT an effective-access proof (that's impossible to run AS the
    /// other user), just confirmation the grant landed; the joiner's adopt is the final
    /// arbiter. `Inconclusive` on any output-shape surprise so a readback quirk never
    /// blocks a link that actually worked.
    pub fn verify_grant(dir: &Path, principal: &str) -> super::GrantCheck {
        let p = principal.trim();
        let sid_form = if p.starts_with("S-1-") {
            format!("*{p}")
        } else {
            p.to_string()
        };
        let out = match Command::new("icacls")
            .arg(dir)
            .args(["/findsid", &sid_form])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            Ok(o) => o,
            Err(e) => return super::GrantCheck::Inconclusive(format!("could not run icacls: {e}")),
        };
        let stdout = String::from_utf8_lossy(&out.stdout);
        // `/findsid` prints the dir path on the line where it found the SID, and a
        // "No files with a matching SID..." style trailer when it didn't.
        if !out.status.success() {
            return super::GrantCheck::Inconclusive(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            );
        }
        if stdout.contains(&*dir.to_string_lossy()) {
            super::GrantCheck::Granted
        } else {
            super::GrantCheck::NotFound
        }
    }

    /// Remove a principal's EXPLICIT ACEs from `dir` (recursively, via `run_icacls`'s
    /// `/T`). This is the piece [`restrict_to_owner`] can't do on its own: `/grant:r`
    /// only replaces the principals it names, so an account granted earlier keeps its
    /// explicit (inheritable) ACE unless it is removed by name here. Removing a principal
    /// that holds no ACE is a harmless no-op, so this is idempotent.
    pub fn revoke_access(dir: &Path, principal: &str) -> Result<()> {
        if principal.trim().is_empty() {
            return Err(Error::Other("an account name or SID is required".into()));
        }
        run_icacls(dir, &["/remove:g".to_string(), remove_arg(principal)], true)
    }

    #[cfg(test)]
    mod tests {
        use super::{grant_arg, icacls_failed_count, parse_sid_from_csv, remove_arg};

        #[test]
        fn grant_arg_uses_star_for_sids_and_bare_for_names() {
            assert_eq!(grant_arg("S-1-5-32-544"), "*S-1-5-32-544:(OI)(CI)F");
            assert_eq!(grant_arg("PC\\alice"), "PC\\alice:(OI)(CI)F");
            // Whitespace from a pasted value is trimmed.
            assert_eq!(grant_arg("  S-1-5-21-7  "), "*S-1-5-21-7:(OI)(CI)F");
        }

        #[test]
        fn icacls_failed_count_reads_the_english_trailer() {
            assert_eq!(
                icacls_failed_count(
                    "processed file: C:\\x\nSuccessfully processed 3 files; Failed processing 2 files."
                ),
                Some(2)
            );
            // The all-clear line reads as zero (so exit-0 + 0-failed passes).
            assert_eq!(
                icacls_failed_count("Successfully processed 5 files; Failed processing 0 files."),
                Some(0)
            );
            // A localized / shape-shifted output yields None (exit code stays primary).
            assert_eq!(icacls_failed_count("Vorgang erfolgreich"), None);
            assert_eq!(icacls_failed_count(""), None);
        }

        #[test]
        fn remove_arg_mirrors_grant_arg_without_a_rights_suffix() {
            assert_eq!(remove_arg("S-1-5-21-7"), "*S-1-5-21-7");
            assert_eq!(remove_arg("PC\\alice"), "PC\\alice");
            assert_eq!(remove_arg("  S-1-5-21-7  "), "*S-1-5-21-7");
        }

        #[test]
        fn parses_the_sid_from_whoami_csv() {
            let out = "\"desktop-pc\\bobby\",\"S-1-5-21-111-222-333-1001\"\r\n";
            assert_eq!(
                parse_sid_from_csv(out).as_deref(),
                Some("S-1-5-21-111-222-333-1001")
            );
            assert_eq!(parse_sid_from_csv("no sid here"), None);
        }
    }
}

// --- Linux: chmod 700 + POSIX ACLs via setfacl -------------------------------------

#[cfg(target_os = "linux")]
mod platform {
    use std::path::Path;
    use std::process::Command;

    use crate::error::{Error, Result};

    /// The `setfacl` argument list granting one account read/write plus
    /// search-on-directories (`X`), both as an effective ACE on everything that exists
    /// (`-R -m`) and as a default ACE on directories (`-d -m`) so new children inherit
    /// it. Pure so the shape is unit-testable without touching a filesystem.
    fn setfacl_args(principal: &str) -> Vec<String> {
        let p = principal.trim();
        vec![
            "-R".to_string(),
            "-m".to_string(),
            format!("u:{p}:rwX"),
            "-d".to_string(),
            "-m".to_string(),
            format!("u:{p}:rwX"),
        ]
    }

    /// Run `setfacl` against `dir`, mapping "command not found" to an actionable
    /// message (the tool ships in the `acl` package, present by default on Fedora but
    /// not guaranteed everywhere).
    fn run_setfacl(dir: &Path, args: &[String]) -> Result<()> {
        let out = Command::new("setfacl")
            .args(args)
            .arg(dir)
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Error::Other(
                        "setfacl was not found — install the 'acl' package (e.g. sudo dnf \
                         install acl) to link another account"
                            .into(),
                    )
                } else {
                    Error::Other(format!("could not run setfacl: {e}"))
                }
            })?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(Error::Other(format!(
                "setfacl failed: {}",
                err.trim().lines().last().unwrap_or("(no output)")
            )));
        }
        Ok(())
    }

    /// Lock a shared vault folder down to its owner: `chmod 700` on the root denies
    /// every other account traversal (children become unreachable regardless of their
    /// own modes), then `setfacl` re-admits any explicitly linked accounts.
    pub fn restrict_to_owner(dir: &Path, extra_principals: &[String]) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| Error::Other(format!("could not chmod the vault folder: {e}")))?;
        for p in extra_principals {
            if !p.trim().is_empty() {
                run_setfacl(dir, &setfacl_args(p))?;
            }
        }
        Ok(())
    }

    /// Additively grant one more account (the Settings "link a second account" field)
    /// access, without disturbing the owner lockdown. The ACL mask `setfacl` recomputes
    /// re-admits the account through the 700 root.
    pub fn grant_access(dir: &Path, principal: &str) -> Result<()> {
        if principal.trim().is_empty() {
            return Err(Error::Other("an account name or uid is required".into()));
        }
        run_setfacl(dir, &setfacl_args(principal))
    }

    /// The POSIX counterpart of the Windows DACL reset: reopen the root so the owner can
    /// reach it again (`chmod 700` — the owner already has rwx as the file's Unix owner,
    /// so this is a belt-and-braces normalize, and the abort/repair paths call it
    /// uniformly across platforms). A chmod-700 can't lock out the Unix owner in the first
    /// place, so a POSIX self-lockout isn't reachable — but keeping the call cross-platform
    /// keeps the migration code branch-free.
    pub fn reset_inheritance(dir: &Path) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| Error::Other(format!("could not chmod the vault folder: {e}")))
    }

    /// Read back whether a principal has an ACL entry on `dir`. `getfacl` is part of the
    /// same `acl` package as `setfacl`; an absent tool or clean-but-unmatched read is
    /// `Inconclusive`/`NotFound` rather than an error, mirroring the Windows contract.
    pub fn verify_grant(dir: &Path, principal: &str) -> super::GrantCheck {
        let p = principal.trim();
        let out = match Command::new("getfacl").arg("-p").arg(dir).output() {
            Ok(o) => o,
            Err(e) => {
                return super::GrantCheck::Inconclusive(format!("could not run getfacl: {e}"))
            }
        };
        if !out.status.success() {
            return super::GrantCheck::Inconclusive(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            );
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        if stdout
            .lines()
            .any(|l| l.trim_start().starts_with(&format!("user:{p}:")))
        {
            super::GrantCheck::Granted
        } else {
            super::GrantCheck::NotFound
        }
    }

    /// The `setfacl` argument list REMOVING one account's effective and default ACEs —
    /// the removal counterpart of [`setfacl_args`]. Pure so the shape is unit-testable.
    fn setfacl_remove_args(principal: &str) -> Vec<String> {
        let p = principal.trim();
        vec![
            "-R".to_string(),
            "-x".to_string(),
            format!("u:{p}"),
            "-d".to_string(),
            "-x".to_string(),
            format!("u:{p}"),
        ]
    }

    /// Remove a previously linked account's ACEs. The 700 root already denies traversal
    /// to everyone but the owner, but a make-private that leaves the vault in a shared
    /// folder should also strip the explicit ACL entries the account was granted, so this
    /// is called alongside the owner lockdown. `setfacl -x` on an absent entry is a no-op.
    pub fn revoke_access(dir: &Path, principal: &str) -> Result<()> {
        if principal.trim().is_empty() {
            return Err(Error::Other("an account name or uid is required".into()));
        }
        run_setfacl(dir, &setfacl_remove_args(principal))
    }

    #[cfg(test)]
    mod tests {
        use super::{setfacl_args, setfacl_remove_args};

        #[test]
        fn setfacl_args_grant_effective_and_default_aces() {
            assert_eq!(
                setfacl_args("alice"),
                ["-R", "-m", "u:alice:rwX", "-d", "-m", "u:alice:rwX"]
            );
            // Whitespace from a pasted value is trimmed.
            assert_eq!(
                setfacl_args("  1001  "),
                ["-R", "-m", "u:1001:rwX", "-d", "-m", "u:1001:rwX"]
            );
        }

        #[test]
        fn setfacl_remove_args_drop_effective_and_default_aces() {
            assert_eq!(
                setfacl_remove_args(" alice "),
                ["-R", "-x", "u:alice", "-d", "-x", "u:alice"]
            );
        }

        #[test]
        fn restrict_to_owner_chmods_the_root_to_700() {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            super::restrict_to_owner(dir.path(), &[]).unwrap();
            let mode = std::fs::metadata(dir.path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }
}

// --- macOS: flagged stub ------------------------------------------------------------

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    use std::path::Path;

    use crate::error::{Error, Result};

    /// Folder ACLs aren't wired on macOS in this release. Encryption is still the
    /// real protection, so callers treat this as a warning. TODO(mac, deferred):
    /// `chmod 700` + a `chmod +a` ACE for the linked account.
    pub fn restrict_to_owner(_dir: &Path, _extra_principals: &[String]) -> Result<()> {
        Err(Error::Other(
            "shared-folder ACLs aren't applied on macOS in this release".into(),
        ))
    }

    /// See [`restrict_to_owner`]: not implemented on macOS.
    pub fn grant_access(_dir: &Path, _principal: &str) -> Result<()> {
        Err(Error::Other(
            "shared-folder ACLs aren't applied on macOS in this release".into(),
        ))
    }

    /// See [`restrict_to_owner`]: not implemented on macOS.
    pub fn revoke_access(_dir: &Path, _principal: &str) -> Result<()> {
        Err(Error::Other(
            "shared-folder ACLs aren't applied on macOS in this release".into(),
        ))
    }

    /// No-op on macOS: nothing was locked down, so there's nothing to reset. Returns Ok so
    /// the migration's abort path and the repair command stay branch-free across platforms
    /// (the fatal-lockdown gate is off here via `lockdown_supported()`, so a self-lockout
    /// this would need to undo can't arise).
    pub fn reset_inheritance(_dir: &Path) -> Result<()> {
        Ok(())
    }

    /// See [`restrict_to_owner`]: ACLs aren't wired on macOS, so a grant can't be read back.
    pub fn verify_grant(_dir: &Path, _principal: &str) -> super::GrantCheck {
        super::GrantCheck::Inconclusive(
            "shared-folder ACLs aren't applied on macOS in this release".into(),
        )
    }
}

#[cfg(windows)]
pub use platform::current_user_sid;
pub use platform::{
    grant_access, reset_inheritance, restrict_to_owner, revoke_access, verify_grant,
};
