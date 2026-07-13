// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Proton Drive integration for encrypted backup: locate the official `proton-drive`
//! CLI on this machine and (later in this PR) push/pull `.pmbackup` archives through it.
//!
//! **Provisioning model = locate-then-guide** (decided over auto-download). PM never
//! bundles or downloads the CLI (a ~116 MiB Bun-embedded binary); it probes for an
//! existing install and, when absent, the Backup UI points the user at Proton's official
//! download page. The CLI owns the Proton session itself — browser sign-in, stored in the
//! OS secret store under service `ch.proton.drive/drive-sdk-cli` — so PM never handles
//! Proton credentials. This is the same shell-out trust model as the Python sidecar
//! ([`crate::sidecar`]); the difference is PM does not manage the binary's lifecycle.
//!
//! The CLI is the official one from <https://github.com/ProtonDriveApps/sdk> (`cli/`),
//! invoked as `proton-drive <group> <command> … --json`. The user's real Drive root is
//! `/my-files`; PM's archives live in a fixed, human-recognizable `Personal Manager
//! Backups` folder there (the CLI has no hidden app-private area, so a clear name is the
//! mitigation).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::naming::{self, BackupEntry, ARCHIVE_EXT};
use crate::error::{Error, Result};

/// Official download page for the Proton Drive CLI (per-OS pre-built binaries). The Backup
/// UI links here when the CLI is not found; PM does not fetch it automatically.
pub const INSTALL_URL: &str = "https://proton.me/support/drive-cli";

/// Settings key holding a user-chosen absolute path to the `proton-drive` binary (the Backup UI's
/// "Locate manually…" escape hatch, for when the portable binary lives somewhere auto-detection
/// doesn't probe). Empty / absent means "auto-detect only".
pub(crate) const CLI_PATH_SETTING: &str = "proton_cli_path";

/// The user's real Drive root (the CLI's `/` only lists virtual sections). Everything PM
/// writes lives under here.
const REMOTE_ROOT: &str = "/my-files";
/// The single, fixed, human-recognizable folder PM keeps its archives in (visible in the
/// Proton apps — the CLI has no hidden app-private area, so a clear name is the mitigation).
const BACKUP_FOLDER_NAME: &str = "Personal Manager Backups";
/// `"/my-files/Personal Manager Backups"` — the remote directory archives are pushed to.
const REMOTE_BACKUP_DIR: &str = "/my-files/Personal Manager Backups";

/// Message PM raises (and the UI shows) when the CLI reports no active session. Detected via
/// [`looks_not_logged_in`] so `proton_status` can report "not connected" rather than erroring.
pub(crate) const NOT_CONNECTED_MSG: &str = "Not signed in to Proton Drive — connect your account.";

/// The executable file name(s) to probe, most-specific first. Windows carries `.exe`.
fn cli_binary_names() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        &["proton-drive.exe"]
    }
    #[cfg(not(windows))]
    {
        &["proton-drive"]
    }
}

/// Well-known directories (beyond `PATH`) a user might have dropped the CLI into. Pure over
/// its injected environment values so it stays unit-testable and platform-agnostic: a
/// caller passes whichever of `%LOCALAPPDATA%` / `%ProgramFiles%` / `$HOME` exist, and only
/// the matching candidates are produced (non-existent ones are skipped by the `is_file`
/// probe later, so listing a Windows dir on Unix is harmless).
fn extra_install_dirs(
    local_app_data: Option<&Path>,
    program_files: Option<&Path>,
    home: Option<&Path>,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(lad) = local_app_data {
        dirs.push(lad.join("Programs").join("proton-drive"));
        dirs.push(lad.join("proton-drive"));
    }
    if let Some(pf) = program_files {
        dirs.push(pf.join("Proton").join("Drive"));
        dirs.push(pf.join("proton-drive"));
    }
    if let Some(h) = home {
        dirs.push(h.join(".local").join("bin"));
        dirs.push(h.join("bin"));
        // The CLI is a portable single binary, so users often just leave it where it downloaded —
        // most commonly the browser's default folder — rather than "installing" it onto PATH.
        dirs.push(h.join("Downloads"));
        dirs.push(h.join("Desktop"));
    }
    dirs
}

