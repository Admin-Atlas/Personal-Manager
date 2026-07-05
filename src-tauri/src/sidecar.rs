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

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::photos::ImageAnalysis;
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

/// Hard cap on a single *request* line we will send to the child, kept below the child's own
/// `MAX_LINE_CHARS` (64 MiB, mirrored in `pm_sidecar.py`). The child *silently drops* a line
/// over that cap — no reply — which would leave [`SidecarManager::request`] blocked forever on
/// a reply that never comes, wedging the whole (serialized) sidecar: ingest, chat retrieval,
/// rerank, transcribe and the map all queue behind the dead call until restart (F-06 / B3-1).
/// Refusing to *send* an oversized line turns that permanent wedge into a clean per-call error.
/// Byte length here is conservative against the child's *character* cap (UTF-8 bytes ≥ chars),
/// and the 16 MiB headroom to 64 MiB absorbs JSON quoting/escaping of the payload. The gateway
/// keeps embed / count-token requests well under this by batching (see `model_gateway`); this
/// guard is the backstop for any caller that doesn't.
const MAX_SIDECAR_REQUEST_LINE: usize = 48 * 1024 * 1024;

/// The OPTIONAL t-SNE reducer for the semantic memory map, pinned. **Not** in `requirements.txt` —
/// the base venv stays lean; the user installs it on demand from Settings (see
/// [`SidecarManager::install_optional_tsne`]). openTSNE pulls scikit-learn + scipy (numpy is already
/// present via fastembed) and ships a binary wheel for our pinned Python, so there's no compile step.
const OPTIONAL_TSNE_PIN: &str = "openTSNE==1.0.4";

/// The OPTIONAL photo-OCR component, pinned. **Not** in `requirements.txt` — like t-SNE, the base
/// venv stays lean and the user installs it on demand (see [`SidecarManager::install_optional_ocr`]).
/// `rapidocr` runs OCR on the `onnxruntime` fastembed already ships (it pulls no runtime of its own)
/// and downloads its small detection/recognition ONNX models on first use; `pillow-heif` adds HEIC
/// decoding (Pillow itself is already present via markitdown). Both — and rapidocr's image deps
/// (opencv/shapely/pyclipper) — ship binary wheels for the bundled 3.12 release interpreter and dev
/// 3.14, so there's no compile step.
const OPTIONAL_OCR_PINS: &[&str] = &["rapidocr==3.9.0", "pillow-heif==1.4.0"];

