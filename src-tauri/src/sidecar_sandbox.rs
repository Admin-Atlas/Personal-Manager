// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Windows sidecar confinement (#286 PR2b): run the untrusted-file WORKER in a no-network AppContainer.
//!
//! The long-lived worker parses fully-untrusted file bytes. PR1 made it run OFFLINE by configuration;
//! this makes it OS-enforced: the worker launches inside an AppContainer with ZERO capabilities, so the
//! kernel refuses every outbound socket, and it can read only the folders whose ACL we explicitly grant
//! the container SID (the venv, the base interpreter, the model cache, and a staging dir) — never the
//! vault. The short-lived `--fetch` helper stays a normal process (it needs the network to download
//! models); it is the one component allowed out, and it never touches untrusted file bytes.
//!
//! Feasibility was proven before this was written: the venv Python + onnxruntime + fastembed +
//! faster-whisper all run inside such a container, and an outbound socket is refused.
//!
//! Everything here is a best-effort HARDENING layer on top of the offline worker + at-rest encryption:
//! if setup fails (`ensure` returns `Err(reason)`) the caller runs the worker unconfined rather than
//! break ingest — failing open is the right trade for defence in depth on an alpha, and is logged (the
//! reason also surfaces in the Developer-mode sandbox readout).

#![cfg(windows)]

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::fs::File;
use std::mem::size_of;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::process::Command;