/// Return the first `dir/name` that is an existing file. Split out from the environment
/// plumbing so it can be exercised directly in tests.
fn find_cli_in_dirs<I>(dirs: I, names: &[&str]) -> Option<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    for dir in dirs {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Locate the `proton-drive` CLI, or `None` if it isn't found. A user-set `override_path` (from the
/// Backup UI's "Locate manually…" picker) wins outright — the CLI is a portable binary that can live
/// anywhere, so an explicit pointer is the reliable escape hatch when auto-detection misses. Otherwise
/// checks `PATH` first (where an installed CLI is meant to land), then the per-OS install + common
/// download directories. Cheap enough to call on demand — a handful of `stat`s, no process spawn.
pub(crate) fn locate_proton_cli(override_path: Option<&Path>) -> Option<PathBuf> {
    // 0) An explicit user-chosen path wins, as long as it still points at a real file.
    if let Some(p) = override_path {
        if p.is_file() {
            return Some(p.to_path_buf());
        }
    }

    let names = cli_binary_names();

    // 1) Anything on PATH wins — that's where a CLI install is meant to land.
    if let Some(path) = std::env::var_os("PATH") {
        let hit = find_cli_in_dirs(std::env::split_paths(&path), names);
        if hit.is_some() {
            return hit;
        }
    }

    // 2) Fall back to well-known install locations (Windows has no reliable PATH entry for
    //    a per-user install; `%LOCALAPPDATA%\Programs` is the common target).
    let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from);
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    let dirs = extra_install_dirs(
        local_app_data.as_deref(),
        program_files.as_deref(),
        home.as_deref(),
    );
    find_cli_in_dirs(dirs, names)
}

// --- Invoking the CLI ---------------------------------------------------------------

/// A generous ceiling on any single Proton CLI call. Without it a hung transfer (a stalled network)
/// holds `backup_busy` forever — blocking every later backup/restore until the app restarts — and
/// Cancel does nothing during the longest phase (F-13). 2h doubles the 1h gdrive reqwest ceiling,
/// since the CLI path is slower.
const CLI_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);
/// How often the poll loop wakes to check for child exit / deadline / cancel — responsive to Cancel
/// (a fraction of a second) without busy-spinning the CPU.
const CLI_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Why a polled CLI run was stopped before the child exited. Cancel takes priority over timeout
/// (both can read true if the deadline lapses in the same tick a cancel lands). Pure, so the
/// priority is unit-tested.
#[derive(Debug, PartialEq, Eq)]
enum CliStop {
    Cancelled,
    TimedOut,
}

fn cli_stop_reason(cancelled: bool, timed_out: bool) -> Option<CliStop> {
    if cancelled {
        Some(CliStop::Cancelled)
    } else if timed_out {
        Some(CliStop::TimedOut)
    } else {
        None
    }
}

/// Windows: run the child without flashing a console window. No-op elsewhere. Mirrors the
/// sidecar's helper (kept local — one line, avoids a shared-module dependency).
#[cfg(windows)]
fn no_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}
#[cfg(not(windows))]
fn no_window(_command: &mut Command) {}

/// Does this CLI output signal "no active session"? Deliberately narrow — matches the CLI's
/// actual `You need to login first` (captured), NOT the `auth login` flow's "Sign in in your
/// browser" prompt, so a *connect* is never misread as *not-connected*.
fn looks_not_logged_in(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    l.contains("need to login") || l.contains("need to log in") || l.contains("not logged in")
}

/// Run `proton-drive <args…>` to completion and return stdout on success. Blocking — call from
/// `spawn_blocking`, never while holding the DB lock (like the sidecar). A "not signed in"
/// result (on any exit code — the CLI is inconsistent) is mapped to [`NOT_CONNECTED_MSG`] so
/// callers can distinguish it; any other non-zero exit surfaces the trimmed CLI output.
///
/// This is a thin wrapper over [`run_proton_polled`] with no cancel flag — so every CLI call (list,
/// connection probe, ensure-folder, disconnect, trash, retention) still gets the [`CLI_TIMEOUT`]
/// ceiling for free; only the transfer phases additionally honour Cancel.
fn run_proton(cli: &Path, args: &[&str]) -> Result<String> {
    run_proton_polled(cli, args, None)
}

