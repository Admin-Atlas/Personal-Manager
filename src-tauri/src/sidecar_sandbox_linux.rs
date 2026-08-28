// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Linux sidecar confinement (#286 PR2d): run the untrusted-file WORKER with no outbound network and a
//! restricted filesystem, mirroring the shipped Windows AppContainer (#425) and the macOS profile
//! (PR2c). Two kernel primitives a process can impose on ITSELF, unprivileged — which is exactly why
//! they survive hardened machines (no namespaces, no root, no external binary):
//!
//!   * **Network** — a fixed classic-BPF seccomp filter (see [`crate::sidecar_seccomp`]) that refuses
//!     `socket()` for the IP families. Universal (seccomp needs no special kernel config).
//!   * **Filesystem** — a Landlock ruleset (via the `landlock` crate, best-effort) granting read/execute
//!     on the interpreter + model trees and read/write on a staging dir, and nothing else — so the
//!     vault and `$HOME` are simply absent from the worker's view. Needs kernel ≥ 5.13 with the LSM
//!     active; where it's missing we run seccomp-only and report [`Degraded`](crate::sidecar::SandboxReport::Degraded),
//!     never mislabel it confined.
//!
//! The load-bearing safety property is the parent/child split. The `landlock` crate builds the ruleset
//! in the PARENT (it allocates freely there) and we extract the raw ruleset fd; the CHILD's `pre_exec`
//! then does ONLY raw, allocation-free syscalls — the async-signal-safety rule after `fork()` in a
//! multi-threaded process. Unlike Windows, the confined child is an ordinary [`std::process::Child`]
//! (a `Command` with a `pre_exec` hook that execs the venv python), so the existing `StdChild` +
//! request loop are reused verbatim — no custom child type.
//!
//! Everything here is best-effort HARDENING on top of the offline worker + at-rest encryption: if
//! setup fails the caller runs the worker unconfined (logged with its `SBX-####` code, surfaced in the
//! Developer-mode readout) rather than break ingest. Gated to the 64-bit arches PM ships a desktop
//! build for; other Linux arches report `Unsupported`.
#![cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]

use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use landlock::{
    Access, AccessFs, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset, RulesetAttr,
    RulesetCreatedAttr, ABI,
};

use crate::sidecar::{sbx, SbxError};
use crate::sidecar_seccomp::{build_block_inet_filter, SeccompArch, SockFilter};

// The allow-set — which directories go in which bucket — lives in `sidecar_allowset`, pure and
// cross-platform, so it is unit-tested on every OS including the Windows dev box where none of the
// Landlock machinery below even compiles. This file keeps the half that genuinely is Linux and
// genuinely is I/O: opening each path with `O_PATH` and folding it into a kernel ruleset.

/// The Landlock ruleset (the raw owned fd) plus the seccomp program and the staging dir, built once and
/// reused for every worker spawn. `ruleset_fd == None` means the kernel lacks Landlock (best-effort
/// no-op) → the filesystem layer is NOT enforced and we report `Degraded`.
pub struct Sandbox {
    /// Where untrusted input files are copied before the confined worker reads them (also the worker's
    /// cwd and `TMPDIR`, so `tempfile` lands somewhere the Landlock ruleset grants).
    staging_dir: PathBuf,
    /// The interpreter/model/script/staging dirs the worker can reach, for the Developer-mode readout
    /// only (the system RX/RO paths are implied). Never consulted at runtime.
    granted: Vec<PathBuf>,
    /// The Landlock ruleset fd, kept alive here so every `fork()`ed child inherits it and can
    /// `landlock_restrict_self` with it. `None` ⇒ Landlock unavailable ⇒ degraded (seccomp only).
    ruleset_fd: Option<OwnedFd>,
    /// The seccomp filter that refuses IP sockets, cloned into each child's `pre_exec` closure.
    filter: Vec<SockFilter>,
}

