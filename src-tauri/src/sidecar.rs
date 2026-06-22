// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The document sidecar: a long-lived Python child process that converts files
//! to Markdown (MarkItDown) and embeds text locally with an ONNX model
//! (fastembed). It is the only Python in PM; everything else is Rust.
//!
//! Python is provided by a *managed venv* created on first run (spec decision):
//! the app locates a base interpreter — the standalone one bundled with the app
//! on Windows release builds, else a system Python — builds an isolated venv
//! under the data directory, and pip-installs the pinned `requirements.txt`. A
//! `.ready` marker keyed by a hash of that file lets later runs skip the slow
//! setup.
//!
//! Talking to the child is newline-delimited JSON over stdio. Requests are
//! serialized by the `Mutex<Option<Process>>`, so each reply is the next line on
//! stdout (tracebacks and download progress go to stderr). Callers must run
//! these methods off the async runtime (see `tokio::task::spawn_blocking` in the
//! ingest command) — they block. Never hold the DB lock across a sidecar call.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// Hard cap on a single sidecar reply line (see [`read_line_capped`]). A reply
/// carries converted Markdown derived from an untrusted ingested file, so a
/// crafted document could otherwise make the child emit a multi-hundred-MB line
/// and exhaust memory before we ever parse it (rule #6). 64 MiB is far above any
/// legitimate reply.
const MAX_SIDECAR_LINE: usize = 64 * 1024 * 1024;

/// How many non-matching stdout lines we'll skip while waiting for our reply before
/// giving up and respawning. With the monotonic request id a stale/old line can
/// never match, so anything skipped is noise; this bounds a chatty or wedged child
/// to a failed call instead of an infinite loop.
const MAX_SKIP_LINES: usize = 1024;

/// Where the sidecar script and its requirements live, and where the venv goes.
pub struct SidecarPaths {
    /// Directory containing `pm_sidecar.py` + `requirements.txt`.
    pub source_dir: PathBuf,
    /// The managed venv, e.g. `<data_dir>/runtime/venv`.
    pub venv_dir: PathBuf,
}

impl SidecarPaths {
    fn script(&self) -> PathBuf {
        self.source_dir.join("pm_sidecar.py")
    }

    fn requirements(&self) -> PathBuf {
        self.source_dir.join("requirements.txt")
    }

    /// The venv's Python interpreter (per-OS layout).
    fn venv_python(&self) -> PathBuf {
        if cfg!(windows) {
            self.venv_dir.join("Scripts").join("python.exe")
        } else {
            self.venv_dir.join("bin").join("python")
        }
    }

    /// The bundled standalone interpreter shipped beside the sidecar resources
    /// (`<resource_dir>/python/`), if present. Release Windows builds ship it
    /// (scripts/fetch-python.mjs + tauri.windows.conf.json) so the venv is built
    /// without a system Python. In dev `source_dir` is the repo's `sidecar/`,
    /// which has no `python/` sibling, so this is `None` and we fall back to a
    /// base interpreter on PATH.
    fn bundled_python(&self) -> Option<PathBuf> {
        let dir = self.source_dir.parent()?.join("python");
        let exe = if cfg!(windows) {
            dir.join("python.exe")
        } else {
            dir.join("bin").join("python3")
        };
        exe.exists().then_some(exe)
    }

    fn ready_marker(&self) -> PathBuf {
        self.venv_dir.join(".pm-ready")
    }

    /// Where the speech model's weights are cached — a sibling of the venv under
    /// `runtime/`, so they live inside PM's data dir and uninstall with it.
    fn models_dir(&self) -> Option<PathBuf> {
        self.venv_dir.parent().map(|p| p.join("models"))
    }
}

/// Status reported to the UI so first-run setup is visible.
#[derive(Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SidecarStatus {
    /// The venv has not been provisioned yet.
    NotInstalled,
    /// Provisioning (venv create + pip install) is in progress.
    Installing,
    /// Ready to convert and embed.
    Ready,
    /// Setup or the process failed; carries a message for the UI.
    Error { message: String },
}