/// [`run_proton`], but spawned and polled so it can be bounded by [`CLI_TIMEOUT`] and — when a
/// `cancel` flag is supplied (the transfer phases) — stopped promptly when the user hits Cancel. On
/// either stop the child is killed and reaped, which lets the caller's `BusyGuard` drop and release
/// `backup_busy` instead of wedging it until app restart (F-13). Blocking — call from
/// `spawn_blocking`, never while holding the DB lock.
fn run_proton_polled(
    cli: &Path,
    args: &[&str],
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<String> {
    use std::sync::atomic::Ordering;

    let mut cmd = Command::new(cli);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    no_window(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| Error::Other(format!("could not run the Proton Drive CLI: {e}")))?;

    // Drain both pipes on side threads: a chatty CLI could otherwise fill a pipe buffer and block
    // (deadlock) while we're polling for exit rather than reading. Each thread ends at EOF, which
    // arrives when the child exits or we kill it.
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let out_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = stdout_pipe.as_mut() {
            let _ = std::io::Read::read_to_end(p, &mut buf);
        }
        buf
    });
    let err_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = stderr_pipe.as_mut() {
            let _ = std::io::Read::read_to_end(p, &mut buf);
        }
        buf
    });

    let deadline = Instant::now() + CLI_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                let cancelled = cancel.is_some_and(|c| c.load(Ordering::Relaxed));
                let timed_out = Instant::now() >= deadline;
                if let Some(reason) = cli_stop_reason(cancelled, timed_out) {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = out_thread.join();
                    let _ = err_thread.join();
                    return Err(Error::Other(match reason {
                        CliStop::Cancelled => "Backup cancelled.".into(),
                        CliStop::TimedOut => {
                            "The Proton Drive CLI timed out and was stopped.".into()
                        }
                    }));
                }
                std::thread::sleep(CLI_POLL_INTERVAL);
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Other(format!(
                    "could not run the Proton Drive CLI: {e}"
                )));
            }
        }
    };

    let stdout = String::from_utf8_lossy(&out_thread.join().unwrap_or_default()).into_owned();
    let err_bytes = err_thread.join().unwrap_or_default();
    let stderr = String::from_utf8_lossy(&err_bytes);
    // "not signed in" is a plain-text message the CLI prints on failure. Only treat it as such
    // when it's NOT inside an expected JSON payload — otherwise a decrypted node *name* that
    // happens to contain "need to login" would misclassify a genuine listing as not-connected.
    // So: check stderr always, and stdout only when it isn't the JSON we asked for.
    let stdout_is_json = {
        let t = stdout.trim_start();
        t.starts_with('[') || t.starts_with('{')
    };
    if looks_not_logged_in(&stderr) || (!stdout_is_json && looks_not_logged_in(&stdout)) {
        return Err(Error::Other(NOT_CONNECTED_MSG.into()));
    }
    if status.success() {
        Ok(stdout)
    } else {
        let detail = format!("{}\n{}", stdout.trim(), stderr.trim());
        Err(Error::Other(format!(
            "Proton Drive CLI error: {}",
            detail.trim()
        )))
    }
}

// --- Parsing `--json` output --------------------------------------------------------

/// The CLI wraps decryptable fields (names, revisions) in a `{ ok, value }` Result so an
/// un-decryptable node still serializes. We only read `value` when `ok`.
#[derive(Deserialize)]
struct ProtonResult<T> {
    ok: bool,
    value: Option<T>,
}

/// The subset of a Drive node PM needs from `filesystem list`/`create-folder --json`. Unknown
/// fields are ignored (serde default), so a future CLI adding fields won't break parsing.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteNode {
    name: Option<ProtonResult<String>>,
    #[serde(rename = "type")]
    node_type: Option<String>,
    active_revision: Option<ProtonResult<RevisionValue>>,
    owned_by: Option<OwnedBy>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RevisionValue {
    /// The real (cleartext) content size in bytes; `storageSize` is the larger encrypted size.
    claimed_size: Option<u64>,
}

#[derive(Deserialize)]
struct OwnedBy {
    email: Option<String>,
}