use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, SetHandleInformation, HANDLE, HANDLE_FLAGS, HANDLE_FLAG_INHERIT, HLOCAL,
};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows::Win32::Security::{PSID, SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
    TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject, CREATE_NO_WINDOW,
    CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, INFINITE,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

use crate::error::{Error, Result};

/// Stable AppContainer name. Derived deterministically to the same SID on every run, so the profile is
/// created once and the (slow) filesystem grants are cached against it. Never rename — see the bundle-id
/// rule; a rename orphans the grants.
const CONTAINER_NAME: &str = "org.itsatlas.pm.sidecar.worker";

fn other<E: std::fmt::Display>(ctx: &str, e: E) -> Error {
    Error::Other(format!("sidecar sandbox: {ctx}: {e}"))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The AppContainer identity plus the staging dir, set up once and reused for every worker spawn.
pub struct Sandbox {
    /// Where untrusted input files are copied before the confined worker reads them.
    staging_dir: PathBuf,
    /// The dirs whose ACL grants the container SID access (staging F, everything else RX). Retained
    /// only so the Developer-mode readout can show the confined worker's exact filesystem view; it is
    /// never consulted at runtime. Includes the marker-cached trees even on a run that reused the
    /// cache (they stay granted from the run that set them), so it reflects what the container CAN
    /// read, not just what was granted this launch.
    granted: Vec<PathBuf>,
}

impl Sandbox {
    /// Idempotent setup: create the AppContainer profile (or derive its SID if it already exists), then
    /// grant that SID read/execute on the interpreter + model trees and full control on a staging dir.
    /// The tree grants walk thousands of files, so they run once and are cached behind a marker keyed on
    /// the SID (a SID change re-grants).
    ///
    /// Returns `Err(reason)` — caller runs unconfined — if any step fails; the `reason` is a short human
    /// string the caller both logs and surfaces in the Developer-mode readout, so a fall-open is visible
    /// without digging the log.
    ///
    /// `runtime_dir` is the parent of the venv (`…/runtime`); the staging dir lives under it. The grant
    /// marker lives INSIDE the venv, so a venv rebuild deletes it and re-triggers the grant.
    pub fn ensure(
        venv_dir: &Path,
        base_python_dir: &Path,
        models_dir: &Path,
        script_dir: &Path,
        runtime_dir: &Path,
    ) -> std::result::Result<Sandbox, String> {
        let sid_string = ensure_profile_sid().map_err(|e| format!("profile: {e}"))?;

        let staging_dir = runtime_dir.join("sandbox-in");
        std::fs::create_dir_all(&staging_dir).map_err(|e| format!("staging dir: {e}"))?;

        // Grant the small, always-needed dirs every time (cheap, few files): the staging dir (writable)
        // and the script dir that holds `pm_sidecar.py` (readable). The script dir is NOT marker-cached
        // because its location differs between the dev repo and the bundled release resource dir. The
        // big interpreter/model trees ARE grant-once, marker-cached on the SID.
        icacls_grant(&staging_dir, &sid_string, "(OI)(CI)F")
            .map_err(|e| format!("staging grant: {e}"))?;
        icacls_grant(script_dir, &sid_string, "(OI)(CI)RX")
            .map_err(|e| format!("script grant: {e}"))?;

        // The models dir is granted EVERY spawn, NOT marker-cached: it is cheap (a handful of model
        // files, not the venv's ~17k) and, unlike the venv, it is routinely deleted and re-created out
        // from under us — the fetcher re-downloads into it, and a cache reset / cold refresh wipes it.
        // A recreated models dir inherits `runtime`'s DACL (no container ACE), so a cached "granted"
        // marker would leave the confined worker denied on freshly downloaded models (WinError 5 on
        // `models\fastembed`). Granting the (possibly empty) dir here with inheritance means the models
        // the fetcher writes right afterwards inherit the ACE. Read/execute only — the worker never
        // writes the cache; downloads are the unconfined fetcher's job.
        let _ = std::fs::create_dir_all(models_dir);
        icacls_grant(models_dir, &sid_string, "(OI)(CI)RX")
            .map_err(|e| format!("models grant: {e}"))?;

        // The venv + base interpreter ARE grant-once, marker-cached on the SID (the venv walk is the
        // expensive one, ~17k files). The marker lives INSIDE the venv (not runtime_dir) on purpose: a
        // venv rebuild (`remove_dir_all(venv_dir)` on a torn/too-old install) deletes it, forcing a
        // re-grant — the recreated venv_dir would otherwise inherit runtime_dir's DACL (no container
        // ACE) while a runtime_dir marker still claimed "granted", leaving the confined worker unable to
        // read the fresh venv.
        let marker = venv_dir.join(".pm-sandbox-granted");
        let granted_for = std::fs::read_to_string(&marker).unwrap_or_default();
        if granted_for.trim() != sid_string {
            for (path, label) in [(venv_dir, "venv"), (base_python_dir, "base python")] {
                icacls_grant(path, &sid_string, "(OI)(CI)RX")
                    .map_err(|e| format!("{label} grant: {e}"))?;
            }
            let _ = std::fs::write(&marker, &sid_string);
        }

        // The container's readable set, kept only for the readout. Order = broadest first.
        let granted = vec![
            venv_dir.to_path_buf(),
            base_python_dir.to_path_buf(),
            models_dir.to_path_buf(),
            script_dir.to_path_buf(),
            staging_dir.clone(),
        ];
        Ok(Sandbox {
            staging_dir,
            granted,
        })
    }

    /// The container-writable dir input files are staged into (also a sane cwd for the confined child).
    pub fn staging_dir(&self) -> &Path {
        &self.staging_dir
    }

    /// The stable AppContainer name, for the Developer-mode readout.
    pub fn container_name(&self) -> &'static str {
        CONTAINER_NAME
    }

    /// The dirs the container SID is granted (for the Developer-mode readout only — see [`Sandbox`]).
    pub fn granted_dirs(&self) -> Vec<String> {
        self.granted
            .iter()
            .map(|p| p.display().to_string())
            .collect()
    }

    /// Copy `src` into the granted staging dir under a unique name, returning a handle whose path the
    /// confined worker CAN read and which is deleted on drop. The worker's filesystem view is thus the
    /// venv + models + exactly the one file being parsed — never the user's real tree or the vault.
    pub fn stage_input(&self, src: &Path) -> Result<StagedInput> {
        let ext = src
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"))
            .unwrap_or_default();
        // Unique name without pulling in a uuid dep here: a monotonic counter is enough (single
        // process, serialized sidecar), and the file is deleted right after the request.
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dst = self.staging_dir.join(format!("in-{n}{ext}"));
        std::fs::copy(src, &dst).map_err(|e| other("stage input", e))?;
        Ok(StagedInput { path: dst })
    }

    /// Launch `exe` with `args` inside the no-network AppContainer, with `envs` overriding and `removes`
    /// dropping keys from the inherited environment, and `cwd` as the working directory. Returns the
    /// running process handle plus its stdin (write) and stdout (read) pipe ends.
    pub fn spawn_confined(
        &self,
        exe: &Path,
        args: &[&Path],
        envs: &[(&str, &str)],
        removes: &[&str],
        cwd: &Path,
    ) -> Result<ConfinedChild> {
        unsafe { self.spawn_confined_inner(exe, args, envs, removes, cwd) }
    }

    unsafe fn spawn_confined_inner(
        &self,
        exe: &Path,
        args: &[&Path],
        envs: &[(&str, &str)],
        removes: &[&str],
        cwd: &Path,
    ) -> Result<ConfinedChild> {
        // Derive the container SID fresh for this launch (cheap); freed at the end of the call.
        let name = HSTRING::from(CONTAINER_NAME);
        let sid: PSID = DeriveAppContainerSidFromAppContainerName(PCWSTR(name.as_ptr()))
            .map_err(|e| other("derive sid", e))?;
        let _sid_guard = SidGuard(sid);

        // Inheritable pipes for the child's stdin/stdout/stderr; the child ends inherit, the parent ends
        // do not. These three are the ONLY handles we let the child inherit (whitelist below).
        let (stdin_read, stdin_write) = make_pipe(PipeInherit::Read)?;
        let (stdout_read, stdout_write) = make_pipe(PipeInherit::Write)?;
        let (stderr_read, stderr_write) = make_pipe(PipeInherit::Write)?;

        let sec_caps = SECURITY_CAPABILITIES {
            AppContainerSid: sid,
            Capabilities: std::ptr::null_mut(),
            CapabilityCount: 0,
            Reserved: 0,
        };
        // The ONLY handles the confined worker may inherit. Without this whitelist, bInheritHandles=TRUE
        // inherits EVERY currently-inheritable handle in the app — a live vault-DB handle or an open
        // network socket would then be usable INSIDE the container, escaping the filesystem ACL and the
        // no-network capability. Pinning the list to the three pipes closes that whole class.
        let inherit: [HANDLE; 3] = [stdin_read.0, stdout_write.0, stderr_write.0];

        // Two attributes: the AppContainer security capabilities, and the inherited-handle whitelist.
        let mut size: usize = 0;
        let _ = InitializeProcThreadAttributeList(None, 2, None, &mut size);
        let mut attr_buf = vec![0u8; size];
        let attr_list = LPPROC_THREAD_ATTRIBUTE_LIST(attr_buf.as_mut_ptr() as *mut c_void);
        InitializeProcThreadAttributeList(Some(attr_list), 2, None, &mut size)
            .map_err(|e| other("init attr list", e))?;
        let attr_guard = AttrListGuard(attr_list);
        UpdateProcThreadAttribute(
            attr_list,
            0,
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
            Some(&sec_caps as *const _ as *const c_void),
            size_of::<SECURITY_CAPABILITIES>(),
            None,
            None,
        )
        .map_err(|e| other("update caps attr", e))?;
        UpdateProcThreadAttribute(
            attr_list,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            Some(inherit.as_ptr() as *const c_void),
            std::mem::size_of_val(&inherit),
            None,
            None,
        )
        .map_err(|e| other("update handle-list attr", e))?;

        let mut si = STARTUPINFOEXW::default();
        si.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.StartupInfo.hStdInput = stdin_read.0;
        si.StartupInfo.hStdOutput = stdout_write.0;
        si.StartupInfo.hStdError = stderr_write.0;
        si.lpAttributeList = attr_list;

        // Command line: "exe" "arg1" "arg2" …  (quoted; our paths never contain quotes).
        let mut cmd = format!("\"{}\"", exe.display());
        for a in args {
            cmd.push_str(&format!(" \"{}\"", a.display()));
        }
        let mut cmd_wide = wide(&cmd);
        let cwd_wide = wide(&cwd.to_string_lossy());
        let mut env_block = build_env_block(envs, removes);

        let mut pi = PROCESS_INFORMATION::default();
        CreateProcessW(
            PCWSTR::null(),
            Some(PWSTR(cmd_wide.as_mut_ptr())),
            None,
            None,
            true, // inherit handles — but ONLY the three whitelisted above
            EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
            Some(env_block.as_mut_ptr() as *mut c_void),
            PCWSTR(cwd_wide.as_ptr()),
            &si.StartupInfo,
            &mut pi,
        )
        .map_err(|e| other("CreateProcessW", e))?;

        // Close our copies of the child ends so EOF propagates when the child closes them; the parent
        // ends stay (stdin write / stdout read → the Files we return; stderr read → the drain below).
        drop(stdin_read);
        drop(stdout_write);
        drop(stderr_write);
        drop(attr_guard);
        let _ = CloseHandle(pi.hThread);

        // Forward the confined worker's stderr (tracebacks / progress) to ours — the equivalent of the
        // std spawn's inherited stderr. The thread ends when the worker exits and its write end closes.
        let stderr_owned = OwnedHandle::from_raw_handle(stderr_read.into_raw());
        std::thread::spawn(move || {
            let mut src = File::from(stderr_owned);
            let _ = std::io::copy(&mut src, &mut std::io::stderr());
        });

        let stdin = File::from_raw_handle(stdin_write.into_raw());
        let stdout = File::from_raw_handle(stdout_read.into_raw());
        Ok(ConfinedChild {
            process: pi.hProcess,
            stdin: Some(stdin),
            stdout: Some(stdout),
        })
    }
}

/// A staged copy of an untrusted input file, deleted when dropped.
pub struct StagedInput {
    path: PathBuf,
}

impl StagedInput {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StagedInput {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A confined worker process plus its stdio pipe ends. `stdin`/`stdout` are taken by the caller into the
/// generic `Process`; the handle drives kill/wait.
pub struct ConfinedChild {
    process: HANDLE,
    pub stdin: Option<File>,
    pub stdout: Option<File>,
}

// SAFETY: a Windows process HANDLE is a kernel object usable from any thread; kill/wait/close are
// thread-agnostic Win32 calls, and the File ends are already Send. Needed so the generic `Process` that
// boxes this can live in the manager's `Mutex`, shared across the app's threads.
unsafe impl Send for ConfinedChild {}

impl ConfinedChild {
    pub fn kill(&mut self) {
        unsafe {
            let _ = TerminateProcess(self.process, 1);
        }
    }

    pub fn wait(&mut self) {
        unsafe {
            let _ = WaitForSingleObject(self.process, INFINITE);
        }
    }
}

impl Drop for ConfinedChild {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.process);
        }
    }
}

