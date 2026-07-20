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

/// System trees the dynamic loader + native extensions (onnxruntime's `.so`, libstdc++, libgomp) need
/// to read AND execute. Nonexistent entries (a distro without `/lib64`, etc.) are skipped at build
/// time, so listing the superset is safe.
const SYSTEM_RX: &[&str] = &[
    "/usr/lib",
    "/usr/lib64",
    "/lib",
    "/lib64",
    "/usr/local/lib",
    "/usr/local/lib64",
];

/// Read-only system data the loader, glibc, and onnxruntime consult. `/etc` is granted broadly — it is
/// configuration, and Linux DAC still guards the secret bits (`/etc/shadow` stays unreadable). `/proc`
/// is deliberately NARROW: only the stable, non-per-pid entries onnxruntime/glibc read, so a worker
/// can't reach another same-uid process's `/proc/<pid>/environ`. (`/proc/self/*` is intentionally
/// absent — a `/proc/self` rule resolves to the PARENT's pid at rule-build time and so wouldn't apply
/// to the child anyway; python/glibc tolerate its absence, and the preflight falls open if not.)
const SYSTEM_RO: &[&str] = &[
    "/etc",
    "/proc/cpuinfo",
    "/proc/meminfo",
    "/proc/stat",
    "/proc/sys",
    "/sys/devices/system/cpu",
    "/dev/urandom",
    "/dev/random",
    "/dev/zero",
];

/// Read+write device/scratch that need no directory operations and MUST NOT be executable
/// (finding #7 — a writable+executable scratch would let the worker drop and run a binary).
const SYSTEM_RW_FILES: &[&str] = &["/dev/null"];

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

        // The base interpreter: grant its `bin` dir (`base_python_home` — needed to EXECUTE the real
        // interpreter that the venv's python is a launcher/symlink for) and its sibling `lib` dir (the
        // standard library + `libpython`), NOT the whole install root. Granting the grandparent would
        // expose `$HOME` / the vault when the interpreter is a rootless install like
        // `~/.local/bin/python3` (whose grandparent `~/.local` holds PM's data dir). For a system
        // interpreter (`/usr/bin`) the `lib` sibling is `/usr/lib`, already in `SYSTEM_RX`.
        let mut rx: Vec<PathBuf> = vec![
            venv_dir.to_path_buf(),
            base_python_home.to_path_buf(),
            script_dir.to_path_buf(),
        ];
        if let Some(base_lib) = base_python_home.parent().map(|p| p.join("lib")) {
            rx.push(base_lib);
        }
        rx.extend(SYSTEM_RX.iter().map(PathBuf::from));

        // Read-only: model cache + system config/info.
        let mut ro: Vec<PathBuf> = vec![models_dir.to_path_buf()];
        ro.extend(SYSTEM_RO.iter().map(PathBuf::from));

        // Read+write, no execute: staging (input copies + TMPDIR) and POSIX shared memory (finding #7 —
        // granted write WITHOUT execute), plus the device files that need writing (/dev/null).
        let mut rw: Vec<PathBuf> = vec![staging_dir.clone(), PathBuf::from("/dev/shm")];
        rw.extend(SYSTEM_RW_FILES.iter().map(PathBuf::from));

        let ruleset_fd = build_landlock_ruleset(&rx, &ro, &rw)?;

        // The concise "what the worker can see" set for the readout: the app-specific dirs (system
        // RX/RO paths are implied and omitted to keep the readout legible).
        let granted = vec![
            venv_dir.to_path_buf(),
            base_python_home.to_path_buf(),
            models_dir.to_path_buf(),
            script_dir.to_path_buf(),
            staging_dir.clone(),
        ];

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