impl RemoteNode {
    /// The node's cleartext name, or `None` if it wasn't decryptable.
    fn decoded_name(&self) -> Option<&str> {
        self.name
            .as_ref()
            .filter(|r| r.ok)
            .and_then(|r| r.value.as_deref())
    }
    fn is_file(&self) -> bool {
        self.node_type.as_deref() == Some("file")
    }
    fn is_folder(&self) -> bool {
        self.node_type.as_deref() == Some("folder")
    }
    fn claimed_size(&self) -> Option<u64> {
        self.active_revision
            .as_ref()
            .and_then(|r| r.value.as_ref())
            .and_then(|v| v.claimed_size)
    }
}

/// The upload/download summary object (`filesystem upload/download --json`).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransferSummary {
    #[serde(default)]
    transferred_items: u64,
    #[serde(default)]
    failed_items: u64,
    /// Per-item failure details. Cross-checked against `failed_items` (belt-and-braces: the CLI
    /// could populate this while reporting `failedItems: 0`).
    #[serde(default)]
    failures: Vec<serde_json::Value>,
}

fn parse_nodes(stdout: &str) -> Result<Vec<RemoteNode>> {
    // `list` prints a (multi-line) JSON array of nodes; a trimmed whole-stdout parse handles it.
    // (`create-folder`'s single-object output is intentionally discarded by its caller.)
    serde_json::from_str(stdout.trim())
        .map_err(|e| Error::Other(format!("could not parse Proton Drive listing: {e}")))
}

fn node_to_entry(n: &RemoteNode) -> Option<BackupEntry> {
    if !n.is_file() {
        return None;
    }
    let name = n.decoded_name()?;
    if !name.ends_with(ARCHIVE_EXT) {
        return None;
    }
    Some(BackupEntry {
        name: name.to_string(),
        size: n.claimed_size(),
    })
}

/// Filter a `list --json` payload down to PM's archives, newest first. Names are
/// `pm-backup-<UTC-stamp>.pmbackup`, so a reverse lexical sort is reverse-chronological.
fn parse_backup_listing(stdout: &str) -> Result<Vec<BackupEntry>> {
    let mut entries: Vec<BackupEntry> = parse_nodes(stdout)?
        .iter()
        .filter_map(node_to_entry)
        .collect();
    entries.sort_by(|a, b| b.name.cmp(&a.name));
    Ok(entries)
}

/// The account email off the first node that carries one (for the "Connected as …" line).
fn first_owner_email(stdout: &str) -> Option<String> {
    parse_nodes(stdout)
        .ok()?
        .into_iter()
        .find_map(|n| n.owned_by.and_then(|o| o.email))
}

fn check_transfer(stdout: &str) -> Result<()> {
    let summary: TransferSummary = serde_json::from_str(stdout.trim())
        .map_err(|e| Error::Other(format!("could not parse Proton Drive transfer result: {e}")))?;
    let failed = summary.failed_items.max(summary.failures.len() as u64);
    if failed > 0 {
        return Err(Error::Other(format!(
            "{failed} item(s) failed to transfer to/from Proton Drive"
        )));
    }
    // We always transfer exactly one archive, so zero transferred means it silently didn't
    // happen — never report that as a successful backup.
    if summary.transferred_items == 0 {
        return Err(Error::Other(
            "Proton Drive reported that nothing was transferred".into(),
        ));
    }
    Ok(())
}

// --- Operations ----------------------------------------------------------------------

/// Connection state for the Backup UI. `connected` drives the whole Proton section; `account`
/// is best-effort (absent if the CLI returned no owned node); `error` carries a real failure
/// (network, CLI crash) as distinct from a clean "not signed in".
#[derive(Serialize)]
pub struct ProtonConnStatus {
    pub connected: bool,
    pub account: Option<String>,
    pub error: Option<String>,
}

/// Probe whether the CLI has an active session, via a cheap auth-required listing of the root.
/// Never returns `Err` for the ordinary "not signed in" case — that's `connected: false`.
pub(crate) fn connection(cli: &Path) -> ProtonConnStatus {
    match run_proton(cli, &["filesystem", "list", REMOTE_ROOT, "--json"]) {
        Ok(stdout) => ProtonConnStatus {
            connected: true,
            account: first_owner_email(&stdout),
            error: None,
        },
        Err(e) if e.to_string() == NOT_CONNECTED_MSG => ProtonConnStatus {
            connected: false,
            account: None,
            error: None,
        },
        Err(e) => ProtonConnStatus {
            connected: false,
            account: None,
            error: Some(e.to_string()),
        },
    }
}

