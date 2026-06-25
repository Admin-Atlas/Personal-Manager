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

use crate::registry::{ModelEntry, Pooling, Source};

/// Build the fastembed `add_custom_model` spec for a non-bundled model, as a JSON object the
/// sidecar registers on first use. `None` for a bundled model (fastembed already knows it) or a
/// local-path model (not used in PR 2). One shape serves both embedders and rerankers — the
/// reranker registration ignores the pooling/normalize/dim fields.
fn custom_spec(m: &ModelEntry) -> Option<Value> {
    let model_file = m.model_file?;
    let hf = match &m.source {
        Source::HuggingFace(repo) => *repo,
        Source::LocalPath(_) => return None,
    };
    let pooling = match m.pooling {
        Pooling::Mean => "mean",
        Pooling::Cls => "cls",
        Pooling::None => "none",
    };
    Some(json!({
        "model": m.id,
        "hf": hf,
        "model_file": model_file,
        "pooling": pooling,
        "normalize": m.normalize,
        "dim": m.dimension,
    }))
}

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
    /// Setup or the process failed; carries a human message plus a
    /// machine-readable `kind` so the UI can show a tailored fix-it guide instead
    /// of only the raw error text.
    Error {
        message: String,
        kind: SidecarErrorKind,
    },
}

/// Why setup failed, in a form the UI can switch on. Serializes to the
/// snake_case strings the frontend matches (see `SidecarErrorKind` in types.ts).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SidecarErrorKind {
    /// A Python interpreter was found, but it's older than [`MIN_PYTHON`].
    PythonTooOld,
    /// No usable Python interpreter could be found at all.
    PythonMissing,
    /// `pip install` failed — network, PyPI, or a dependency problem.
    PipFailed,
    /// The bundled sidecar files (script / requirements) are missing.
    RequirementsMissing,
    /// The bundled interpreter is present but can't start — its standard library
    /// is incomplete (e.g. a packaging defect flattened the tree so `encodings` is
    /// gone). This is a PM bug, not the user's environment, so the UI tells them to
    /// report it rather than chase a local fix.
    PackagingBug,
    /// Anything not otherwise classified.
    Unknown,
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
            Err(ProvisionError { kind, source }) => {
                // The UI reads the cause off the polled status; the thrown error
                // stays a plain string (unchanged for `ensure_sidecar`'s caller).
                let message = source.to_string();
                // Persist the full failure so a user-reported problem can be
                // diagnosed after the fact (best effort — never masks the error).
                self.log_failure(kind, &message);
                self.set_status(SidecarStatus::Error { message, kind });
                Err(source)
            }
        }
    }

    fn provision(&self) -> std::result::Result<(), ProvisionError> {
        let requirements = self.paths.requirements();
        if !requirements.exists() {
            return Err(ProvisionError {
                kind: SidecarErrorKind::RequirementsMissing,
                source: Error::Other(format!(
                    "sidecar requirements not found at {} (is the sidecar/ folder present?)",
                    requirements.display()
                )),
            });
        }

        // A base interpreter that meets MIN_PYTHON (the bundled one on Windows
        // release builds, else the best system Python — see resolve_base_python).
        let base = self.resolve_base_python()?;

        if let Some(parent) = self.paths.venv_dir.parent() {
            std::fs::create_dir_all(parent).map_err(unknown)?;
        }

        // Tear down a venv we can't trust — a previous attempt that died before
        // stamping the marker, or one whose interpreter is now too old (e.g. it
        // was first built against macOS's system 3.9, then the user installed
        // 3.10+). This is what removes the manual "delete the venv and retry".
        let venv_python_exists = self.paths.venv_python().exists();
        let detected = venv_python_exists
            .then(|| detect_python_version(&self.paths.venv_python()))
            .flatten();
        if should_rebuild_venv(
            venv_python_exists,
            self.paths.ready_marker().exists(),
            detected,
        ) {
            // Drop any live child first: on Windows it would hold a lock on the
            // venv's python.exe and block removal.
            *self.proc.lock().unwrap() = None;
            match std::fs::remove_dir_all(&self.paths.venv_dir) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(unknown(e)),
            }
        }

        // Create the venv (skip if a good interpreter is already there).
        if !self.paths.venv_python().exists() {
            let mut cmd = Command::new(&base);
            cmd.arg("-m").arg("venv").arg(&self.paths.venv_dir);
            clean_python_env(&mut cmd);
            no_window(&mut cmd);
            run_command(&mut cmd, "create venv").map_err(|e| {
                // Belt-and-braces: preflight_interpreter (in resolve_base_python)
                // should already have caught a bundled interpreter that can't boot,
                // but if venv creation still dies with that signature, classify it
                // as a packaging bug rather than a generic failure.
                let kind = if looks_like_packaging_bug(&e.to_string()) {
                    SidecarErrorKind::PackagingBug
                } else {
                    SidecarErrorKind::Unknown
                };
                ProvisionError { kind, source: e }
            })?;
        }

        // Install the pinned requirements into the venv.
        let py = self.paths.venv_python();
        let mut pip = Command::new(&py);
        pip.args(["-m", "pip", "install", "--disable-pip-version-check", "-r"])
            .arg(&requirements);
        clean_python_env(&mut pip);
        no_window(&mut pip);
        run_command(&mut pip, "pip install requirements").map_err(|e| ProvisionError {
            kind: classify_pip_failure(&e.to_string()),
            source: e,
        })?;

        // Stamp the marker with the requirements hash so we can skip next time.
        let hash = self.requirements_hash().map_err(unknown)?;
        std::fs::write(self.paths.ready_marker(), hash).map_err(unknown)?;
        Ok(())
    }

    /// Pick a base interpreter that satisfies [`MIN_PYTHON`]: the bundled one
    /// (Windows release) if present and new enough, else the best system Python
    /// found by [`probe_base_candidates`]. The bundled interpreter is
    /// version-checked too, so a mis-fetched bundle can't silently build an old
    /// venv. Distinguishes "found nothing" from "found only too-old" so the UI
    /// can show the right guide.
    fn resolve_base_python(&self) -> std::result::Result<PathBuf, ProvisionError> {
        if let Some(p) = self.paths.bundled_python() {
            // The app ships this interpreter, so we commit to it — but first make
            // sure it can actually start. A packaging defect that flattened the
            // bundled stdlib tree (losing Lib/encodings) leaves python.exe unable
            // to boot; without this check it would resurface three steps later as a
            // baffling "create venv failed". Fail fast with a precise, reportable
            // cause instead.
            preflight_interpreter(&p)?;
            if detect_python_version(&p).is_some_and(meets_min) {
                return Ok(p);
            }
        }
        match probe_base_candidates() {
            BaseProbe::Found(p) => Ok(p),
            BaseProbe::TooOld => Err(ProvisionError {
                kind: SidecarErrorKind::PythonTooOld,
                source: Error::Other(too_old_message()),
            }),
            BaseProbe::None => Err(ProvisionError {
                kind: SidecarErrorKind::PythonMissing,
                source: Error::Other(missing_message()),
            }),
        }
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
        if stamped.trim() != self.requirements_hash()? {
            return Ok(false);
        }
        // requirements.txt can be unchanged yet the venv's interpreter be too old
        // (built against macOS system 3.9, then the user installed 3.10+). Don't
        // report Ready in that case — let provision() rebuild it. This probe runs
        // only here, in the otherwise-ready branch, not on every status() poll.
        Ok(detect_python_version(&self.paths.venv_python()).is_some_and(meets_min))
    }

    /// `<data_dir>/document-engine.log`. `venv_dir` is `<data_dir>/runtime/venv`,
    /// so two parents up is the data home.
    fn engine_log_path(&self) -> Option<PathBuf> {
        self.paths
            .venv_dir
            .parent()? // runtime/
            .parent() // data home
            .map(|d| d.join("document-engine.log"))
    }

    /// Persist the latest document-engine setup failure (cause + full captured
    /// output) to a log in the data home, so a problem a user reports can be
    /// diagnosed afterwards. Best effort: a logging failure must never replace the
    /// real error. Overwrites rather than appends, so the file stays small and
    /// always holds the most recent failure.
    fn log_failure(&self, kind: SidecarErrorKind, message: &str) {
        let Some(path) = self.engine_log_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(
            &path,
            format!("document-engine setup failed (kind = {kind:?}):\n\n{message}\n"),
        );
    }

    /// Convert a file to Markdown. Returns `(markdown, title)`.
    pub fn convert(&self, path: &Path) -> Result<(String, String)> {
        let result = self.request("convert", json!({ "path": path.to_string_lossy() }))?;
        let markdown = result["markdown"].as_str().unwrap_or_default().to_string();
        let title = result["title"].as_str().unwrap_or_default().to_string();
        Ok((markdown, title))
    }

    /// The on-device model-cache dir for the Whisper weights (a sibling of the venv under
    /// `runtime/models`), created if missing, so they live inside PM's data dir and uninstall
    /// with it. (The embedder keeps fastembed's default cache in PR 1 — pinning it would force
    /// existing users to re-download the model; that hygiene is deferred.)
    fn model_dir_param(&self) -> Option<String> {
        let dir = self.paths.models_dir()?;
        let _ = std::fs::create_dir_all(&dir);
        Some(dir.to_string_lossy().into_owned())
    }

    /// Embed a batch of strings into the given embedder's vectors. The first call for a model
    /// downloads its weights and is slow; later calls are fast and fully local. Any retrieval
    /// prefix has already been applied by the gateway, so the text is embedded as-is. For a custom
    /// (non-bundled) model the spec registers it with fastembed on first use.
    pub fn embed(&self, texts: &[String], embedder: &ModelEntry) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut params = json!({ "texts": texts, "model": embedder.id });
        if let Some(spec) = custom_spec(embedder) {
            params["custom"] = spec;
        }
        let result = self.request("embed", params)?;
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

    /// Count tokens for a batch of strings with the given embedder's tokenizer — the splitter
    /// sizes chunks by this so a chunk never overflows the model's input window. Local, batched
    /// (one call per document), and uses the same tokenizer that embeds.
    pub fn count_tokens(&self, texts: &[String], embedder: &ModelEntry) -> Result<Vec<usize>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut params = json!({ "texts": texts, "model": embedder.id });
        if let Some(spec) = custom_spec(embedder) {
            params["custom"] = spec;
        }
        let result = self.request("count_tokens", params)?;
        let counts = result["counts"]
            .as_array()
            .ok_or_else(|| Error::Other("sidecar count_tokens returned no counts".into()))?
            .iter()
            .map(|n| {
                n.as_u64().map(|u| u as usize).ok_or_else(|| {
                    Error::Other("sidecar count_tokens returned a non-integer".into())
                })
            })
            .collect::<Result<Vec<usize>>>()?;
        if counts.len() != texts.len() {
            return Err(Error::Other(
                "sidecar count_tokens returned the wrong number of counts".into(),
            ));
        }
        Ok(counts)
    }

    /// Score each passage against the query with the given reranker's cross-encoder (higher =
    /// more relevant), returning one score per passage. The first call for a model downloads its
    /// weights and is slow; later calls are fast and fully local. Must run **off the DB lock** —
    /// it can block on a download. For a custom (non-bundled) reranker the spec registers it on
    /// first use.
    pub fn rerank(
        &self,
        query: &str,
        passages: &[&str],
        reranker: &ModelEntry,
    ) -> Result<Vec<f32>> {
        if passages.is_empty() {
            return Ok(Vec::new());
        }
        let mut params = json!({ "query": query, "passages": passages, "model": reranker.id });
        if let Some(spec) = custom_spec(reranker) {
            params["custom"] = spec;
        }
        let result = self.request("rerank", params)?;
        let scores = result["scores"]
            .as_array()
            .ok_or_else(|| Error::Other("sidecar rerank returned no scores".into()))?
            .iter()
            .map(|n| {
                // Reject a non-numeric / NaN / infinite score rather than mis-ordering on it.
                n.as_f64()
                    .filter(|f| f.is_finite())
                    .map(|f| f as f32)
                    .ok_or_else(|| {
                        Error::Other("sidecar rerank returned a non-numeric score".into())
                    })
            })
            .collect::<Result<Vec<f32>>>()?;
        if scores.len() != passages.len() {
            return Err(Error::Other(
                "sidecar rerank returned the wrong number of scores".into(),
            ));
        }
        Ok(scores)
    }

    /// Transcribe an audio clip to text with the local Whisper model. The first
    /// call downloads the model (~145 MB) and is slow; later calls are fast and
    /// fully local. `path` is a temp file the caller writes (and then deletes).
    pub fn transcribe(&self, path: &Path) -> Result<String> {
        let result = self.request(
            "transcribe",
            json!({
                "path": path.to_string_lossy(),
                "model_dir": self.model_dir_param(),
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
        clean_python_env(&mut command);
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

/// markitdown 0.1.6 (see requirements.txt) needs Python >= 3.10, so the venv's
/// base interpreter must too.
const MIN_PYTHON: (u32, u32) = (3, 10);

/// Highest `python3.N` minor we probe by versioned name. Gives headroom past
/// today's releases; anything newer is still found via the bare `python3` name.
const MAX_PROBE_MINOR: u32 = 16;

fn meets_min(v: (u32, u32)) -> bool {
    v >= MIN_PYTHON
}

/// Whether to delete and rebuild the venv before provisioning. Pure, so it's
/// unit-tested without real interpreters.
/// - no venv yet → false (the normal create path handles it)
/// - venv but no marker → true (a previous attempt died before stamping)
/// - venv + marker but the interpreter is too old / unprobeable → true (e.g.
///   built against macOS system 3.9, then the user installed 3.10+)
fn should_rebuild_venv(
    venv_python_exists: bool,
    marker_present: bool,
    detected_version: Option<(u32, u32)>,
) -> bool {
    if !venv_python_exists {
        return false;
    }
    if !marker_present {
        return true;
    }
    !detected_version.is_some_and(meets_min)
}

/// Parse a `python --version`-style string into `(major, minor)`. Tolerates the
/// optional `Python ` prefix, a trailing patch / rc / build suffix, and `\r`.
fn parse_python_version(s: &str) -> Option<(u32, u32)> {
    let s = s.trim();
    let rest = s
        .strip_prefix("Python ")
        .or_else(|| s.strip_prefix("python "))
        .unwrap_or(s);
    let token = rest.split_whitespace().next()?;
    let mut parts = token.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// Run `candidate --version` and return its `(major, minor)`. `None` if the
/// command can't run (a missing path, or a Windows Store shim that exits
/// non-zero) or the output doesn't parse — so this also serves as the "is it a
/// real interpreter" probe.
fn detect_python_version(candidate: &Path) -> Option<(u32, u32)> {
    let mut command = Command::new(candidate);
    command.arg("--version");
    clean_python_env(&mut command);
    no_window(&mut command);
    let out = command.output().ok()?;
    if !out.status.success() {
        return None;
    }
    // CPython has printed --version to stdout or stderr across versions; try both.
    parse_python_version(&String::from_utf8_lossy(&out.stdout))
        .or_else(|| parse_python_version(&String::from_utf8_lossy(&out.stderr)))
}

/// Outcome of scanning for a base interpreter, so the caller can tell "none at
/// all" (PythonMissing) from "found some, all too old" (PythonTooOld).
enum BaseProbe {
    Found(PathBuf),
    TooOld,
    None,
}

/// Probe `path`; accept it if it meets the minimum, else note it was too old.
fn consider_candidate(path: &Path, saw_too_old: &mut bool) -> Option<PathBuf> {
    match detect_python_version(path) {
        Some(v) if meets_min(v) => Some(path.to_path_buf()),
        Some(_) => {
            *saw_too_old = true;
            None
        }
        None => None,
    }
}

/// Find a base Python that meets [`MIN_PYTHON`]. Probes, first acceptable wins:
/// `PM_PYTHON`, then versioned names newest-first, then `python3`/`python`, then
/// common macOS install locations. The macOS absolute paths matter because a
/// Finder-launched .app gets a PATH stripped to the system dirs (no Homebrew /
/// python.org); they resolve to nothing on other platforms.
fn probe_base_candidates() -> BaseProbe {
    let mut saw_too_old = false;

    if let Some(p) = std::env::var_os("PM_PYTHON") {
        if let Some(found) = consider_candidate(&PathBuf::from(p), &mut saw_too_old) {
            return BaseProbe::Found(found);
        }
    }

    let mut names: Vec<String> = (MIN_PYTHON.1..=MAX_PROBE_MINOR)
        .rev()
        .map(|minor| format!("python{}.{}", MIN_PYTHON.0, minor))
        .collect();
    names.push("python3".into());
    names.push("python".into());
    for name in &names {
        if let Some(found) = consider_candidate(Path::new(name), &mut saw_too_old) {
            return BaseProbe::Found(found);
        }
    }

    for path in macos_python_candidates() {
        if let Some(found) = consider_candidate(&path, &mut saw_too_old) {
            return BaseProbe::Found(found);
        }
    }

    if saw_too_old {
        BaseProbe::TooOld
    } else {
        BaseProbe::None
    }
}

/// Absolute locations of a modern Python on macOS, newest-first; empty elsewhere.
/// These rescue a Finder-launched app whose PATH is stripped to
/// `/usr/bin:/bin:/usr/sbin:/sbin` (so Homebrew / python.org aren't on it).
fn macos_python_candidates() -> Vec<PathBuf> {
    if !cfg!(target_os = "macos") {
        return Vec::new();
    }
    let mut out = Vec::new();
    for dir in ["/opt/homebrew/bin", "/usr/local/bin"] {
        for minor in (MIN_PYTHON.1..=MAX_PROBE_MINOR).rev() {
            out.push(PathBuf::from(format!(
                "{dir}/python{}.{}",
                MIN_PYTHON.0, minor
            )));
        }
        out.push(PathBuf::from(format!("{dir}/python3")));
    }
    // python.org installs under /Library/Frameworks/Python.framework/Versions/X.Y.
    let versions_dir = Path::new("/Library/Frameworks/Python.framework/Versions");
    if let Ok(entries) = std::fs::read_dir(versions_dir) {
        let names: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        for (maj, min) in pick_framework_versions(&names) {
            out.push(
                versions_dir
                    .join(format!("{maj}.{min}"))
                    .join("bin")
                    .join("python3"),
            );
        }
    }
    out
}

/// From `Python.framework/Versions` directory names, the ones that parse as
/// `MAJOR.MINOR >= MIN_PYTHON`, newest-first (skips `Current` and junk).
fn pick_framework_versions(names: &[String]) -> Vec<(u32, u32)> {
    let mut vers: Vec<(u32, u32)> = names
        .iter()
        .filter_map(|n| parse_python_version(n))
        .filter(|&v| meets_min(v))
        .collect();
    vers.sort_unstable_by(|a, b| b.cmp(a));
    vers.dedup();
    vers
}

/// A provisioning failure carrying the underlying error (for the message) plus a
/// machine-readable [`SidecarErrorKind`] for the UI. Private to this module.
struct ProvisionError {
    kind: SidecarErrorKind,
    source: Error,
}

/// Wrap any error as an unclassified provisioning failure.
fn unknown(e: impl Into<Error>) -> ProvisionError {
    ProvisionError {
        kind: SidecarErrorKind::Unknown,
        source: e.into(),
    }
}

/// Map a pip / `run_command` failure message to a cause. The common one is
/// markitdown needing Python >= 3.10, which pip reports as "No matching
/// distribution found" / "requires-python".
fn classify_pip_failure(msg: &str) -> SidecarErrorKind {
    let m = msg.to_ascii_lowercase();
    if m.contains("no matching distribution")
        || m.contains("requires-python")
        || m.contains("requires a different python")
    {
        SidecarErrorKind::PythonTooOld
    } else {
        SidecarErrorKind::PipFailed
    }
}

fn missing_message() -> String {
    "No Python interpreter was found. PM's document engine needs Python 3.10 or newer — \
     install it and make sure it's on your PATH, or set PM_PYTHON to its full path."
        .to_string()
}

fn too_old_message() -> String {
    let mut m = String::from(
        "The Python that was found is older than 3.10, which PM's document engine requires. \
         Install Python 3.10 or newer, then retry.",
    );
    if std::env::var_os("PM_PYTHON").is_some() {
        m.push_str(" (PM_PYTHON points to an interpreter that is too old.)");
    }
    m
}

/// Give a spawned Python a clean, predictable environment. Drops the `PYTHON*`
/// overrides that could point the interpreter at the wrong stdlib (so a poisoned
/// environment can't break the bundled interpreter or the venv), and forces UTF-8
/// mode so a non-UTF-8 OEM codepage can't fail interpreter init. Applied to every
/// Python process PM launches.
fn clean_python_env(command: &mut Command) {
    command
        .env_remove("PYTHONHOME")
        .env_remove("PYTHONPATH")
        .env_remove("PYTHONSTARTUP")
        .env("PYTHONUTF8", "1");
}

/// Whether a failure message is the signature of an interpreter that can't boot
/// because its standard library is missing/incomplete — a packaging defect on our
/// side, not the user's environment. The flattened-bundle bug produced exactly
/// this (`No module named 'encodings'` during `init_fs_encoding`).
fn looks_like_packaging_bug(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("no module named 'encodings'")
        || m.contains("init_fs_encoding")
        || m.contains("could not find platform independent libraries")
        || m.contains("failed to get the python codec of the filesystem encoding")
}

/// Verify a base interpreter can actually start before we build a venv from it, by
/// importing a few core modules in a clean environment. If that fails the
/// interpreter is broken — most often a packaging defect that flattened the
/// bundled stdlib so `encodings` is gone — and we surface it as a reportable
/// [`SidecarErrorKind::PackagingBug`] carrying the real interpreter output,
/// instead of letting it resurface downstream as an inscrutable "create venv
/// failed". Only ever run against the bundled interpreter.
fn preflight_interpreter(py: &Path) -> std::result::Result<(), ProvisionError> {
    let packaging_bug = |source| ProvisionError {
        kind: SidecarErrorKind::PackagingBug,
        source,
    };

    let mut cmd = Command::new(py);
    cmd.args(["-c", "import encodings, ssl, venv"]);
    clean_python_env(&mut cmd);
    no_window(&mut cmd);

    let output = cmd.output().map_err(|e| {
        packaging_bug(Error::Other(format!(
            "the bundled document-engine Python could not be launched ({}): {e}",
            py.display()
        )))
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        return Err(packaging_bug(Error::Other(format!(
            "the bundled document-engine Python is incomplete and can't start — this is a \
             problem with PM's packaging, not with your computer.\n\n{}",
            if detail.is_empty() {
                "(the interpreter produced no output)"
            } else {
                detail
            }
        ))));
    }
    Ok(())
}

fn run_command(command: &mut Command, what: &str) -> Result<()> {
    let output = command
        .output()
        .map_err(|e| Error::Other(format!("could not run {what}: {e}")))?;
    if !output.status.success() {
        // Surface the FULL stderr (was: only the last line) so the cause is
        // diagnosable — the UI tucks it into a collapsible and the engine log
        // persists it. A misleading trailing line like "<no Python frame>" used to
        // hide the real fatal error printed above it.
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        return Err(Error::Other(format!(
            "{what} failed:\n{}",
            if detail.is_empty() {
                "(no output)"
            } else {
                detail
            }
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

    #[test]
    fn parses_python_versions() {
        assert_eq!(parse_python_version("Python 3.12.4"), Some((3, 12)));
        assert_eq!(parse_python_version("Python 3.9.6"), Some((3, 9)));
        assert_eq!(parse_python_version("3.10.0 (main, foo)"), Some((3, 10)));
        assert_eq!(parse_python_version("python 3.13"), Some((3, 13)));
        assert_eq!(parse_python_version("Python 3.12.4\r\n"), Some((3, 12)));
        assert_eq!(parse_python_version(""), None);
        assert_eq!(parse_python_version("not a version"), None);
        assert_eq!(parse_python_version("Python"), None);
        assert_eq!(parse_python_version("Python 3"), None);
    }

    #[test]
    fn enforces_minimum_python() {
        assert!(meets_min((3, 10)));
        assert!(meets_min((3, 14)));
        assert!(meets_min((4, 0)));
        assert!(!meets_min((3, 9)));
        assert!(!meets_min((2, 7)));
    }

    #[test]
    fn rebuilds_only_stale_or_old_venvs() {
        // No venv yet → the normal create path handles it.
        assert!(!should_rebuild_venv(false, false, None));
        assert!(!should_rebuild_venv(false, true, Some((3, 12))));
        // Venv but the marker never got stamped → a previous attempt died.
        assert!(should_rebuild_venv(true, false, Some((3, 12))));
        assert!(should_rebuild_venv(true, false, None));
        // Venv + marker but the interpreter is too old / unprobeable.
        assert!(should_rebuild_venv(true, true, Some((3, 9))));
        assert!(should_rebuild_venv(true, true, None));
        // Venv + marker + new enough → keep it.
        assert!(!should_rebuild_venv(true, true, Some((3, 10))));
        assert!(!should_rebuild_venv(true, true, Some((3, 12))));
    }

    #[test]
    fn classifies_pip_failures() {
        // The exact symptom we hit: markitdown needs >= 3.10.
        assert_eq!(
            classify_pip_failure(
                "ERROR: No matching distribution found for markitdown[pdf]==0.1.6"
            ),
            SidecarErrorKind::PythonTooOld
        );
        assert_eq!(
            classify_pip_failure("NO MATCHING DISTRIBUTION found"), // case-insensitive
            SidecarErrorKind::PythonTooOld
        );
        assert_eq!(
            classify_pip_failure("x requires a different Python: 3.9.6 not in '>=3.10'"),
            SidecarErrorKind::PythonTooOld
        );
        assert_eq!(
            classify_pip_failure("Failed to establish a new connection"),
            SidecarErrorKind::PipFailed
        );
        assert_eq!(
            classify_pip_failure("some other random error"),
            SidecarErrorKind::PipFailed
        );
    }

    #[test]
    fn detects_packaging_bug_signature() {
        // The real fatal output from the flattened-bundle bug.
        assert!(looks_like_packaging_bug(
            "Fatal Python error: init_fs_encoding: failed to get the Python codec of the \
             filesystem encoding\nModuleNotFoundError: No module named 'encodings'"
        ));
        assert!(looks_like_packaging_bug(
            "Could not find platform independent libraries <prefix>"
        ));
        // A normal failure must not be mistaken for a packaging bug.
        assert!(!looks_like_packaging_bug(
            "create venv failed: [Errno 13] Permission denied"
        ));
        assert!(!looks_like_packaging_bug(
            "Failed to establish a new connection"
        ));
    }

    #[test]
    fn packaging_bug_serializes_to_snake_case() {
        // The frontend matches this exact string in its SidecarErrorKind union.
        let v = serde_json::to_value(SidecarStatus::Error {
            message: "boom".into(),
            kind: SidecarErrorKind::PackagingBug,
        })
        .unwrap();
        assert_eq!(v["kind"], "packaging_bug");
    }

    #[test]
    fn picks_framework_versions_newest_first() {
        let names = vec![
            "3.9".to_string(),
            "3.12".to_string(),
            "3.11".to_string(),
            "2.7".to_string(),
            "Current".to_string(),
            "3.10".to_string(),
        ];
        assert_eq!(
            pick_framework_versions(&names),
            vec![(3, 12), (3, 11), (3, 10)]
        );

        let empty: Vec<String> = Vec::new();
        assert!(pick_framework_versions(&empty).is_empty());
        let junk = vec!["junk".to_string(), "Current".to_string()];
        assert!(pick_framework_versions(&junk).is_empty());
    }

    #[test]
    fn error_status_serializes_with_kind() {
        // The exact shape the frontend SidecarStatus union expects.
        let v = serde_json::to_value(SidecarStatus::Error {
            message: "boom".into(),
            kind: SidecarErrorKind::PythonTooOld,
        })
        .unwrap();
        assert_eq!(v["state"], "error");
        assert_eq!(v["message"], "boom");
        assert_eq!(v["kind"], "python_too_old");

        let v = serde_json::to_value(SidecarStatus::NotInstalled).unwrap();
        assert_eq!(v["state"], "not_installed");
    }
}
