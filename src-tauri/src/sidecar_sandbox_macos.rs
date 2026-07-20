// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! macOS sidecar confinement (#286 PR2c): run the untrusted-file WORKER with no outbound network and a
//! restricted filesystem, mirroring the shipped Windows AppContainer (#425) and the Linux Landlock +
//! seccomp arm (#429). The mechanism is `/usr/bin/sandbox-exec` applying a generated Seatbelt (SBPL)
//! profile to the worker process — the one confinement a process can impose on an ordinary spawned
//! child on macOS without code-signing entitlements (App Sandbox confines a whole SIGNED app, which PM
//! can't do while Atlas is unregistered).
//!
//! The profile is `(deny default)` — deny everything, then allow only:
//!   * **Network**: nothing. No socket operation is ever allowed, so the worker can open neither an IP
//!     socket nor a local one. `(deny network*)` is added as belt-and-suspenders documentation; the
//!     real guarantee is that the allow-list never grants a socket. Crucially, `mach-lookup` to the DNS
//!     daemons (`com.apple.mDNSResponder`, `com.apple.dnssd.service`) is NOT allowed either, so the
//!     worker cannot resolve hostnames out-of-process — the macOS DNS-via-daemon exfil path a bare
//!     `(deny network*)` would miss (finding #1).
//!   * **Filesystem**: read + execute on the interpreter/model/script trees and the system libraries
//!     dyld resolves, read+write on a staging dir, and nothing else — so the vault and `$HOME` are
//!     simply absent from the worker's view. Metadata (`stat`) is allowed broadly because dyld and
//!     CoreFoundation stat paths all over at load; that leaks the vault's STRUCTURE but never its
//!     CONTENTS (file-read-DATA on it is never granted), a documented residual (finding #6).
//!
//! **Injection-safe by construction**: every dynamic path rides as a `-D KEY=VALUE` parameter referenced
//! in the profile as `(subpath (param "KEY"))`. The path VALUE is bound by libsandbox as an opaque
//! string and never re-enters the SBPL parser, so a vault path containing spaces / parens / quotes
//! cannot inject profile syntax (the Chromium / WebKit Seatbelt pattern). Only fixed KEY *names* and our
//! own constant system paths are ever written into the profile text.
//!
//! Everything here is best-effort HARDENING on top of the offline worker + at-rest encryption: if setup
//! fails the caller runs the worker unconfined (logged with its `SBX-####` code, surfaced in the
//! Developer-mode readout) rather than break ingest — the same fail-OPEN contract as the other arms.
#![cfg(target_os = "macos")]

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::sidecar::{sbx, SbxError};

/// Absolute, hardcoded path to the profile applier. Deliberately NOT resolved through `PATH`: a
/// `PATH`-hijacked `sandbox-exec` would silently run the worker UNconfined while we believed it
/// sandboxed. This is the only binary we trust to impose the profile.
const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// The Seatbelt profile, with `(param "KEY")` placeholders for the five dynamic dirs (bound via `-D`)
/// and fixed system paths as literals. SBPL is evaluated last-match-wins, so `(deny default)` first then
/// narrow `(allow …)` lines produce an allow-list. See the module docs for the security rationale of
/// each block.
///
/// Kept as one const string (not `format!`'d) precisely so no runtime value is ever spliced into the
/// profile text — the injection-safety boundary.
const PROFILE: &str = r#"(version 1)
(deny default)
(deny network*)