impl Sandbox {
    /// Idempotent setup: create the staging dir, build the seccomp network filter, and build the
    /// Landlock filesystem ruleset (best-effort). Returns `Err(SbxError)` — caller runs unconfined —
    /// only on a hard failure (staging dir, or a Landlock build error on a kernel that HAS Landlock);
    /// a kernel simply WITHOUT Landlock is not an error, it yields a `Sandbox` whose `ruleset_fd` is
    /// `None` and which reports `Degraded`.
    ///
    /// `base_python_home` is the base interpreter's `bin` dir (from `pyvenv.cfg` `home =`); its parent
    /// (the install root) is granted so `lib/pythonX.Y` is reachable for a relocatable interpreter.
    pub fn ensure(
        venv_dir: &Path,
        base_python_home: &Path,
        models_dir: &Path,
        script_dir: &Path,
        runtime_dir: &Path,
    ) -> std::result::Result<Sandbox, SbxError> {
        let staging_dir = runtime_dir.join("sandbox-in");
        std::fs::create_dir_all(&staging_dir)
            .map_err(|e| SbxError::new(sbx::STAGING_DIR, format!("staging dir: {e}")))?;
        // Take any staged copy a previous run left here. StagedInput deletes on drop, but a crash
        // skips destructors and what survives is a PLAINTEXT copy of the user's document. This
        // process has staged nothing yet, so everything matching is orphaned.
        crate::sidecar_stage::sweep_staging(&staging_dir);
        // The worker only reads the model cache; the unconfined --fetch helper writes it. Make sure the
        // dir exists so the read-only rule below can be added (Landlock add_rule needs an extant path).
        let _ = std::fs::create_dir_all(models_dir);

        let arch = SeccompArch::current().ok_or_else(|| {
            SbxError::new(
                sbx::LINUX_SECCOMP,
                "no seccomp filter for this Linux architecture",
            )
        })?;
        let filter = build_block_inet_filter(arch);

        let allow = crate::sidecar_allowset::allow_set(crate::sidecar_allowset::SandboxPaths {
            venv_dir,
            base_python_home,
            models_dir,
            script_dir,
            staging_dir: &staging_dir,
        });

        let ruleset_fd = build_landlock_ruleset(&allow.rx, &allow.ro, &allow.rw)?;
        let granted = allow.granted;

        Ok(Sandbox {
            staging_dir,
            granted,
            ruleset_fd,
            filter,
        })
    }

    /// The container-writable dir input files are staged into (also the confined child's cwd + TMPDIR).
    pub fn staging_dir(&self) -> &Path {
        &self.staging_dir
    }

    /// The confinement mechanism, for the Developer-mode readout. Names only the axes actually enforced
    /// so a degraded run reads honestly.
    pub fn mechanism(&self) -> &'static str {
        if self.ruleset_fd.is_some() {
            "Landlock (files) + seccomp (network)"
        } else {
            "seccomp (network)"
        }
    }

    /// The app dirs the worker can reach (for the readout only — see [`Sandbox::granted`]).
    pub fn granted_dirs(&self) -> Vec<String> {
        self.granted
            .iter()
            .map(|p| p.display().to_string())
            .collect()
    }

    /// The confinement axes actually enforced: `network` always; `filesystem` only when Landlock was
    /// available.
    pub fn layers(&self) -> Vec<String> {
        let mut layers = vec!["network".to_string()];
        if self.ruleset_fd.is_some() {
            layers.push("filesystem".to_string());
        }
        layers
    }

    /// `Some((code, detail))` when a confinement axis is missing — here, Landlock unavailable so the
    /// filesystem is NOT restricted (network still is). `None` when fully confined.
    pub fn degraded(&self) -> Option<(&'static str, String)> {
        if self.ruleset_fd.is_none() {
            Some((
                sbx::LINUX_DEGRADED,
                "Landlock unavailable (kernel < 5.13 or its LSM is not active) — the worker's outbound \
                 network is blocked but its filesystem is NOT restricted"
                    .to_string(),
            ))
        } else {
            None
        }
    }

    /// Attach the confinement to `command`: set its cwd to the staging dir and install the `pre_exec`
    /// hook that self-confines the child just before it execs python. The hook captures an owned clone
    /// of the seccomp program and a copy of the ruleset fd (both `'static`), so it never borrows `self`
    /// and never allocates in the post-fork child.
    pub fn install_into(&self, command: &mut Command) {
        command.current_dir(&self.staging_dir);
        let filter = self.filter.clone();
        let ruleset_fd: Option<RawFd> = self.ruleset_fd.as_ref().map(|f| f.as_raw_fd());
        // SAFETY: the closure runs in the forked child before exec and does only async-signal-safe raw
        // syscalls (no allocation, no locks, no panics) — see `confine_child`. The ruleset fd it copies
        // stays open for the child because `self` (which owns the `OwnedFd`) lives on the manager for
        // the app's lifetime, and the fd is inherited across the fork.
        unsafe {
            command.pre_exec(move || confine_child(ruleset_fd, &filter));
        }
    }
}