/// A running sidecar child plus its stdio handles.
struct Process {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

pub struct SidecarManager {
    paths: SidecarPaths,
    proc: Mutex<Option<Process>>,
    status: Mutex<SidecarStatus>,
    /// Serializes provisioning so two concurrent first-run callers can't both
    /// create the venv / pip-install into the same directory.
    install: Mutex<()>,
    /// Monotonic request id, kept on the manager (not the `Process`) so it keeps
    /// climbing across respawns — a stale line from a dead child can never match
    /// a fresh request's id.
    req_seq: AtomicU64,
}

impl SidecarManager {
    pub fn new(paths: SidecarPaths) -> Self {
        let installed = paths.ready_marker().exists();
        let status = if installed {
            SidecarStatus::Ready
        } else {
            SidecarStatus::NotInstalled
        };
        Self {
            paths,
            proc: Mutex::new(None),
            status: Mutex::new(status),
            install: Mutex::new(()),
            req_seq: AtomicU64::new(0),
        }
    }

    pub fn status(&self) -> SidecarStatus {
        self.status.lock().unwrap().clone()
    }

    fn set_status(&self, status: SidecarStatus) {
        *self.status.lock().unwrap() = status;
    }

    /// Provision the venv if needed: create it from a base Python and install the
    /// pinned requirements. Idempotent and cheap once the `.ready` marker matches
    /// the current `requirements.txt`. Blocking and slow on first run.
    pub fn ensure_installed(&self) -> Result<()> {
        if self.is_ready_marker_current()? {
            self.set_status(SidecarStatus::Ready);
            return Ok(());
        }

        // Hold the install lock across provisioning. Re-check the marker once we
        // have it: another caller may have finished while we were waiting.
        let _install = self.install.lock().unwrap();
        if self.is_ready_marker_current()? {
            self.set_status(SidecarStatus::Ready);
            return Ok(());
        }

        self.set_status(SidecarStatus::Installing);
        match self.provision() {
            Ok(()) => {
                self.set_status(SidecarStatus::Ready);
                Ok(())
            }
            Err(e) => {
                self.set_status(SidecarStatus::Error {
                    message: e.to_string(),
                });
                Err(e)
            }
        }
    }

    fn provision(&self) -> Result<()> {
        let requirements = self.paths.requirements();
        if !requirements.exists() {
            return Err(Error::Other(format!(
                "sidecar requirements not found at {} (is the sidecar/ folder present?)",
                requirements.display()
            )));
        }

        // Prefer the interpreter bundled with the app (Windows release builds);
        // fall back to a system Python (dev, or a build with no bundled one).
        let base = self
            .paths
            .bundled_python()
            .or_else(find_base_python)
            .ok_or_else(|| {
                Error::Other(
                    "No Python interpreter found: the bundled interpreter is missing and no \
                     system Python is on PATH. Install Python 3.10+ and ensure it is on PATH, \
                     or set PM_PYTHON to its full path."
                        .into(),
                )
            })?;

        if let Some(parent) = self.paths.venv_dir.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Create the venv (skip if the interpreter already exists).
        if !self.paths.venv_python().exists() {
            run_command(
                Command::new(&base)
                    .arg("-m")
                    .arg("venv")
                    .arg(&self.paths.venv_dir),
                "create venv",
            )?;
        }

        // Install the pinned requirements into the venv.
        let py = self.paths.venv_python();
        run_command(
            Command::new(&py)
                .args(["-m", "pip", "install", "--disable-pip-version-check", "-r"])
                .arg(&requirements),
            "pip install requirements",
        )?;

        // Stamp the marker with the requirements hash so we can skip next time.
        std::fs::write(self.paths.ready_marker(), self.requirements_hash()?)?;
        Ok(())
    }

    fn requirements_hash(&self) -> Result<String> {
        let bytes = std::fs::read(self.paths.requirements())?;
        Ok(hex_digest(&bytes))
    }

    fn is_ready_marker_current(&self) -> Result<bool> {
        let marker = self.paths.ready_marker();
        if !marker.exists() || !self.paths.venv_python().exists() {
            return Ok(false);
        }
        let stamped = std::fs::read_to_string(&marker).unwrap_or_default();
        Ok(stamped.trim() == self.requirements_hash()?)
    }

    /// Convert a file to Markdown. Returns `(markdown, title)`.
    pub fn convert(&self, path: &Path) -> Result<(String, String)> {
        let result = self.request("convert", json!({ "path": path.to_string_lossy() }))?;
        let markdown = result["markdown"].as_str().unwrap_or_default().to_string();
        let title = result["title"].as_str().unwrap_or_default().to_string();
        Ok((markdown, title))
    }