;; --- Executable code: exec + read + memory-map-as-executable the interpreter trees and the system
;; libraries dyld resolves. These trees ONLY carry `process-exec*`/`file-map-executable`; the writable
;; staging dir below never does, so the worker cannot drop-and-run a binary (finding #7).
(allow process-exec*
    (subpath (param "VENV"))
    (subpath (param "PYROOT"))
    (subpath (param "SCRIPTS")))
(allow file-read* file-map-executable
    (subpath (param "VENV"))
    (subpath (param "PYROOT"))
    (subpath (param "SCRIPTS"))
    (subpath "/usr/lib")
    (subpath "/System/Library/Frameworks")
    (subpath "/System/Library/PrivateFrameworks")
    ;; The dyld shared cache. Its location moved into a Cryptex in macOS 13; list every location so one
    ;; profile boots the interpreter across macOS 11 → the current release.
    (subpath "/System/Library/dyld")
    (subpath "/System/Cryptexes/OS")
    (subpath "/System/Volumes/Preboot/Cryptexes/OS"))

;; --- Metadata (stat / path resolution) broadly: dyld and CoreFoundation stat paths across the whole
;; filesystem during load, so denying this cascades into boot failures. This exposes the vault's
;; directory STRUCTURE (names) but NEVER its bytes — file-read-DATA on the vault is never granted, so
;; the encrypted store and $HOME stay unreadable (documented residual, finding #6).
(allow file-read-metadata (subpath "/"))

;; --- App data: the model cache read-only (the unconfined --fetch helper is what writes it), and the
;; staging dir read+write (input copies + TMPDIR + cwd). Staging is deliberately not executable.
(allow file-read* (subpath (param "MODELS")))
(allow file-read* file-write* (subpath (param "STAGING")))

;; --- Device nodes libc / OpenSSL / onnxruntime touch.
(allow file-read* (literal "/dev/random") (literal "/dev/urandom"))
(allow file-read* file-write-data (literal "/dev/null"))

;; --- CPU / core + feature detection (onnxruntime + numpy thread-pool sizing, os.cpu_count). Broad by
;; design: newer macOS reads more hw.*/kern.* than any hand-picked list, and a too-tight list fails
;; opaquely — the preflight would catch it, but breadth avoids a needless fall-open.
(allow sysctl-read)

;; --- Self process management: `multiprocessing` (spawn re-execs the granted interpreter), thread
;; pools, POSIX semaphores, and self-signaling. None of these reach another process or the network.
(allow process-fork)
(allow process-info* (target self))
(allow signal (target self))
(allow ipc-posix-sem)

;; --- The single mach service a booting CPython may need: opendirectoryd's libinfo endpoint, reached
;; by getpwuid() when $HOME is unset (we also SET $HOME to the staging dir so this usually never fires).
;; This is NOT a hostname-resolution path — DNS goes through com.apple.mDNSResponder, which is
;; deliberately absent from this list — so allowing it does not open network egress (finding #1).
(allow mach-lookup (global-name "com.apple.system.opendirectoryd.libinfo"))
"#;

/// The generated profile plus the staging dir and the `-D` parameter bindings, built once per worker
/// spawn and reused. Unlike the Linux arm there is no `Degraded` state: `sandbox-exec` either applies
/// the profile (fully confined) or setup fails and the caller falls open — macOS confinement is
/// all-or-nothing.
pub struct Sandbox {
    /// Where untrusted input files are copied before the confined worker reads them (also the worker's
    /// cwd and `TMPDIR`, so `tempfile` lands somewhere the profile grants).
    staging_dir: PathBuf,
    /// The interpreter/model/script/staging dirs the worker can reach, for the Developer-mode readout
    /// only (the system trees are implied). Never consulted when building the command.
    granted: Vec<PathBuf>,
    /// The `-D` bindings the profile's `(param "…")` placeholders resolve to, in `(KEY, path)` form.
    params: Vec<(&'static str, PathBuf)>,
}

impl Sandbox {
    /// Idempotent setup: create the staging dir, confirm `sandbox-exec` is present, and compute the
    /// five `-D` path bindings. Returns `Err(SbxError)` — caller runs unconfined — only on a hard
    /// failure (staging dir, or a missing `sandbox-exec`).
    ///
    /// `base_python_home` is the base interpreter's `bin` dir (from `pyvenv.cfg` `home =`); its parent
    /// (the install root, holding `bin` + `lib`) is what we grant, so `lib/pythonX.Y` is reachable. PM
    /// ships its standalone interpreter under the `runtime/` dir, so this root is never `$HOME`.
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
        // The worker only reads the model cache; the unconfined --fetch helper writes it. Ensure the dir
        // exists so the read-only grant resolves (an absent path would still be denied-by-default, but a
        // present one keeps the readout honest).
        let _ = std::fs::create_dir_all(models_dir);

        // Deprecated but still shipping on every macOS; a genuinely missing binary is essentially
        // unreachable in practice. We fall OPEN rather than closed if it's absent, matching PM's
        // best-effort-hardening contract across all three platforms (the worker is already offline and
        // the store encrypted at rest).
        if !Path::new(SANDBOX_EXEC).exists() {
            return Err(SbxError::new(
                sbx::MAC_SANDBOX_EXEC,
                "/usr/bin/sandbox-exec is not present",
            ));
        }

        // The base interpreter's install root: `pyvenv.cfg home=` is its `bin` dir, so grant the parent
        // (which holds `bin` + `lib/pythonX.Y`). Never the grandparent — same over-broad-grant caution
        // as the Linux arm — though PM's bundled interpreter lives under `runtime/`, well away from the
        // vault regardless.
        let pyroot = base_python_home
            .parent()
            .unwrap_or(base_python_home)
            .to_path_buf();

        let params = vec![
            ("VENV", venv_dir.to_path_buf()),
            ("PYROOT", pyroot),
            ("SCRIPTS", script_dir.to_path_buf()),
            ("MODELS", models_dir.to_path_buf()),
            ("STAGING", staging_dir.clone()),
        ];

        // The concise "what the worker can see" set for the readout (system trees omitted for legibility).
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
            params,
        })
    }

    /// The container-writable dir input files are staged into (also the confined child's cwd + TMPDIR).
    pub fn staging_dir(&self) -> &Path {
        &self.staging_dir
    }

    /// The confinement mechanism, for the Developer-mode readout.
    pub fn mechanism(&self) -> &'static str {
        "macOS sandbox-exec (no network)"
    }

    /// The app dirs the worker can reach (for the readout only — see [`Sandbox::granted`]).
    pub fn granted_dirs(&self) -> Vec<String> {
        self.granted
            .iter()
            .map(|p| p.display().to_string())
            .collect()
    }

    /// The confinement axes enforced. macOS always enforces both (the profile is applied whole or the
    /// spawn falls open), so this is constant — present for parity with the Linux arm's readout.
    pub fn layers(&self) -> Vec<String> {
        vec!["network".to_string(), "filesystem".to_string()]
    }

    /// macOS confinement is all-or-nothing (never partially applied), so it is never `Degraded`. Present
    /// for interface parity with the Linux [`Sandbox`], whose seccomp-only fallback IS a degraded state.
    pub fn degraded(&self) -> Option<(&'static str, String)> {
        None
    }

    /// Build the confined worker command: `sandbox-exec -p <profile> -DKEY=VALUE … -- <py> <script>`.
    /// The caller then layers on the shared stdio/env/offline posture. The `-DKEY=VALUE` values are
    /// passed as single argv tokens (never shell-split), so a path with spaces/quotes stays intact, and
    /// as opaque profile parameters they cannot inject SBPL. Also sets cwd to staging and installs a
    /// close-on-exec fd sweep so no inherited handle rides into the worker (finding #2).
    pub fn wrap_command(&self, py: &Path, script: &Path) -> Command {
        let mut command = Command::new(SANDBOX_EXEC);
        command.arg("-p").arg(PROFILE);
        for (key, value) in &self.params {
            // Attached form `-DKEY=VALUE` as one token — the production (Chromium/codex) shape, with no
            // ambiguity about where the value begins. `value` is a real path; argv preserves it verbatim.
            command.arg(format!("-D{key}={}", value.display()));
        }
        command.arg("--").arg(py).arg(script);
        command.current_dir(&self.staging_dir);
        // SAFETY: the closure runs in the forked child before exec and does only async-signal-safe raw
        // syscalls (fcntl on integer fds — no allocation, no locks, no panics). See `close_extra_fds`.
        unsafe {
            command.pre_exec(close_extra_fds);
        }
        command
    }
}

/// Post-fork, pre-exec: mark every inherited fd ≥ 3 close-on-exec so a leaked vault-DB handle or open
/// TLS socket cannot ride into the confined worker (finding #2 — the sandbox restricts syscalls, not
/// fds you already hold). macOS has no `close_range`/`closefrom`, so bound an `fcntl` loop by the fd
/// table size. stdio (0/1/2 — the pipes) is left untouched; the marked fds close when `sandbox-exec`
/// execs. Runs between `fork()` and `execvp()` in a multi-threaded parent, so it must allocate nothing
/// and take no locks — hence raw `libc` on integer fds only.
fn close_extra_fds() -> std::io::Result<()> {
    unsafe {
        // `getdtablesize()` is the per-process soft fd cap; clamp to a sane range in case it's absurd.
        let max = {
            let n = libc::getdtablesize();
            if (4..(1 << 20)).contains(&n) {
                n
            } else {
                4096
            }
        };
        let mut fd: libc::c_int = 3;
        while fd < max {
            // EBADF on a gap in the fd table is expected and ignored; we only ever ADD FD_CLOEXEC.
            let _ = libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            fd += 1;
        }
    }
    Ok(())
}