// --- helpers -----------------------------------------------------------------------------------

/// Create the AppContainer profile if absent, and return its SID as a string. Deriving the SID from the
/// name is deterministic, so this is safe to call every launch.
fn ensure_profile_sid() -> Result<String> {
    unsafe {
        let name = HSTRING::from(CONTAINER_NAME);
        let display = HSTRING::from("PM document sidecar");
        let desc =
            HSTRING::from("Confines PM's file-parsing helper (no network, no vault access).");
        let sid: PSID = match CreateAppContainerProfile(
            PCWSTR(name.as_ptr()),
            PCWSTR(display.as_ptr()),
            PCWSTR(desc.as_ptr()),
            None,
        ) {
            Ok(s) => s,
            // Already registered (or a transient error): fall back to deriving the deterministic SID.
            Err(_) => DeriveAppContainerSidFromAppContainerName(PCWSTR(name.as_ptr()))
                .map_err(|e| other("derive sid", e))?,
        };
        let _guard = SidGuard(sid);
        let mut ptr = PWSTR::null();
        ConvertSidToStringSidW(sid, &mut ptr).map_err(|e| other("sid->string", e))?;
        let s = ptr.to_string().map_err(|e| other("sid utf16", e))?;
        let _ = LocalFree(Some(HLOCAL(ptr.0 as *mut c_void)));
        Ok(s)
    }
}