/// Extract the first `https://…` token from a line of CLI output, stopping at the first
/// whitespace or control byte (so a trailing ANSI reset or newline is excluded) and trimming
/// trailing punctuation. `auth login` prints the sign-in URL in its output; PM opens it.
fn first_https_url(line: &str) -> Option<String> {
    let start = line.find("https://")?;
    let rest = &line[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c.is_control())
        .unwrap_or(rest.len());
    let url = rest[..end].trim_end_matches(['"', '\'', ')', ']', '>', '.', ',']);
    (url.len() > "https://".len()).then(|| url.to_string())
}

/// Open the first sign-in URL seen across the CLI's streams, exactly once. `open::that` launches
/// the user's default browser — the same approach the Google/Microsoft OAuth flows use.
fn open_url_once(line: &str, opened: &std::sync::atomic::AtomicBool) {
    use std::sync::atomic::Ordering;
    if opened.load(Ordering::Relaxed) {
        return;
    }
    if let Some(url) = first_https_url(line) {
        if opened
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let _ = open::that(&url); // best-effort; the browser is the user's own
        }
    }
}

/// Drain a child stream line-by-line (lossy UTF-8, so odd bytes never stall it), opening the
/// sign-in URL the instant it appears. Draining fully also stops a filled pipe from deadlocking
/// the `wait()` in [`connect`].
fn scan_for_url<R: std::io::Read>(reader: R, opened: &std::sync::atomic::AtomicBool) {
    use std::io::{BufRead, BufReader};
    let mut r = BufReader::new(reader);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match r.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => open_url_once(&String::from_utf8_lossy(&buf), opened),
        }
    }
}

