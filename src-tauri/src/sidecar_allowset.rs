// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the confined sidecar worker is allowed to reach, as a pure decision.
//!
//! This is the allow-set half of the Linux confinement, split out from `sidecar_sandbox_linux.rs`
//! the way [`crate::sidecar_stage`] is split out of the staging path — and for the same reason.
//! The Landlock half is unavoidably Linux-only and unavoidably I/O: it opens each directory with
//! `O_PATH` and folds it into a kernel ruleset. **Deciding which directories go in which bucket is
//! neither.** It is a list of paths derived from five other paths, and it is the part where a
//! mistake is silent and serious: granting one directory too many hands the worker — which exists
//! precisely to parse untrusted files — a route to the user's vault.
//!
//! So the decision lives here, cross-platform and pure, and is tested on every platform including
//! the Windows dev box, where nothing else in the confinement compiles at all.
//!
//! The three buckets map to Landlock access rights in the caller:
//!
//! | bucket | rights | why |
//! | --- | --- | --- |
//! | `rx` | read + execute | the interpreter, its standard library, the loader's system trees |
//! | `ro` | read | the model cache and the system config/info the loader consults |
//! | `rw` | read + write, **never execute** | staging, shared memory, `/dev/null` |
//!
//! `rw` withholding execute is finding #7: a scratch directory that is both writable and executable
//! lets a worker drop a binary and run it, which would make the seccomp filter the only thing left.

// Only the Linux sandbox consumes this, so off Linux every item here is unused by the build (the
// tests still exercise all of it, on every platform — which is the reason the module exists).
// Same arrangement as `sidecar_seccomp`.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::path::{Path, PathBuf};

/// System trees the dynamic loader + native extensions (onnxruntime's `.so`, libstdc++, libgomp) need
/// to read AND execute. Nonexistent entries (a distro without `/lib64`, etc.) are skipped when the
/// ruleset is built, so listing the superset is safe.
pub const SYSTEM_RX: &[&str] = &[
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
pub const SYSTEM_RO: &[&str] = &[
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
pub const SYSTEM_RW_FILES: &[&str] = &["/dev/null"];

/// POSIX shared memory. Written by the worker (numpy/torch use it), never executed.
pub const SHM_DIR: &str = "/dev/shm";

/// The five paths the confinement is derived from.
#[derive(Debug, Clone, Copy)]
pub struct SandboxPaths<'a> {
    /// The venv PM created — holds the `python` launcher the worker execs.
    pub venv_dir: &'a Path,
    /// The BASE interpreter's `bin` dir, read from `pyvenv.cfg`'s `home =`.
    pub base_python_home: &'a Path,
    /// The embedding/whisper model cache. The worker reads it; the unconfined `--fetch` helper
    /// writes it.
    pub models_dir: &'a Path,
    /// Where `pm_sidecar.py` lives.
    pub script_dir: &'a Path,
    /// Where untrusted input is copied before the worker reads it — also its cwd and `TMPDIR`.
    pub staging_dir: &'a Path,
}

/// The decided allow-set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowSet {
    /// Read + execute.
    pub rx: Vec<PathBuf>,
    /// Read only.
    pub ro: Vec<PathBuf>,
    /// Read + write, never execute.
    pub rw: Vec<PathBuf>,
    /// The concise "what the worker can see" set for the Developer-mode readout: the app-specific
    /// dirs only, since the system RX/RO paths are implied and would drown the list. Never consulted
    /// at runtime.
    pub granted: Vec<PathBuf>,
}