/// Additive `icacls` grant of `perms` for the container SID string on `path` (recursive). Additive —
/// it never removes the owner's existing access. Errors on a non-zero exit.
fn icacls_grant(path: &Path, sid: &str, perms: &str) -> Result<()> {
    let out = Command::new("icacls")
        .arg(path)
        .arg("/grant")
        .arg(format!("*{sid}:{perms}"))
        .arg("/T")
        .arg("/Q")
        .output()
        .map_err(|e| other("run icacls", e))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(other(
            "icacls",
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .last()
                .unwrap_or("failed"),
        ))
    }
}

/// Build a `CREATE_UNICODE_ENVIRONMENT` block: the parent environment, minus `removes`, plus `envs`
/// (override or add), as a sorted, double-NUL-terminated UTF-16 buffer.
fn build_env_block(envs: &[(&str, &str)], removes: &[&str]) -> Vec<u16> {
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::OsStrExt;
    // `vars_os`, NOT `vars`: `std::env::vars()` PANICS on any environment entry that isn't valid Unicode
    // (the Windows environment is UTF-16 and can hold unpaired surrogates), and this runs under the held
    // process mutex — a panic there would poison it and wedge the sidecar. Overrides are applied after
    // the parent copy, so an override (e.g. HF_HUB_OFFLINE=1) wins over an inherited value.
    let mut map: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    for k in removes {
        map.remove(OsStr::new(k));
    }
    for (k, v) in envs {
        map.insert(OsString::from(k), OsString::from(v));
    }
    let mut block: Vec<u16> = Vec::new();
    for (k, v) in map {
        block.extend(k.encode_wide());
        block.push(b'=' as u16);
        block.extend(v.encode_wide());
        block.push(0);
    }
    block.push(0); // terminating NUL for the block
    block
}