/// Build the Landlock ruleset in the PARENT and return its raw owned fd. `Ok(None)` when the kernel
/// lacks Landlock (best-effort makes every step a silent no-op and the final fd is `None`) — the caller
/// treats that as `Degraded`, not an error. `Err` only on a genuine build failure on a Landlock-capable
/// kernel.
fn build_landlock_ruleset(
    rx: &[PathBuf],
    ro: &[PathBuf],
    rw: &[PathBuf],
) -> std::result::Result<Option<OwnedFd>, SbxError> {
    // All the access rights we need are ABI v1 (universal on any Landlock kernel, ≥ 5.13). Requesting
    // v1 and letting BestEffort no-op on older kernels keeps this simple; ops added in later ABIs
    // (rename-across-dirs, truncate, device ioctl) are left unhandled, an acceptable, documented
    // loosening for this threat model — the worker still can't reach the vault or the network.
    let abi = ABI::V1;
    let rx_access = AccessFs::Execute | AccessFs::ReadFile | AccessFs::ReadDir;
    let ro_access = AccessFs::ReadFile | AccessFs::ReadDir;
    let rw_access = AccessFs::ReadFile
        | AccessFs::ReadDir
        | AccessFs::WriteFile
        | AccessFs::MakeReg
        | AccessFs::MakeDir
        | AccessFs::RemoveFile
        | AccessFs::RemoveDir;

    let mut created = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(abi))
        .map_err(|e| SbxError::new(sbx::LINUX_LANDLOCK, format!("handle_access: {e}")))?
        .create()
        .map_err(|e| SbxError::new(sbx::LINUX_LANDLOCK, format!("create: {e}")))?;

    for (dirs, access) in [(rx, rx_access), (ro, ro_access), (rw, rw_access)] {
        for dir in dirs {
            // A path that doesn't exist on this distro (e.g. /lib64) can't be opened; skip it rather
            // than fail the whole ruleset. PathFd::new opens with O_PATH.
            let Ok(pf) = PathFd::new(dir) else { continue };
            created = created
                .add_rule(PathBeneath::new(pf, access))
                .map_err(|e| {
                    SbxError::new(
                        sbx::LINUX_LANDLOCK,
                        format!("add_rule {}: {e}", dir.display()),
                    )
                })?;
        }
    }

    let ofd: Option<OwnedFd> = created.into();
    // Make the parent's ruleset fd close-on-exec so it doesn't leak into unrelated subprocesses (the
    // --fetch helper, etc.). Our confined children still use it: they inherit it across fork and
    // restrict with it in pre_exec, BEFORE exec closes it. Best-effort.
    if let Some(fd) = ofd.as_ref() {
        unsafe {
            let _ = libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
    Ok(ofd)
}

/// The post-fork child body: impose no-new-privs, close inherited fds, then apply Landlock + seccomp,
/// all via raw allocation-free syscalls. Runs between `fork()` and `execvp()` in a multi-threaded
/// parent, so it must touch nothing that could take a lock another thread held at fork time — hence no
/// allocation, no `std::io` beyond `Error::last_os_error` (which only reads `errno`), and raw
/// `libc::syscall`.
fn confine_child(ruleset_fd: Option<RawFd>, filter: &[SockFilter]) -> std::io::Result<()> {
    unsafe {
        // 1. no_new_privs — required for unprivileged seccomp AND landlock_restrict_self. Set in the
        //    CHILD (not the parent) so it doesn't leak onto the whole app or the --fetch helper. The
        //    variadic args are cast to c_ulong explicitly (glibc reads them as unsigned long) rather than
        //    relying on bare-int register zero-extension.
        if libc::prctl(
            libc::PR_SET_NO_NEW_PRIVS,
            1 as libc::c_ulong,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
        ) != 0
        {
            return Err(std::io::Error::last_os_error());
        }

        // 2. fd hygiene: mark every fd ≥ 3 close-on-exec so a leaked vault-DB handle or open socket
        //    can't ride into the confined worker (Windows pins the child to 3 stdio handles; unix has
        //    only O_CLOEXEC, and not every fd is guaranteed to have it). CRITICAL details:
        //      * via `syscall()`, NOT `libc::close_range` — the glibc wrapper needs glibc ≥ 2.34 and
        //        its mere PRESENCE would make the binary fail to LOAD on older distros (Ubuntu 20.04).
        //      * `CLOSE_RANGE_CLOEXEC` MARKS the fds close-on-exec rather than closing them now — an
        //        immediate close would reap Rust's internal fork/exec error-report pipe and silently
        //        misreport a failed exec as success. Marked fds (including the ruleset fd) close at our
        //        own exec; Rust's error pipe (already CLOEXEC) keeps working.
        //    The syscall fails on old kernels — ENOSYS (< 5.9, no close_range) or EINVAL (5.9/5.10, the
        //    CLOEXEC flag is 5.11+). Fall back to a bounded fcntl loop so fd hygiene still holds there.
        let close_range_ok = libc::syscall(
            libc::SYS_close_range,
            3 as libc::c_uint as libc::c_long,
            libc::c_uint::MAX as libc::c_long,
            libc::CLOSE_RANGE_CLOEXEC as libc::c_long,
        ) == 0;
        if !close_range_ok {
            // Async-signal-safe, allocation-free fallback: mark fds close-on-exec one at a time. Bounded
            // (we can't cheaply enumerate the fd table post-fork); typical processes stay far under the
            // cap, and this is defence-in-depth on top of Rust's own O_CLOEXEC-by-default. EBADF on a
            // gap in the fd table is expected and ignored.
            let mut fd: libc::c_int = 3;
            while fd < 4096 {
                let _ = libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
                fd += 1;
            }
        }

        // 3. Landlock filesystem restriction, if the parent built a ruleset (else this is the degraded,
        //    seccomp-only path). Raw syscall so we don't consume the crate's `RulesetCreated` (its
        //    `restrict_self` takes `self` by value, unusable from an `FnMut`) or allocate.
        if let Some(fd) = ruleset_fd {
            if libc::syscall(
                libc::SYS_landlock_restrict_self,
                fd as libc::c_long,
                0 as libc::c_long,
            ) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
        }

        // 4. seccomp network filter. The kernel copies the program during the call, so the borrowed
        //    `filter` slice and the stack `sock_fprog` need only outlive this syscall.
        let prog = libc::sock_fprog {
            len: filter.len() as libc::c_ushort,
            filter: filter.as_ptr() as *mut libc::sock_filter,
        };
        if libc::syscall(
            libc::SYS_seccomp,
            libc::SECCOMP_SET_MODE_FILTER as libc::c_long,
            0 as libc::c_long,
            &prog as *const libc::sock_fprog as libc::c_long,
        ) != 0
        {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Stdio;

    /// Build a sandbox over throwaway directories, with the BASE interpreter pointed at the real
    /// `/usr/bin`.
    ///
    /// That last part is what makes an end-to-end test possible at all. The allow-set grants
    /// read+execute on `base_python_home`, and `/usr/bin` appears nowhere else in it — so pointing
    /// it there (the "system interpreter" shape `sidecar_allowset`'s own tests already model) is
    /// what lets the confined child exec a real binary. Everything else is a temp dir, and `/tmp`
    /// is granted by nothing, which is what leaves an honest denied path to aim at.
    fn sandbox_over(root: &Path) -> Sandbox {
        for sub in ["venv", "models", "script", "runtime"] {
            fs::create_dir_all(root.join(sub)).expect("fixture dirs");
        }
        Sandbox::ensure(
            &root.join("venv"),
            Path::new("/usr/bin"),
            &root.join("models"),
            &root.join("script"),
            &root.join("runtime"),
        )
        .expect("a kernel with OR without Landlock still yields a Sandbox")
    }

    /// `cat` a path inside a confined child. `/usr/bin/cat` by absolute path, not `cat`: the
    /// ruleset grants that directory specifically, and PATH lookup is not what is under test.
    fn confined_cat(sandbox: &Sandbox, path: &Path) -> std::process::Output {
        let mut cmd = Command::new("/usr/bin/cat");
        cmd.arg(path).stdout(Stdio::piped()).stderr(Stdio::piped());
        sandbox.install_into(&mut cmd);
        cmd.output().expect("the confined child spawns")
    }

    /// The property this whole file exists for, and the one no unit test can reach: a real child,
    /// after a real `fork` + `landlock_restrict_self`, cannot open a file the ruleset does not
    /// grant — and can still open one it does.
    ///
    /// Everything else here is parent-side bookkeeping that would pass just as happily with a
    /// ruleset that enforced nothing. This is the assertion that would notice.
    #[test]
    fn a_confined_child_reads_inside_the_allow_set_and_not_outside_it() {
        let root = tempfile::tempdir().expect("tempdir");
        let sandbox = sandbox_over(root.path());

        // Granted: `models_dir` is read-only in the allow-set.
        let allowed = root.path().join("models").join("allowed.txt");
        fs::write(&allowed, b"readable").expect("write the granted file");

        // Not granted: a SEPARATE temp dir, so it sits under no granted path.
        let outside = tempfile::tempdir().expect("tempdir");
        let denied = outside.path().join("secret.txt");
        fs::write(&denied, b"must not be readable").expect("write the denied file");

        if sandbox.ruleset_fd.is_none() {
            // No active Landlock LSM. The sandbox is filesystem-degraded BY DESIGN here (seccomp
            // still blocks the network), so there is no filesystem confinement to assert. Say so
            // out loud rather than passing silently — a quiet pass is how this test would rot into
            // one that never actually runs.
            eprintln!(
                "sandbox enforcement: SKIPPED the filesystem half — this kernel reports no \
                 Landlock, so `Sandbox` is degraded to seccomp-only and grants nothing to test"
            );
            return;
        }

        let ok = confined_cat(&sandbox, &allowed);
        assert!(
            ok.status.success() && ok.stdout == b"readable",
            "a GRANTED path must stay readable inside the sandbox — status {:?}, stderr {}",
            ok.status,
            String::from_utf8_lossy(&ok.stderr),
        );

        let blocked = confined_cat(&sandbox, &denied);
        assert!(
            !blocked.status.success(),
            "an UNGRANTED path must not be readable inside the sandbox, but cat exited 0 with {:?}",
            String::from_utf8_lossy(&blocked.stdout),
        );
        assert!(
            blocked.stdout.is_empty(),
            "a denied read must yield no content at all, got {:?}",
            String::from_utf8_lossy(&blocked.stdout),
        );
    }

    /// The control. Without it the deny assertion above passes for free the moment the fixture is
    /// wrong — a mistyped path or an unwritable temp dir makes `cat` fail for reasons that have
    /// nothing to do with Landlock, and the test would still go green. So: the same file, the same
    /// binary, no sandbox, must be readable.
    #[test]
    fn the_denied_file_is_readable_when_the_sandbox_is_not_applied() {
        let outside = tempfile::tempdir().expect("tempdir");
        let denied = outside.path().join("secret.txt");
        fs::write(&denied, b"must not be readable").expect("write the denied file");

        let out = Command::new("/usr/bin/cat")
            .arg(&denied)
            .output()
            .expect("unconfined cat runs");
        assert!(
            out.status.success() && out.stdout == b"must not be readable",
            "the fixture itself must be readable unconfined, else the deny test proves nothing — \
             status {:?}, stderr {}",
            out.status,
            String::from_utf8_lossy(&out.stderr),
        );
    }
}