    /// Embed a batch of strings into 384-d vectors. The first call downloads the
    /// model (~90 MB) and is slow; subsequent calls are fast and fully local.
    pub fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let result = self.request("embed", json!({ "texts": texts }))?;
        let vectors = result["vectors"]
            .as_array()
            .ok_or_else(|| Error::Other("sidecar embed returned no vectors".into()))?
            .iter()
            .map(|row| {
                row.as_array()
                    .ok_or_else(|| {
                        Error::Other("sidecar embed returned a malformed vector".into())
                    })?
                    .iter()
                    .map(|n| {
                        // Reject a non-numeric / NaN / infinite component rather than
                        // silently storing a zeroed dimension in the index.
                        n.as_f64()
                            .filter(|f| f.is_finite())
                            .map(|f| f as f32)
                            .ok_or_else(|| {
                                Error::Other(
                                    "sidecar embed returned a non-numeric component".into(),
                                )
                            })
                    })
                    .collect::<Result<Vec<f32>>>()
            })
            .collect::<Result<Vec<Vec<f32>>>>()?;
        Ok(vectors)
    }

    /// Transcribe an audio clip to text with the local Whisper model. The first
    /// call downloads the model (~145 MB) and is slow; later calls are fast and
    /// fully local. `path` is a temp file the caller writes (and then deletes).
    pub fn transcribe(&self, path: &Path) -> Result<String> {
        let model_dir = self.paths.models_dir();
        if let Some(dir) = &model_dir {
            let _ = std::fs::create_dir_all(dir);
        }
        let result = self.request(
            "transcribe",
            json!({
                "path": path.to_string_lossy(),
                "model_dir": model_dir.as_ref().map(|d| d.to_string_lossy()),
            }),
        )?;
        Ok(result["text"].as_str().unwrap_or_default().to_string())
    }

    /// Send one request to the child and read its reply. Spawns the child lazily.
    /// Serialized by the process mutex, so the next stdout line is our reply.
    fn request(&self, method: &str, params: Value) -> Result<Value> {
        let mut guard = self.proc.lock().unwrap();
        if guard.is_none() {
            *guard = Some(self.spawn()?);
        }
        let id = self.req_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let proc = guard.as_mut().unwrap();

        let line = serde_json::to_string(&json!({
            "id": id, "method": method, "params": params,
        }))
        .map_err(|e| Error::Other(format!("encode sidecar request: {e}")))?;

        // On any IO failure, drop the (possibly dead) child so the next call
        // respawns it.
        let send = (|| -> std::io::Result<Value> {
            proc.stdin.write_all(line.as_bytes())?;
            proc.stdin.write_all(b"\n")?;
            proc.stdin.flush()?;

            let mut skipped = 0usize;
            loop {
                // Bounded read: a runaway/oversized reply fails the call (and
                // respawns the child) instead of buffering unbounded into memory.
                let Some(bytes) = read_line_capped(&mut proc.stdout, MAX_SIDECAR_LINE)? else {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "sidecar closed its output",
                    ));
                };
                let trimmed = std::str::from_utf8(&bytes).map(str::trim).unwrap_or("");
                if !trimmed.is_empty() {
                    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
                        if value["id"].as_u64() == Some(id) {
                            return Ok(value);
                        }
                    }
                }
                // Empty or non-matching line — bound how many we'll wade through.
                skipped += 1;
                if skipped > MAX_SKIP_LINES {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "sidecar produced too many unmatched lines",
                    ));
                }
            }
        })();

        match send {
            Ok(value) => {
                if value["ok"].as_bool() == Some(true) {
                    Ok(value["result"].clone())
                } else {
                    let msg = value["error"].as_str().unwrap_or("unknown sidecar error");
                    Err(Error::Other(format!("sidecar {method} failed: {msg}")))
                }
            }
            Err(e) => {
                *guard = None; // force a respawn next time
                Err(Error::Other(format!("sidecar {method} IO error: {e}")))
            }
        }
    }

    fn spawn(&self) -> Result<Process> {
        let py = self.paths.venv_python();
        if !py.exists() {
            return Err(Error::Other(
                "document engine is not installed yet — run setup first".into(),
            ));
        }

        let mut command = Command::new(&py);
        command
            .arg(self.paths.script())
            // Keep the model downloads quiet and private: no Hugging Face
            // telemetry (PM's "nothing leaves the device" rule), and silence the
            // cosmetic symlink warning on Windows without Developer Mode — the
            // copy-based cache fallback is fine for our single pinned model.
            .env("HF_HUB_DISABLE_TELEMETRY", "1")
            .env("HF_HUB_DISABLE_SYMLINKS_WARNING", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        no_window(&mut command);

        let mut child = command
            .spawn()
            .map_err(|e| Error::Other(format!("could not start the document sidecar: {e}")))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Ok(Process {
            child,
            stdin,
            stdout,
        })
    }
}