/// Sign in — runs `auth login`. The CLI prints the browser sign-in URL to its output and then
/// blocks on the loopback OAuth callback ("Sign in in your browser. Keep the terminal open."),
/// but spawned head-less (no console / non-TTY) it won't launch a browser itself — so PM reads
/// the URL off the stream and opens it, the same way the Google/Microsoft flows do. The session
/// is stored by the OS secret store, owned by the CLI. On any failure we DISCARD the CLI's output
/// and return a generic message: that output carries a one-time-token URL that must never land in
/// a UI-visible error string.
pub(crate) fn connect(cli: &Path) -> Result<()> {
    use std::process::Stdio;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    let mut cmd = Command::new(cli);
    cmd.args(["auth", "login"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    no_window(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| Error::Other(format!("could not run the Proton Drive CLI: {e}")))?;

    let opened = Arc::new(AtomicBool::new(false));

    // The URL can surface on either stream; scan stderr on a side thread (also draining it so a
    // full pipe can't deadlock the wait) and stdout here. Open-once is shared via the flag.
    let stderr = child.stderr.take();
    let opened_err = Arc::clone(&opened);
    let stderr_thread = std::thread::spawn(move || {
        if let Some(e) = stderr {
            scan_for_url(e, &opened_err);
        }
    });
    if let Some(out) = child.stdout.take() {
        scan_for_url(out, &opened);
    }

    let status = child
        .wait()
        .map_err(|e| Error::Other(format!("could not run the Proton Drive CLI: {e}")))?;
    let _ = stderr_thread.join();

    if status.success() {
        Ok(())
    } else {
        Err(Error::Other(
            "Proton Drive sign-in didn't complete — please try connecting again.".into(),
        ))
    }
}

/// Sign out — `auth logout`. Being already signed out is a successful end state, not an error
/// (the CLI answers `auth logout` with the "not signed in" message in that case).
pub(crate) fn disconnect(cli: &Path) -> Result<()> {
    match run_proton(cli, &["auth", "logout"]) {
        Ok(_) => Ok(()),
        Err(e) if e.to_string() == NOT_CONNECTED_MSG => Ok(()),
        Err(e) => Err(e),
    }
}

/// Ensure `/my-files/Personal Manager Backups` exists (idempotent). `create-folder` throws if
/// the folder already exists, so we list the root and only create when it's genuinely absent.
fn ensure_backup_folder(cli: &Path) -> Result<()> {
    let listing = run_proton(cli, &["filesystem", "list", REMOTE_ROOT, "--json"])?;
    let exists = parse_nodes(&listing)?
        .iter()
        .any(|n| n.is_folder() && n.decoded_name() == Some(BACKUP_FOLDER_NAME));
    if exists {
        return Ok(());
    }
    // `create-folder` is non-idempotent (throws if the folder exists). The list-first check
    // above normally prevents that, but it can miss — a concurrent client created it, or its
    // (encrypted) name didn't decrypt so `decoded_name()` was `None`. Treat an "already exists"
    // failure as success rather than failing the whole backup; the folder is what we wanted.
    match run_proton(
        cli,
        &[
            "filesystem",
            "create-folder",
            REMOTE_ROOT,
            BACKUP_FOLDER_NAME,
            "--json",
        ],
    ) {
        Ok(_) => Ok(()),
        Err(e) if is_already_exists(&e.to_string()) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Whether a `create-folder` failure means "the folder is already there" — as opposed to an
/// unrelated error. Matches both a human "already exists" / "same name" message AND the bare
/// SDK type name `NodeWithSameNameExistsValidationError` (lowercased → contains `samename`),
/// since the CLI may surface either. Deliberately does NOT match a bare "exist" (which would
/// also match a "does not exist" failure and wrongly swallow it).
fn is_already_exists(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("already exist") || m.contains("same name") || m.contains("samename")
}

/// Upload a finished archive into the backup folder (folder ensured first). `-c replace` keeps
/// it non-interactive (names are unique, so no real conflict); `-t` skips thumbnailing. The
/// `cancel` flag is threaded into the transfer itself so a mid-upload Cancel is honoured (F-13);
/// the folder-ensure call is a fast metadata op left on the plain (timeout-only) path.
pub(crate) fn upload_archive(
    cli: &Path,
    local: &Path,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<()> {
    ensure_backup_folder(cli)?;
    let local = local.to_string_lossy();
    let out = run_proton_polled(
        cli,
        &[
            "filesystem",
            "upload",
            &local,
            REMOTE_BACKUP_DIR,
            "-c",
            "replace",
            "-t",
            "--json",
        ],
        cancel,
    )?;
    check_transfer(&out)
}

/// Download one archive (by bare name) into `dest_dir`. The CLI writes it as `dest_dir/<name>`.
/// `cancel` makes the (longest) Download phase interruptible; the live caller is the direct
/// `restore_from_proton` path — the enum destination's download arm passes `None` (see there).
pub(crate) fn download_archive(
    cli: &Path,
    name: &str,
    dest_dir: &Path,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<()> {
    if !naming::valid_archive_name(name) {
        return Err(Error::Other("invalid backup name".into()));
    }
    let remote = format!("{REMOTE_BACKUP_DIR}/{name}");
    let dest = dest_dir.to_string_lossy();
    let out = run_proton_polled(
        cli,
        &[
            "filesystem",
            "download",
            &remote,
            &dest,
            "-c",
            "replace",
            "--json",
        ],
        cancel,
    )?;
    check_transfer(&out)
}

/// List PM's archives in the remote folder (folder ensured first, so a first-time user gets an
/// empty list rather than an error), newest first.
pub(crate) fn list_archives(cli: &Path) -> Result<Vec<BackupEntry>> {
    ensure_backup_folder(cli)?;
    let out = run_proton(cli, &["filesystem", "list", REMOTE_BACKUP_DIR, "--json"])?;
    parse_backup_listing(&out)
}

// --- Retention (keep last N, trash oldest) ------------------------------------------

/// Move the given archives (by bare name) to Proton Trash — recoverable, never a hard delete.
/// One `filesystem trash` call with every path. No-op on an empty list.
fn trash_archives(cli: &Path, names: &[String]) -> Result<()> {
    if names.is_empty() {
        return Ok(());
    }
    let paths: Vec<String> = names
        .iter()
        .map(|n| format!("{REMOTE_BACKUP_DIR}/{n}"))
        .collect();
    let mut args = vec!["filesystem", "trash"];
    args.extend(paths.iter().map(String::as_str));
    args.push("--json");
    run_proton(cli, &args).map(|_| ())
}

/// Keep-last-N retention **for one vault**: list PM's archives, keep the newest `keep_n` whose
/// name carries `prefix` (this vault's — see [`archive_prefix`]), and trash the rest to Proton
/// Trash (recoverable). Scoping by `prefix` means it NEVER touches another vault's/device's
/// archives sharing the folder, and NEVER a non-PM file; the extra `valid_archive_name` filter is
/// belt-and-braces so a hostile listing entry can't splice a path into the trash call. Returns how
/// many were trashed. Used after a successful scheduled backup.
pub(crate) fn apply_retention(cli: &Path, keep_n: usize, prefix: &str) -> Result<usize> {
    let names: Vec<String> = list_archives(cli)?
        .into_iter()
        .map(|e| e.name)
        .filter(|n| n.starts_with(prefix) && naming::valid_archive_name(n))
        .collect();
    let doomed = naming::select_for_deletion(&names, keep_n);
    let count = doomed.len();
    trash_archives(cli, &doomed)?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_stop_reason_prioritizes_cancel_then_timeout() {
        // Neither → keep waiting; both → cancel wins (a user-initiated stop reads better than a
        // timeout message); each alone maps to itself. This is the poll loop's kill decision.
        assert_eq!(cli_stop_reason(false, false), None);
        assert_eq!(cli_stop_reason(false, true), Some(CliStop::TimedOut));
        assert_eq!(cli_stop_reason(true, false), Some(CliStop::Cancelled));
        assert_eq!(cli_stop_reason(true, true), Some(CliStop::Cancelled));
    }

    /// A real `filesystem list --json` node captured from the CLI (2026-07-02), so the parser
    /// is pinned to the actual serialized shape, not a guess.
    const CAPTURED_FILE_NODE: &str = r#"[
{"uid":"scope~aaa","parentUid":"scope~bbb","name":{"ok":true,"value":"pm-backup-20260702T161659Z.pmbackup"},"keyAuthor":{"ok":true,"value":"me@proton.me"},"directRole":"admin","ownedBy":{"email":"me@proton.me"},"type":"file","mediaType":"application/octet-stream","isShared":false,"creationTime":"2026-07-02T16:17:02.000Z","totalStorageSize":0,"activeRevision":{"ok":true,"value":{"uid":"rev","state":"active","storageSize":84,"claimedSize":4096,"claimedDigests":{"sha1":"abc","sha1Verified":false}}}}
]"#;

    #[test]
    fn parses_captured_backup_listing() {
        let entries = parse_backup_listing(CAPTURED_FILE_NODE).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "pm-backup-20260702T161659Z.pmbackup");
        assert_eq!(entries[0].size, Some(4096));
        assert_eq!(
            first_owner_email(CAPTURED_FILE_NODE).as_deref(),
            Some("me@proton.me")
        );
    }

    #[test]
    fn listing_excludes_folders_and_foreign_files() {
        let json = r#"[
{"name":{"ok":true,"value":"Personal Manager Backups"},"type":"folder","ownedBy":{"email":"me@proton.me"}},
{"name":{"ok":true,"value":"notes.txt"},"type":"file"},
{"name":{"ok":true,"value":"pm-backup-20260101T000000Z.pmbackup"},"type":"file"},
{"name":{"ok":false},"type":"file"}
]"#;
        let entries = parse_backup_listing(json).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "pm-backup-20260101T000000Z.pmbackup");
        assert_eq!(entries[0].size, None);
    }

    #[test]
    fn backup_listing_sorts_newest_first() {
        let json = r#"[
{"name":{"ok":true,"value":"pm-backup-20260101T000000Z.pmbackup"},"type":"file"},
{"name":{"ok":true,"value":"pm-backup-20260703T000000Z.pmbackup"},"type":"file"},
{"name":{"ok":true,"value":"pm-backup-20260202T000000Z.pmbackup"},"type":"file"}
]"#;
        let names: Vec<_> = parse_backup_listing(json)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(
            names,
            vec![
                "pm-backup-20260703T000000Z.pmbackup",
                "pm-backup-20260202T000000Z.pmbackup",
                "pm-backup-20260101T000000Z.pmbackup",
            ]
        );
    }

    #[test]
    fn transfer_result_flags_failures() {
        assert!(check_transfer(r#"{"transferredItems":1,"failedItems":0,"failures":[]}"#).is_ok());
        assert!(
            check_transfer(r#"{"transferredItems":0,"failedItems":1,"failures":[{}]}"#).is_err()
        );
        // A failure the CLI counts as 0 but still lists must not read as success.
        assert!(check_transfer(
            r#"{"transferredItems":0,"failedItems":0,"failures":[{"error":"x"}]}"#
        )
        .is_err());
        // Nothing transferred, no failures reported — still not a successful backup.
        assert!(check_transfer(r#"{"transferredItems":0,"failedItems":0,"failures":[]}"#).is_err());
    }

    #[test]
    fn not_logged_in_detection_is_narrow() {
        assert!(looks_not_logged_in("You need to login first"));
        assert!(looks_not_logged_in("Error: not logged in"));
        // Must NOT trip on the auth-login prompt, or a connect reads as not-connected.
        assert!(!looks_not_logged_in(
            "Sign in in your browser. Keep the terminal open."
        ));
    }

    #[test]
    fn extracts_sign_in_url() {
        assert_eq!(
            first_https_url("Sign in in your browser: https://account.proton.me/authorize?x=1"),
            Some("https://account.proton.me/authorize?x=1".to_string())
        );
        // A trailing ANSI reset and surrounding text are excluded; only the first URL is taken.
        assert_eq!(
            first_https_url("\u{1b}[36mhttps://drive.proton.me/login\u{1b}[0m keep terminal open"),
            Some("https://drive.proton.me/login".to_string())
        );
        assert_eq!(first_https_url("no url on this line"), None);
    }

    #[test]
    fn already_exists_matches_only_the_conflict_signature() {
        assert!(is_already_exists("Node with same name exists"));
        assert!(is_already_exists(
            "Error: a folder with that name already exists"
        ));
        // The bare SDK type name (no spaces) must match too — the CLI may surface it verbatim.
        assert!(is_already_exists(
            "Proton Drive CLI error: NodeWithSameNameExistsValidationError"
        ));
        // Must NOT swallow an unrelated "does not exist" failure.
        assert!(!is_already_exists("parent /my-files does not exist"));
        assert!(!is_already_exists("network error"));
    }

    #[test]
    fn extra_install_dirs_only_emits_for_present_env() {
        assert!(extra_install_dirs(None, None, None).is_empty());

        let lad = PathBuf::from("/lad");
        let dirs = extra_install_dirs(Some(&lad), None, None);
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/lad").join("Programs").join("proton-drive"),
                PathBuf::from("/lad").join("proton-drive"),
            ]
        );

        let home = PathBuf::from("/home/u");
        let dirs = extra_install_dirs(None, None, Some(&home));
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/home/u").join(".local").join("bin"),
                PathBuf::from("/home/u").join("bin"),
                PathBuf::from("/home/u").join("Downloads"),
                PathBuf::from("/home/u").join("Desktop"),
            ]
        );
    }

    #[test]
    fn a_user_override_wins_over_auto_detection() {
        let tmp = tempfile::tempdir().unwrap();
        let picked = tmp.path().join("wherever-they-put-it.exe");
        std::fs::write(&picked, b"binary").unwrap();
        // An override pointing at a real file is returned verbatim, regardless of PATH / install dirs.
        assert_eq!(locate_proton_cli(Some(&picked)), Some(picked.clone()));
        // A stale override (file since deleted / never existed) is ignored, falling back to the probe.
        std::fs::remove_file(&picked).unwrap();
        // We can't assert the fallback's result (depends on the host), only that a dead override does
        // not masquerade as a hit.
        assert_ne!(locate_proton_cli(Some(&picked)), Some(picked));
    }

    #[test]
    fn find_cli_in_dirs_matches_only_an_existing_file() {
        let empty = tempfile::tempdir().unwrap();
        let present = tempfile::tempdir().unwrap();
        let name = cli_binary_names()[0];
        std::fs::write(present.path().join(name), b"#!/bin/sh\n").unwrap();

        // Missing everywhere → None.
        assert!(find_cli_in_dirs([empty.path().to_path_buf()], cli_binary_names()).is_none());

        // Found in the second dir, and the returned path is the file itself.
        let hit = find_cli_in_dirs(
            [empty.path().to_path_buf(), present.path().to_path_buf()],
            cli_binary_names(),
        )
        .expect("should locate the fake CLI");
        assert_eq!(hit, present.path().join(name));

        // A directory of the same name must not count as the binary.
        let dir_named = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir_named.path().join(name)).unwrap();
        assert!(find_cli_in_dirs([dir_named.path().to_path_buf()], cli_binary_names()).is_none());
    }
}