/// The marker's expected contents — the OCR pins joined, so a future bump re-installs. Kept in sync
/// with [`OPTIONAL_OCR_PINS`]; the `optional_ocr_marker_matches_pins` test guards the join.
const OPTIONAL_OCR_MARKER: &str = "rapidocr==3.9.0;pillow-heif==1.4.0";

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

    /// Marker that the OPTIONAL t-SNE component is installed in this venv. Separate from
    /// `.pm-ready` so the base requirements hash is untouched and an existing user never re-installs
    /// the base venv just because t-SNE became available. Holds the pin, so a future bump re-installs.
    fn tsne_marker(&self) -> PathBuf {
        self.venv_dir.join(".pm-tsne")
    }

    /// Marker that the OPTIONAL photo-OCR component is installed in this venv. Separate from
    /// `.pm-ready` (so the base requirements hash is untouched) and from `.pm-tsne`. Holds the pins,
    /// so a future bump re-installs.
    fn ocr_marker(&self) -> PathBuf {
        self.venv_dir.join(".pm-ocr")
    }

    /// Where the speech model's weights are cached — a sibling of the venv under
    /// `runtime/`, so they live inside PM's data dir and uninstall with it.
    fn models_dir(&self) -> Option<PathBuf> {
        self.venv_dir.parent().map(|p| p.join("models"))
    }

    /// Where a runtime-downloaded macOS interpreter is unpacked — a sibling of the
    /// venv under `runtime/`, so it lives inside PM's data dir and uninstalls with
    /// it. macOS-only fallback (see [`crate::python_fetch`]); Windows/Linux never
    /// populate this, so the method is legitimately unused there.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn downloaded_python_dir(&self) -> Option<PathBuf> {
        self.venv_dir.parent().map(|p| p.join("python-standalone"))
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
    /// macOS only: no suitable interpreter was found, so PM tried to download a
    /// standalone one into its data dir — and that download (or its verification /
    /// unpack) failed. Distinct from [`PythonMissing`] because the fix is different
    /// (check the network / a firewall blocking GitHub, not "install Python").
    PythonDownloadFailed,
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

    /// Whether the engine is already provisioned and current — a cheap, non-building probe (no install
    /// lock, no provisioning). Lets a best-effort background job (the chat-index launch sweep) skip
    /// itself when the sidecar isn't ready yet rather than triggering a slow first-run build at startup.
    pub fn is_ready(&self) -> bool {
        self.is_ready_marker_current().unwrap_or(false)
    }

    fn set_status(&self, status: SidecarStatus) {
        *self.status.lock().unwrap() = status;
    }

    /// Stop the running child (if any) and mark the engine un-provisioned, so the caller can
    /// delete the whole `runtime/` directory out from under it. On Windows the interpreter's
    /// `python.exe`/DLLs are held open while the child lives, so the file locks must be released
    /// first or the removal fails; taking the `Process` out of the mutex drops it, and its `Drop`
    /// kills the child (releasing the handles). We flip the status to `NotInstalled` so the next
    /// ingest re-provisions a fresh venv rather than trusting an in-memory `Ready` for a directory
    /// that no longer exists. Best-effort and idempotent — used only by the "Remove PM data"
    /// teardown ([`crate::wipe`]).
    pub fn prepare_for_runtime_removal(&self) {
        if let Ok(mut proc) = self.proc.lock() {
            proc.take(); // drop → Process::drop kills the child, freeing the interpreter's locks
        }
        self.set_status(SidecarStatus::NotInstalled);
    }

    /// Provision the venv if needed: create it from a base Python and install the
    /// pinned requirements. Idempotent and cheap once the `.ready` marker matches
    /// the current `requirements.txt`. Blocking and slow on first run.
    pub fn ensure_installed(&self) -> Result<()> {
        self.ensure_installed_with_progress(|_| {})
    }

    /// Same as [`ensure_installed`], but reports a `0.0..=1.0` fraction while the
    /// macOS interpreter download runs (the one slow, byte-countable phase of
    /// first-run setup). Only the first-run-setup command passes a real callback;
    /// every other caller (ingest, the chat-index sweep, …) uses the no-op
    /// [`ensure_installed`]. A dedicated method — rather than a parameter bolted
    /// onto the shared one — keeps those many call sites untouched, mirroring how
    /// `install_optional_tsne` is its own method.
    ///
    /// [`ensure_installed`]: SidecarManager::ensure_installed
    pub fn ensure_installed_with_progress(&self, on_progress: impl FnMut(f32)) -> Result<()> {
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
        match self.provision(on_progress) {
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

    fn provision(&self, on_progress: impl FnMut(f32)) -> std::result::Result<(), ProvisionError> {
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
        // release builds, else the best system Python — see resolve_base_python;
        // on macOS, a downloaded standalone interpreter if none is found).
        let base = self.resolve_base_python(on_progress)?;

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
    /// found by [`probe_base_candidates`], else — on macOS, when nothing suitable
    /// is on the machine — a standalone interpreter downloaded into the data dir
    /// ([`crate::python_fetch`]). The bundled interpreter is version-checked too,
    /// so a mis-fetched bundle can't silently build an old venv. Distinguishes
    /// "found nothing" from "found only too-old" so the UI can show the right guide.
    /// `on_progress` reports the download's byte fraction (macOS fallback only).
    fn resolve_base_python(
        &self,
        #[allow(unused_variables, unused_mut)] mut on_progress: impl FnMut(f32),
    ) -> std::result::Result<PathBuf, ProvisionError> {
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

        // Probe exactly once; an early return keeps the download strictly behind a
        // failed probe.
        let probe = probe_base_candidates();
        if let BaseProbe::Found(p) = probe {
            return Ok(p);
        }

        // macOS: no interpreter on the machine → download the pinned standalone one
        // into the data dir. Strictly gated to macOS, so Windows/Linux behaviour and
        // error messages are provably unchanged. The downloaded interpreter is
        // version-checked (same defence the bundled one gets) before we trust it.
        #[cfg(target_os = "macos")]
        if let Some(dest) = self.paths.downloaded_python_dir() {
            match crate::python_fetch::fetch_macos_python(&dest, &mut on_progress) {
                Ok(p) if detect_python_version(&p).is_some_and(meets_min) => return Ok(p),
                Ok(p) => {
                    // Downloaded but doesn't report a usable version — a bad pin. The
                    // SHA-256 check already passed, so this is our mistake, not the
                    // user's; surface it as a download failure rather than "install
                    // Python yourself".
                    return Err(ProvisionError {
                        kind: SidecarErrorKind::PythonDownloadFailed,
                        source: Error::Other(format!(
                            "PM downloaded a Python interpreter to {} but it did not report a \
                             usable version (3.10+). This is a problem with PM's build, not your \
                             computer.",
                            p.display()
                        )),
                    });
                }
                Err(fetch_err) => {
                    // Fold the download's real cause onto the original probe outcome so
                    // the message reflects both ("none found, and the auto-download
                    // failed because …").
                    let base = match probe {
                        BaseProbe::TooOld => too_old_message(),
                        _ => missing_message(),
                    };
                    return Err(ProvisionError {
                        kind: SidecarErrorKind::PythonDownloadFailed,
                        source: Error::Other(format!(
                            "{base}\n\nPM also tried to download a Python interpreter \
                             automatically, but that failed: {fetch_err}"
                        )),
                    });
                }
            }
        }

        match probe {
            BaseProbe::Found(_) => unreachable!("handled by the early return above"),
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
    /// (non-bundled) model the spec registers it with fastembed on first use. `batch` caps how many
    /// texts the embedder processes per forward pass (`None` = its own default); a small cap is the
    /// "gentle" memory lever — it bounds peak activation memory at index time.
    pub fn embed(
        &self,
        texts: &[String],
        embedder: &ModelEntry,
        batch: Option<usize>,
    ) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut params = json!({ "texts": texts, "model": embedder.id });
        if let Some(spec) = custom_spec(embedder) {
            params["custom"] = spec;
        }
        if let Some(b) = batch {
            params["batch_size"] = json!(b);
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

    /// Analyse one image for photo ingestion: EXIF capture metadata (always) + OCR text (only when
    /// `run_ocr`). The caller sets `run_ocr` from [`optional_ocr_ready`](Self::optional_ocr_ready),
    /// so a user who declined the optional OCR component still gets dimensions + EXIF for the photo's
    /// metadata chunk. The first OCR call downloads rapidocr's small models and is slow; later calls
    /// are fast and fully local. Blocking, like every sidecar call. EXIF/OCR output is untrusted data —
    /// scored/indexed, never executed.
    pub fn analyze_image(&self, path: &Path, run_ocr: bool) -> Result<ImageAnalysis> {
        let result = self.request(
            "analyze_image",
            json!({ "path": path.to_string_lossy(), "run_ocr": run_ocr }),
        )?;
        // Treat an empty string the same as a missing field, and drop any non-finite coordinate
        // rather than placing a photo at infinity on the map.
        let text_opt = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_string);
        let coord_opt = |v: &Value| v.as_f64().filter(|f| f.is_finite());
        let dim_opt = |v: &Value| v.as_u64().and_then(|u| u32::try_from(u).ok());
        Ok(ImageAnalysis {
            ocr_text: result["ocr_text"].as_str().unwrap_or_default().to_string(),
            ocr_ran: result["ocr_ran"].as_bool().unwrap_or(false),
            capture_date: text_opt(&result["capture_date"]),
            lat: coord_opt(&result["lat"]),
            lon: coord_opt(&result["lon"]),
            width: dim_opt(&result["width"]),
            height: dim_opt(&result["height"]),
        })
    }

    /// Parse a spreadsheet (`.xlsx`/`.xls`/`.csv`) into per-sheet structure for the dedicated ingest
    /// path, bypassing MarkItDown. Values only — no formula evaluation, no styling. Each sheet reports
    /// its headers, TRUE row count, per-column inferred types, an optional date range, and up to the
    /// sidecar's row cap of stringified rows (flagged `truncated` when it had more). `ext` selects the
    /// reader (openpyxl / xlrd / stdlib csv). Blocking, like every sidecar call; cell text is untrusted
    /// data — indexed, never executed.
    pub fn analyze_spreadsheet(
        &self,
        path: &Path,
        ext: &str,
    ) -> Result<Vec<crate::spreadsheets::SheetData>> {
        let result = self.request(
            "analyze_spreadsheet",
            json!({ "path": path.to_string_lossy(), "ext": ext }),
        )?;
        serde_json::from_value(result["sheets"].clone()).map_err(|e| {
            Error::Other(format!(
                "sidecar analyze_spreadsheet: bad sheets payload: {e}"
            ))
        })
    }

    /// Project per-document vectors to 2-D for the semantic memory map. `method` is `"pca"` (the
    /// numpy-only default) or `"tsne"` (only requested when the optional component is installed; the
    /// sidecar falls back to PCA if it isn't, so this always returns a usable layout). Returns the
    /// coordinates (already scaled into `[0,1]²`) and the method actually used. Blocking — run off the
    /// async runtime and never while holding the DB lock, like every other sidecar call.
    pub fn reduce(&self, vectors: &[Vec<f32>], method: &str) -> Result<(Vec<[f32; 2]>, String)> {
        if vectors.is_empty() {
            return Ok((Vec::new(), "none".to_string()));
        }
        let result = self.request("reduce", json!({ "vectors": vectors, "method": method }))?;
        let used = result["method"].as_str().unwrap_or("pca").to_string();
        let coords = result["coords"]
            .as_array()
            .ok_or_else(|| Error::Other("sidecar reduce returned no coords".into()))?
            .iter()
            .map(|row| {
                let point = row.as_array().ok_or_else(|| {
                    Error::Other("sidecar reduce returned a malformed point".into())
                })?;
                // Reject a non-finite coordinate rather than placing a node at infinity.
                let at = |i: usize| {
                    point
                        .get(i)
                        .and_then(|n| n.as_f64())
                        .filter(|f| f.is_finite())
                        .map(|f| f as f32)
                        .ok_or_else(|| {
                            Error::Other("sidecar reduce returned a non-finite coordinate".into())
                        })
                };
                Ok([at(0)?, at(1)?])
            })
            .collect::<Result<Vec<[f32; 2]>>>()?;
        Ok((coords, used))
    }

    /// Whether the OPTIONAL t-SNE reducer is installed in this venv at the pinned version. Cheap (a
    /// marker read) so it can be polled on every layout request to decide PCA-vs-t-SNE.
    pub fn optional_tsne_ready(&self) -> bool {
        self.paths.venv_python().exists()
            && std::fs::read_to_string(self.paths.tsne_marker())
                .map(|s| s.trim() == OPTIONAL_TSNE_PIN)
                .unwrap_or(false)
    }

    /// Install the OPTIONAL t-SNE reducer (openTSNE) into the managed venv on demand. The base venv
    /// must exist first, so this provisions it if needed, then `pip install`s the pin and stamps the
    /// t-SNE marker. Blocking and slow (a download); serialised by the install lock. Idempotent — a
    /// no-op once the marker is current.
    ///
    /// `on_progress` is called with a monotonic `0.0..=1.0` fraction as the install advances (derived
    /// from pip's `Collecting/Downloading/Installing` markers — see [`pip_phase_fraction`]), so the
    /// Map and Settings can show a real progress bar instead of an indeterminate spinner. The download
    /// has no file-count, so the UI renders this as a percentage.
    pub fn install_optional_tsne(&self, mut on_progress: impl FnMut(f32)) -> Result<()> {
        on_progress(0.03);
        // openTSNE goes into the base venv, which must exist with its requirements first.
        self.ensure_installed()?;
        on_progress(0.10);

        let _install = self.install.lock().unwrap();
        if self.optional_tsne_ready() {
            on_progress(1.0);
            return Ok(());
        }

        let py = self.paths.venv_python();
        // `--progress-bar off` so pip emits clean newline-terminated phase lines (no carriage-return
        // byte bar) we can parse; the side-thread stderr drain in run_pip_streaming avoids a deadlock.
        let mut downloads = 0u32;
        let mut last = 0.10f32;
        run_pip_streaming(
            &py,
            &[
                "install",
                "--disable-pip-version-check",
                "--progress-bar",
                "off",
                OPTIONAL_TSNE_PIN,
            ],
            |line| {
                if let Some(f) = pip_phase_fraction(line, &mut downloads) {
                    if f > last {
                        last = f;
                        on_progress(f);
                    }
                }
            },
        )?;

        std::fs::write(self.paths.tsne_marker(), OPTIONAL_TSNE_PIN)?;
        on_progress(1.0);
        Ok(())
    }

    /// Remove the OPTIONAL t-SNE component again (the "delete" action). Drops the marker first — that
    /// alone disables t-SNE (`optional_tsne_ready` then reports false and the map falls back to PCA) —
    /// then `pip uninstall`s openTSNE to reclaim space. Only openTSNE is removed: its heavier transitive
    /// deps (scipy / scikit-learn) are left in place so we can never accidentally break the base venv by
    /// pulling a package something else relies on; a later re-install is then quick. Idempotent.
    pub fn uninstall_optional_tsne(&self) -> Result<()> {
        let _install = self.install.lock().unwrap();
        // Drop the marker first so the feature is off even if the pip call below fails.
        let _ = std::fs::remove_file(self.paths.tsne_marker());
        let py = self.paths.venv_python();
        if !py.exists() {
            return Ok(());
        }
        let mut pip = Command::new(&py);
        pip.args([
            "-m",
            "pip",
            "uninstall",
            "-y",
            "--disable-pip-version-check",
            "openTSNE",
        ]);
        clean_python_env(&mut pip);
        no_window(&mut pip);
        // Best-effort: the marker is already gone, so a pip hiccup just leaves the (unused) package on
        // disk rather than failing the user's "remove".
        let _ = run_command(&mut pip, "pip uninstall openTSNE");
        Ok(())
    }

    /// Whether the OPTIONAL photo-OCR component is installed in this venv at the pinned versions.
    /// Cheap (a marker read) so the ingest path can check it per photo to decide whether to request
    /// OCR, and so Settings can show the install/remove state.
    pub fn optional_ocr_ready(&self) -> bool {
        self.paths.venv_python().exists()
            && std::fs::read_to_string(self.paths.ocr_marker())
                .map(|s| s.trim() == OPTIONAL_OCR_MARKER)
                .unwrap_or(false)
    }

    /// Install the OPTIONAL photo-OCR component (rapidocr + pillow-heif) into the managed venv on
    /// demand — provisions the base venv first if needed, `pip install`s the pins, then stamps the OCR
    /// marker. Blocking and slow (a download); serialised by the install lock. Idempotent — a no-op
    /// once the marker is current. `on_progress` reports a monotonic `0.0..=1.0` fraction (from pip's
    /// phase markers, see [`pip_phase_fraction`]) so Settings can show a real percentage bar.
    pub fn install_optional_ocr(&self, mut on_progress: impl FnMut(f32)) -> Result<()> {
        on_progress(0.03);
        self.ensure_installed()?;
        on_progress(0.10);

        let _install = self.install.lock().unwrap();
        if self.optional_ocr_ready() {
            on_progress(1.0);
            return Ok(());
        }

        let py = self.paths.venv_python();
        let mut downloads = 0u32;
        let mut last = 0.10f32;
        let mut args: Vec<&str> = vec![
            "install",
            "--disable-pip-version-check",
            "--progress-bar",
            "off",
        ];
        args.extend_from_slice(OPTIONAL_OCR_PINS);
        run_pip_streaming(&py, &args, |line| {
            if let Some(f) = pip_phase_fraction(line, &mut downloads) {
                if f > last {
                    last = f;
                    on_progress(f);
                }
            }
        })?;

        std::fs::write(self.paths.ocr_marker(), OPTIONAL_OCR_MARKER)?;
        on_progress(1.0);
        Ok(())
    }

    /// Remove the OPTIONAL photo-OCR component (the "delete" action). Drops the marker first — that
    /// alone disables OCR (`optional_ocr_ready` then reports false and future photos ingest EXIF-only)
    /// — then `pip uninstall`s rapidocr + pillow-heif. Like the t-SNE removal, the heavier transitive
    /// image deps (opencv / shapely / pyclipper) are LEFT in place here so we can never break the base
    /// venv; the Storage manager (components.rs) does the guarded cascade that reclaims them. Idempotent.
    pub fn uninstall_optional_ocr(&self) -> Result<()> {
        let _install = self.install.lock().unwrap();
        // Drop the marker first so the feature is off even if the pip call below fails.
        let _ = std::fs::remove_file(self.paths.ocr_marker());
        let py = self.paths.venv_python();
        if !py.exists() {
            return Ok(());
        }
        let mut pip = Command::new(&py);
        pip.args([
            "-m",
            "pip",
            "uninstall",
            "-y",
            "--disable-pip-version-check",
            "rapidocr",
            "pillow-heif",
        ]);
        clean_python_env(&mut pip);
        no_window(&mut pip);
        // Best-effort: the marker is already gone, so a pip hiccup just leaves the (unused) packages
        // on disk rather than failing the user's "remove".
        let _ = run_command(&mut pip, "pip uninstall rapidocr pillow-heif");
        Ok(())
    }

    /// `pip uninstall -y` the given packages from the managed venv — the Storage manager's guarded
    /// teardown of the optional t-SNE libraries (scikit-learn / scipy / their small siblings). The
    /// caller MUST enforce the dependency cascade (see `components.rs`); this only adds a final
    /// backstop that **refuses to ever remove `numpy`** (shared with the embedder). Blocking;
    /// serialised by the install lock. Idempotent — pip skips a package that isn't installed.
    pub fn pip_uninstall(&self, packages: &[&str]) -> Result<()> {
        if packages.iter().any(|p| p.eq_ignore_ascii_case("numpy")) {
            return Err(Error::Other(
                "refusing to remove numpy (it's shared with the search model)".into(),
            ));
        }
        let py = self.paths.venv_python();
        if !py.exists() {
            return Ok(());
        }
        let _install = self.install.lock().unwrap();
        let mut pip = Command::new(&py);
        pip.args([
            "-m",
            "pip",
            "uninstall",
            "-y",
            "--disable-pip-version-check",
        ]);
        pip.args(packages);
        clean_python_env(&mut pip);
        no_window(&mut pip);
        run_command(&mut pip, "pip uninstall")?;
        Ok(())
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

        // Refuse to send a line the child would silently drop (see MAX_SIDECAR_REQUEST_LINE):
        // that drop yields no reply and would wedge this call — and every queued call behind
        // the process mutex — forever. Nothing has been written yet, so the child stays healthy
        // and only this one call fails. (Normal callers stay far under this; the gateway
        // batches embed/count-token requests so a large document never reaches it.)
        if line.len() > MAX_SIDECAR_REQUEST_LINE {
            return Err(Error::Other(format!(
                "sidecar {method} request line is {} bytes, over the {MAX_SIDECAR_REQUEST_LINE}-byte \
                 cap — refusing to send it (the child would drop it silently and wedge)",
                line.len()
            )));
        }

        // On any IO failure — or a deadline with no reply — drop the (possibly dead) child so
        // the next call respawns it.
        let timeout = request_timeout(method);
        let send = (|| -> std::io::Result<Value> {
            proc.stdin.write_all(line.as_bytes())?;
            proc.stdin.write_all(b"\n")?;
            proc.stdin.flush()?;
            read_reply_with_timeout(proc, id, timeout)
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

/// Read newline-JSON replies from the child until one carries our request `id`, bounding how
/// many blank/non-matching lines we wade through first. With the monotonic request id a stale
/// line can never match, so anything skipped is noise. Returns the matched reply, an
/// `UnexpectedEof` if the child closed its output, or `InvalidData` if it is too chatty or a
/// single line overflows the size cap. Generic over the reader so it unit-tests against a
/// `Cursor` without a live child.
fn read_reply<R: BufRead>(stdout: &mut R, id: u64) -> std::io::Result<Value> {
    let mut skipped = 0usize;
    loop {
        // Bounded read: a runaway/oversized reply fails the call (and respawns the child)
        // instead of buffering unbounded into memory.
        let Some(bytes) = read_line_capped(stdout, MAX_SIDECAR_LINE)? else {
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
}

/// Read the reply to request `id`, but give up after `timeout` by killing the child so a
/// wedged or silent handler (a stalled first-use model download, an ONNX hang, a pathological
/// convert) can't block the caller — and thus the whole serialized sidecar — forever
/// (F-06 / B3-1). The blocking read runs on a scoped thread so this thread can, on the
/// deadline, kill the child; killing closes the child's stdout, which unblocks the reader at
/// EOF so the scope's implicit join always completes. This needs no shared kill handle and no
/// platform-specific process API — `std::thread::scope` + `child.kill()` is portable. On
/// timeout the child is killed and reaped, and `request` respawns it next call (the same
/// recovery as an IO error). A reply that arrives in time — success *or* a protocol error — is
/// returned unchanged, so nothing about the fast path changes.
fn read_reply_with_timeout(
    proc: &mut Process,
    id: u64,
    timeout: std::time::Duration,
) -> std::io::Result<Value> {
    // Disjoint borrows: the reader thread owns `stdout`, the watchdog owns `child`.
    let Process { child, stdout, .. } = &mut *proc;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            // If the watchdog already timed out and dropped `rx`, the send fails — harmless.
            let _ = tx.send(read_reply(stdout, id));
        });
        match rx.recv_timeout(timeout) {
            Ok(reply) => reply,
            Err(_) => {
                // Deadline hit (or the reader panicked and dropped its sender): kill the child
                // so its stdout closes and the scoped reader unblocks at EOF, then reap it. The
                // scope waits for that reader to finish before returning.
                let _ = child.kill();
                let _ = child.wait();
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "sidecar did not reply within its deadline",
                ))
            }
        }
    })
}

/// A per-method deadline for a sidecar request. These are **backstops against a permanently
/// wedged child**, not tight SLAs, so they are deliberately generous: the reachable, guaranteed
/// trigger (an oversized request line) is already closed by [`MAX_SIDECAR_REQUEST_LINE`] + the
/// gateway's batching, leaving only rare true stalls for the timeout to catch. Download-bearing
/// methods get the longest grace because a first-use model download (hundreds of MB up to
/// ~1 GB) runs *inside* the request with no stdout output, and killing a slow-but-real download
/// would break local ML — far worse than waiting. Values are intentionally round; tune on the
/// live rig if a legitimate operation is ever cut short.
fn request_timeout(method: &str) -> std::time::Duration {
    use std::time::Duration;
    match method {
        // First use of any of these can download a model; keep the grace long.
        "embed" | "rerank" | "transcribe" | "analyze_image" => Duration::from_secs(30 * 60),
        // CPU-bound conversion / parse / projection of a possibly-large or pathological input.
        "convert" | "analyze_spreadsheet" | "reduce" => Duration::from_secs(10 * 60),
        // Pure tokeniser pass — fast once the tokeniser is loaded.
        "count_tokens" => Duration::from_secs(5 * 60),
        // Any other (or future) method: a safe, generous default.
        _ => Duration::from_secs(10 * 60),
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

/// Run `python -m pip <pip_args>` in the venv, streaming stdout line-by-line to `on_line` so the
/// caller can turn pip's phase markers into progress. stderr is drained on a side thread (a full
/// stderr pipe could otherwise block our stdout reads) and surfaced in the error on failure.
fn run_pip_streaming(py: &Path, pip_args: &[&str], mut on_line: impl FnMut(&str)) -> Result<()> {
    let mut cmd = Command::new(py);
    cmd.arg("-m").arg("pip").args(pip_args);
    clean_python_env(&mut cmd);
    no_window(&mut cmd);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| Error::Other(format!("could not start pip install: {e}")))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = BufReader::new(stderr).read_to_string(&mut s);
        s
    });

    for line in BufReader::new(stdout).lines() {
        match line {
            Ok(l) => on_line(&l),
            Err(_) => break,
        }
    }

    let status = child
        .wait()
        .map_err(|e| Error::Other(format!("pip install did not exit cleanly: {e}")))?;
    let stderr_text = err_thread.join().unwrap_or_default();
    if !status.success() {
        let detail = stderr_text.trim();
        return Err(Error::Other(format!(
            "pip install failed:\n{}",
            if detail.is_empty() {
                "(no output)"
            } else {
                detail
            }
        )));
    }
    Ok(())
}

/// Map one line of pip's (`--progress-bar off`) stdout to a coarse, monotonic install fraction. Pure
/// (unit-tested). `downloads` accumulates real wheel downloads — `.metadata` fetches are skipped — so
/// the bar advances across packages. The caller keeps the running maximum, so a slightly miscounted
/// line can never make the bar jump backwards; the final 1.0 is emitted only once the marker is
/// stamped. This is phase-derived, not byte-exact (pip doesn't expose clean byte totals), but it is
/// honest: it only reaches completion when the install actually finishes.
fn pip_phase_fraction(line: &str, downloads: &mut u32) -> Option<f32> {
    let l = line.trim();
    if l.starts_with("Collecting ") || l.starts_with("Obtaining ") {
        return Some(0.16);
    }
    if (l.starts_with("Downloading ") || l.starts_with("Using cached ")) && !l.contains(".metadata")
    {
        *downloads += 1;
        // openTSNE pulls roughly scikit-learn, scipy, joblib, threadpoolctl, openTSNE (+ numpy if it
        // isn't already cached). An estimate is fine — the running max keeps it monotonic regardless.
        const EST_PACKAGES: f32 = 6.0;
        let frac = (*downloads as f32 / EST_PACKAGES).min(1.0);
        return Some(0.28 + 0.50 * frac); // ramp 0.28 -> 0.78 across the downloads
    }
    if l.starts_with("Installing collected packages") {
        return Some(0.86);
    }
    if l.starts_with("Successfully installed") {
        return Some(0.96);
    }
    None
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

    /// The OCR marker string must be exactly the pins joined by ';' — the installer writes the marker
    /// and `optional_ocr_ready` compares it, so a drift here would silently re-install on every check
    /// or never detect an install. Guards the hand-kept duplication of the two constants.
    #[test]
    fn optional_ocr_marker_matches_pins() {
        assert_eq!(OPTIONAL_OCR_PINS.join(";"), OPTIONAL_OCR_MARKER);
    }

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

    #[test]
    fn read_reply_returns_the_line_matching_the_request_id() {
        // The reply is preceded by a blank line and a stale reply for a different id (both noise
        // the monotonic id can never match); read_reply skips them and returns our reply.
        let mut r = Cursor::new(
            b"\n{\"id\":1,\"ok\":true,\"result\":{}}\n{\"id\":7,\"ok\":true,\"result\":{\"v\":42}}\n"
                .to_vec(),
        );
        let reply = read_reply(&mut r, 7).unwrap();
        assert_eq!(reply["id"].as_u64(), Some(7));
        assert_eq!(reply["result"]["v"].as_u64(), Some(42));
    }

    #[test]
    fn read_reply_reports_eof_when_the_child_closes_without_replying() {
        // The oversized-line drop on the Python side (and any dead child) shows up here as a
        // closed stdout — read_reply surfaces it as UnexpectedEof so request() respawns.
        let mut r = Cursor::new(b"{\"id\":3,\"ok\":true,\"result\":{}}\n".to_vec());
        let err = read_reply(&mut r, 9).unwrap_err(); // id 9 never appears → read to EOF
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn read_reply_gives_up_after_too_many_unmatched_lines() {
        // A chatty child that never sends our id must fail the call, not loop forever.
        let mut buf = Vec::new();
        for _ in 0..(MAX_SKIP_LINES + 5) {
            buf.extend_from_slice(b"{\"id\":1,\"ok\":true,\"result\":{}}\n");
        }
        let mut r = Cursor::new(buf);
        let err = read_reply(&mut r, 999).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn request_timeouts_are_generous_for_download_bearing_methods() {
        // Methods that can trigger a first-use model download get the longest grace so a slow
        // but real download is never mistaken for a hang; a plain tokeniser pass gets less; an
        // unknown method still gets a safe non-zero default.
        let secs = |m| request_timeout(m).as_secs();
        assert_eq!(secs("embed"), 30 * 60);
        assert_eq!(secs("transcribe"), 30 * 60);
        assert!(secs("count_tokens") < secs("embed"));
        assert!(secs("count_tokens") > 0);
        assert!(
            secs("something_new") > 0,
            "unknown methods get a safe default"
        );
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
    fn pip_phase_fraction_is_monotonic_across_a_real_install() {
        // Walk a representative pip transcript and assert the derived fraction never goes backwards
        // and lands in the expected phase bands. (A free fn, not a capturing closure, so we can also
        // read `downloads` directly between steps.)
        fn band(f: f32, lo: f32, hi: f32, last: &mut f32, line: &str) {
            assert!(
                f + 1e-6 >= *last,
                "fraction regressed at {line:?}: {f} < {last}"
            );
            assert!(
                f >= lo && f <= hi,
                "fraction {f} for {line:?} not in [{lo},{hi}]"
            );
            *last = f;
        }
        let mut downloads = 0u32;
        let mut last = 0.10f32;

        let l = "Collecting openTSNE==1.0.4";
        band(
            pip_phase_fraction(l, &mut downloads).unwrap(),
            0.15,
            0.20,
            &mut last,
            l,
        );

        // A `.metadata` fetch must NOT count as a package download.
        let before = downloads;
        assert_eq!(
            pip_phase_fraction(
                "  Downloading openTSNE-1.0.4.whl.metadata (5 kB)",
                &mut downloads
            ),
            None
        );
        assert_eq!(
            downloads, before,
            "metadata fetch must not advance the download count"
        );

        let l = "  Downloading scipy-1.14.0-cp314-win_amd64.whl (44 MB)";
        band(
            pip_phase_fraction(l, &mut downloads).unwrap(),
            0.30,
            0.40,
            &mut last,
            l,
        );
        let l = "  Downloading scikit_learn-1.5.0-cp314-win_amd64.whl (11 MB)";
        band(
            pip_phase_fraction(l, &mut downloads).unwrap(),
            0.35,
            0.50,
            &mut last,
            l,
        );
        let l =
            "Installing collected packages: threadpoolctl, scipy, joblib, scikit-learn, openTSNE";
        band(
            pip_phase_fraction(l, &mut downloads).unwrap(),
            0.85,
            0.87,
            &mut last,
            l,
        );
        let l = "Successfully installed openTSNE-1.0.4 scipy-1.14.0";
        band(
            pip_phase_fraction(l, &mut downloads).unwrap(),
            0.95,
            0.97,
            &mut last,
            l,
        );

        // Unrelated chatter is ignored.
        assert_eq!(
            pip_phase_fraction("WARNING: something cosmetic", &mut downloads),
            None
        );
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
