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
    fn run_icacls(dir: &Path, args: &[String]) -> Result<()> {
        let out = Command::new("icacls")
            .arg(dir)
            .args(args)
            // Apply to existing children too (`/T`) and keep going past per-file errors
            // (`/C`) so one locked sub-item doesn't abort the whole pass.
            .args(["/T", "/C"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| Error::Other(format!("could not run icacls: {e}")))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(Error::Other(format!(
                "icacls failed: {}",
                err.trim().lines().last().unwrap_or("(no output)")
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
        run_icacls(dir, &args)
    }

    /// Additively grant one more account (the Settings "link a second account" field)
    /// Full inheritable access, without disturbing the existing ACEs.
    pub fn grant_access(dir: &Path, principal: &str) -> Result<()> {
        if principal.trim().is_empty() {
            return Err(Error::Other("an account name or SID is required".into()));
        }
        run_icacls(dir, &["/grant:r".to_string(), grant_arg(principal)])
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
        run_icacls(dir, &["/remove:g".to_string(), remove_arg(principal)])
    }

    #[cfg(test)]
    mod tests {
        use super::{grant_arg, parse_sid_from_csv, remove_arg};

        #[test]
        fn grant_arg_uses_star_for_sids_and_bare_for_names() {
            assert_eq!(grant_arg("S-1-5-32-544"), "*S-1-5-32-544:(OI)(CI)F");
            assert_eq!(grant_arg("PC\\alice"), "PC\\alice:(OI)(CI)F");
            // Whitespace from a pasted value is trimmed.
            assert_eq!(grant_arg("  S-1-5-21-7  "), "*S-1-5-21-7:(OI)(CI)F");
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
}

#[cfg(windows)]
pub use platform::current_user_sid;
pub use platform::{grant_access, restrict_to_owner, revoke_access};