/// Decide what the worker may reach.
///
/// The one subtle rule is the base interpreter. Its `bin` dir is granted so the worker can execute
/// the real interpreter that the venv's `python` is a launcher for, and its SIBLING `lib` dir is
/// granted for the standard library and `libpython`. The install root itself is **not** — granting
/// the grandparent would expose `$HOME` for a rootless install like `~/.local/bin/python3`, whose
/// grandparent `~/.local` is where PM keeps its data dir and therefore the vault. For a system
/// interpreter at `/usr/bin` the `lib` sibling is `/usr/lib`, already in [`SYSTEM_RX`].
pub fn allow_set(paths: SandboxPaths) -> AllowSet {
    let mut rx: Vec<PathBuf> = vec![
        paths.venv_dir.to_path_buf(),
        paths.base_python_home.to_path_buf(),
        paths.script_dir.to_path_buf(),
    ];
    if let Some(base_lib) = paths.base_python_home.parent().map(|p| p.join("lib")) {
        rx.push(base_lib);
    }
    rx.extend(SYSTEM_RX.iter().map(PathBuf::from));

    let mut ro: Vec<PathBuf> = vec![paths.models_dir.to_path_buf()];
    ro.extend(SYSTEM_RO.iter().map(PathBuf::from));

    let mut rw: Vec<PathBuf> = vec![paths.staging_dir.to_path_buf(), PathBuf::from(SHM_DIR)];
    rw.extend(SYSTEM_RW_FILES.iter().map(PathBuf::from));

    let granted = vec![
        paths.venv_dir.to_path_buf(),
        paths.base_python_home.to_path_buf(),
        paths.models_dir.to_path_buf(),
        paths.script_dir.to_path_buf(),
        paths.staging_dir.to_path_buf(),
    ];

    AllowSet {
        rx,
        ro,
        rw,
        granted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rootless install — the shape that makes the base-interpreter rule load-bearing. PM's data
    /// dir (and so the vault) lives under `~/.local/share`, a sibling of `~/.local/bin`.
    fn rootless() -> AllowSet {
        allow_set(SandboxPaths {
            venv_dir: Path::new("/home/u/.local/share/pm/python/venv"),
            base_python_home: Path::new("/home/u/.local/bin"),
            models_dir: Path::new("/home/u/.local/share/pm/models"),
            script_dir: Path::new("/home/u/.local/share/pm/python/scripts"),
            staging_dir: Path::new("/home/u/.local/share/pm/runtime/sandbox-in"),
        })
    }

    /// A distro interpreter, where the `lib` sibling lands on a path already in `SYSTEM_RX`.
    fn system() -> AllowSet {
        allow_set(SandboxPaths {
            venv_dir: Path::new("/home/u/.local/share/pm/python/venv"),
            base_python_home: Path::new("/usr/bin"),
            models_dir: Path::new("/home/u/.local/share/pm/models"),
            script_dir: Path::new("/home/u/.local/share/pm/python/scripts"),
            staging_dir: Path::new("/home/u/.local/share/pm/runtime/sandbox-in"),
        })
    }

    fn all(set: &AllowSet) -> Vec<&PathBuf> {
        set.rx.iter().chain(&set.ro).chain(&set.rw).collect()
    }

    #[test]
    fn the_interpreters_install_root_is_never_granted() {
        // The whole reason the base interpreter is granted as `bin` + `lib` rather than as its
        // parent: `~/.local` holds PM's data dir, so granting it would put the vault inside the
        // confinement of the process whose entire job is parsing untrusted files.
        let set = rootless();
        for path in all(&set) {
            assert_ne!(path.as_path(), Path::new("/home/u/.local"), "install root");
            assert_ne!(path.as_path(), Path::new("/home/u"), "home");
            assert_ne!(
                path.as_path(),
                Path::new("/home/u/.local/share"),
                "data dir"
            );
            assert_ne!(
                path.as_path(),
                Path::new("/home/u/.local/share/pm"),
                "PM's own data dir — the vault lives under it"
            );
        }
    }

    #[test]
    fn the_base_interpreters_bin_and_lib_siblings_are_granted_and_nothing_between() {
        let set = rootless();
        assert!(set.rx.iter().any(|p| p == Path::new("/home/u/.local/bin")));
        assert!(set.rx.iter().any(|p| p == Path::new("/home/u/.local/lib")));
        // A system interpreter's lib sibling is /usr/lib, which SYSTEM_RX already carries — the
        // duplicate is harmless, but it must be present either way.
        assert!(system().rx.iter().any(|p| p == Path::new("/usr/lib")));
    }

    #[test]
    fn nothing_writable_is_also_executable() {
        // Finding #7. A path in both buckets would be granted execute AND write, which is all a
        // worker needs to drop a binary and run it — at which point seccomp is the only layer left.
        for set in [rootless(), system()] {
            for w in &set.rw {
                assert!(
                    !set.rx.contains(w),
                    "{} is writable and executable",
                    w.display()
                );
            }
            assert!(set.rw.iter().any(|p| p == Path::new(SHM_DIR)));
            assert!(set.rw.iter().any(|p| p == Path::new("/dev/null")));
        }
    }

    #[test]
    fn the_model_cache_is_read_only_and_staging_is_not() {
        // The worker consults the cache; only the UNCONFINED `--fetch` helper writes it. If the
        // cache were writable a malicious document could swap the embedding model under PM.
        let set = rootless();
        let models = Path::new("/home/u/.local/share/pm/models");
        assert!(set.ro.iter().any(|p| p == models));
        assert!(!set.rw.iter().any(|p| p == models));
        assert!(!set.rx.iter().any(|p| p == models));

        // Staging is the one app path that must be writable — it is the worker's cwd and TMPDIR.
        let staging = Path::new("/home/u/.local/share/pm/runtime/sandbox-in");
        assert!(set.rw.iter().any(|p| p == staging));
        assert!(!set.rx.iter().any(|p| p == staging));
    }

    #[test]
    fn the_script_and_venv_are_executable_not_writable() {
        let set = rootless();
        for p in [
            "/home/u/.local/share/pm/python/venv",
            "/home/u/.local/share/pm/python/scripts",
        ] {
            assert!(set.rx.iter().any(|q| q == Path::new(p)), "{p} must be rx");
            assert!(
                !set.rw.iter().any(|q| q == Path::new(p)),
                "{p} must not be rw"
            );
        }
    }

    #[test]
    fn proc_is_granted_narrowly_enough_to_miss_another_processs_environ() {
        // `/proc` as a whole would expose `/proc/<pid>/environ` of every same-uid process — which
        // on this app includes the process holding the vault key material.
        for entry in SYSTEM_RO {
            assert_ne!(*entry, "/proc", "granting all of /proc defeats the point");
            assert_ne!(*entry, "/proc/self");
        }
        assert!(SYSTEM_RO.contains(&"/proc/cpuinfo"));
    }

    #[test]
    fn the_readout_names_the_app_paths_and_omits_the_system_ones() {
        // The Developer-mode readout is the only place a user can see what the worker reaches, so
        // it must stay legible — but it must not omit an app path either.
        let set = rootless();
        assert_eq!(set.granted.len(), 5);
        for p in &set.granted {
            let s = p.to_string_lossy();
            assert!(!s.starts_with("/usr"), "{s} is a system path");
            assert!(!s.starts_with("/etc"), "{s} is a system path");
        }
        assert!(set
            .granted
            .contains(&PathBuf::from("/home/u/.local/share/pm/models")));
    }
}