/// Read one `\n`-terminated line from `r` into a byte buffer, but never buffer
/// more than `max` bytes. Returns `Ok(None)` at EOF with nothing buffered, and
/// an `InvalidData` error if a single line exceeds the cap — so a runaway child
/// fails the call instead of exhausting memory.
fn read_line_capped<R: BufRead>(r: &mut R, max: usize) -> std::io::Result<Option<Vec<u8>>> {
    let mut buf = Vec::new();
    loop {
        let available = match r.fill_buf() {
            Ok(b) => b,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            return Ok(if buf.is_empty() { None } else { Some(buf) });
        }
        if let Some(i) = available.iter().position(|&b| b == b'\n') {
            buf.extend_from_slice(&available[..=i]);
            r.consume(i + 1);
            return Ok(Some(buf));
        }
        buf.extend_from_slice(available);
        let consumed = available.len();
        r.consume(consumed);
        if buf.len() > max {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "sidecar reply exceeded the size cap",
            ));
        }
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Find a base Python to build the venv from: `PM_PYTHON` wins, then the usual
/// names. We only need it once, to create the venv.
fn find_base_python() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("PM_PYTHON") {
        let path = PathBuf::from(p);
        if probe_python(&path) {
            return Some(path);
        }
    }
    for name in ["python3", "python"] {
        let path = PathBuf::from(name);
        if probe_python(&path) {
            return Some(path);
        }
    }
    None
}

/// True if `candidate --version` runs (a real interpreter, not a Windows Store
/// shim that just prints a message).
fn probe_python(candidate: &Path) -> bool {
    let mut command = Command::new(candidate);
    command.arg("--version");
    no_window(&mut command);
    matches!(command.output(), Ok(out) if out.status.success())
}

fn run_command(command: &mut Command, what: &str) -> Result<()> {
    let output = command
        .output()
        .map_err(|e| Error::Other(format!("could not run {what}: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Other(format!(
            "{what} failed: {}",
            stderr.trim().lines().last().unwrap_or("(no output)")
        )));
    }
    Ok(())
}

/// Suppress the console window that would otherwise flash when spawning a
/// child process from a GUI app on Windows. No-op elsewhere.
#[cfg(windows)]
fn no_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

/// No-op on non-Windows platforms.
#[cfg(not(windows))]
fn no_window(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn reads_lines_and_stops_at_eof() {
        let mut r = Cursor::new(b"{\"a\":1}\n{\"b\":2}\n".to_vec());
        assert_eq!(
            read_line_capped(&mut r, 1024).unwrap().as_deref(),
            Some(&b"{\"a\":1}\n"[..])
        );
        assert_eq!(
            read_line_capped(&mut r, 1024).unwrap().as_deref(),
            Some(&b"{\"b\":2}\n"[..])
        );
        assert_eq!(read_line_capped(&mut r, 1024).unwrap(), None);
    }

    #[test]
    fn errors_when_a_line_exceeds_the_cap() {
        // A 5 KB line with no newline, read against a 1 KB cap, must fail rather
        // than buffer the whole thing.
        let mut r = Cursor::new(vec![b'x'; 5000]);
        let err = read_line_capped(&mut r, 1024).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    fn paths_with_source(source_dir: PathBuf) -> SidecarPaths {
        let venv_dir = source_dir.join("runtime").join("venv");
        SidecarPaths {
            source_dir,
            venv_dir,
        }
    }

    #[test]
    fn bundled_python_found_beside_sidecar_resources() {
        // The bundle ships the interpreter as a `python/` sibling of the
        // `sidecar/` resource dir; bundled_python() resolves it.
        let root = tempfile::tempdir().unwrap();
        let source_dir = root.path().join("sidecar");
        std::fs::create_dir_all(&source_dir).unwrap();
        let exe = if cfg!(windows) {
            root.path().join("python").join("python.exe")
        } else {
            root.path().join("python").join("bin").join("python3")
        };
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"").unwrap();

        assert_eq!(paths_with_source(source_dir).bundled_python(), Some(exe));
    }

    #[test]
    fn bundled_python_absent_in_dev_layout() {
        // No `python/` sibling (the dev repo layout) → None, so provisioning
        // falls back to a system interpreter on PATH.
        let root = tempfile::tempdir().unwrap();
        let source_dir = root.path().join("sidecar");
        std::fs::create_dir_all(&source_dir).unwrap();

        assert_eq!(paths_with_source(source_dir).bundled_python(), None);
    }
}