/// Which end of a pipe the CHILD keeps (and must stay inheritable); the parent's end is made
/// non-inheritable so the child can't accidentally hold a copy.
enum PipeInherit {
    Read,
    Write,
}

/// Create an anonymous pipe and set only the child's end inheritable. Returns `(read, write)`.
fn make_pipe(child: PipeInherit) -> Result<(OwnedH, OwnedH)> {
    unsafe {
        let sa = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: true.into(),
        };
        let mut read = HANDLE::default();
        let mut write = HANDLE::default();
        CreatePipe(&mut read, &mut write, Some(&sa), 0).map_err(|e| other("CreatePipe", e))?;
        // Make the PARENT's end non-inheritable.
        let parent = match child {
            PipeInherit::Read => write,
            PipeInherit::Write => read,
        };
        SetHandleInformation(parent, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0))
            .map_err(|e| other("SetHandleInformation", e))?;
        Ok((OwnedH(read), OwnedH(write)))
    }
}

/// A HANDLE closed on drop, unless taken via `into_raw`.
struct OwnedH(HANDLE);
impl OwnedH {
    fn into_raw(self) -> *mut c_void {
        let h = self.0 .0;
        std::mem::forget(self);
        h
    }
}
impl Drop for OwnedH {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Frees an AppContainer SID (allocated by Create/Derive) on drop via LocalFree.
struct SidGuard(PSID);
impl Drop for SidGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.0 .0)));
        }
    }
}

/// Frees a proc-thread attribute list on drop.
struct AttrListGuard(LPPROC_THREAD_ATTRIBUTE_LIST);
impl Drop for AttrListGuard {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.0);
        }
    }
}
