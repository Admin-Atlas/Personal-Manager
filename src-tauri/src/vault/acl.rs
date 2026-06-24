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
//! macOS is a flagged stub (shared vaults are weaker there; the UI says so).

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

    /// Resolve the current user's SID (so the lockdown grants *this* account, by a
    /// stable identifier rather than a localizable name).
    fn current_user_sid() -> Result<String> {
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

    #[cfg(test)]
    mod tests {
        use super::{grant_arg, parse_sid_from_csv};

        #[test]
        fn grant_arg_uses_star_for_sids_and_bare_for_names() {
            assert_eq!(grant_arg("S-1-5-32-544"), "*S-1-5-32-544:(OI)(CI)F");
            assert_eq!(grant_arg("PC\\alice"), "PC\\alice:(OI)(CI)F");
            // Whitespace from a pasted value is trimmed.
            assert_eq!(grant_arg("  S-1-5-21-7  "), "*S-1-5-21-7:(OI)(CI)F");
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

// --- macOS / other: flagged stub --------------------------------------------------

#[cfg(not(windows))]
mod platform {
    use std::path::Path;

    use crate::error::{Error, Result};

    /// Folder ACLs aren't wired outside Windows in this release. Encryption is still the
    /// real protection, so callers treat this as a warning. On macOS this is the flagged
    /// stub — TODO(mac, deferred): `chmod 700` + a `chmod +a` ACE for the linked account.
    pub fn restrict_to_owner(_dir: &Path, _extra_principals: &[String]) -> Result<()> {
        Err(Error::Other(
            "shared-folder ACLs are only applied on Windows in this release".into(),
        ))
    }

    /// See [`restrict_to_owner`]: not implemented off Windows.
    pub fn grant_access(_dir: &Path, _principal: &str) -> Result<()> {
        Err(Error::Other(
            "shared-folder ACLs are only applied on Windows in this release".into(),
        ))
    }
}

pub use platform::grant_access;
// `restrict_to_owner` is exercised by the migration routine (Build 8); re-exported
// there. Until then it is reachable as `acl::platform::restrict_to_owner` and kept
// alive by the module-wide `allow(dead_code)` in `vault/mod.rs`.
