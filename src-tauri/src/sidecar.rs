// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The document sidecar: a long-lived Python child process that converts files
//! to Markdown (MarkItDown) and embeds text locally with an ONNX model
//! (fastembed). It is the only Python in PM; everything else is Rust.
//!
//! Python is provided by a *managed venv* created on first run (spec decision):
//! the app locates a base interpreter — the standalone one bundled with the app
//! on Windows release builds, else a system Python — builds an isolated venv
//! under the data directory, and pip-installs `requirements.lock` with
//! `--require-hashes` — a fully resolved, hash-pinned set covering every transitive
//! dependency, so an artifact whose digest does not match is refused rather than
//! executed. A `.ready` marker keyed by a hash of the LOCK lets later runs skip the
//! slow setup.
//!
//! Talking to the child is newline-delimited JSON over stdio. Requests are
//! serialized by the `Mutex<Option<Process>>`, so each reply is the next line on
//! stdout (tracebacks and download progress go to stderr). Callers must run
//! these methods off the async runtime (see `tokio::task::spawn_blocking` in the
//! ingest command) — they block. Never hold the DB lock across a sidecar call.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use serde_json::{json, Value};

use crate::photos::ImageAnalysis;
use crate::registry::{ModelEntry, Pooling, Source};

/// The `hf` repo a locally-trained model is registered under.
///
/// fastembed's `ModelSource` accepts only `hf` or `url` and rejects a description with neither, so a
/// disk-resident model still needs one — but it is never fetched: passing `specific_model_path` at
/// construction short-circuits resolution before any source is consulted (verified against
/// fastembed 0.8.1 and 0.9.0, `common/model_management.py`). The placeholder is deliberately not a real repo,
/// so that if the short-circuit ever regressed the failure would be an immediate 404 on a
/// nonexistent name rather than a silent download of somebody else's weights.
const LOCAL_MODEL_SOURCE: &str = "pm-local/not-a-hub-model";

/// Build the fastembed `add_custom_model` spec for a non-bundled model, as a JSON object the
/// sidecar registers on first use. `None` for a bundled model (fastembed already knows it). One
/// shape serves both embedders and rerankers — the reranker registration ignores the
/// pooling/normalize/dim fields.
///
/// A [`Source::LocalPath`] model additionally carries `local_path` (its directory), which the
/// sidecar passes as `specific_model_path` so the weights are loaded from disk instead of fetched.
/// This is the Stage-4 learned-reranker seam: without it a locally-trained ONNX model could be
/// described in the registry but never actually loaded.
fn custom_spec(m: &ModelEntry) -> Option<Value> {
    let model_file = m.model_file?;
    let mut local_path = None;
    let hf = match &m.source {
        Source::HuggingFace(repo) => *repo,
        Source::LocalPath(dir) => {
            local_path = Some(dir.to_string_lossy().into_owned());
            LOCAL_MODEL_SOURCE
        }
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
        // Absent (null) for a hub model — the sidecar only passes `specific_model_path` when set.
        "local_path": local_path,
    }))
}

use crate::error::{Error, Result};

/// Hard cap on a single sidecar reply line (see [`read_line_capped`]). A reply
/// carries converted Markdown derived from an untrusted ingested file, so a
/// crafted document could otherwise make the child emit a multi-hundred-MB line
/// and exhaust memory before we ever parse it (rule #6). 64 MiB is far above any
/// legitimate reply.
const MAX_SIDECAR_LINE: usize = 64 * 1024 * 1024;

/// The largest source file the sidecar will read for convert / image / spreadsheet analysis (F-57). A
/// file past this balloons the Python child's memory (the reader materialises it) before the reply cap
/// above could ever trip — an OOM on the 8 GB target. Pre-flighted in Rust so the work is refused before
/// the child is even asked; the sidecar refuses it too (defense in depth). Keep in sync with
/// `pm_sidecar.py` `MAX_INPUT_FILE_BYTES`.
const MAX_SIDECAR_INPUT_BYTES: u64 = 128 * 1024 * 1024;

/// The input cap for TEXT-FAMILY files, whose converted Markdown is roughly the size of the input
/// (a .txt converts to about itself; a .pdf or .docx extracts to a small fraction). Those files
/// could pass the 128 MiB input cap and then produce a reply that CANNOT fit under
/// [`MAX_SIDECAR_LINE`] — a guaranteed failure the guard is supposed to prevent, arriving only after
/// minutes of conversion and costing a child kill + respawn. That broke this guard's own stated
/// promise that oversized work is "refused before the child is even asked".
///
/// 40 MiB, not 64: the reply is Markdown-wrapped and JSON-escaped, so it is somewhat LARGER than the
/// source. The headroom keeps the refusal honest rather than merely moving the cliff. Far above any
/// real document either way — 40 MiB of plain text is roughly 20,000 pages.
const MAX_SIDECAR_TEXT_INPUT_BYTES: u64 = 40 * 1024 * 1024;

/// Extensions whose conversion output is roughly the input's own size. Container/binary formats
/// (pdf, docx, pptx, epub, …) extract to far less text, so they keep the full 128 MiB allowance.
/// Mirrors `pm_sidecar.py`'s `TEXT_FAMILY_EXTS`.
const TEXT_FAMILY_EXTS: &[&str] = &["txt", "md", "markdown", "html", "htm", "json", "xml"];

/// Refuse an over-cap input file before handing its path to the sidecar (F-57), with a clear message
/// rather than a wedged/OOM'd child. A missing/unreadable file is NOT this guard's concern — it passes
/// through so the call reports the real IO error (and the Python side keeps its own graceful handling,
/// e.g. `analyze_image`'s null metadata for an unreadable image).
fn guard_input_size(path: &Path) -> Result<()> {
    match std::fs::metadata(path) {
        Ok(m) => check_input_size(m.len(), input_cap_for(path)),
        Err(_) => Ok(()),
    }
}

/// The cap that applies to `path`: the reply-safe one for text-family files, else the full input cap.
/// An unknown/absent extension gets the generous cap — this guard exists to stop a KNOWN-futile
/// conversion, not to second-guess files it can't classify.
fn input_cap_for(path: &Path) -> u64 {
    let text_family = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| TEXT_FAMILY_EXTS.contains(&e.as_str()));
    if text_family {
        MAX_SIDECAR_TEXT_INPUT_BYTES
    } else {
        MAX_SIDECAR_INPUT_BYTES
    }
}

/// The pure size check behind [`guard_input_size`], split out so the cap logic is unit-tested without
/// materialising a multi-hundred-MB file.
fn check_input_size(size: u64, cap: u64) -> Result<()> {
    if size > cap {
        return Err(Error::Other(format!(
            "file is too large to process ({} MiB; the limit is {} MiB)",
            size / (1024 * 1024),
            cap / (1024 * 1024)
        )));
    }
    Ok(())
}

/// What a DOCUMENT states about itself, as opposed to what its container on disk says (#709).
///
/// The refinement of #701's "a filesystem has no author": that is still true of the filesystem, and
/// PM still never names the OS account — but an OOXML container carries `docProps/core.xml` and a
/// PDF carries an Info dictionary, and a document that plainly names its author should not read
/// "Unknown" just because it arrived from a folder rather than from Drive.
///
/// `None` throughout means the document did not say, which is exactly what an absent provider fact
/// means, so these flow into the same columns and render the same way.
///
/// No `modified_at`, deliberately. For a local file `source_modified_at` is the filesystem mtime,
/// which is what the connector diffs to notice a change; a second, differently-sourced "modified"
/// would sooner or later be substituted for it, and a document whose docProps date predates the copy
/// on disk would then look permanently stale.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileProperties {
    pub author: Option<String>,
    pub last_modified_by: Option<String>,
    pub created_at: Option<String>,
}

/// The extensions whose format defines a document-properties block. Everything else — .md, .txt,
/// .html, .csv — genuinely has nowhere to state an author, so asking costs a round trip to be told
/// nothing. .epub is a zip too, but indirects its metadata through META-INF/container.xml, which is
/// a different reader than the one the sidecar has.
const DOCUMENT_PROPERTY_EXTS: &[&str] = &["docx", "pptx", "xlsx", "xlsm", "pdf"];

/// Whether this file's format can state an author at all. Pure; the gate that keeps a folder of
/// plain-text notes from paying for a sidecar call per file.
pub fn carries_document_properties(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| DOCUMENT_PROPERTY_EXTS.contains(&e.as_str()))
}

/// The sidecar's `file_properties` reply as [`FileProperties`]. Split out and pure so the wire
/// contract with `pm_sidecar.py` — including its "" and null cases — is unit-tested without a child
/// process. A blank string reads as unstated: an empty author rendered under "Author" looks like PM
/// lost the value rather than never having had it.
fn file_properties_from_reply(reply: &Value) -> FileProperties {
    let field = |key: &str| {
        reply[key]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    FileProperties {
        author: field("author"),
        last_modified_by: field("last_modified_by"),
        created_at: field("created"),
    }
}

/// How many non-matching stdout lines we'll skip while waiting for our reply before
/// giving up and respawning. With the monotonic request id a stale/old line can
/// never match, so anything skipped is noise; this bounds a chatty or wedged child
/// to a failed call instead of an infinite loop.
const MAX_SKIP_LINES: usize = 1024;

/// Hard cap on a single *request* line we will send to the child, kept below the child's own
/// `MAX_LINE_CHARS` (64 MiB, mirrored in `pm_sidecar.py`). The child *silently drops* a line
/// over that cap — no reply — which would leave [`SidecarManager::request`] blocked forever on
/// a reply that never comes, wedging the whole (serialized) sidecar: ingest, chat retrieval,
/// rerank and the map all queue behind the dead call until restart (F-06 / B3-1).
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
/// and downloads its small detection/recognition ONNX models on first use; `pi-heif` adds HEIC
/// decoding (Pillow itself is already present via markitdown). Both — and rapidocr's image deps
/// (opencv/shapely/pyclipper) — ship binary wheels for the bundled 3.12 release interpreter and dev
/// 3.14, so there's no compile step.
///
/// **`pi-heif`, not `pillow-heif`.** They are the same bindings by the same author, built from the
/// same repository, and both expose the `register_heif_opener` the sidecar imports — but they are
/// packaged differently, and the licence gate (`just sidecar-licences`) is what surfaced it.
/// pillow-heif's binary wheels bundle an HEVC *encoder*, x265, so upstream's own
/// `LICENSES_bundled.txt` reads "License for pillow-heif binary wheels: GPLv2". PM only ever
/// DECODES a HEIC, so that encoder was never reachable — it was 22 MB of GPL-2.0 PM asked a user's
/// machine to install for nothing, in an AGPL-3.0-or-later product it is not compatible with.
/// pi-heif is the decode-only build of the same bindings: libheif + libde265, both LGPLv3, in a
/// wheel a third of the size.
const OPTIONAL_OCR_PINS: &[&str] = &["rapidocr==3.10.0", "pi-heif==1.4.0"];

/// The speech engine voice input used to install, as pip names its packages, in the order the
/// clean-up asks pip to remove them. Voice input is gone, and pip never removes a package because a
/// lock stopped listing it, so every venv that ever had the engine (about 240 MB, nearly all of it
/// CTranslate2 and the FFmpeg inside PyAV) keeps it until [`SidecarManager::retire_voice`] removes
/// it. None of the four is in the base lock or another component's lock
/// (`no_lock_installs_a_package_the_voice_clean_up_removes` reads them all), so removing them
/// can't take anything the document engine or an optional component runs on.
///
/// setuptools is here because CTranslate2 depends on it, but finding it proves nothing: a venv
/// built on Python 3.11 has its own setuptools from `venv`, voice or not. So it is uninstalled only
/// beside an engine pip can remove, one of the other three with a dist-info and its RECORD in the
/// site-packages of the venv's own interpreter ([`pip_can_uninstall_voice_engine`]), and it is
/// never removed by hand ([`VOICE_ENGINE_DIRS`]).
///
/// pip is asked for one package at a time: one `pip uninstall` naming several gives up on all of
/// them at the first it can't remove (a dist-info with no RECORD, or a native library something
/// still holds open on Windows). faster-whisper goes last, after the two heavy libraries it runs
/// on, so a clean-up cut short (PM closed mid-way) never leaves them behind the package that is
/// the plainest sign of an engine.
const VOICE_ENGINE_PACKAGES: &[&str] = &["av", "ctranslate2", "setuptools", "faster-whisper"];

/// The engine's own three packages by their import names, which also name their other entries in
/// site-packages ([`is_voice_engine_entry`]). setuptools is deliberately not one of them (see
/// [`VOICE_ENGINE_PACKAGES`]), so it neither counts as an engine nor is ever removed by hand.
const VOICE_ENGINE_DIRS: &[&str] = &["faster_whisper", "ctranslate2", "av"];

/// Whether a site-packages entry belongs to one of the engine's own packages: exactly `<name>` (the
/// import dir), `<name>.libs` (the native libraries a Linux or Windows wheel bundles beside it) or
/// `<name>-<version>.dist-info`, for a name in [`VOICE_ENGINE_DIRS`]. Exact, so PyAV's `av-` never
/// matches `avro-1.12.0.dist-info`. The look on disk and the removal by hand both ask this, so the
/// clean-up can never find something it would not then remove.
fn is_voice_engine_entry(name: &str) -> bool {
    is_voice_engine_dist_info(name)
        || VOICE_ENGINE_DIRS.iter().any(|d| {
            name.strip_prefix(d)
                .is_some_and(|rest| rest.is_empty() || rest == ".libs")
        })
}

/// Whether a site-packages entry is the `<name>-<version>.dist-info` of one of the engine's own
/// packages: the record pip finds an installed package by, and uninstalls it from.
fn is_voice_engine_dist_info(name: &str) -> bool {
    VOICE_ENGINE_DIRS.iter().any(|d| {
        name.strip_prefix(d)
            .and_then(|rest| rest.strip_prefix('-'))
            .is_some_and(|rest| rest.ends_with(".dist-info"))
    })
}

/// The characters pip puts after the `~` that starts a stash's name, in the order it tries them
/// (`AdjacentTempDirectory.LEADING_CHARS` in pip's `utils/temp_dir.py`).
const PIP_STASH_CHARS: &[u8] = b"-~.=%0123456789";

/// Whether `run` is one pip draws from [`PIP_STASH_CHARS`]: any number of them, repeats allowed,
/// never one after another that comes later in that order (`itertools.combinations_with_replacement`).
fn is_pip_stash_run(run: &[u8]) -> bool {
    let mut last = 0;
    run.iter()
        .all(|c| match PIP_STASH_CHARS.iter().position(|s| s == c) {
            Some(at) if at >= last => {
                last = at;
                true
            }
            _ => false,
        })
}

/// Whether pip could have named its stash of the folder `name` as `stash`. pip removes a package
/// by first moving each of its folders aside, to the first of these names not already taken
/// (`AdjacentTempDirectory._generate_names`), and deletes those once the removal is done: `~` and a
/// run of i - 1 stash characters in place of the name's first i characters, for i from 1 to one
/// short of the whole name (so `av` becomes `~v`, and `ctranslate2` `~translate2`, then
/// `~-ranslate2`), then `~` and a run of at most 14 before the whole name (`~av`, `~-av`).
fn is_pip_stash_name(stash: &str, name: &str) -> bool {
    let Some(rest) = stash.as_bytes().strip_prefix(b"~") else {
        return false;
    };
    let name = name.as_bytes();
    let before_whole = rest
        .strip_suffix(name)
        .is_some_and(|run| run.len() < PIP_STASH_CHARS.len() && is_pip_stash_run(run));
    let in_place = rest.len() + 1 == name.len()
        && (1..name.len()).any(|i| rest[i - 1..] == name[i..] && is_pip_stash_run(&rest[..i - 1]));
    before_whole || in_place
}

/// Whether a site-packages entry is pip's stash ([`is_pip_stash_name`]) of one of the engine's
/// entries: an import dir, a `.libs`, or a dist-info. A removal cut short, or a delete Windows
/// refused, leaves stashes behind. A dist-info's version can be anything, so for one only the
/// names that keep its `-<version>.dist-info` count; past the hyphen it could be any package's.
/// A stash is no installed package, so it never has pip run (and another package's can share the
/// name), but it is a leftover the quick look counts, so one stranded after the engine's last live
/// entry still goes by hand.
fn is_voice_engine_stash(name: &str) -> bool {
    VOICE_ENGINE_DIRS.iter().any(|d| {
        is_pip_stash_name(name, d)
            || is_pip_stash_name(name, &format!("{d}.libs"))
            || name
                .strip_suffix(".dist-info")
                .and_then(|stem| stem.rsplit_once('-'))
                .is_some_and(|(head, version)| {
                    !version.is_empty() && is_pip_stash_name(&format!("{head}-"), &format!("{d}-"))
                })
    })
}

/// Whether a site-packages entry is anything of the engine's the clean-up removes by hand: one of
/// its entries, or pip's stash of one. The quick look and the removal both ask this, so the
/// clean-up can never find something it would not then remove.
fn is_voice_engine_leftover(name: &str) -> bool {
    is_voice_engine_entry(name) || is_voice_engine_stash(name)
}

/// Whether `path` is a real folder: not a file, and not a link or a Windows junction, which could
/// lead out of PM's data dir. `symlink_metadata` looks at the entry itself, never at its target.
fn real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

/// Every site-packages folder in the venv: `Lib/site-packages` on Windows, and every
/// `lib/python3.X/site-packages` elsewhere, not only the first. A venv rebuilt in place on another
/// Python minor keeps its old `lib/python3.X` beside the new one, with whatever was installed into
/// it, and pip on the new interpreter never looks there. Real folders only, all the way down, so
/// nothing the clean-up removes is ever reached through a link.
fn venv_site_dirs(venv: &Path) -> Vec<PathBuf> {
    if !real_dir(venv) {
        return Vec::new();
    }
    if cfg!(windows) {
        let lib = venv.join("Lib");
        let site = lib.join("site-packages");
        return if real_dir(&lib) && real_dir(&site) {
            vec![site]
        } else {
            Vec::new()
        };
    }
    let lib = venv.join("lib");
    if !real_dir(&lib) {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(&lib) else {
        return Vec::new();
    };
    let mut sites: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("python"))
        // `file_type` is the entry's own type, so a `python3.X` that is a link is passed over.
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path().join("site-packages"))
        .filter(|site| real_dir(site))
        .collect();
    sites.sort();
    sites
}

/// Whether the speech engine, or any part of it, is in the venv: an entry of one of its own three
/// packages ([`is_voice_engine_entry`]) in any of its site-packages. Any one is enough: a removal
/// that failed part-way can leave CTranslate2 or PyAV (nearly all of the ~240 MB) without
/// faster-whisper.
fn voice_engine_on_disk(venv: &Path) -> bool {
    venv_site_has(venv, is_voice_engine_entry)
}

/// Whether any of the venv's site-packages ([`venv_site_dirs`]) holds an entry `wanted` names.
fn venv_site_has(venv: &Path, wanted: impl Fn(&str) -> bool) -> bool {
    venv_site_dirs(venv).iter().any(|site| {
        std::fs::read_dir(site).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|e| wanted(&e.file_name().to_string_lossy()))
        })
    })
}

/// Whether pip can remove any of the engine from `site`: one of its three packages has a dist-info
/// there with a RECORD, which is what pip uninstalls a package by. Without one, pip only reports
/// each package not installed, or refuses it, and a leftover nothing can delete would cost a worker
/// stop and four interpreter starts on every launch it outlives.
fn pip_can_uninstall_voice_engine(site: &Path) -> bool {
    std::fs::read_dir(site).is_ok_and(|entries| {
        entries.flatten().any(|e| {
            is_voice_engine_dist_info(&e.file_name().to_string_lossy())
                && e.file_type().is_ok_and(|t| t.is_dir())
                && std::fs::symlink_metadata(e.path().join("RECORD")).is_ok_and(|m| m.is_file())
        })
    })
}

/// Remove by hand, in every site-packages, what pip left of the engine's own three packages, or was
/// never asked to remove: their entries and pip's stashes of them ([`is_voice_engine_leftover`]).
/// That is what pip can't take: an import dir with no dist-info (pip calls it not installed), a
/// dist-info with no RECORD (pip refuses it), anything in a site-packages other than its own
/// interpreter's, and a stash Windows wouldn't let pip delete. It leaves the few small launcher
/// scripts pip would also have taken from the venv's `bin/` (nothing runs them), and setuptools
/// always.
fn remove_voice_engine_by_hand(venv: &Path) {
    for site in venv_site_dirs(venv) {
        let Ok(entries) = std::fs::read_dir(&site) else {
            continue;
        };
        // Collected first, so nothing is removed from the folder while it is being read.
        let leftovers: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| is_voice_engine_leftover(&e.file_name().to_string_lossy()))
            .map(|e| e.path())
            .collect();
        for path in leftovers {
            remove_voice_leftover(&path);
        }
    }
}

/// The Whisper model's own folder under `runtime/models`, where voice input downloaded it: the
/// worker loaded `WhisperModel("base.en", download_root=<runtime/models>)`, and faster-whisper
/// handed that root to huggingface_hub as its `cache_dir`, which lays the repo out as
/// `models--<org>--<name>`. The same root also holds the embedder and reranker (`fastembed/`), the
/// hf and xet caches (`hf/`) and the photo-OCR weights (`rapidocr/`), so the clean-up removes this
/// folder and its lock folder ([`whisper_paths`]) and nothing else.
const WHISPER_REPO_DIR: &str = "models--Systran--faster-whisper-base.en";

/// Every path the Whisper download left under `models`: the repo folder and huggingface_hub's lock
/// folder for it. Each only inside a real folder ([`real_dir`]): a `models` or `.locks` that is a
/// link or a junction leads somewhere PM doesn't own, so nothing is looked for or removed there.
fn whisper_paths(models: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if real_dir(models) {
        paths.push(models.join(WHISPER_REPO_DIR));
        let locks = models.join(".locks");
        if real_dir(&locks) {
            paths.push(locks.join(WHISPER_REPO_DIR));
        }
    }
    paths
}

/// The recordings voice input left in `runtime/tmp`, named `voice-<random>.webm`. Each was deleted
/// as soon as it was transcribed, so only a crash or a kill mid-transcription left one behind.
/// None when `tmp` is not a real folder ([`real_dir`]).
fn voice_clips(tmp: &Path) -> Vec<PathBuf> {
    if !real_dir(tmp) {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(tmp) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("voice-") && name.ends_with(".webm")
        })
        .map(|e| e.path())
        .collect()
}

/// Whether `path` is a real folder ([`real_dir`]) with nothing in it. An empty folder a link points
/// to does not count: the clean-up never removes a link it finds there, so counting it would have
/// every launch find it again.
fn is_empty_dir(path: &Path) -> bool {
    real_dir(path) && std::fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_none())
}

/// Whether an entry is a link to a folder that only `remove_dir` takes away: on Windows a directory
/// symlink or a junction, which `remove_file` refuses. `remove_dir` removes the link itself and
/// leaves its target alone. On other systems a link to a folder is removed like a file.
#[cfg(windows)]
fn is_dir_link(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::FileTypeExt;
    meta.file_type().is_symlink_dir()
}

#[cfg(not(windows))]
fn is_dir_link(_meta: &std::fs::Metadata) -> bool {
    false
}

/// Remove a file or a folder (a link itself, never what it points at) if it is there, and log a
/// failure rather than return it: every step of the voice clean-up is best-effort, and whatever is
/// left is tried again at the next launch.
fn remove_voice_leftover(path: &Path) {
    let removed = match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => Err(e),
        // A real folder: `remove_dir_all` deletes what is in it without following a link inside.
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(meta) if is_dir_link(&meta) => std::fs::remove_dir(path),
        // A file, or a link (to a file, or on Linux and macOS to a folder): the entry alone goes.
        Ok(_) => std::fs::remove_file(path),
    };
    if let Err(e) = removed {
        eprintln!(
            "sidecar: couldn't remove {} that voice input left ({e}); the next launch tries again",
            path.display()
        );
    }
}

/// One on-demand pip component (t-SNE / photo-OCR). The per-component ready/install/uninstall
/// operations differ only in these fields, so they share one implementation each
/// (`optional_ready` / `install_optional` / `uninstall_optional` on [`SidecarManager`]); the
/// public per-component methods are one-line delegates.
struct OptionalComponent {
    /// The venv marker recording what is installed (`.pm-tsne` / `.pm-ocr`). Its contents are
    /// derived by [`SidecarManager::optional_stamp`], never stored as a second literal — the
    /// hand-kept `OPTIONAL_OCR_MARKER` copy of the joined pins used to need its own test to stop
    /// the two drifting.
    marker: fn(&SidecarPaths) -> PathBuf,
    /// The component's hash-pinned lock, alongside `requirements.lock` in `source_dir`. This is
    /// what pip actually installs (`--require-hashes -r <lock>`); `pins` below stays the source of
    /// truth for WHICH packages the component is, and `just requirements-lock` fails if the two
    /// disagree. Each optional lock is resolved AGAINST the base lock, so installing a component
    /// on demand can never move a package the base venv is already running on.
    lock: &'static str,
    /// The component's top-level pins — the source of truth for its identity, mirrored into
    /// `sidecar/requirements-optional.txt` for `just pip-audit` and into the lock's `pm-pins` stamp.
    pins: &'static [&'static str],
    /// The packages `pip uninstall` removes, and never one anything else needs. For t-SNE and OCR
    /// that means the top-level ones only: their heavier transitive deps are LEFT in place so a
    /// removal can never break the base venv by pulling a package something else relies on (see the
    /// public delegates' docs for each component's cascade).
    uninstall: &'static [&'static str],
    /// Packages an older lock of this component installed that the current lock replaces with a
    /// differently-named distribution of the SAME files, uninstalled before the lock is installed.
    /// pip never removes a package because another one supersedes it, so without this both would
    /// end up owning one folder, and removing either later deletes the files the other still
    /// claims.
    superseded: &'static [&'static str],
}

const OPTIONAL_TSNE_COMPONENT: OptionalComponent = OptionalComponent {
    marker: SidecarPaths::tsne_marker,
    lock: "requirements-tsne.lock",
    pins: &[OPTIONAL_TSNE_PIN],
    uninstall: &["openTSNE"],
    superseded: &[],
};

const OPTIONAL_OCR_COMPONENT: OptionalComponent = OptionalComponent {
    marker: SidecarPaths::ocr_marker,
    lock: "requirements-ocr.lock",
    pins: OPTIONAL_OCR_PINS,
    // `pillow-heif` is still listed although nothing installs it any more: a user who added photo
    // OCR before the pi-heif swap has it in their venv, and "remove photo OCR" should take it with
    // them rather than strand 28 MB (and the GPL-2.0 x265 inside it) forever. `pip uninstall -y`
    // skips a package that isn't installed, so this costs the common case nothing.
    uninstall: &["rapidocr", "pi-heif", "pillow-heif"],
    // rapidocr 3.10 depends on `opencv-python-headless` where 3.9 depended on `opencv-python`; both
    // install `cv2/`. On an upgrade pip would add the headless build BESIDE the old one, and the
    // Storage tab's "remove OpenCV" (or any later uninstall of one of them) would then delete the
    // shared `cv2/` while the other's dist-info stayed: the next OCR install is a pip no-op,
    // `import cv2` fails, and photos silently fall back to date and location only.
    superseded: &["opencv-python"],
};

/// Every optional component, so a caller that must treat them all alike can't miss one. `provision`
/// is the reason this exists: a venv rebuild removes their markers along with the venv, and each new
/// component would otherwise need remembering there too.
const ALL_OPTIONAL_COMPONENTS: &[&OptionalComponent] =
    &[&OPTIONAL_TSNE_COMPONENT, &OPTIONAL_OCR_COMPONENT];

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

    /// The fully resolved, hash-pinned lock generated from `requirements.txt` — what pip actually
    /// installs. `requirements.txt` pins only the top-level packages, so it stays the file a human
    /// edits and `just lock-regen` regenerates this from it; nothing reads it at runtime.
    fn lock(&self) -> PathBuf {
        self.source_dir.join("requirements.lock")
    }

    /// An optional component's lock (see [`OptionalComponent::lock`]).
    fn optional_lock(&self, component: &OptionalComponent) -> PathBuf {
        self.source_dir.join(component.lock)
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

    /// The two markers voice input kept in the venv: `.pm-voice` (its engine installed as an optional
    /// component) and `.pm-voice-kept` (an engine an older PM installed, kept when voice was switched
    /// off). Nothing reads them any more; [`SidecarManager::retire_voice`] removes them.
    fn voice_markers(&self) -> [PathBuf; 2] {
        [
            self.venv_dir.join(".pm-voice"),
            self.venv_dir.join(".pm-voice-kept"),
        ]
    }

    /// Where the downloaded models are cached (the embedder, the reranker, the photo-OCR weights and
    /// the hf and xet caches) — a sibling of the venv under `runtime/`, so they live inside PM's data
    /// dir and uninstall with it.
    fn models_dir(&self) -> Option<PathBuf> {
        self.venv_dir.parent().map(|p| p.join("models"))
    }

    /// `runtime/tmp`, where voice input wrote each recording while it was transcribed. Nothing else
    /// used it, so the clean-up removes the folder once it is empty.
    fn voice_tmp_dir(&self) -> Option<PathBuf> {
        self.venv_dir.parent().map(|p| p.join("tmp"))
    }

    /// Where a standalone interpreter that must live OUTSIDE the install lands — a
    /// sibling of the venv under `runtime/`, so it lives inside PM's data dir and
    /// uninstalls with it. Two producers share the location (and its teardown):
    /// the macOS runtime download ([`crate::python_fetch`]) and the Linux AppImage
    /// stable copy ([`Self::stable_bundled_python`]). Windows never populates it.
    #[cfg_attr(windows, allow(dead_code))]
    fn downloaded_python_dir(&self) -> Option<PathBuf> {
        self.venv_dir.parent().map(|p| p.join("python-standalone"))
    }

    /// Linux: the stable home for the bundled interpreter when the app runs out of
    /// an AppImage. AppImages mount their squashfs at a randomized
    /// `/tmp/.mount_XXXXXX` per launch, and a venv records its base interpreter by
    /// path (the `bin/python` symlink and `pyvenv.cfg`'s `home=`) — so a venv built
    /// straight against the mounted resource dir works on the FIRST launch and
    /// breaks on the second. Materialize the bundled `python/` tree into
    /// `runtime/python-standalone/python/` (stamp-keyed by the `.pm-pyver` file
    /// fetch-python ships in the tree, so a pin bump re-copies) and build the venv
    /// against that instead. rpm and dev installs return `None`: their resource
    /// dir is already a stable path.
    #[cfg(target_os = "linux")]
    fn stable_bundled_python(&self, bundled_exe: &Path) -> std::io::Result<Option<PathBuf>> {
        if !running_from_appimage(
            std::env::var("APPDIR").ok().as_deref(),
            std::env::var("APPIMAGE").ok().as_deref(),
        ) {
            return Ok(None);
        }
        // bundled exe = <mount>/…/python/bin/python3 → ancestors().nth(2) is the
        // `python/` tree root fetch-python unpacked.
        let (Some(src_tree), Some(dest_parent)) =
            (bundled_exe.ancestors().nth(2), self.downloaded_python_dir())
        else {
            return Ok(None);
        };
        let dest_tree = dest_parent.join("python");
        let copied_exe = dest_tree.join("bin").join("python3");
        let stamp = |dir: &Path| std::fs::read_to_string(dir.join(".pm-pyver")).ok();
        let src_stamp = stamp(src_tree);
        if stable_copy_current(src_stamp.as_deref(), stamp(&dest_tree).as_deref())
            && copied_exe.exists()
        {
            return Ok(Some(copied_exe));
        }
        match std::fs::remove_dir_all(&dest_tree) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        // copy_tree skips `.pm-pyver`; it is written LAST, below, so an interrupted
        // copy (kill / OOM / power loss mid-tree) can never leave a stamped-but-torn
        // tree that the currency check above would then trust forever — the same
        // write-the-marker-last ordering fetch-python.mjs and `.pm-ready` use.
        copy_tree(src_tree, &dest_tree)?;
        if !copied_exe.exists() {
            return Err(std::io::Error::other(format!(
                "copied the bundled interpreter to {} but bin/python3 is missing",
                dest_tree.display()
            )));
        }
        let Some(src_stamp) = src_stamp else {
            return Err(std::io::Error::other(format!(
                "the bundled interpreter at {} has no .pm-pyver stamp — a packaging \
                 bug (fetch-python.mjs always writes one)",
                src_tree.display()
            )));
        };
        std::fs::write(dest_tree.join(".pm-pyver"), src_stamp)?;
        Ok(Some(copied_exe))
    }
}

/// True when this process runs out of a mounted AppImage (either env var is set by
/// the AppImage runtime). Pure over the env values so the decision is unit-testable.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn running_from_appimage(appdir: Option<&str>, appimage: Option<&str>) -> bool {
    appdir.is_some_and(|v| !v.is_empty()) || appimage.is_some_and(|v| !v.is_empty())
}

/// Whether an existing stable interpreter copy matches the bundled tree, by the
/// `.pm-pyver` stamp (version+tag+hash — see scripts/fetch-python.mjs). Missing or
/// empty stamps read as stale, so a half-finished copy is always redone.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn stable_copy_current(bundled_stamp: Option<&str>, copied_stamp: Option<&str>) -> bool {
    match (bundled_stamp, copied_stamp) {
        (Some(b), Some(c)) => !b.trim().is_empty() && b.trim() == c.trim(),
        _ => false,
    }
}

/// Recursive copy for the bundled interpreter tree. `fs::copy` preserves the unix
/// exec bits; the standalone tree's relative symlinks (e.g. `bin/python3` →
/// `python3.12`) are recreated as symlinks so they stay valid inside the copy.
/// Deliberately SKIPS `.pm-pyver` at any depth: the caller writes the stamp only
/// after the whole copy succeeded (see [`SidecarPaths::stable_bundled_python`]).
#[cfg(target_os = "linux")]
fn copy_tree(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        if entry.file_name() == ".pm-pyver" {
            continue;
        }
        let ty = entry.file_type()?;
        let to = dest.join(entry.file_name());
        if ty.is_dir() {
            copy_tree(&entry.path(), &to)?;
        } else if ty.is_symlink() {
            let target = std::fs::read_link(entry.path())?;
            match std::fs::remove_file(&to) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            std::os::unix::fs::symlink(target, &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
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

/// Stable DIAGNOSTIC CODES for the sidecar worker confinement (issue #286). A tester or an ordinary
/// user who hits a fall-back can quote the code and it maps to the EXACT failure site here — a precise,
/// greppable id beats a generic message, and flagging the error loudly beats burying it. Ranges:
/// `1xxx` cross-platform/setup, `2xxx` Windows AppContainer, `3xxx` macOS sandbox-exec, `4xxx` Linux
/// Landlock+seccomp. **Never renumber or reuse a shipped code** — they travel in logs and bug reports.
///
/// This is the canonical cross-platform registry, so on any single target some codes are inevitably
/// unused (a Windows build never references the `4xxx` Linux codes, and vice-versa) — `allow(dead_code)`
/// on the whole module rather than a cfg maze per code. A typo'd code still fails to compile at its
/// reference site, so this hides only a genuinely-orphaned entry (which stays valid documentation in
/// ERROR_CODES.md regardless).
#[allow(dead_code)]
pub mod sbx {
    // --- 1xxx cross-platform setup (sidecar.rs) ---
    /// The venv directory has no parent `runtime/` dir, so the staging/allow-set can't be anchored.
    pub const NO_RUNTIME_DIR: &str = "SBX-1101";
    /// The model-cache dir could not be resolved.
    pub const NO_MODELS_DIR: &str = "SBX-1102";
    /// The base interpreter dir could not be read from the venv's `pyvenv.cfg` `home=`.
    pub const NO_BASE_PYTHON: &str = "SBX-1103";
    /// The staging dir (`runtime/sandbox-in`) could not be created.
    pub const STAGING_DIR: &str = "SBX-1104";
    /// Copying an input file into the staging dir failed (the request runs on the original path).
    pub const STAGE_COPY: &str = "SBX-1105";
    /// The confined child process failed to spawn/launch.
    pub const CONFINED_SPAWN: &str = "SBX-1106";

    // --- 2xxx Windows AppContainer (sidecar_sandbox.rs) ---
    /// AppContainer profile creation or SID derivation failed.
    pub const WIN_PROFILE: &str = "SBX-2101";
    /// Granting the container SID full control of the staging dir failed.
    pub const WIN_STAGING_GRANT: &str = "SBX-2102";
    /// Granting the container SID read/execute on the sidecar script dir failed.
    pub const WIN_SCRIPT_GRANT: &str = "SBX-2103";
    /// Granting the container SID read/execute on the model-cache dir failed.
    pub const WIN_MODELS_GRANT: &str = "SBX-2104";
    /// Granting the container SID read/execute on the venv / base-python tree failed.
    pub const WIN_TREE_GRANT: &str = "SBX-2105";

    // --- 4xxx Linux Landlock + seccomp (sidecar_sandbox_linux.rs) ---
    /// Building the Landlock filesystem ruleset failed on a kernel that HAS Landlock (a genuine error,
    /// not merely an old kernel — that path degrades instead).
    pub const LINUX_LANDLOCK: &str = "SBX-4101";
    /// The seccomp network filter could not be built (a Linux CPU architecture PM has no filter for).
    pub const LINUX_SECCOMP: &str = "SBX-4102";
    /// NOT an error — the readout code for a `Degraded` run: Landlock is unavailable (kernel < 5.13 or
    /// its LSM is not active), so the worker's network is blocked but its filesystem is not restricted.
    pub const LINUX_DEGRADED: &str = "SBX-4105";
    /// The confined worker failed its post-spawn self-test (it could not load its libraries under the
    /// sandbox), so it was killed and re-run unconfined rather than break ingest.
    pub const LINUX_PREFLIGHT: &str = "SBX-4106";

    // --- 3xxx macOS sandbox-exec (sidecar_sandbox_macos.rs) ---
    /// `/usr/bin/sandbox-exec` is not present, so the worker can't be confined (it runs unconfined).
    /// Essentially unreachable on a shipping macOS — the binary is deprecated but always installed.
    pub const MAC_SANDBOX_EXEC: &str = "SBX-3101";
    /// The confined worker failed its post-spawn self-test (it could not load its libraries or read its
    /// model cache under the profile), so it was killed and re-run unconfined rather than break ingest.
    pub const MAC_PREFLIGHT: &str = "SBX-3106";
}

/// A confinement setup/launch failure: a stable [`sbx`] code plus human detail. Formats as
/// `"[SBX-####] detail"`, and that exact string surfaces in the log line, the Developer-mode readout
/// ([`SandboxReport::Unconfined`]/[`SandboxReport::Degraded`]), and any user-facing error — so one code
/// pins the failure across all three. Cheap to construct at every failure site (issue #286). The
/// Windows + Linux arms construct it; `allow(dead_code)` only on a build with no worker sandbox.
#[cfg_attr(
    not(any(
        windows,
        target_os = "macos",
        all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    )),
    allow(dead_code)
)]
#[derive(Clone, Debug)]
pub struct SbxError {
    pub code: &'static str,
    pub detail: String,
}

#[cfg_attr(
    not(any(
        windows,
        target_os = "macos",
        all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    )),
    allow(dead_code)
)]
impl SbxError {
    pub fn new(code: &'static str, detail: impl std::fmt::Display) -> Self {
        Self {
            code,
            detail: detail.to_string(),
        }
    }
}

impl std::fmt::Display for SbxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.detail)
    }
}

/// Whether the untrusted-file worker is OS-confined, for the Developer-mode readout (issue #286). The
/// worker spawns lazily, so this reflects the LAST spawn this session — `NotSpawned` until the first
/// convert/embed. Sandboxing fails OPEN by design, so `Unconfined` is a normal state, not an
/// error: it names (with a code) why setup fell back so a hardened-machine surprise is visible without
/// the log. `Degraded` is the middle ground — some axes enforced, some not (e.g. Linux with no Landlock:
/// network blocked but filesystem open) — surfaced honestly rather than mislabeled `Confined`.
#[derive(Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SandboxReport {
    /// This build has no worker confinement for this OS (yet).
    Unsupported,
    /// The worker hasn't been launched yet this session — nothing to report.
    NotSpawned,
    /// The worker is fully confined. Carries the mechanism label (e.g. "Windows AppContainer",
    /// "Landlock (files) + seccomp (network)"), the staging dir, the exact dirs it can read (the
    /// confined filesystem view), and which axes are enforced (`layers`, e.g. `["network","filesystem"]`).
    Confined {
        mechanism: String,
        staging_dir: String,
        granted_dirs: Vec<String>,
        layers: Vec<String>,
    },
    /// PARTIAL confinement: `layers` names the axes that ARE enforced; `code`/`detail` say what's
    /// missing and why (e.g. Landlock unavailable on an old kernel, so filesystem isn't restricted even
    /// though the network is blocked). Never mislabel this `Confined`.
    Degraded {
        layers: Vec<String>,
        code: String,
        detail: String,
    },
    /// Sandbox setup or the confined launch failed, so the worker is running UNCONFINED (fail-open).
    /// `code` is the [`sbx`] id of the failing site; `detail` is the cause.
    Unconfined { code: String, detail: String },
}

/// The kill/wait half of a running sidecar child, abstracted so the confined Windows worker (a raw
/// AppContainer process — see [`crate::sidecar_sandbox`]) and a plain `std::process::Child` (every
/// other platform, and the `--fetch` helper) share one [`Process`]. `kill` closes the child's stdout,
/// which is what unblocks the read watchdog at EOF.
trait ChildControl: Send {
    fn kill(&mut self);
    fn wait(&mut self);
    /// Non-blocking reap probe: `true` once the child is really gone and its process-table slot has
    /// been released, `false` for "still dying, ask again". The bounded counterpart of [`wait`], used
    /// by [`reap_within`] so a `Drop` never blocks on a child that refuses to die.
    ///
    /// An error counts as `true`: a child we can no longer query is one we can never reap, so
    /// re-probing it would only burn the budget.
    ///
    /// [`wait`]: ChildControl::wait
    fn try_reap(&mut self) -> bool;
}

/// The ordinary, unconfined backend: a `std::process::Child` whose stdio has been taken out into the
/// [`Process`]. Retained so the child can still be killed/reaped after its pipes are owned elsewhere.
struct StdChild(Child);
impl ChildControl for StdChild {
    fn kill(&mut self) {
        let _ = self.0.kill();
    }
    fn wait(&mut self) {
        let _ = self.0.wait();
    }
    fn try_reap(&mut self) -> bool {
        // `try_wait` is `waitpid(WNOHANG)` on Unix: `Ok(Some(_))` means the child exited AND was
        // reaped (std caches the status, so a later `wait` is a no-op rather than a second syscall).
        // `Ok(None)` is the only "still running" answer; an `Err` is effectively ECHILD — already
        // reaped by someone else, nothing left to wait for.
        !matches!(self.0.try_wait(), Ok(None))
    }
}

/// How long [`Process::drop`] waits for the child it just killed to actually die.
///
/// Bounded on purpose. A `Drop` that can block forever is a worse defect than the zombie it exists to
/// prevent: this `Drop` runs while the manager's `proc` mutex is held (the respawn path at the IO-error
/// branch of `request`) and on a tokio worker (the wipe's [`SidecarManager::prepare_for_runtime_removal`]),
/// so an unbounded wait on a child stuck in uninterruptible sleep — Linux D state on a stalled network
/// mount, where even SIGKILL is only delivered once the process leaves D — would freeze the sidecar, and
/// on the wipe path the UI, for the rest of the session. A kill is unblockable in every other case, so the
/// child is normally gone in tens of milliseconds: this is a ceiling, not a latency.
const REAP_BUDGET: std::time::Duration = std::time::Duration::from_millis(1_500);

/// How often [`reap_within`] re-probes inside [`REAP_BUDGET`].
const REAP_POLL: std::time::Duration = std::time::Duration::from_millis(5);

/// Reap an already-killed child, giving up after `budget`. `true` if it was reaped in time.
///
/// A free function over the trait, with the timing passed in, so the give-up behaviour is unit-testable
/// against a stub — the reaping syscall itself is not (see the tests).
fn reap_within(
    control: &mut dyn ChildControl,
    budget: std::time::Duration,
    poll: std::time::Duration,
) -> bool {
    let deadline = std::time::Instant::now() + budget;
    loop {
        // Probe first: the kill has usually already landed by the time we get here.
        if control.try_reap() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(poll);
    }
}

/// A running sidecar child plus its stdio handles, over any backend. Boxed trait objects so the same
/// request/read/kill machinery drives both the std child and the confined worker.
struct Process {
    stdin: Box<dyn Write + Send>,
    stdout: Box<dyn BufRead + Send>,
    control: Box<dyn ChildControl>,
}

impl Drop for Process {
    fn drop(&mut self) {
        self.control.kill();
        // Then REAP it. `kill` alone leaves a killed-but-unwaited child as a zombie in the Unix
        // process table for the life of the app, and every respawn route funnels through this one
        // `Drop`: the IO-error branch of `request` (`*guard = None`, the common case — a crashed or
        // OOM-killed worker), the two confined-preflight fall-opens on Linux/macOS (which leak TWO
        // per spawn on a box where confinement is permanently broken, since `fall_open` memoises
        // nothing), and the wipe's `prepare_for_runtime_removal`. The read watchdog is the one path
        // that already did this by hand (`control.kill(); control.wait();`), which is why a
        // timeout-driven respawn never leaked; the second reap here is then a no-op.
        //
        // Windows has no zombies, but it gets the other half: `TerminateProcess` is asynchronous, so
        // waiting is what makes `prepare_for_runtime_removal` actually deliver its stated purpose —
        // the interpreter's file locks are released before `wipe` deletes `runtime/`, instead of
        // racing it and being papered over by `remove_dir_all_retrying`'s 3×200 ms back-off.
        //
        // Bounded, never blocking — see [`REAP_BUDGET`]. Giving up re-leaks the zombie, but only for
        // a child that cannot be killed at all, and a hung `Drop` under the `proc` mutex would take
        // the whole sidecar down with it.
        if !reap_within(self.control.as_mut(), REAP_BUDGET, REAP_POLL) {
            eprintln!(
                "sidecar: the killed worker had not exited after {REAP_BUDGET:?}; \
                 continuing without reaping it"
            );
        }
    }
}

// The confining `Sandbox` is a different type per OS (Windows AppContainer / Linux Landlock+seccomp /
// macOS sandbox-exec) but each exposes the same `staging_dir()` the shared input-staging path needs;
// alias it so the manager field and `maybe_stage_input` are written once.
#[cfg(windows)]
use crate::sidecar_sandbox::Sandbox;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
use crate::sidecar_sandbox_linux::Sandbox;
#[cfg(target_os = "macos")]
use crate::sidecar_sandbox_macos::Sandbox;

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
    /// The OS sandbox that confines the worker (Windows AppContainer / Linux Landlock+seccomp), set on
    /// first successful confined spawn and reused for per-request input staging (issue #286). `None` =
    /// unconfined (setup failed, or a platform with no worker sandbox). Held here so `request` can stage
    /// the file a path-bearing call parses.
    #[cfg(any(
        windows,
        target_os = "macos",
        all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    ))]
    sandbox: Mutex<Option<Sandbox>>,
    /// The last worker-spawn confinement outcome, for the Developer-mode readout (issue #286). Distinct
    /// from `sandbox` (which holds the live handle only while confined): this also records WHY a spawn
    /// fell open, and survives as `NotSpawned` before the first spawn.
    #[cfg(any(
        windows,
        target_os = "macos",
        all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    ))]
    sandbox_report: Mutex<SandboxReport>,
}

impl SidecarManager {
    pub fn new(paths: SidecarPaths) -> Self {
        let manager = Self {
            paths,
            proc: Mutex::new(None),
            status: Mutex::new(SidecarStatus::NotInstalled),
            install: Mutex::new(()),
            req_seq: AtomicU64::new(0),
            #[cfg(any(
                windows,
                target_os = "macos",
                all(
                    target_os = "linux",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                )
            ))]
            sandbox: Mutex::new(None),
            #[cfg(any(
                windows,
                target_os = "macos",
                all(
                    target_os = "linux",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                )
            ))]
            sandbox_report: Mutex::new(SandboxReport::NotSpawned),
        };
        // Boot status asks the SAME question every other readiness check asks — is the marker
        // CURRENT — rather than merely whether the file exists. The marker stamps the
        // lock hash, so bare existence goes on reporting Ready after an app update
        // changes the lock, right up until the next `ensure_installed`. In that window
        // chat grounding trusts `status()` and runs the NEW sidecar script against the OLD pinned
        // deps. Costs one extra `--version` probe at boot (the hash read is cheap), and a false
        // NotInstalled only means provision() re-checks — which it was going to do anyway.
        if manager.is_ready_marker_current().unwrap_or(false) {
            *manager.status.lock().unwrap() = SidecarStatus::Ready;
        }
        manager
    }

    pub fn status(&self) -> SidecarStatus {
        self.status.lock().unwrap().clone()
    }

    /// The worker's confinement state, for the Developer-mode readout (issue #286). Reflects the LAST
    /// spawn this session (`NotSpawned` before the first). `Unsupported` on a platform (or Linux CPU
    /// arch) with no worker confinement yet — the readout tells the maintainer that plainly instead of
    /// implying a hole.
    #[cfg(any(
        windows,
        target_os = "macos",
        all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    ))]
    pub fn sandbox_report(&self) -> SandboxReport {
        self.sandbox_report.lock().unwrap().clone()
    }
    #[cfg(not(any(
        windows,
        target_os = "macos",
        all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    )))]
    pub fn sandbox_report(&self) -> SandboxReport {
        SandboxReport::Unsupported
    }

    /// Ask the running worker to attempt one outbound socket and report whether the OS refused it — the
    /// Developer-mode network-block probe (issue #286). Spawns the worker lazily like any request, so on
    /// Windows it exercises the confined worker; the handler is unlocked only by `PM_SIDECAR_DEV`, which
    /// a debug build alone sets (see [`Self::worker_env`]), so a release worker refuses the method.
    /// Returns the worker's raw `{ blocked, detail, errno }` result.
    pub fn net_selftest(&self) -> Result<Value> {
        self.request("net_selftest", json!({}))
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
        self.stop_worker();
        self.set_status(SidecarStatus::NotInstalled);
    }

    /// Stop the running child, if any; the next request spawns a fresh one. Taking the `Process`
    /// out of the mutex drops it, and its `Drop` kills the child, releasing every file it holds
    /// open. Waits for a request in flight, since each holds `proc` for its whole run.
    ///
    /// Before pip removes packages too, not only before `runtime/` goes: a worker that has read a
    /// photo has OpenCV's native libraries loaded, and Windows refuses to delete a DLL a running
    /// process has mapped, which would leave the package half removed.
    fn stop_worker(&self) {
        if let Ok(mut proc) = self.proc.lock() {
            proc.take(); // drop → Process::drop kills the child, freeing the interpreter's locks
        }
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
        // Nothing gets provisioned back into a vault the user has just erased. This is the one
        // recreator the data-dir latch cannot catch on its own: `paths::data_dir` stops CREATING
        // after a purge, but provisioning builds `runtime/venv` with a `create_dir_all` on a path
        // BELOW it, which re-makes every parent — so a connector poll landing after the erase would
        // rebuild a few hundred MB of Python inside the folder the user was told was gone. Unlike
        // the empty directories, that is real content coming back.
        //
        // Guarded here rather than at the call sites: `run_cloud_pass` and `run_local_sync` both
        // call this BEFORE their `state.conn()` gate, so the closed store does not stop them, and a
        // future caller would inherit the same trap. One check, in the one place that matters.
        if crate::paths::data_dir_is_purged() {
            return Err(Error::Other(
                "PM's data has been removed on this device — restart PM to set it up again".into(),
            ));
        }
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
        // The LOCK, not requirements.txt: the lock is what pip installs. requirements.txt is the
        // human-edited top-level list `just lock-regen` generates the lock from, and is not read
        // at runtime at all.
        let lock = self.paths.lock();
        if !lock.exists() {
            return Err(ProvisionError {
                kind: SidecarErrorKind::RequirementsMissing,
                source: Error::Other(format!(
                    "sidecar dependency lock not found at {} (is the sidecar/ folder present?)",
                    lock.display()
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
        // was first built against macOS's system 3.9, or on a 3.10 from before
        // the floor rose to 3.11). This is what removes the manual "delete the
        // venv and retry".
        let venv_python_exists = self.paths.venv_python().exists();
        let detected = venv_python_exists
            .then(|| detect_python_version(&self.paths.venv_python()))
            .flatten();
        // What the user has PAID FOR, noted before the teardown can erase it. The optional markers
        // live inside the venv dir, so `remove_dir_all` took them with it — silently uninstalling
        // OCR and t-SNE. Nothing told the user: photos then ingested EXIF-only (no OCR text, and
        // permanently for those documents unless re-ingested) and the memory map quietly fell back
        // to PCA. A rebuild is triggered by a torn install or an outdated interpreter — neither of
        // which is a request to remove components.
        //
        // The in-place update loses them too, more quietly. Every optional lock is resolved against
        // the base lock and stamps its hash, so a base lock that moved (a fastembed bump) makes each
        // installed component's marker stale: `optional_ready` turned false, photos went EXIF-only,
        // and Settings → Storage — which then saw only the packages on disk — went on showing OCR
        // as installed; only dropping a photo by hand offered the install. A marker that is there
        // at all means the user installed the component, so it is put back whenever the venv no
        // longer satisfies it.
        let rebuild = should_rebuild_venv(
            venv_python_exists,
            self.paths.ready_marker().exists(),
            detected,
        );
        let reinstall: Vec<&OptionalComponent> = ALL_OPTIONAL_COMPONENTS
            .iter()
            .copied()
            .filter(|c| {
                should_reinstall_optional(
                    rebuild,
                    (c.marker)(&self.paths).exists(),
                    self.optional_ready(c),
                )
            })
            .collect();
        if rebuild {
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
        // `--no-cache-dir`: pip's wheel cache lives at `~/.cache/pip` (or `~/Library/Caches/pip`),
        // OUTSIDE everything PM owns, and nothing has ever removed it — hundreds of MB of wheels
        // surviving a full "remove PM completely". Relocating it under `runtime/` would also work,
        // but the cache only earns its keep on a venv REBUILD, which is rare, so not writing it at
        // all is both smaller and simpler than tracking it. First-run download volume is unchanged.
        // `--require-hashes`: every entry in the lock carries the SHA-256 of every artifact PyPI
        // publishes for that version, and pip refuses anything whose digest does not match. Without
        // it the six top-level pins were the only fixed points and every transitive dependency was
        // whatever PyPI served that day — in the one process that opens untrusted PDFs, Office
        // documents and images. The flag is also fail-closed in a second, useful way: it makes pip
        // reject a lock that is not FULLY pinned, so a partially-regenerated file cannot install.
        // `--no-deps`: an override in sidecar/requirements-override.txt resolves the lock past a
        // dependency's declared bound (fastembed 0.9.0's `pillow<13` until 0.9.1 lifted it), and
        // pip's own resolver would then refuse the lock. The lock needs no resolving here anyway: uv resolved every dependency into it, and
        // `just requirements-lock` fails one that has drifted from its inputs. Every line is still
        // pinned and hash-checked; what pip no longer does is notice a dependency MISSING from the
        // lock, which only a hand-edited lock could produce. The optional installs and CI's
        // installs of the lock (pr.yml, release.yml) take the same flag.
        pip.args(["-m", "pip", "install"])
            .args(LOCK_INSTALL_FLAGS)
            .arg("-r")
            .arg(&lock);
        clean_python_env(&mut pip);
        no_window(&mut pip);
        run_command(&mut pip, "pip install requirements").map_err(|e| ProvisionError {
            kind: classify_pip_failure(&e.to_string()),
            source: e,
        })?;

        // Put back the optional components the rebuild removed or the moved locks left stale (see
        // `reinstall` above). Best-effort and deliberately NOT fatal: the base venv is what the app
        // needs to function, and failing the whole provision over an optional extra would turn "OCR
        // is missing" into "nothing works". A failure leaves the component's marker absent or stale,
        // never stamped over packages that aren't there, so the feature is honestly off and
        // Settings → Storage offers Install: after a rebuild the packages are gone, and after an
        // in-place update the stale copy still on disk shows as needing installing again
        // (`components::ocr_row`). Not retried here: the next launch has a current base marker,
        // and retrying a ~200 MB download on every launch would be worse on a metered link.
        //
        // `install_optional_locked`, not `install_optional`: we already hold `self.install` (see
        // `ensure_installed_with_progress`) and the base venv is up two statements ago.
        for component in reinstall {
            if let Err(e) = self.install_optional_locked(component, |_| {}) {
                eprintln!(
                    "sidecar: provisioned the venv but could not reinstall an optional component ({e}); \
                     reinstall it from Settings -> Storage"
                );
            }
        }
        // What voice input left (see `retire_voice`), after the base install, not before: a failure
        // part-way leaves the venv as usable as it was. Best-effort like the reinstalls above, and
        // under the install lock this already holds. A rebuilt venv took the engine with it; the
        // Whisper model and the rest outside the venv go here all the same.
        self.retire_voice_locked();

        // Stamp the marker with the lock's hash so we can skip next time.
        let hash = self.lock_hash().map_err(unknown)?;
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
                // Linux: when running out of an AppImage, the resource dir is a
                // transient mount — hand the venv a stable copy instead, or the
                // venv dies on the second launch (see stable_bundled_python).
                #[cfg(target_os = "linux")]
                match self.paths.stable_bundled_python(&p) {
                    Ok(Some(stable)) => return Ok(stable),
                    Ok(None) => {}
                    Err(e) => {
                        return Err(ProvisionError {
                            kind: SidecarErrorKind::Unknown,
                            source: Error::Other(format!(
                                "PM couldn't copy its bundled Python out of the AppImage \
                                 into its data folder (needed so the document engine \
                                 survives relaunches): {e}"
                            )),
                        });
                    }
                }
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
                             usable version ({}.{}+). This is a problem with PM's build, not your \
                             computer.",
                            p.display(),
                            MIN_PYTHON.0,
                            MIN_PYTHON.1
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

    /// Hash of the LOCK, which is what actually got installed. Keyed on requirements.txt before the
    /// lock existed — which would now under-report: regenerating the lock can move a transitive
    /// dependency without touching requirements.txt at all, and the venv would go on reporting
    /// Ready with the old resolution in it.
    fn lock_hash(&self) -> Result<String> {
        let bytes = std::fs::read(self.paths.lock())?;
        Ok(crate::ingest::hex_digest(&bytes))
    }

    fn is_ready_marker_current(&self) -> Result<bool> {
        let marker = self.paths.ready_marker();
        if !marker.exists() || !self.paths.venv_python().exists() {
            return Ok(false);
        }
        let stamped = std::fs::read_to_string(&marker).unwrap_or_default();
        if stamped.trim() != self.lock_hash()? {
            return Ok(false);
        }
        // The lock can be unchanged yet the venv's interpreter be too old
        // (built against macOS system 3.9, then the user installed 3.11+). Don't
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
        guard_input_size(path)?;
        let result = self.request("convert", json!({ "path": path.to_string_lossy() }))?;
        let markdown = result["markdown"].as_str().unwrap_or_default().to_string();
        let title = result["title"].as_str().unwrap_or_default().to_string();
        Ok((markdown, title))
    }

    /// What a document says about ITSELF — never what the filesystem or the OS account says (#709).
    ///
    /// Returns a value rather than a `Result` on purpose: a property is a nicety, and the type is
    /// how "this can never be the reason a file fails to land" is enforced rather than remembered.
    /// A file the sidecar can't read, an engine that isn't there, a format with no property block —
    /// all of them are the same answer here, and all of them render "Unknown".
    ///
    /// Skips the round trip entirely for the formats that state nothing, so the common case (a
    /// dropped .md, a folder of text notes) costs nothing at all.
    pub fn file_properties(&self, path: &Path) -> FileProperties {
        if !carries_document_properties(path) {
            return FileProperties::default();
        }
        match self.request("file_properties", json!({ "path": path.to_string_lossy() })) {
            Ok(reply) => file_properties_from_reply(&reply),
            Err(e) => {
                eprintln!(
                    "sidecar: reading {}'s properties failed ({e})",
                    path.display()
                );
                FileProperties::default()
            }
        }
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

    /// Analyse one image for photo ingestion: EXIF capture metadata (always) + OCR text (only when
    /// `run_ocr`). The caller sets `run_ocr` from [`optional_ocr_ready`](Self::optional_ocr_ready),
    /// so a user who declined the optional OCR component still gets dimensions + EXIF for the photo's
    /// metadata chunk. The first OCR call reports a cold cache, [`Self::request`] runs the
    /// network-allowed `--fetch` helper to download rapidocr's small models, and the retry is slow;
    /// later calls are fast and fully local. Blocking, like every sidecar call. EXIF/OCR output is
    /// untrusted data — scored/indexed, never executed.
    ///
    /// `ocr_ran` is the reply's only signal that OCR was asked for and did not happen. Read it —
    /// `ocr_text` alone cannot tell "the component broke" from "this image holds no text".
    pub fn analyze_image(&self, path: &Path, run_ocr: bool) -> Result<ImageAnalysis> {
        guard_input_size(path)?;
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

    /// Parse a spreadsheet (`.xlsx`/`.csv`) into per-sheet structure for the dedicated ingest
    /// path, bypassing MarkItDown. Values only — no formula evaluation, no styling. Each sheet reports
    /// its headers, TRUE row count, per-column inferred types, an optional date range, and up to the
    /// sidecar's row cap of stringified rows (flagged `truncated` when it had more). `ext` selects the
    /// reader (openpyxl / stdlib csv). Blocking, like every sidecar call; cell text is untrusted
    /// data — indexed, never executed.
    pub fn analyze_spreadsheet(
        &self,
        path: &Path,
        ext: &str,
    ) -> Result<Vec<crate::spreadsheets::SheetData>> {
        guard_input_size(path)?;
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
        self.optional_ready(&OPTIONAL_TSNE_COMPONENT)
    }

    /// Install the OPTIONAL t-SNE reducer (openTSNE) into the managed venv on demand — the shared
    /// [`Self::install_optional`] flow with the t-SNE marker + pin. openTSNE pulls scikit-learn +
    /// scipy (numpy is already present via fastembed). The download has no file-count, so the UI
    /// renders `on_progress` as a percentage.
    pub fn install_optional_tsne(&self, on_progress: impl FnMut(f32)) -> Result<()> {
        self.install_optional(&OPTIONAL_TSNE_COMPONENT, on_progress)
    }

    /// Remove the OPTIONAL t-SNE component again (the "delete" action) — the shared
    /// [`Self::uninstall_optional`] flow. Only openTSNE is removed: its heavier transitive deps
    /// (scipy / scikit-learn) are left in place; a later re-install is then quick. Once the marker is
    /// gone the map falls back to PCA.
    pub fn uninstall_optional_tsne(&self) -> Result<()> {
        self.uninstall_optional(&OPTIONAL_TSNE_COMPONENT)
    }

    /// Whether the OPTIONAL photo-OCR component is installed in this venv at the pinned versions.
    /// Cheap (a marker read) so the ingest path can check it per photo to decide whether to request
    /// OCR, and so Settings can show the install/remove state.
    pub fn optional_ocr_ready(&self) -> bool {
        self.optional_ready(&OPTIONAL_OCR_COMPONENT)
    }

    /// Install the OPTIONAL photo-OCR component (rapidocr + pi-heif) into the managed venv on
    /// demand — the shared [`Self::install_optional`] flow with the OCR marker + pins.
    pub fn install_optional_ocr(&self, on_progress: impl FnMut(f32)) -> Result<()> {
        self.install_optional(&OPTIONAL_OCR_COMPONENT, on_progress)
    }

    /// Remove the OPTIONAL photo-OCR component (the "delete" action) — the shared
    /// [`Self::uninstall_optional`] flow. Only rapidocr + pi-heif are removed: the heavier
    /// transitive image deps (opencv / shapely / pyclipper) are LEFT in place here; the Storage
    /// manager (components.rs) does the guarded cascade that reclaims them. Once the marker is gone,
    /// future photos ingest EXIF-only.
    pub fn uninstall_optional_ocr(&self) -> Result<()> {
        self.uninstall_optional(&OPTIONAL_OCR_COMPONENT)
    }

    /// Clear away what voice input left on this computer, now that PM has none: the speech engine in
    /// the venv ([`VOICE_ENGINE_PACKAGES`]), its two markers, the Whisper model's folders
    /// ([`WHISPER_REPO_DIR`]) and any recording a crash left in `runtime/tmp`. It acts on every
    /// computer alike: there is no voice switch left to ask, and nothing here is any use without
    /// voice input.
    ///
    /// Run once per launch, on a background thread (it can run pip), and by `provision` after the
    /// base install. A quick look on disk comes first, so once everything is gone a launch costs a
    /// few directory reads and never waits on the install lock. With something to remove, it waits
    /// for the lock, so pip never runs beside a provision or an optional install. Every step is
    /// best-effort and logged, and nothing is recorded as done: what a failure leaves is still on
    /// disk, so the next launch's look finds it and tries again. A leftover PM may not delete at
    /// all (a folder without write permission) is tried again on every launch, at the cost of a
    /// few directory reads, and of pip's four runs only while it still has a RECORD pip could
    /// remove it by.
    pub fn retire_voice(&self) {
        if !self.voice_leftovers_on_disk() {
            return;
        }
        let _install = self.install.lock().unwrap();
        self.retire_voice_locked();
    }

    /// Whether anything [`Self::retire_voice`] removes is still here.
    fn voice_leftovers_on_disk(&self) -> bool {
        // A dangling link counts: `symlink_metadata` looks at the link, not at what it points to.
        let there = |p: &PathBuf| std::fs::symlink_metadata(p).is_ok();
        let venv = &self.paths.venv_dir;
        venv_site_has(venv, is_voice_engine_leftover)
            // The markers only in a real venv: through a link they are somewhere PM doesn't own.
            || (real_dir(venv) && self.paths.voice_markers().iter().any(there))
            || self
                .paths
                .models_dir()
                .is_some_and(|models| whisper_paths(&models).iter().any(there))
            || self
                .paths
                .voice_tmp_dir()
                .is_some_and(|tmp| !voice_clips(&tmp).is_empty() || is_empty_dir(&tmp))
    }

    /// The clean-up itself, for a caller that already holds `self.install` (`provision`, and
    /// [`Self::retire_voice`] once it has taken it).
    fn retire_voice_locked(&self) {
        let venv = &self.paths.venv_dir;
        // pip only where it can help: an engine piece is in the venv (looked for first, as asking
        // the interpreter its version starts it), and one of the three has a dist-info with its
        // RECORD in the site-packages of the venv's own interpreter. setuptools alone is no sign
        // of an engine (a Python 3.11 venv has its own), and neither is an engine left in a stale
        // `lib/python3.X` beside the one in use, so neither ever has pip take the venv's copy.
        let pip_helps = voice_engine_on_disk(venv)
            && self
                .pip_site_dir()
                .is_some_and(|site| pip_can_uninstall_voice_engine(&site));
        if pip_helps {
            // Stopped first, as before every removal (see `stop_worker`). This version's worker
            // never imports the engine, so it is only a guard, and a cheap one: it happens once,
            // and the next request respawns the worker.
            self.stop_worker();
            self.uninstall_voice_engine();
        }
        // Then by hand, whether pip ran or not: whatever is left of the three packages and pip's
        // stashes of them, in every site-packages, so the quick look finds nothing next time. With
        // nothing left it is one directory read.
        remove_voice_engine_by_hand(venv);
        // The markers only in a real venv: through a link they are somewhere PM doesn't own.
        if real_dir(venv) {
            for marker in self.paths.voice_markers() {
                remove_voice_leftover(&marker);
            }
        }
        if let Some(models) = self.paths.models_dir() {
            for path in whisper_paths(&models) {
                remove_voice_leftover(&path);
            }
        }
        if let Some(tmp) = self.paths.voice_tmp_dir() {
            // Only a real folder: a link here leads out of PM's data dir, and on Windows
            // `remove_dir` would take the link itself.
            if real_dir(&tmp) {
                for clip in voice_clips(&tmp) {
                    remove_voice_leftover(&clip);
                }
                // The folder too, once nothing is left in it. `remove_dir` refuses one that isn't
                // empty, which leaves anything else someone put there alone.
                let _ = std::fs::remove_dir(&tmp);
            }
        }
    }

    /// The site-packages the venv's own interpreter, and so its pip, installs into and uninstalls
    /// from: `Lib/site-packages` on Windows, and elsewhere `lib/python3.<minor>/site-packages` for
    /// the minor the interpreter reports. None without an interpreter, or when that folder is not
    /// one of the venv's real ones ([`venv_site_dirs`]).
    fn pip_site_dir(&self) -> Option<PathBuf> {
        let venv = &self.paths.venv_dir;
        let py = self.paths.venv_python();
        if !py.exists() {
            return None;
        }
        let own = if cfg!(windows) {
            venv.join("Lib").join("site-packages")
        } else {
            let (major, minor) = detect_python_version(&py)?;
            venv.join("lib")
                .join(format!("python{major}.{minor}"))
                .join("site-packages")
        };
        venv_site_dirs(venv).contains(&own).then_some(own)
    }

    /// `pip uninstall` the speech engine one package at a time ([`VOICE_ENGINE_PACKAGES`]), logging a
    /// failure and going on to the next. pip skips a package it doesn't find installed and refuses
    /// one whose dist-info has no RECORD; whatever it leaves, the by-hand pass after this takes.
    fn uninstall_voice_engine(&self) {
        let py = self.paths.venv_python();
        if !py.exists() {
            // No interpreter, so no pip: the by-hand pass after this does it all.
            return;
        }
        for package in VOICE_ENGINE_PACKAGES {
            let mut pip = Command::new(&py);
            pip.args([
                "-m",
                "pip",
                "uninstall",
                "-y",
                "--disable-pip-version-check",
                package,
            ]);
            clean_python_env(&mut pip);
            no_window(&mut pip);
            if let Err(e) = run_command(&mut pip, &format!("pip uninstall {package}")) {
                eprintln!("sidecar: couldn't uninstall {package}, which voice input left ({e})");
            }
        }
    }

    /// Shared readiness check for an [`OptionalComponent`]: the venv exists and the component's
    /// marker holds exactly the pinned stamp (so a pin bump reads as not-installed and re-installs).
    /// What a current marker must hold, and what a fresh install stamps: the component's pins AND
    /// the hash of the lock they were installed from.
    ///
    /// The pins alone were enough while the pins WERE the install. Now the lock is, and
    /// regenerating it can move a component's entire transitive tree — opencv, shapely and
    /// pyclipper for OCR, scikit-learn and scipy for t-SNE — without a single pin changing. Keyed
    /// on pins only, an installed component would go on reporting Ready with the old, unhashed
    /// packages still in it: exactly the gap the base venv's `.ready` marker had before it moved
    /// onto the lock's hash.
    fn optional_stamp(&self, component: &OptionalComponent) -> Result<String> {
        let bytes = std::fs::read(self.paths.optional_lock(component))?;
        Ok(format!(
            "{};lock={}",
            component.pins.join(";"),
            crate::ingest::hex_digest(&bytes)
        ))
    }

    fn optional_ready(&self, component: &OptionalComponent) -> bool {
        // No readable lock ⇒ we cannot say what is installed. Report not-ready rather than vouch
        // for a component we can't verify; Settings → Storage can reinstall it.
        let Ok(expected) = self.optional_stamp(component) else {
            return false;
        };
        self.paths.venv_python().exists()
            && std::fs::read_to_string((component.marker)(&self.paths))
                .map(|s| s.trim() == expected)
                .unwrap_or(false)
    }

    /// Shared install flow for an [`OptionalComponent`]. The base venv must exist first, so this
    /// provisions it if needed, then `pip install`s the pins and stamps the marker. Blocking and
    /// slow (a download); serialised by the install lock. Idempotent — a no-op once the marker is
    /// current.
    ///
    /// `on_progress` is called with a monotonic `0.0..=1.0` fraction as the install advances
    /// (derived from pip's `Collecting/Downloading/Installing` markers — see
    /// [`pip_phase_fraction`]), so the UI can show a real progress bar instead of an indeterminate
    /// spinner.
    fn install_optional(
        &self,
        component: &OptionalComponent,
        mut on_progress: impl FnMut(f32),
    ) -> Result<()> {
        on_progress(0.03);
        // The optional pins go into the base venv, which must exist with its requirements first.
        self.ensure_installed()?;
        on_progress(0.10);

        let _install = self.install.lock().unwrap();
        if self.optional_ready(component) {
            on_progress(1.0);
            return Ok(());
        }
        self.install_optional_locked(component, on_progress)
    }

    /// The pip half of an optional install: pins into an EXISTING venv, then stamp the marker.
    ///
    /// Split out because `provision` has to re-install components after a venv rebuild and CANNOT
    /// call [`install_optional`]: that would re-enter `ensure_installed` and re-take
    /// `self.install` — which `ensure_installed_with_progress` already holds across the whole
    /// provision. `Mutex` is not reentrant, so it would deadlock the installer against itself.
    ///
    /// So this takes neither the lock nor the base-venv guarantee: the caller owns both.
    fn install_optional_locked(
        &self,
        component: &OptionalComponent,
        mut on_progress: impl FnMut(f32),
    ) -> Result<()> {
        let py = self.paths.venv_python();
        // `--progress-bar off` so pip emits clean newline-terminated phase lines (no carriage-return
        // byte bar) we can parse; the side-thread stderr drain in run_pip_streaming avoids a deadlock.
        let mut downloads = 0u32;
        let mut last = 0.10f32;
        // `--no-cache-dir` for the same reason as the base install: pip's cache is the largest thing
        // PM leaves outside its own folders, and nothing ever collects it.
        //
        // `--require-hashes -r <the component's lock>` rather than the bare pins. Photo OCR is the
        // case that makes this matter: it is the code that parses an untrusted image, and it used to
        // arrive with its whole transitive tree (opencv, shapely, pyclipper) free-resolved at install
        // time. The lock is also resolved against the BASE lock, so an on-demand install can no
        // longer move a package the running venv depends on — pip was previously free to satisfy
        // rapidocr by swapping the numpy that fastembed and onnxruntime are using. `--no-deps` as
        // in the base install (see `provision`): the component's lock is just as complete, so
        // every lock installs by one rule — pip resolves nothing on the user's machine.
        let lock = self.paths.optional_lock(component);
        if !lock.exists() {
            return Err(Error::Other(format!(
                "optional component lock not found at {} (is the sidecar/ folder present?)",
                lock.display()
            )));
        }
        if !component.superseded.is_empty() {
            // Not `pip_uninstall`: that takes `self.install`, which the caller already holds (see
            // above). Not best-effort either: installing beside a stale copy is the failure this
            // step exists to prevent, so a failed removal stops the install. `-y` skips a package
            // that isn't installed, which is every fresh install.
            let mut pip = Command::new(&py);
            pip.args([
                "-m",
                "pip",
                "uninstall",
                "-y",
                "--disable-pip-version-check",
            ]);
            pip.args(component.superseded);
            clean_python_env(&mut pip);
            no_window(&mut pip);
            run_command(
                &mut pip,
                &format!("pip uninstall {}", component.superseded.join(" ")),
            )?;
        }
        let lock_arg = lock.to_string_lossy().into_owned();
        let mut args: Vec<&str> = vec!["install"];
        args.extend(LOCK_INSTALL_FLAGS);
        args.extend(["--progress-bar", "off", "-r", &lock_arg]);
        run_pip_streaming(&py, &args, |line| {
            if let Some(f) = pip_phase_fraction(line, &mut downloads) {
                if f > last {
                    last = f;
                    on_progress(f);
                }
            }
        })?;

        std::fs::write(
            (component.marker)(&self.paths),
            self.optional_stamp(component)?,
        )?;
        on_progress(1.0);
        Ok(())
    }

    /// Shared removal flow for an [`OptionalComponent`]. Drops the marker first — that alone
    /// disables the feature (`optional_ready` then reports false) — then `pip uninstall`s the
    /// component's packages (its `uninstall` list) to reclaim space. Idempotent.
    ///
    /// The marker goes first, so the feature is off whatever pip then does. pip's own failure is
    /// returned, not swallowed: pip stops at the first package it can't remove (a native library
    /// something else holds open, on Windows), and the person who asked for the space back has to
    /// be told some of it is still taken. What is left stays on disk, measured on its Storage row
    /// with its own Remove.
    fn uninstall_optional(&self, component: &OptionalComponent) -> Result<()> {
        let _install = self.install.lock().unwrap();
        let _ = std::fs::remove_file((component.marker)(&self.paths));
        let py = self.paths.venv_python();
        if !py.exists() {
            return Ok(());
        }
        self.stop_worker();
        let mut pip = Command::new(&py);
        pip.args([
            "-m",
            "pip",
            "uninstall",
            "-y",
            "--disable-pip-version-check",
        ]);
        pip.args(component.uninstall);
        clean_python_env(&mut pip);
        no_window(&mut pip);
        run_command(
            &mut pip,
            &format!("pip uninstall {}", component.uninstall.join(" ")),
        )
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
        // OpenCV, shapely and scipy are native too (see `stop_worker`).
        self.stop_worker();
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

        // Built once so `params` (which can be a large embed batch) is never cloned per attempt;
        // only the id is stamped fresh on each send. There are at most two attempts: the second
        // happens only after a `model_not_cached` miss has been satisfied by `fetch_model`.
        let mut req = json!({ "id": 0u64, "method": method, "params": params });
        let timeout = request_timeout(method);
        let mut fetched = false;
        // Kept alive for the WHOLE request (including the fetch-and-retry) so the staged copy isn't
        // deleted mid-flight. Staged exactly once, and only AFTER the worker is spawned below — because
        // the request that triggers the first confined spawn only sets `self.sandbox` inside `spawn`,
        // so staging before it would hand the confined worker the un-granted original path (issue #286).
        #[cfg(any(
            windows,
            target_os = "macos",
            all(
                target_os = "linux",
                any(target_arch = "x86_64", target_arch = "aarch64")
            )
        ))]
        let mut staged: Option<crate::sidecar_stage::StagedInput> = None;

        loop {
            if guard.is_none() {
                *guard = Some(self.spawn()?);
            }
            // Stage the file this call parses into the sandbox-readable dir and point the request at
            // the staged copy (no-op for non-path methods and the unconfined worker). After the spawn
            // above so `self.sandbox` reflects confinement; once, so a retry reuses the same copy.
            #[cfg(any(
                windows,
                target_os = "macos",
                all(
                    target_os = "linux",
                    any(target_arch = "x86_64", target_arch = "aarch64")
                )
            ))]
            if staged.is_none() {
                staged = self.maybe_stage_input(&mut req);
            }
            let id = self.req_seq.fetch_add(1, Ordering::Relaxed) + 1;
            req["id"] = json!(id);
            let proc = guard.as_mut().unwrap();

            let line = serde_json::to_string(&req)
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
            let send = (|| -> std::io::Result<Value> {
                proc.stdin.write_all(line.as_bytes())?;
                proc.stdin.write_all(b"\n")?;
                proc.stdin.flush()?;
                read_reply_with_timeout(proc, id, timeout)
            })();

            match send {
                Ok(value) => {
                    if value["ok"].as_bool() == Some(true) {
                        return Ok(value["result"].clone());
                    }
                    // The offline worker doesn't have this model cached yet (issue #286). Download
                    // it with the network-allowed fetcher and retry ONCE against the SAME live
                    // worker (the reply was well-formed, so the child is healthy — do NOT respawn).
                    // Holding the proc lock across the download matches the pre-existing behaviour
                    // of a first-use inline download, which blocked the serialized sidecar for
                    // exactly as long.
                    if !fetched && is_model_not_cached(&value) {
                        fetched = true;
                        self.fetch_model(method, &fetch_params(method, &req["params"]))?;
                        continue;
                    }
                    let msg = value["error"].as_str().unwrap_or("unknown sidecar error");
                    // A reply that classified ITSELF keeps that classification in the message.
                    // The Python side reports `str(exc)`, which carries no type information, so
                    // this marker is the only thing that lets a caller tell "the engine refused
                    // this file" from "the engine is broken" — a distinction
                    // `cloud_sync::is_permanently_unindexable` has to get right, because one is a
                    // skip and the other must hold the account's delta cursor. Untagged errors
                    // keep their existing wording byte-for-byte.
                    return Err(Error::Other(match value["error_kind"].as_str() {
                        Some(kind) => format!("sidecar {method} failed [{kind}]: {msg}"),
                        None => format!("sidecar {method} failed: {msg}"),
                    }));
                }
                Err(e) => {
                    *guard = None; // force a respawn next time
                    return Err(Error::Other(format!("sidecar {method} IO error: {e}")));
                }
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

        let script = self.paths.script();
        let envs = self.worker_env();

        // Windows: confine the worker in a no-network AppContainer (issue #286). Best-effort — if the
        // sandbox can't be set up we fall through to the unconfined child rather than break ingest
        // (this is defence in depth ON TOP OF the offline worker + at-rest encryption; the failure is
        // logged). The `--fetch` helper is never confined — it needs the network.
        #[cfg(windows)]
        if let Some(proc) = self.try_spawn_confined(&py, &script, &envs)? {
            return Ok(proc);
        }

        // Linux: the same fall-open contract, via Landlock (filesystem) + seccomp (network) self-imposed
        // in the child's pre_exec. The confined worker is an ordinary child, and it must survive a
        // post-spawn self-test (it can load its libraries under the sandbox) or we fall open here too.
        #[cfg(all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ))]
        if let Some(proc) = self.try_spawn_confined_linux(&py, &script, &envs)? {
            return Ok(proc);
        }

        // macOS: the same fall-open contract, via `sandbox-exec` applying a `(deny default)` Seatbelt
        // profile (no network, restricted filesystem). Like Linux, the confined worker is an ordinary
        // child and must survive a post-spawn self-test or we fall open here too.
        #[cfg(target_os = "macos")]
        if let Some(proc) = self.try_spawn_confined_macos(&py, &script, &envs)? {
            return Ok(proc);
        }

        // The ordinary, unconfined child (every non-confined platform, and the confined fall-open). The
        // offline posture comes from the SAME `worker_env` list the confined path uses, so the two can
        // never drift — a stray offline flag missing from one would be a silent hole.
        let mut command = Command::new(&py);
        command
            .arg(&script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        for (k, v) in &envs {
            command.env(k, v);
        }
        for k in PYTHON_ENV_REMOVES {
            command.env_remove(k);
        }
        no_window(&mut command);

        let mut child = command
            .spawn()
            .map_err(|e| Error::Other(format!("could not start the document sidecar: {e}")))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Ok(Process {
            stdin: Box::new(stdin),
            stdout: Box::new(stdout),
            control: Box::new(StdChild(child)),
        })
    }

    /// The environment the WORKER runs with, as a single list so the confined and unconfined spawn
    /// paths can never disagree (issue #286). Carries the authoritative offline posture
    /// (`PM_SIDECAR_OFFLINE` + the HF offline flags — see the long note history), the quiet-hub flags,
    /// the shared model-cache root, and `PYTHONUTF8`. Keys in [`PYTHON_ENV_REMOVES`] are dropped from
    /// the inherited environment on top of this.
    fn worker_env(&self) -> Vec<(String, String)> {
        let mut envs = vec![
            ("PM_SIDECAR_OFFLINE".to_string(), "1".to_string()),
            ("HF_HUB_OFFLINE".to_string(), "1".to_string()),
            ("TRANSFORMERS_OFFLINE".to_string(), "1".to_string()),
            ("HF_HUB_DISABLE_TELEMETRY".to_string(), "1".to_string()),
            (
                "HF_HUB_DISABLE_SYMLINKS_WARNING".to_string(),
                "1".to_string(),
            ),
            ("PYTHONUTF8".to_string(), "1".to_string()),
            (
                "PM_SIDECAR_THREADS".to_string(),
                sidecar_thread_budget(std::thread::available_parallelism().map(|n| n.get()).ok())
                    .to_string(),
            ),
        ];
        // Debug builds only: unlock the worker's dev-only `net_selftest` handler (the Developer-mode
        // network-block probe, issue #286). A release worker never sets this, so it refuses the method
        // — the untrusted worker attempts a socket only under a dev build's explicit dev-tab click.
        if cfg!(debug_assertions) {
            envs.push(("PM_SIDECAR_DEV".to_string(), "1".to_string()));
        }
        if let Some(dir) = self.paths.models_dir() {
            let _ = std::fs::create_dir_all(&dir);
            envs.push((
                "PM_MODELS_DIR".to_string(),
                dir.to_string_lossy().into_owned(),
            ));
        }
        envs
    }

    /// Try to launch the worker confined in the no-network AppContainer (issue #286). Returns
    /// `Ok(None)` — the caller then runs it unconfined — if the sandbox can't be set up or the confined
    /// launch fails. On success the [`Sandbox`] is stashed on the manager so `request` can stage the
    /// files path-bearing calls parse.
    #[cfg(windows)]
    fn try_spawn_confined(
        &self,
        py: &Path,
        script: &Path,
        envs: &[(String, String)],
    ) -> Result<Option<Process>> {
        let venv_dir = &self.paths.venv_dir;
        let Some(runtime) = venv_dir.parent().map(|p| p.to_path_buf()) else {
            return self.fall_open(SbxError::new(
                sbx::NO_RUNTIME_DIR,
                "the venv has no parent runtime dir",
            ));
        };
        let Some(models) = self.paths.models_dir() else {
            return self.fall_open(SbxError::new(
                sbx::NO_MODELS_DIR,
                "the models dir could not be resolved",
            ));
        };
        let Some(base) = base_python_dir(venv_dir) else {
            return self.fall_open(SbxError::new(
                sbx::NO_BASE_PYTHON,
                "base python unresolved from pyvenv.cfg",
            ));
        };
        let sandbox =
            match Sandbox::ensure(venv_dir, &base, &models, &self.paths.source_dir, &runtime) {
                Ok(s) => s,
                Err(e) => return self.fall_open(e),
            };

        let env_refs: Vec<(&str, &str)> =
            envs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let cwd = sandbox.staging_dir().to_path_buf();
        let mut confined =
            match sandbox.spawn_confined(py, &[script], &env_refs, PYTHON_ENV_REMOVES, &cwd) {
                Ok(c) => c,
                Err(e) => return self.fall_open(SbxError::new(sbx::CONFINED_SPAWN, e)),
            };
        let stdin = confined.stdin.take().unwrap();
        let stdout = BufReader::new(confined.stdout.take().unwrap());
        // Record the confined view for the Developer-mode readout BEFORE the sandbox moves into the
        // staging slot below. The AppContainer enforces both axes (no sockets + ACL read-set).
        *self.sandbox_report.lock().unwrap() = SandboxReport::Confined {
            mechanism: sandbox.mechanism().to_string(),
            staging_dir: sandbox.staging_dir().display().to_string(),
            granted_dirs: sandbox.granted_dirs(),
            layers: vec!["network".to_string(), "filesystem".to_string()],
        };
        *self.sandbox.lock().unwrap() = Some(sandbox);
        Ok(Some(Process {
            stdin: Box::new(stdin),
            stdout: Box::new(stdout),
            control: Box::new(confined),
        }))
    }

    /// Record a fall-open: log the coded reason, stamp the Developer-mode readout, and return `Ok(None)`
    /// so the caller runs the worker unconfined (issue #286). The log line keeps "running unconfined" as
    /// the stable grep tell AND carries the `[SBX-####]` code so a tester/user can quote it. Shared by
    /// the Windows, Linux, and macOS confinement paths.
    #[cfg(any(
        windows,
        target_os = "macos",
        all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    ))]
    fn fall_open(&self, err: SbxError) -> Result<Option<Process>> {
        eprintln!("sidecar sandbox: running unconfined — {err}");
        // Clear any Sandbox from an earlier confined spawn: after a fall-open the next worker runs
        // unconfined, so `maybe_stage_input` must NOT keep staging inputs (which would rewrite the path
        // to a copy and leave the readout — now Unconfined — disagreeing with a still-present handle).
        *self.sandbox.lock().unwrap() = None;
        *self.sandbox_report.lock().unwrap() = SandboxReport::Unconfined {
            code: err.code.to_string(),
            detail: err.detail,
        };
        Ok(None)
    }

    /// When the worker is confined, copy the file a path-bearing request parses into the
    /// sandbox-readable staging dir and rewrite `params.path` to it — so the confined worker sees ONLY
    /// that one file, never the user's real tree or the vault. The returned guard deletes the staged
    /// copy when the request finishes. No-op (returns `None`) when unconfined or the request has no path.
    /// Shared by the Windows, Linux, and macOS confinement paths (all stage into a granted dir).
    #[cfg(any(
        windows,
        target_os = "macos",
        all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    ))]
    fn maybe_stage_input(&self, req: &mut Value) -> Option<crate::sidecar_stage::StagedInput> {
        let guard = self.sandbox.lock().unwrap();
        let sandbox = guard.as_ref()?;
        let path = req["params"]["path"].as_str()?.to_string();
        match crate::sidecar_stage::stage_into(sandbox.staging_dir(), Path::new(&path)) {
            Ok(staged) => {
                req["params"]["path"] = json!(staged.path().to_string_lossy());
                Some(staged)
            }
            Err(e) => {
                // Flag with a code rather than bury it: the request then runs on the ORIGINAL path,
                // which the confined worker can't read, so it will fail — the code points at why.
                eprintln!(
                    "sidecar sandbox: running unconfined-for-this-request — [{}] could not stage {path}: {e}",
                    sbx::STAGE_COPY
                );
                None
            }
        }
    }

    /// Try to launch the worker confined with Landlock (filesystem) + seccomp (network), self-imposed in
    /// the child's `pre_exec` (issue #286 PR2d). Returns `Ok(None)` — caller runs it unconfined — if the
    /// sandbox can't be set up, the confined launch fails, OR the confined worker fails its self-test
    /// (it couldn't load its libraries under the sandbox). On success the [`Sandbox`] is stashed on the
    /// manager for per-request input staging, exactly like the Windows path.
    #[cfg(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    fn try_spawn_confined_linux(
        &self,
        py: &Path,
        script: &Path,
        envs: &[(String, String)],
    ) -> Result<Option<Process>> {
        let venv_dir = &self.paths.venv_dir;
        let Some(runtime) = venv_dir.parent().map(|p| p.to_path_buf()) else {
            return self.fall_open(SbxError::new(
                sbx::NO_RUNTIME_DIR,
                "the venv has no parent runtime dir",
            ));
        };
        let Some(models) = self.paths.models_dir() else {
            return self.fall_open(SbxError::new(
                sbx::NO_MODELS_DIR,
                "the models dir could not be resolved",
            ));
        };
        let Some(base) = base_python_dir(venv_dir) else {
            return self.fall_open(SbxError::new(
                sbx::NO_BASE_PYTHON,
                "base python unresolved from pyvenv.cfg",
            ));
        };
        let sandbox =
            match Sandbox::ensure(venv_dir, &base, &models, &self.paths.source_dir, &runtime) {
                Ok(s) => s,
                Err(e) => return self.fall_open(e),
            };

        // Build the confined worker command: same stdio/env/offline posture as the unconfined child,
        // plus TMPDIR pointed at the (Landlock-granted) staging dir so `tempfile` doesn't hit the
        // ungranted system /tmp, and no .pyc writes into the read-only interpreter trees. `install_into`
        // sets the cwd to staging and installs the pre_exec confinement hook.
        let mut command = Command::new(py);
        command
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        for (k, v) in envs {
            command.env(k, v);
        }
        command.env("TMPDIR", sandbox.staging_dir());
        command.env("PYTHONDONTWRITEBYTECODE", "1");
        for k in PYTHON_ENV_REMOVES {
            command.env_remove(k);
        }
        no_window(&mut command);
        sandbox.install_into(&mut command);

        let mut child = match command.spawn() {
            Ok(c) => c,
            // A pre_exec confinement syscall failing surfaces here as a spawn error — fall open rather
            // than break ingest (the preflight below would also have caught a booted-but-broken worker).
            Err(e) => return self.fall_open(SbxError::new(sbx::CONFINED_SPAWN, e)),
        };
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut proc = Process {
            stdin: Box::new(stdin),
            stdout: Box::new(stdout),
            control: Box::new(StdChild(child)),
        };

        // Preflight: prove the confined worker can boot AND load its native libraries before we commit
        // to it. If the Landlock allow-set is missing something the interpreter/onnxruntime needs, this
        // is where it surfaces as a clean, coded fall-open instead of every later request failing.
        if let Err(e) = self.confined_preflight(&mut proc) {
            drop(proc); // kills the confined worker
            return self.fall_open(SbxError::new(sbx::LINUX_PREFLIGHT, e));
        }

        // Kept it. Stamp the readout (honestly Degraded when Landlock was unavailable) and stash the
        // sandbox for per-request staging.
        let report = match sandbox.degraded() {
            Some((code, detail)) => SandboxReport::Degraded {
                layers: sandbox.layers(),
                code: code.to_string(),
                detail,
            },
            None => SandboxReport::Confined {
                mechanism: sandbox.mechanism().to_string(),
                staging_dir: sandbox.staging_dir().display().to_string(),
                granted_dirs: sandbox.granted_dirs(),
                layers: sandbox.layers(),
            },
        };
        *self.sandbox_report.lock().unwrap() = report;
        *self.sandbox.lock().unwrap() = Some(sandbox);
        Ok(Some(proc))
    }

    /// Try to launch the worker confined by `sandbox-exec` applying a `(deny default)` Seatbelt profile
    /// — no network, restricted filesystem (issue #286 PR2c). Returns `Ok(None)` — caller runs it
    /// unconfined — if the sandbox can't be set up, the confined launch fails, OR the confined worker
    /// fails its self-test. On success the [`Sandbox`] is stashed on the manager for per-request input
    /// staging, exactly like the Windows and Linux paths.
    #[cfg(target_os = "macos")]
    fn try_spawn_confined_macos(
        &self,
        py: &Path,
        script: &Path,
        envs: &[(String, String)],
    ) -> Result<Option<Process>> {
        let venv_dir = &self.paths.venv_dir;
        let Some(runtime) = venv_dir.parent().map(|p| p.to_path_buf()) else {
            return self.fall_open(SbxError::new(
                sbx::NO_RUNTIME_DIR,
                "the venv has no parent runtime dir",
            ));
        };
        let Some(models) = self.paths.models_dir() else {
            return self.fall_open(SbxError::new(
                sbx::NO_MODELS_DIR,
                "the models dir could not be resolved",
            ));
        };
        let Some(base) = base_python_dir(venv_dir) else {
            return self.fall_open(SbxError::new(
                sbx::NO_BASE_PYTHON,
                "base python unresolved from pyvenv.cfg",
            ));
        };
        let sandbox =
            match Sandbox::ensure(venv_dir, &base, &models, &self.paths.source_dir, &runtime) {
                Ok(s) => s,
                Err(e) => return self.fall_open(e),
            };

        // Build the confined worker command: `sandbox-exec -p <profile> -D… -- <py> <script>` (the
        // Sandbox owns the wrapping) with the same stdio/env/offline posture as the unconfined child,
        // plus TMPDIR pointed at the (profile-granted) staging dir, no .pyc writes into the read-only
        // interpreter trees, and HOME set to staging so getpwuid() never fires (keeping the
        // opendirectoryd mach-lookup off the boot path).
        let mut command = sandbox.wrap_command(py, script);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        for (k, v) in envs {
            command.env(k, v);
        }
        command.env("TMPDIR", sandbox.staging_dir());
        command.env("PYTHONDONTWRITEBYTECODE", "1");
        command.env("HOME", sandbox.staging_dir());
        for k in PYTHON_ENV_REMOVES {
            command.env_remove(k);
        }
        no_window(&mut command);

        let mut child = match command.spawn() {
            Ok(c) => c,
            // sandbox-exec rejecting the profile, or the pre_exec fd-sweep failing, surfaces here — fall
            // open rather than break ingest (the preflight below would also catch a broken worker).
            Err(e) => return self.fall_open(SbxError::new(sbx::CONFINED_SPAWN, e)),
        };
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut proc = Process {
            stdin: Box::new(stdin),
            stdout: Box::new(stdout),
            control: Box::new(StdChild(child)),
        };

        // Preflight: prove the confined worker can boot, load its native libraries, AND read its model
        // cache before committing to it. A profile missing something surfaces here as a clean, coded
        // fall-open instead of every later request failing.
        if let Err(e) = self.confined_preflight(&mut proc) {
            drop(proc); // kills the confined worker
            return self.fall_open(SbxError::new(sbx::MAC_PREFLIGHT, e));
        }

        // Kept it. macOS confinement is all-or-nothing, so `degraded()` is always `None` here (this
        // stays a full Confined stamp); the match mirrors the Linux arm for one shared shape and so the
        // Degraded path is already wired if macOS ever gains a partial mode.
        let report = match sandbox.degraded() {
            Some((code, detail)) => SandboxReport::Degraded {
                layers: sandbox.layers(),
                code: code.to_string(),
                detail,
            },
            None => SandboxReport::Confined {
                mechanism: sandbox.mechanism().to_string(),
                staging_dir: sandbox.staging_dir().display().to_string(),
                granted_dirs: sandbox.granted_dirs(),
                layers: sandbox.layers(),
            },
        };
        *self.sandbox_report.lock().unwrap() = report;
        *self.sandbox.lock().unwrap() = Some(sandbox);
        Ok(Some(proc))
    }

    /// One `worker_selftest` round-trip against the freshly-spawned confined worker (issue #286). Proves
    /// it can boot, load its native libraries (onnxruntime's `.so`/`.dylib`, which the lazy imports a
    /// plain `ping` wouldn't touch), and read its model cache under the sandbox. Any failure — a dead
    /// worker, a timeout, or a non-ok reply — means the confinement broke the worker, so the caller
    /// falls open. Shared by the Linux and macOS arms (Windows proves confinement a different way).
    #[cfg(any(
        target_os = "macos",
        all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    ))]
    fn confined_preflight(&self, proc: &mut Process) -> Result<()> {
        let id = self.req_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let line =
            serde_json::to_string(&json!({ "id": id, "method": "worker_selftest", "params": {} }))
                .map_err(|e| Error::Other(format!("encode selftest: {e}")))?;
        proc.stdin
            .write_all(line.as_bytes())
            .and_then(|()| proc.stdin.write_all(b"\n"))
            .and_then(|()| proc.stdin.flush())
            .map_err(|e| Error::Other(format!("selftest write: {e}")))?;
        let reply = read_reply_with_timeout(proc, id, SELFTEST_TIMEOUT)
            .map_err(|e| Error::Other(format!("no selftest reply: {e}")))?;
        if reply["ok"].as_bool() == Some(true) {
            Ok(())
        } else {
            Err(Error::Other(format!(
                "worker_selftest failed under confinement: {}",
                reply["error"].as_str().unwrap_or("unknown")
            )))
        }
    }

    /// Download the model that `method`/`params` needs by running the short-lived, network-ALLOWED
    /// `--fetch` helper (issue #286). The offline worker reports a cold-cache model as
    /// `model_not_cached`; this turns that miss into a real download WITHOUT ever giving the worker —
    /// the process that parses untrusted files — a socket. It reuses the worker's own loaders, so the
    /// cache it fills is exactly the one the worker then reads (same OS user, same fastembed
    /// defaults, the same `PM_MODELS_DIR`). Blocks — a first download can take minutes — bounded by
    /// a generous deadline so a stalled download can't wedge the caller forever.
    fn fetch_model(&self, method: &str, params: &Value) -> Result<()> {
        let py = self.paths.venv_python();
        let request = serde_json::to_string(&json!({ "method": method, "params": params }))
            .map_err(|e| Error::Other(format!("encode fetch request: {e}")))?;

        let mut command = Command::new(&py);
        command
            .arg(self.paths.script())
            .arg("--fetch")
            // The one child that MAY reach the network: it only ever downloads a model named by
            // Rust and never touches untrusted file bytes. Force offline OFF (drop any inherited
            // flag) so it can always fetch; keep the download quiet/private like the worker.
            .env_remove("PM_SIDECAR_OFFLINE")
            .env_remove("HF_HUB_OFFLINE")
            .env_remove("TRANSFORMERS_OFFLINE")
            .env("HF_HUB_DISABLE_TELEMETRY", "1")
            .env("HF_HUB_DISABLE_SYMLINKS_WARNING", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        clean_python_env(&mut command);
        set_models_dir(&mut command, &self.paths);
        no_window(&mut command);

        let mut child = command
            .spawn()
            .map_err(|e| Error::Other(format!("could not start the model fetcher: {e}")))?;

        // Hand it the one-line request and close stdin so it reads EOF and proceeds.
        {
            let mut stdin = child.stdin.take().unwrap();
            let write = stdin
                .write_all(request.as_bytes())
                .and_then(|()| stdin.write_all(b"\n"));
            if let Err(e) = write {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Other(format!("model fetcher stdin: {e}")));
            }
        }

        // Read the single reply line on a worker thread so this thread can enforce a deadline:
        // on timeout we kill the child, which closes its stdout and unblocks the reader at EOF.
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            let _ = reader.read_line(&mut line);
            let _ = tx.send(line);
        });

        let outcome = match rx.recv_timeout(FETCH_TIMEOUT) {
            Ok(line) => {
                let reply: Value = serde_json::from_str(line.trim()).unwrap_or(Value::Null);
                if reply["ok"].as_bool() == Some(true) {
                    Ok(())
                } else {
                    let msg = reply["error"].as_str().unwrap_or("unknown fetcher error");
                    Err(Error::Other(format!("could not download the model: {msg}")))
                }
            }
            Err(_) => {
                let _ = child.kill();
                Err(Error::Other("downloading the model timed out".to_string()))
            }
        };
        let _ = child.wait();
        let _ = reader.join();
        outcome
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
    // Disjoint borrows: the reader thread owns `stdout`, the watchdog owns `control`.
    let Process {
        control, stdout, ..
    } = &mut *proc;
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
                control.kill();
                control.wait();
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
/// Deadline for the `--fetch` helper (issue #286). A first model download is slow; this matches the
/// worker's own download-bearing grace (`request_timeout` for embed/rerank/analyze_image) so the fetch
/// path is no more likely to time out than the inline first-use download it replaces.
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Deadline for the confined Linux worker's one-shot self-test (issue #286 PR2d). Generous: a cold
/// `import onnxruntime` (its native `.so` load + CPU-feature probing) can take several seconds on a
/// slow disk; if it hasn't answered by now the confinement has almost certainly broken the worker, so
/// we fall open.
#[cfg(any(
    target_os = "macos",
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )
))]
const SELFTEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// What the network-allowed `--fetch` helper is given for `method`: what names the model to
/// download, and nothing else.
///
/// The fetcher is the ONE child that keeps a socket, so it must never receive the user's content.
/// An embed, count_tokens or rerank request carries it beside the model (the texts, the query, the
/// passages), and the helper reads only `model` and, for a custom model, its `custom` spec, so it
/// gets those two, each only when the request has it. `analyze_image` names no model (all the
/// fetcher does for it is build the OCR engine) and its params carry the path of the image
/// mid-ingest, so it gets `null`, which the helper reads as no params at all (`params or {}`). So
/// does any other method: the helper has nothing to fetch for one.
fn fetch_params(method: &str, params: &Value) -> Value {
    match method {
        "embed" | "count_tokens" | "rerank" => {
            let mut model = serde_json::Map::new();
            for key in ["model", "custom"] {
                if let Some(value) = params.get(key) {
                    model.insert(key.to_string(), value.clone());
                }
            }
            Value::Object(model)
        }
        _ => Value::Null,
    }
}

/// Whether a sidecar reply is the offline worker signalling a model isn't downloaded yet (as opposed
/// to a genuine failure) — the trigger for a fetch-and-retry (issue #286). Pure, so it unit-tests the
/// classification without a live child.
fn is_model_not_cached(reply: &Value) -> bool {
    reply["ok"].as_bool() != Some(true) && reply["error_kind"].as_str() == Some("model_not_cached")
}

fn request_timeout(method: &str) -> std::time::Duration {
    use std::time::Duration;
    match method {
        // First use of any of these can download a model; keep the grace long.
        "embed" | "rerank" | "analyze_image" => Duration::from_secs(30 * 60),
        // CPU-bound conversion / parse / projection of a possibly-large or pathological input.
        "convert" | "analyze_spreadsheet" | "reduce" => Duration::from_secs(10 * 60),
        // Pure tokeniser pass — fast once the tokeniser is loaded.
        "count_tokens" => Duration::from_secs(5 * 60),
        // A zip directory entry or a PDF trailer: milliseconds on any real file, and generous even
        // for a damaged PDF whose cross-reference table has to be reconstructed by scanning. Kept
        // well under the default so a stall on one file's properties can't hold a sync for ten
        // minutes over a fact the document was free not to state in the first place.
        "file_properties" => Duration::from_secs(2 * 60),
        // Any other (or future) method: a safe, generous default.
        _ => Duration::from_secs(10 * 60),
    }
}

/// The flags every pip install of a lock passes, the base venv's and each optional component's.
/// `--require-hashes` and `--no-deps` are the two that carry weight; `provision` says why each.
const LOCK_INSTALL_FLAGS: [&str; 4] = [
    "--disable-pip-version-check",
    "--no-cache-dir",
    "--require-hashes",
    "--no-deps",
];

/// pillow 13 (pinned in requirements.txt: the first release with the decoder fixes
/// published from 2026-10-17) needs Python >= 3.11, so the venv's base interpreter
/// must too. It was 3.10, markitdown's floor, until then; 3.10 reaches end of life
/// in October 2026. Every installer already provides 3.12 (bundled on Windows and
/// Linux, downloaded on macOS when the machine has nothing new enough), so a venv
/// left on 3.10 is rebuilt once on the next launch. `just lock-regen` reads this
/// value as the floor the locks resolve from.
const MIN_PYTHON: (u32, u32) = (3, 11);

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
///   built against macOS system 3.9, or on a 3.10 from before the floor rose)
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

/// Whether provisioning must put an optional component back: the user installed it (its marker is
/// present, current or not; removing a component deletes the marker first) and the venv being
/// provisioned no longer satisfies it — rebuilt from scratch, or updated in place to locks that
/// moved its stamp.
fn should_reinstall_optional(rebuild: bool, marker_present: bool, ready: bool) -> bool {
    marker_present && (rebuild || !ready)
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

/// Map a pip / `run_command` failure message to a cause. The common one is a
/// pinned package needing a newer Python than the venv's (pillow 13 needs 3.11),
/// which pip reports as "No matching distribution found" / "requires-python".
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

// Both messages name the floor from MIN_PYTHON itself, so raising it cannot leave them asking for
// a version PM then refuses.
fn missing_message() -> String {
    let (major, minor) = MIN_PYTHON;
    format!(
        "No Python interpreter was found. PM's document engine needs Python {major}.{minor} or \
         newer — install it and make sure it's on your PATH, or set PM_PYTHON to its full path."
    )
}

fn too_old_message() -> String {
    let (major, minor) = MIN_PYTHON;
    let mut m = format!(
        "The Python that was found is older than {major}.{minor}, which PM's document engine \
         requires. Install Python {major}.{minor} or newer, then retry."
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
/// Environment keys stripped from any Python child PM launches: the `PYTHON*` overrides that could
/// point the interpreter at the wrong stdlib (so a poisoned environment can't break the bundled
/// interpreter or the venv). Shared by [`clean_python_env`] and the worker spawn so the confined and
/// unconfined paths strip exactly the same set.
const PYTHON_ENV_REMOVES: &[&str] = &["PYTHONHOME", "PYTHONPATH", "PYTHONSTARTUP"];

/// How many threads any ONE of the sidecar's native pools may use.
///
/// Three of them size themselves independently — onnxruntime's intra-op pool, the OpenBLAS inside
/// numpy, and the rayon pool inside the `tokenizers` extension — and each defaults to one thread per
/// core. On a 24-core machine that is ~94 threads fighting over 24 cores during a single embedding
/// pass, which is what "PM maxes out the CPU when it opens" looks like from the outside.
///
/// Half the cores, clamped to 2..=8. The clamp carries the decision: measured on 24 cores, moving
/// from an unbounded pool to 8 cost ~14% embedding throughput and handed back 16 cores. The floor of
/// 2 keeps a single-core VM from serializing. The sidecar derives the same number itself when the
/// variable is absent (a raw dev run), so the two paths agree — keep them in step.
///
/// Fed from `available_parallelism` rather than `hardware::scan()`: that reports PHYSICAL cores via
/// sysinfo and runs a GPU probe to do it, while this figure wants the LOGICAL, cgroup-aware count —
/// a container with a CPU quota must size its pools to the quota, not to the host.
fn sidecar_thread_budget(cores: Option<usize>) -> usize {
    (cores.unwrap_or(4) / 2).clamp(2, 8)
}

fn clean_python_env(command: &mut Command) {
    for k in PYTHON_ENV_REMOVES {
        command.env_remove(k);
    }
    command.env("PYTHONUTF8", "1");
}

/// Bridge the confined worker's process control to the generic [`ChildControl`] the request loop drives.
#[cfg(windows)]
impl ChildControl for crate::sidecar_sandbox::ConfinedChild {
    fn kill(&mut self) {
        crate::sidecar_sandbox::ConfinedChild::kill(self);
    }
    fn wait(&mut self) {
        crate::sidecar_sandbox::ConfinedChild::wait(self);
    }
    fn try_reap(&mut self) -> bool {
        crate::sidecar_sandbox::ConfinedChild::try_reap(self)
    }
}

/// The base interpreter a venv was built from, read from its `pyvenv.cfg` `home =` line (stripping the
/// `\\?\` long-path prefix, a Windows-only nicety that's a harmless no-op elsewhere). The confined
/// worker needs read/execute on this tree as well as the venv, because the venv's python is only a thin
/// launcher that defers to it. On Windows `home` is the interpreter dir; on Linux and macOS it's the
/// `bin` dir, and those sandboxes grant its parent (the install root) so `lib/pythonX.Y` is reachable.
#[cfg(any(
    windows,
    target_os = "macos",
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )
))]
fn base_python_dir(venv_dir: &Path) -> Option<PathBuf> {
    let cfg = std::fs::read_to_string(venv_dir.join("pyvenv.cfg")).ok()?;
    for line in cfg.lines() {
        if let Some(rest) = line.strip_prefix("home") {
            let val = rest.trim_start_matches([' ', '=']).trim();
            let val = val.strip_prefix(r"\\?\").unwrap_or(val);
            if !val.is_empty() {
                return Some(PathBuf::from(val));
            }
        }
    }
    None
}

/// Point the sidecar's model caches at PM's data dir (`runtime/models`) via `PM_MODELS_DIR`, shared by
/// the long-lived worker and the short-lived `--fetch` helper so they read/write one location (issue
/// #286). The weights then uninstall with the app, and the Windows sidecar sandbox gets a single tidy
/// filesystem allow-set instead of scattered `%TEMP%` / `~/.cache` paths. No-op when the models dir
/// can't be derived (a raw dev layout), where each library falls back to its own default.
fn set_models_dir(command: &mut Command, paths: &SidecarPaths) {
    if let Some(dir) = paths.models_dir() {
        let _ = std::fs::create_dir_all(&dir);
        command.env("PM_MODELS_DIR", &dir);
    }
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
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// A [`ChildControl`] that records what the lifecycle did to it and can be told how stubbornly to
    /// die, so the `Drop` contract and the give-up behaviour are pinned without spawning a real
    /// interpreter. The reaping syscall itself (`waitpid` / `WaitForSingleObject`) is NOT covered by
    /// these tests and cannot be, portably — Windows has no zombie to observe at all.
    struct RecordingControl {
        log: Arc<Mutex<Vec<&'static str>>>,
        /// How many `try_reap` probes report "still dying" before the child is reported gone.
        probes_before_exit: usize,
    }

    impl RecordingControl {
        fn new(log: &Arc<Mutex<Vec<&'static str>>>, probes_before_exit: usize) -> Self {
            Self {
                log: Arc::clone(log),
                probes_before_exit,
            }
        }
    }

    impl ChildControl for RecordingControl {
        fn kill(&mut self) {
            self.log.lock().unwrap().push("kill");
        }
        fn wait(&mut self) {
            self.log.lock().unwrap().push("wait");
        }
        fn try_reap(&mut self) -> bool {
            self.log.lock().unwrap().push("try_reap");
            if self.probes_before_exit == 0 {
                return true;
            }
            self.probes_before_exit -= 1;
            false
        }
    }

    #[test]
    fn dropping_a_process_kills_the_child_and_then_reaps_it() {
        // The whole defect in one assertion: `Drop` used to call only `kill()`, so on Unix every
        // respawn (an IO error, a failed confined preflight, the wipe teardown) left a zombie for the
        // life of the app. The ORDER is pinned too — probing before killing would just spin out the
        // budget against a healthy worker.
        let log = Arc::new(Mutex::new(Vec::new()));
        drop(Process {
            stdin: Box::new(std::io::sink()),
            stdout: Box::new(std::io::empty()),
            control: Box::new(RecordingControl::new(&log, 0)),
        });

        assert_eq!(&*log.lock().unwrap(), &["kill", "try_reap"]);
    }

    #[test]
    fn the_reap_keeps_probing_until_the_child_is_gone() {
        // A kill is not instantaneous — TerminateProcess is asynchronous and a multi-GB Python RSS
        // takes real time to tear down — so one probe is not enough.
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut control = RecordingControl::new(&log, 3);

        assert!(reap_within(
            &mut control,
            Duration::from_secs(30),
            Duration::from_millis(1)
        ));
        assert_eq!(
            log.lock().unwrap().len(),
            4,
            "three 'still dying' answers, then the one that reports it gone"
        );
    }

    #[test]
    fn the_reap_gives_up_rather_than_hanging_on_a_child_that_never_dies() {
        // The reason this is a bounded wait and not the `wait()` the trait already has. `Process::drop`
        // runs under the manager's `proc` mutex and, on the wipe path, on a tokio worker; a child in
        // uninterruptible sleep would otherwise hang it there for the rest of the session. Re-leaking
        // one zombie is the strictly better failure.
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut control = RecordingControl::new(&log, usize::MAX);

        let started = Instant::now();
        let reaped = reap_within(
            &mut control,
            Duration::from_millis(20),
            Duration::from_millis(1),
        );
        let took = started.elapsed();

        assert!(!reaped, "a child that never exits is never reaped");
        assert!(
            took < Duration::from_secs(10),
            "the wait must return on its own; it took {took:?}"
        );
        assert!(
            log.lock().unwrap().len() > 1,
            "and it really did re-probe rather than give up after the first miss"
        );
    }

    /// End-to-end smoke test of the Windows AppContainer confinement (issue #286 PR2b): spawn the REAL
    /// worker confined, `ping` it over the raw pipes, then convert a text file through the staging path,
    /// and confirm it actually ran confined. `#[ignore]` + a `PM_SANDBOX_SMOKE_VENV` env because it
    /// needs the live venv and runs the slow one-time ACL grant. Run manually (PowerShell):
    ///   $env:PM_SANDBOX_SMOKE_VENV = "$env:LOCALAPPDATA\Personal Manager\runtime\venv"
    ///   cargo test --manifest-path src-tauri/Cargo.toml confined_worker_smoke -- --ignored --nocapture
    #[test]
    #[ignore = "windows-only, needs the live venv; validates the confined stdio protocol"]
    #[cfg(windows)]
    fn confined_worker_smoke() {
        let source_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("sidecar");
        // No hardcoded path in a public repo: point PM_SANDBOX_SMOKE_VENV at your installed venv dir
        // (…/runtime/venv). Unset → skip.
        // PANIC, not `return`. An early return in a #[test] is a PASS, so the whole point of
        // asking for these explicitly with `--ignored` was defeated: the run reported green
        // having proved nothing about the confinement it exists to prove (H7). You only reach
        // this test by naming it, so a missing prerequisite is an error, never a skip.
        let venv_dir = PathBuf::from(std::env::var("PM_SANDBOX_SMOKE_VENV").expect(
            "set PM_SANDBOX_SMOKE_VENV to the installed venv dir (.../runtime/venv) to run this",
        ));
        assert!(venv_dir.join("Scripts\\python.exe").exists(), "no venv");
        let mgr = SidecarManager::new(SidecarPaths {
            source_dir,
            venv_dir,
        });

        // The FIRST sidecar call is a convert (path-bearing), with NO prior ping — the regression case
        // for the staging-ordering bug: the SAME request triggers the first confined spawn, so the
        // input must be staged AFTER that spawn or the confined worker gets the un-granted original path.
        let tmp = std::env::temp_dir().join("pm_confined_smoke.txt");
        std::fs::write(&tmp, "hello from a confined worker").unwrap();
        let (markdown, _title) = mgr.convert(&tmp).expect("confined convert (first request)");
        std::fs::remove_file(&tmp).ok();
        assert!(
            markdown.contains("hello from a confined worker"),
            "convert output: {markdown:?}"
        );
        assert!(
            mgr.sandbox.lock().unwrap().is_some(),
            "worker should be CONFINED — the sandbox was not set up"
        );

        // ping confirms the worker is still alive over its pipes after the convert.
        let pong = mgr.request("ping", json!({})).expect("ping");
        assert_eq!(pong["ok"], serde_json::Value::Bool(true), "ping");
    }

    /// End-to-end smoke test of the Linux Landlock + seccomp confinement (issue #286 PR2d): spawn the
    /// REAL worker confined, convert a text file through the staging path, then PROVE both enforcement
    /// axes on the live worker — seccomp refuses an outbound socket, and Landlock refuses reading a path
    /// outside the allow-set. `#[ignore]` + a `PM_SANDBOX_SMOKE_VENV` env because it needs the live venv
    /// and a Landlock-capable kernel (≥ 5.13). This is the enforcement check the Windows dev box cannot
    /// run and CI (compile/lint/unit only) does not — run it on a real Linux box:
    ///   PM_SANDBOX_SMOKE_VENV="$HOME/.local/share/Personal Manager/runtime/venv" \
    ///     cargo test --manifest-path src-tauri/Cargo.toml confined_worker_smoke_linux -- --ignored --nocapture
    #[test]
    #[ignore = "linux-only, needs the live venv + a Landlock kernel; validates real enforcement"]
    #[cfg(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    fn confined_worker_smoke_linux() {
        let source_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("sidecar");
        // No hardcoded path in a public repo: point PM_SANDBOX_SMOKE_VENV at your installed venv dir
        // (…/runtime/venv). Unset → skip.
        // PANIC, not `return`. An early return in a #[test] is a PASS, so the whole point of
        // asking for these explicitly with `--ignored` was defeated: the run reported green
        // having proved nothing about the confinement it exists to prove (H7). You only reach
        // this test by naming it, so a missing prerequisite is an error, never a skip.
        let venv_dir = PathBuf::from(std::env::var("PM_SANDBOX_SMOKE_VENV").expect(
            "set PM_SANDBOX_SMOKE_VENV to the installed venv dir (.../runtime/venv) to run this",
        ));
        assert!(
            venv_dir.join("bin/python").exists(),
            "no venv at {venv_dir:?}"
        );
        let mgr = SidecarManager::new(SidecarPaths {
            source_dir,
            venv_dir,
        });

        // First call is a convert (path-bearing) with NO prior ping — the SAME request triggers the
        // first confined spawn, so the input must be staged AFTER that spawn (the staging-order
        // regression the Windows test also guards).
        let tmp = std::env::temp_dir().join("pm_confined_smoke_linux.txt");
        std::fs::write(&tmp, "hello from a confined linux worker").unwrap();
        let (markdown, _title) = mgr.convert(&tmp).expect("confined convert (first request)");
        std::fs::remove_file(&tmp).ok();
        assert!(
            markdown.contains("hello from a confined linux worker"),
            "convert output: {markdown:?}"
        );

        // The sandbox handle is set, and (on a Landlock kernel) the readout enforces both axes.
        assert!(
            mgr.sandbox.lock().unwrap().is_some(),
            "worker should be confined — the sandbox was not set up"
        );
        let landlocked = match mgr.sandbox_report() {
            SandboxReport::Confined { ref layers, .. } => {
                assert!(layers.iter().any(|l| l == "network"), "network: {layers:?}");
                assert!(
                    layers.iter().any(|l| l == "filesystem"),
                    "filesystem layer should be enforced on a Landlock kernel: {layers:?}"
                );
                true
            }
            SandboxReport::Degraded { ref code, .. } => {
                eprintln!(
                    "NOTE: running Degraded ({code}) — Landlock unavailable on this kernel; the \
                     seccomp/network assertion still applies, the filesystem one is skipped"
                );
                false
            }
            other => panic!(
                "expected confined/degraded, got a fall-open: {}",
                serde_json::to_value(&other).unwrap()
            ),
        };

        // Network: a debug test build sets PM_SIDECAR_DEV=1, so net_selftest is unlocked. The confined
        // worker's outbound socket must be refused by seccomp.
        let net = mgr.net_selftest().expect("net_selftest");
        assert_eq!(
            net["blocked"],
            serde_json::Value::Bool(true),
            "seccomp should refuse the socket: {net:?}"
        );

        // Filesystem (only meaningful when Landlock is active): a path OUTSIDE the allow-set — a file in
        // the system temp dir, which is not granted — must be refused with EACCES. The path rides in
        // `probe_path`, not `path`, so it bypasses the staging that would otherwise copy it into the
        // granted dir and defeat the probe.
        if landlocked {
            let outside = std::env::temp_dir().join("pm_fs_probe_target.txt");
            std::fs::write(&outside, b"not for the worker").unwrap();
            let probe = mgr
                .request(
                    "fs_probe",
                    json!({ "probe_path": outside.to_string_lossy() }),
                )
                .expect("fs_probe");
            std::fs::remove_file(&outside).ok();
            assert_eq!(
                probe["denied"],
                serde_json::Value::Bool(true),
                "Landlock should deny reading an ungranted path: {probe:?}"
            );
        }
    }

    /// The photo-OCR half of the Linux confinement (issue #617 finding 1): prove the OPTIONAL OCR
    /// component can actually run INSIDE the sandbox. Its own card could not be closed by a PR
    /// because nothing headless could answer the question — the audit's claim was that rapidocr
    /// fetches its models from its OWN host on first use, so the no-network worker could never
    /// obtain them, the constructor would raise, and the F-56 guard would swallow it and commit the
    /// photo with an empty body. A receipt would index as textless and look entirely normal.
    ///
    /// The fix routes rapidocr's weights under `PM_MODELS_DIR` (granted read-only in the allow-set)
    /// and its loader through `_load_model`, so a cold cache raises `ModelNotCached` and rides the
    /// same fetch-and-retry every other model gets. This is the check that says the fix holds on a
    /// real kernel, and it is the ONLY one that can: the fall-open is silent by design, so a
    /// regression here produces no error, no crash, and a green gate.
    ///
    /// Needs the live venv WITH the optional OCR component installed (Settings → the OCR download),
    /// and a Landlock-capable kernel (≥ 5.13):
    ///   PM_SANDBOX_SMOKE_VENV="$HOME/.local/share/Personal Manager/runtime/venv" \
    ///     cargo test --manifest-path src-tauri/Cargo.toml confined_worker_ocr_smoke_linux -- --ignored --nocapture
    ///
    /// Run it COLD too, whenever rapidocr moves: `PM_SANDBOX_SMOKE_COLD=1` with
    /// `PM_SANDBOX_SMOKE_VENV` pointing at a SCRATCH `<root>/runtime/venv` (base + OCR locks
    /// installed) whose `<root>/runtime/models/rapidocr` holds no `.onnx`. That walks the whole
    /// recovery: the offline worker misses, the unconfined `--fetch` helper downloads, the retry
    /// reads the image. The warm run alone passed on the rapidocr 3.10.0 bump that left the fetcher
    /// with nothing to fetch (#932): only a machine that has never run OCR could see it, so a live
    /// test on a machine that has would not have either. Never point a cold run at the live install.
    #[test]
    #[ignore = "linux-only, needs the live venv + the OCR component + a Landlock kernel"]
    #[cfg(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    fn confined_worker_ocr_smoke_linux() {
        let source_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("sidecar");
        // PANIC, not `return`, for the same reason as every other smoke test here: an early return
        // in a #[test] is a PASS, and a confinement check that passes without confining anything is
        // worse than no check at all (H7).
        let venv_dir = PathBuf::from(std::env::var("PM_SANDBOX_SMOKE_VENV").expect(
            "set PM_SANDBOX_SMOKE_VENV to the installed venv dir (.../runtime/venv) to run this",
        ));
        let python = venv_dir.join("bin/python");
        assert!(python.exists(), "no venv at {venv_dir:?}");
        // OCR is an OPTIONAL component, so its absence is a legitimate state of a real install —
        // but it is also this test's entire subject, so it is an error here rather than a skip.
        let models = venv_dir.parent().unwrap().join("models/rapidocr");
        let onnx_count = || {
            std::fs::read_dir(&models).map_or(0, |d| {
                d.filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().is_some_and(|x| x == "onnx"))
                    .count()
            })
        };
        let cold = std::env::var("PM_SANDBOX_SMOKE_COLD").is_ok_and(|v| v == "1");
        if cold {
            assert_eq!(
                onnx_count(),
                0,
                "PM_SANDBOX_SMOKE_COLD=1 needs a scratch runtime with no rapidocr weights at \
                 {models:?} — never the live install"
            );
        } else {
            assert!(
                onnx_count() > 0,
                "no rapidocr weights at {models:?} — install the photo-OCR component (Settings → \
                 Storage) before running this, or run it cold (PM_SANDBOX_SMOKE_COLD=1)"
            );
        }

        // Render the fixture with the venv's OWN Pillow rather than checking a binary into a public
        // repo or adding an image crate to the build. Pillow is a pinned base requirement, so this
        // needs nothing the worker does not already have. Big type on white: the point is to prove
        // the engine RUNS under the sandbox, not to measure how good it is.
        let image = std::env::temp_dir().join("pm_confined_ocr_smoke.png");
        let render = std::process::Command::new(&python)
            .arg("-c")
            .arg(
                "import sys\n\
                 from PIL import Image, ImageDraw, ImageFont\n\
                 img = Image.new('RGB', (900, 220), 'white')\n\
                 ImageDraw.Draw(img).text((30, 60), 'INVOICE 12345', fill='black',\n\
                 font=ImageFont.load_default(size=72))\n\
                 img.save(sys.argv[1])\n",
            )
            .arg(&image)
            .output()
            .expect("render the OCR fixture");
        assert!(
            render.status.success(),
            "could not render the fixture: {}",
            String::from_utf8_lossy(&render.stderr)
        );

        let mgr = SidecarManager::new(SidecarPaths {
            source_dir,
            venv_dir,
        });

        // Like the convert smoke test, the FIRST request is the path-bearing one, so the input has
        // to be staged after the confined spawn rather than before it.
        let analysis = mgr
            .analyze_image(&image, true)
            .expect("confined analyze_image (first request)");
        std::fs::remove_file(&image).ok();

        assert!(
            mgr.sandbox.lock().unwrap().is_some(),
            "worker should be confined — the sandbox was not set up"
        );
        match mgr.sandbox_report() {
            SandboxReport::Confined { ref layers, .. } => {
                assert!(layers.iter().any(|l| l == "network"), "network: {layers:?}");
                assert!(
                    layers.iter().any(|l| l == "filesystem"),
                    "filesystem layer should be enforced on a Landlock kernel: {layers:?}"
                );
            }
            other => panic!(
                "this test is only meaningful on a fully confined worker, got: {}",
                serde_json::to_value(&other).unwrap()
            ),
        }

        // The image was read at all — separates "the sandbox blocked the file" from "OCR failed".
        assert_eq!(analysis.width, Some(900), "the worker never read the image");
        if cold {
            // Detection, orientation and recognition: the fetcher has to have built all three.
            assert_eq!(
                onnx_count(),
                3,
                "the --fetch helper did not download every OCR model into {models:?}"
            );
        }

        // The two assertions below are deliberately separate, because they fail for different
        // reasons and the difference is the whole finding:
        //   ocr_ran == false          → the engine could not start (F-56 swallowed it) — the bug
        //   ocr_ran == true, no text  → the engine ran and read nothing — a fixture problem
        assert!(
            analysis.ocr_ran,
            "OCR did not run inside the confinement — the engine could not start, and the F-56 \
             guard degraded the photo to EXIF-only with an empty body (issue #617 finding 1)"
        );
        let text = analysis.ocr_text.to_ascii_uppercase();
        assert!(
            text.contains("INVOICE") && text.contains("12345"),
            "OCR ran under the confinement but read nothing usable — fixture problem, not a \
             sandbox one. Got: {:?}",
            analysis.ocr_text
        );
    }

    /// End-to-end smoke test of the macOS `sandbox-exec` confinement (issue #286 PR2c): spawn the REAL
    /// worker confined, convert a text file through the staging path, then PROVE enforcement on the live
    /// worker — the Seatbelt profile refuses BOTH a direct outbound socket AND out-of-process DNS
    /// resolution (the mach-lookup-to-mDNSResponder exfil path, finding #1), and refuses reading a file
    /// outside the allow-set. `#[ignore]` + a `PM_SANDBOX_SMOKE_VENV` env because it needs the live venv
    /// on a real Mac. This is the enforcement check neither the Windows dev box nor any CI job can run
    /// (the macOS arm only compiles on `macos-latest` CI and only ENFORCES on a real Mac):
    ///   PM_SANDBOX_SMOKE_VENV="$HOME/Library/Application Support/Personal Manager/runtime/venv" \
    ///     cargo test --manifest-path src-tauri/Cargo.toml confined_worker_smoke_macos -- --ignored --nocapture
    #[test]
    #[ignore = "macOS-only, needs the live venv; validates real sandbox-exec enforcement"]
    #[cfg(target_os = "macos")]
    fn confined_worker_smoke_macos() {
        let source_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("sidecar");
        // No hardcoded path in a public repo: point PM_SANDBOX_SMOKE_VENV at your installed venv dir
        // (…/runtime/venv). Unset → skip.
        // PANIC, not `return`. An early return in a #[test] is a PASS, so the whole point of
        // asking for these explicitly with `--ignored` was defeated: the run reported green
        // having proved nothing about the confinement it exists to prove (H7). You only reach
        // this test by naming it, so a missing prerequisite is an error, never a skip.
        let venv_dir = PathBuf::from(std::env::var("PM_SANDBOX_SMOKE_VENV").expect(
            "set PM_SANDBOX_SMOKE_VENV to the installed venv dir (.../runtime/venv) to run this",
        ));
        assert!(
            venv_dir.join("bin/python").exists(),
            "no venv at {venv_dir:?}"
        );
        let mgr = SidecarManager::new(SidecarPaths {
            source_dir,
            venv_dir,
        });

        // First call is a convert (path-bearing) with NO prior ping — the SAME request triggers the
        // first confined spawn, so the input must be staged AFTER that spawn (the staging-order
        // regression the Windows/Linux tests also guard).
        let tmp = std::env::temp_dir().join("pm_confined_smoke_macos.txt");
        std::fs::write(&tmp, "hello from a confined macos worker").unwrap();
        let (markdown, _title) = mgr.convert(&tmp).expect("confined convert (first request)");
        std::fs::remove_file(&tmp).ok();
        assert!(
            markdown.contains("hello from a confined macos worker"),
            "convert output: {markdown:?}"
        );

        // The sandbox handle is set and the readout enforces both axes (macOS is never Degraded).
        assert!(
            mgr.sandbox.lock().unwrap().is_some(),
            "worker should be confined — the sandbox was not set up"
        );
        match mgr.sandbox_report() {
            SandboxReport::Confined { ref layers, .. } => {
                assert!(layers.iter().any(|l| l == "network"), "network: {layers:?}");
                assert!(
                    layers.iter().any(|l| l == "filesystem"),
                    "filesystem: {layers:?}"
                );
            }
            other => panic!(
                "expected fully Confined on macOS, got: {}",
                serde_json::to_value(&other).unwrap()
            ),
        }

        // Network: a debug test build sets PM_SIDECAR_DEV=1, so net_selftest is unlocked. The confined
        // worker's DIRECT outbound socket must be refused, AND out-of-process DNS resolution must be
        // refused too (no mach-lookup to mDNSResponder) — the macOS-specific exfil path, finding #1.
        let net = mgr.net_selftest().expect("net_selftest");
        assert_eq!(
            net["blocked"],
            serde_json::Value::Bool(true),
            "the profile should refuse the socket: {net:?}"
        );
        assert_eq!(
            net["dns_blocked"],
            serde_json::Value::Bool(true),
            "the profile should refuse out-of-process DNS resolution: {net:?}"
        );

        // Filesystem: a path OUTSIDE the allow-set — a file in the system temp dir, which is not granted
        // — must be refused. The path rides in `probe_path`, not `path`, so it bypasses the staging that
        // would otherwise copy it into the granted dir and defeat the probe.
        let outside = std::env::temp_dir().join("pm_fs_probe_target_macos.txt");
        std::fs::write(&outside, b"not for the worker").unwrap();
        let probe = mgr
            .request(
                "fs_probe",
                json!({ "probe_path": outside.to_string_lossy() }),
            )
            .expect("fs_probe");
        std::fs::remove_file(&outside).ok();
        assert_eq!(
            probe["denied"],
            serde_json::Value::Bool(true),
            "the profile should deny reading an ungranted path: {probe:?}"
        );
    }

    /// An optional component's marker covers its pins AND the lock they came from. The installer
    /// writes it and `optional_ready` compares it, so a drift either silently re-installs on every
    /// check or never detects an install at all.
    ///
    /// Replaces `optional_ocr_marker_matches_pins`, which guarded a hand-kept `OPTIONAL_OCR_MARKER`
    /// duplicate of this join — deleted, since the join is now computed. The half that copy could
    /// never have caught is the second assertion: regenerating a lock moves a component's whole
    /// transitive tree without touching a pin, and the marker has to notice.
    #[test]
    fn optional_stamp_covers_the_pins_and_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = dir.path().join("sidecar");
        std::fs::create_dir_all(&source_dir).unwrap();
        let lock = source_dir.join(OPTIONAL_OCR_COMPONENT.lock);
        // Neutral fixture bytes: only their hash reaches the stamp, so no real version belongs here.
        std::fs::write(&lock, b"pkg==0 \\\n    --hash=sha256:aaa\n").unwrap();

        let mgr = SidecarManager::new(SidecarPaths {
            source_dir,
            venv_dir: dir.path().join("venv"),
        });

        let first = mgr.optional_stamp(&OPTIONAL_OCR_COMPONENT).unwrap();
        // Derived from the pins, not a copy of them: a literal here broke on every pin bump.
        let want = format!("{};lock=", OPTIONAL_OCR_PINS.join(";"));
        assert!(
            first.starts_with(&want),
            "the stamp must lead with the pins joined by ';': {first}"
        );

        // Same pins, regenerated lock (a moved transitive dependency) — must invalidate the marker.
        std::fs::write(&lock, b"pkg==0 \\\n    --hash=sha256:bbb\n").unwrap();
        assert_ne!(
            first,
            mgr.optional_stamp(&OPTIONAL_OCR_COMPONENT).unwrap(),
            "a regenerated lock must not go on satisfying the old marker"
        );
    }

    #[test]
    fn an_installed_component_survives_a_base_lock_update() {
        // The in-place update after a base lock moves: the marker is there but stale. Before, this
        // left photo OCR off with Storage still showing it installed.
        assert!(should_reinstall_optional(false, true, false));
        // Rebuilt venv: whatever the user had goes back, current marker or not.
        assert!(should_reinstall_optional(true, true, true));
        assert!(should_reinstall_optional(true, true, false));
        // Already satisfied, or never installed / removed by the user: leave it alone.
        assert!(!should_reinstall_optional(false, true, true));
        assert!(!should_reinstall_optional(true, false, false));
        assert!(!should_reinstall_optional(false, false, false));
    }

    /// A superseded package is uninstalled before its component's lock installs, so it must never be
    /// in that lock: every install would then remove a package only to put it straight back. The
    /// OCR entry is the reason the field exists (rapidocr 3.10 moved `opencv-python` to its
    /// headless build), so it is pinned too.
    #[test]
    fn a_superseded_package_is_never_in_its_own_lock() {
        for component in ALL_OPTIONAL_COMPONENTS {
            let path = format!(
                "{}/../sidecar/{}",
                env!("CARGO_MANIFEST_DIR"),
                component.lock
            );
            let lock = std::fs::read_to_string(&path).expect("the component lock is checked in");
            let locked: Vec<String> = lock
                .lines()
                .filter_map(|l| l.split_once("=="))
                .map(|(name, _)| name.trim().to_ascii_lowercase())
                .collect();
            for gone in component.superseded {
                assert!(
                    !locked.contains(&gone.to_ascii_lowercase()),
                    "{gone} is superseded yet still in {}",
                    component.lock
                );
            }
        }
        assert_eq!(OPTIONAL_OCR_COMPONENT.superseded, &["opencv-python"]);
    }

    /// L-6: the audit-only `sidecar/requirements-optional.txt` (which `just pip-audit` scans) must list
    /// exactly the optional pins whose source of truth is `OPTIONAL_TSNE_PIN` + `OPTIONAL_OCR_PINS`,
    /// so a bumped/added optional pin can't silently escape CVE scanning.
    #[test]
    fn optional_requirements_file_matches_the_pins() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../sidecar/requirements-optional.txt"
        );
        let contents = std::fs::read_to_string(path)
            .expect("sidecar/requirements-optional.txt must exist for the pip-audit scan (L-6)");
        let listed: Vec<String> = contents
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(str::to_string)
            .collect();
        let mut expected = vec![OPTIONAL_TSNE_PIN.to_string()];
        expected.extend(OPTIONAL_OCR_PINS.iter().map(|s| s.to_string()));
        assert_eq!(
            listed, expected,
            "requirements-optional.txt must list exactly the optional pins, in order"
        );
    }

    /// The package names a committed lock pins, PEP 503-normalised (uv writes `opentsne`, a pin may
    /// say `openTSNE`; `faster_whisper` and `faster-whisper` are one package).
    fn locked_names(lock: &str) -> std::collections::BTreeSet<String> {
        let path = format!("{}/../sidecar/{lock}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("{lock} is checked in"))
            .lines()
            .filter(|l| !l.starts_with(char::is_whitespace) && !l.starts_with('#'))
            .filter_map(|l| l.split_once("=="))
            .map(|(name, _)| normalised(name))
            .collect()
    }

    fn normalised(name: &str) -> String {
        name.trim().to_ascii_lowercase().replace(['_', '.'], "-")
    }

    /// The voice clean-up uninstalls its four packages from every venv that has an engine piece,
    /// so none of them may be a package the base venv or an optional component installs: removing
    /// it would break that. Reads every lock in `sidecar/`, so a lock added later is covered
    /// without remembering this test, and a regeneration that brings one of the four in fails here
    /// before it ships.
    #[test]
    fn no_lock_installs_a_package_the_voice_clean_up_removes() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../sidecar");
        let mut locks: Vec<String> = std::fs::read_dir(dir)
            .expect("the sidecar folder is checked in")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".lock"))
            .collect();
        locks.sort();
        // The base lock and every component's are among them, so the walk can't miss one.
        assert!(locks.iter().any(|l| l == "requirements.lock"));
        for c in ALL_OPTIONAL_COMPONENTS {
            assert!(locks.iter().any(|l| l == c.lock), "{} is missing", c.lock);
        }
        for lock in &locks {
            let names = locked_names(lock);
            assert!(
                !names.is_empty(),
                "{lock} pins nothing, so this proves nothing"
            );
            for package in VOICE_ENGINE_PACKAGES {
                assert!(
                    !names.contains(&normalised(package)),
                    "{package} is in {lock}, so the voice clean-up would remove it from every \
                     venv that had the speech engine"
                );
            }
        }
    }

    /// The engine's own packages are the three import dirs it looks for, and faster-whisper is
    /// removed last (see `VOICE_ENGINE_PACKAGES`).
    #[test]
    fn the_voice_engine_lists_agree() {
        for dir in VOICE_ENGINE_DIRS {
            assert!(
                VOICE_ENGINE_PACKAGES
                    .iter()
                    .any(|p| normalised(p) == normalised(dir)),
                "{dir} is not uninstalled"
            );
        }
        assert_eq!(VOICE_ENGINE_PACKAGES.last(), Some(&"faster-whisper"));
        assert!(!VOICE_ENGINE_DIRS.contains(&"setuptools"));
    }

    #[test]
    fn whisper_folder_name_matches_the_model_voice_input_used() {
        // faster-whisper's `base.en` lived in Systran's repo, laid out by huggingface_hub as
        // `models--<org>--<name>`; a typo here would leave ~145 MB on every computer that used it.
        assert_eq!(WHISPER_REPO_DIR, "models--Systran--faster-whisper-base.en");
    }

    /// A stand-in for the venv's interpreter, as a shell script. `--version` answers 3.`minor`;
    /// every other call is logged, a line each, to `log`; and `-m pip uninstall` does what real pip
    /// does in the venv's own `lib/python3.<minor>/site-packages`, the only one pip there looks in:
    /// a package with no dist-info there is skipped as not installed (exit 0), one whose dist-info
    /// has no RECORD stops the whole command (exit 1, pip's UninstallMissingRecord), and otherwise
    /// the package's import dir, its `.libs` and its dist-info go. Anything else (an install) only
    /// logs.
    #[cfg(unix)]
    fn fake_python(venv: &Path, log: &Path, minor: u32) {
        use std::os::unix::fs::PermissionsExt;
        let py = venv.join("bin").join("python");
        std::fs::create_dir_all(py.parent().unwrap()).unwrap();
        let site = venv
            .join("lib")
            .join(format!("python3.{minor}"))
            .join("site-packages");
        std::fs::write(
            &py,
            format!(
                r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo 'Python 3.{minor}.0'; exit 0; fi
echo "$@" >> '{log}'
[ "$1 $2 $3" = "-m pip uninstall" ] || exit 0
shift 3
site='{site}'
for package; do
    case "$package" in -*) continue ;; esac
    name=$(printf '%s' "$package" | sed 's/-/_/g')
    found=
    for d in "$site/$name"-*.dist-info; do
        if [ -d "$d" ]; then found=$d; fi
    done
    if [ -z "$found" ]; then
        echo "WARNING: Skipping $package as it is not installed." >&2
        continue
    fi
    if [ ! -f "$found/RECORD" ]; then
        echo "ERROR: Cannot uninstall $package, RECORD file not found." >&2
        exit 1
    fi
    rm -rf "$site/$name" "$site/$name.libs" "$found"
done
"#,
                log = log.display(),
                site = site.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&py, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A file of `bytes` zeros at `path`, with its folders.
    #[cfg(unix)]
    fn put(path: PathBuf, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![0u8; bytes]).unwrap();
    }

    /// A venv on [`fake_python`] laid out the way an older PM left it for someone who used voice:
    /// the engine in site-packages beside the document engine's own packages, both voice markers,
    /// the Whisper model beside the search and OCR models, and a recording a crash left in
    /// `runtime/tmp`. Returns the manager, the pip log, site-packages and `runtime/`.
    #[cfg(unix)]
    fn fake_voice_venv(dir: &Path) -> (SidecarManager, PathBuf, PathBuf, PathBuf) {
        let runtime = dir.join("runtime");
        let venv = runtime.join("venv");
        let log = dir.join("pip.log");
        fake_python(&venv, &log, 12);

        let site = venv.join("lib").join("python3.12").join("site-packages");
        for engine in [
            "faster_whisper/__init__.py",
            "faster_whisper-1.2.1.dist-info/RECORD",
            "ctranslate2/__init__.py",
            "ctranslate2-4.8.1.dist-info/RECORD",
            "ctranslate2.libs/libct2.so",
            "av/__init__.py",
            "av-18.1.0.dist-info/RECORD",
            "av.libs/libavcodec.so",
            "setuptools/__init__.py",
            "setuptools-80.9.0.dist-info/RECORD",
        ] {
            put(site.join(engine), 3);
        }
        for base in [
            "fastembed/__init__.py",
            "fastembed-0.9.1.dist-info/RECORD",
            "onnxruntime/__init__.py",
            "pip/__init__.py",
        ] {
            put(site.join(base), 5);
        }
        put(venv.join(".pm-ready"), 1);
        put(venv.join(".pm-voice"), 1);
        put(venv.join(".pm-voice-kept"), 1);
        let models = runtime.join("models");
        put(
            models.join("fastembed/models--Qdrant--bge-small-en-v1.5/model.onnx"),
            10,
        );
        put(models.join("hf/xet/chunk"), 10);
        put(models.join("rapidocr/det.onnx"), 10);
        put(
            models
                .join(WHISPER_REPO_DIR)
                .join("snapshots/abc/model.bin"),
            50,
        );
        put(
            models.join(".locks").join(WHISPER_REPO_DIR).join("x.lock"),
            1,
        );
        put(runtime.join("tmp").join("voice-a1b2c3.webm"), 20);

        let mgr = SidecarManager::new(SidecarPaths {
            source_dir: dir.join("sidecar"),
            venv_dir: venv,
        });
        (mgr, log, site, runtime)
    }

    /// Every file and folder under `root`, relative to it, for comparing a tree before and after.
    /// A link is listed but never followed.
    #[cfg(unix)]
    fn tree(root: &Path) -> std::collections::BTreeSet<PathBuf> {
        fn walk(root: &Path, dir: &Path, out: &mut std::collections::BTreeSet<PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let path = e.path();
                out.insert(path.strip_prefix(root).unwrap().to_path_buf());
                if e.file_type().unwrap().is_dir() {
                    walk(root, &path, out);
                }
            }
        }
        let mut out = std::collections::BTreeSet::new();
        walk(root, root, &mut out);
        out
    }

    /// Take the engine and setuptools out of `site`, as a finished removal leaves it, so a test can
    /// put back only the piece it is about.
    #[cfg(unix)]
    fn clear_engine(site: &Path) {
        for e in std::fs::read_dir(site).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if is_voice_engine_entry(&name) || name.starts_with("setuptools") {
                std::fs::remove_dir_all(e.path()).unwrap();
            }
        }
    }

    /// What pip is asked wherever an engine piece is found: each package on its own, so a failure
    /// on one never stops the rest, and faster-whisper last.
    #[cfg(unix)]
    const VOICE_REMOVAL: &str = "\
-m pip uninstall -y --disable-pip-version-check av
-m pip uninstall -y --disable-pip-version-check ctranslate2
-m pip uninstall -y --disable-pip-version-check setuptools
-m pip uninstall -y --disable-pip-version-check faster-whisper
";

    /// After one clean-up, nothing is left for the quick look to find, so the next launch makes no
    /// pip call and changes not a file.
    #[cfg(unix)]
    fn assert_gone_for_good(mgr: &SidecarManager, dir: &Path, log: &Path) {
        assert!(!mgr.voice_leftovers_on_disk(), "something is still found");
        let _ = std::fs::remove_file(log);
        let before = tree(dir);
        mgr.retire_voice();
        assert_eq!(
            std::fs::read_to_string(log).unwrap_or_default(),
            "",
            "no pip call at the next launch"
        );
        assert_eq!(tree(dir), before, "and nothing else changes");
    }

    /// The upgrade end to end, on [`fake_voice_venv`]: pip is asked for the engine's four packages
    /// one at a time, and the markers, the Whisper folders and the recording go with them, while
    /// the document engine, the other models and the base marker stay. The next launch finds
    /// nothing: no pip call, and not a file changes.
    #[cfg(unix)]
    #[test]
    fn an_older_pms_voice_leftovers_are_removed_once() {
        let dir = tempfile::tempdir().unwrap();
        let (mgr, log, site, runtime) = fake_voice_venv(dir.path());

        mgr.retire_voice();
        assert_eq!(std::fs::read_to_string(&log).unwrap(), VOICE_REMOVAL);
        for gone in [
            "faster_whisper",
            "ctranslate2",
            "ctranslate2.libs",
            "av",
            "av.libs",
            "setuptools",
        ] {
            assert!(!site.join(gone).exists(), "{gone} is left");
        }
        let venv = runtime.join("venv");
        assert!(!venv.join(".pm-voice").exists());
        assert!(!venv.join(".pm-voice-kept").exists());
        let models = runtime.join("models");
        assert!(!models.join(WHISPER_REPO_DIR).exists());
        assert!(!models.join(".locks").join(WHISPER_REPO_DIR).exists());
        assert!(
            !runtime.join("tmp").exists(),
            "the recording, and its folder once empty"
        );
        assert!(venv.join(".pm-ready").is_file());
        for kept in [
            "fastembed/models--Qdrant--bge-small-en-v1.5/model.onnx",
            "hf/xet/chunk",
            "rapidocr/det.onnx",
        ] {
            assert!(models.join(kept).is_file(), "{kept} is not voice's");
        }
        for kept in [
            "fastembed",
            "fastembed-0.9.1.dist-info",
            "onnxruntime",
            "pip",
        ] {
            assert!(site.join(kept).is_dir(), "{kept} is not voice's");
        }
        assert_gone_for_good(&mgr, dir.path(), &log);
    }

    /// Import dirs with no dist-info, as a removal cut short leaves them: pip has nothing to
    /// uninstall them by (it would call each package not installed), so it is never run, they go by
    /// hand, and the next launch finds nothing.
    #[cfg(unix)]
    #[test]
    fn an_import_dir_pip_cannot_see_is_removed_by_hand() {
        let dir = tempfile::tempdir().unwrap();
        let (mgr, log, site, _runtime) = fake_voice_venv(dir.path());
        clear_engine(&site);
        put(site.join("ctranslate2").join("__init__.py"), 3);
        put(site.join("ctranslate2.libs").join("libct2.so"), 3);
        put(site.join("av").join("__init__.py"), 3);
        mgr.retire_voice();
        assert_eq!(std::fs::read_to_string(&log).unwrap_or_default(), "");
        for gone in ["ctranslate2", "ctranslate2.libs", "av"] {
            assert!(!site.join(gone).exists(), "{gone} is left");
        }
        assert!(site.join("fastembed").is_dir());
        assert_gone_for_good(&mgr, dir.path(), &log);
    }

    /// A dist-info with no RECORD, which pip refuses to uninstall. It is the first package asked
    /// for, and the rest are still asked for, one each; what pip refused goes by hand. setuptools
    /// went through pip, beside an engine.
    #[cfg(unix)]
    #[test]
    fn a_package_pip_refuses_never_stops_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let (mgr, log, site, _runtime) = fake_voice_venv(dir.path());
        let av_info = site.join("av-18.1.0.dist-info");
        std::fs::remove_file(av_info.join("RECORD")).unwrap();
        put(av_info.join("METADATA"), 3);
        mgr.retire_voice();
        assert_eq!(std::fs::read_to_string(&log).unwrap(), VOICE_REMOVAL);
        for gone in [
            "av",
            "av.libs",
            "av-18.1.0.dist-info",
            "ctranslate2",
            "ctranslate2-4.8.1.dist-info",
            "faster_whisper",
            "setuptools",
            "setuptools-80.9.0.dist-info",
        ] {
            assert!(!site.join(gone).exists(), "{gone} is left");
        }
        assert_gone_for_good(&mgr, dir.path(), &log);
    }

    /// A venv rebuilt in place on a newer Python keeps its old `lib/python3.11`, and pip on 3.12
    /// never looks there, so it is never run. The engine in it goes by hand, and so does what pip
    /// couldn't see in the current one (both are searched, not just the first); the old
    /// site-packages' own setuptools, and everything else in it, stay.
    #[cfg(unix)]
    #[test]
    fn an_engine_in_an_older_pythons_site_packages_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let (mgr, log, site, runtime) = fake_voice_venv(dir.path());
        clear_engine(&site);
        put(site.join("av").join("__init__.py"), 3);
        let old = runtime.join("venv/lib/python3.11/site-packages");
        for piece in [
            "ctranslate2/__init__.py",
            "ctranslate2-4.8.1.dist-info/RECORD",
            "ctranslate2.libs/libct2.so",
            "av/__init__.py",
            "av-18.1.0.dist-info/RECORD",
            "setuptools/__init__.py",
            "setuptools-65.5.0.dist-info/RECORD",
            "fastembed/__init__.py",
        ] {
            put(old.join(piece), 3);
        }
        mgr.retire_voice();
        assert_eq!(std::fs::read_to_string(&log).unwrap_or_default(), "");
        for gone in [
            "ctranslate2",
            "ctranslate2-4.8.1.dist-info",
            "ctranslate2.libs",
            "av",
            "av-18.1.0.dist-info",
        ] {
            assert!(!old.join(gone).exists(), "{gone} is left");
        }
        assert!(!site.join("av").exists(), "av is left in 3.12");
        for kept in ["setuptools", "setuptools-65.5.0.dist-info", "fastembed"] {
            assert!(old.join(kept).is_dir(), "{kept} is not removed by hand");
        }
        assert_gone_for_good(&mgr, dir.path(), &log);
    }

    /// The folders pip had moved aside when a removal of the engine was cut short go with it, and
    /// only those: another package's stash and a package whose name merely starts like one stay.
    #[cfg(unix)]
    #[test]
    fn pips_stashes_of_the_engine_go_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let (mgr, log, site, _runtime) = fake_voice_venv(dir.path());
        for piece in [
            "~v/__init__.py",
            "~v.libs/libavcodec.so",
            "~translate2-4.8.1.dist-info/RECORD",
            "~aster_whisper/__init__.py",
            // The names pip falls back on when the first is taken.
            "~av/__init__.py",
            "~-.libs/libavcodec.so",
            "~--18.1.0.dist-info/RECORD",
            "~-ranslate2/__init__.py",
            "~ip/__init__.py",
            "~vro/__init__.py",
            "~-v/__init__.py",
            "~.-av/__init__.py",
            // pip could make these of the engine's dist-infos, but only by replacing past the
            // hyphen, where any package's dist-info could have given them.
            "~---8.1.0.dist-info/RECORD",
            "~000000000000.8.1.dist-info/RECORD",
            "avro/__init__.py",
            "avro-1.12.0.dist-info/RECORD",
        ] {
            put(site.join(piece), 3);
        }
        mgr.retire_voice();
        for gone in [
            "~v",
            "~v.libs",
            "~translate2-4.8.1.dist-info",
            "~aster_whisper",
            "~av",
            "~-.libs",
            "~--18.1.0.dist-info",
            "~-ranslate2",
        ] {
            assert!(!site.join(gone).exists(), "{gone} is left");
        }
        for kept in [
            "~ip",
            "~vro",
            "~-v",
            "~.-av",
            "~---8.1.0.dist-info",
            "~000000000000.8.1.dist-info",
            "avro",
            "avro-1.12.0.dist-info",
        ] {
            assert!(site.join(kept).is_dir(), "{kept} is not the engine's");
        }
        assert_gone_for_good(&mgr, dir.path(), &log);
    }

    #[test]
    fn the_engines_entries_are_named_exactly() {
        for name in [
            "av",
            "av.libs",
            "av-18.1.0.dist-info",
            "ctranslate2",
            "ctranslate2.libs",
            "ctranslate2-4.8.1.dist-info",
            "faster_whisper",
            "faster_whisper-1.2.1.dist-info",
        ] {
            assert!(is_voice_engine_entry(name), "{name}");
            assert!(!is_voice_engine_stash(name), "{name}");
            let stash = format!("~{}", &name[1..]);
            assert!(is_voice_engine_stash(&stash), "{stash}");
            assert!(!is_voice_engine_entry(&stash), "{stash}");
        }
        for name in [
            "avro",
            "avro-1.12.0.dist-info",
            "av_utils",
            "av-18.1.0.dist-info.bak",
            "ctranslate2_extra",
            "faster_whisper_server-0.1.dist-info",
            "setuptools",
            "setuptools-80.9.0.dist-info",
            "_distutils_hack",
            "~vro",
            "~ip",
            "~etuptools",
        ] {
            assert!(!is_voice_engine_entry(name), "{name}");
            assert!(!is_voice_engine_stash(name), "{name}");
        }

        // pip's other names, tried in turn when one is taken: the first characters swapped for `~`
        // and a run of its stash characters, or a run of up to 14 before the whole name.
        let fourteen = format!("~{}av", "9".repeat(14));
        for stash in [
            "~av",
            "~-av",
            "~%%av",
            fourteen.as_str(),
            "~-.libs",
            "~--libs",
            "~--18.1.0.dist-info",
            "~av-18.1.0.dist-info",
            "~-ranslate2",
            "~09anslate2",
            "~~ctranslate2.libs",
            "~=ster_whisper-1.2.1.dist-info",
        ] {
            assert!(is_voice_engine_stash(stash), "{stash}");
        }
        // And what pip never makes of these names: a run out of its order, one too long, a name
        // of the wrong length, or one past a dist-info's hyphen with no version.
        let fifteen = format!("~{}av", "9".repeat(15));
        for name in [
            "~-v",
            "~.-av",
            fifteen.as_str(),
            "~-v.libs",
            "~=aster_whisper-1.2.1.dist-info",
            "~v-.dist-info",
            "~x",
        ] {
            assert!(!is_voice_engine_stash(name), "{name}");
        }
        // Past a dist-info's hyphen: pip does make these of `av-18.1.0.dist-info` (its first four
        // characters replaced) and `ctranslate2-4.8.1.dist-info` (thirteen), but what is left
        // names no package, so they are never taken for the engine's.
        for (stash, of) in [
            ("~---8.1.0.dist-info", "av-18.1.0.dist-info"),
            ("~000000000000.8.1.dist-info", "ctranslate2-4.8.1.dist-info"),
        ] {
            assert!(is_pip_stash_name(stash, of), "pip makes {stash} of {of}");
            assert!(!is_voice_engine_stash(stash), "{stash}");
        }
    }

    /// setuptools alone is no engine: a venv built on Python 3.11 has its own from `venv`. With no
    /// engine piece in site-packages pip is never called, so that copy stays, while what voice left
    /// outside site-packages (here the Whisper model) still goes.
    #[cfg(unix)]
    #[test]
    fn a_venvs_own_setuptools_is_never_removed() {
        let dir = tempfile::tempdir().unwrap();
        let (mgr, log, site, runtime) = fake_voice_venv(dir.path());
        clear_engine(&site);
        put(site.join("setuptools").join("__init__.py"), 3);
        put(site.join("setuptools-65.5.0.dist-info").join("RECORD"), 3);
        mgr.retire_voice();
        assert_eq!(std::fs::read_to_string(&log).unwrap_or_default(), "");
        assert!(site.join("setuptools").is_dir());
        assert!(site.join("setuptools-65.5.0.dist-info").is_dir());
        assert!(!runtime.join("models").join(WHISPER_REPO_DIR).exists());
        assert!(!runtime.join("venv").join(".pm-voice").exists());
        assert_gone_for_good(&mgr, dir.path(), &log);
    }

    /// An engine someone chose to keep when they switched voice off (`.pm-voice-kept`) goes too:
    /// that choice was about a switch that no longer exists.
    #[cfg(unix)]
    #[test]
    fn a_kept_engine_is_removed_too() {
        let dir = tempfile::tempdir().unwrap();
        let (mgr, log, _site, runtime) = fake_voice_venv(dir.path());
        std::fs::remove_file(runtime.join("venv").join(".pm-voice")).unwrap();
        mgr.retire_voice();
        assert_eq!(std::fs::read_to_string(&log).unwrap(), VOICE_REMOVAL);
        assert!(!runtime.join("venv").join(".pm-voice-kept").exists());
    }

    /// A `runtime/tmp` or `runtime/models` (or its `.locks`) that is a link leads out of PM's data
    /// dir, so nothing is looked for or removed through one, even while the clean-up runs in full
    /// for the real leftovers beside it; and with only links left, the quick look finds nothing.
    #[cfg(unix)]
    #[test]
    fn the_clean_up_never_follows_a_link_out_of_the_data_dir() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let (mgr, log, _site, runtime) = fake_voice_venv(dir.path());
        let outside = dir.path().join("outside");
        let clip = outside.join("tmp").join("voice-x.webm");
        let model = outside
            .join("models")
            .join(WHISPER_REPO_DIR)
            .join("model.bin");
        put(clip.clone(), 3);
        put(model.clone(), 3);
        std::fs::remove_dir_all(runtime.join("tmp")).unwrap();
        std::fs::remove_dir_all(runtime.join("models")).unwrap();
        symlink(outside.join("tmp"), runtime.join("tmp")).unwrap();
        symlink(outside.join("models"), runtime.join("models")).unwrap();

        mgr.retire_voice();
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            VOICE_REMOVAL,
            "the real engine still goes"
        );
        assert!(!runtime.join("venv").join(".pm-voice").exists());
        assert!(clip.is_file(), "a recording through a link stays");
        assert!(model.is_file(), "a model through a link stays");
        assert!(runtime.join("tmp").is_symlink() && runtime.join("models").is_symlink());
        assert_gone_for_good(&mgr, dir.path(), &log);

        // A real `models` whose `.locks` is a link, and a `tmp` that links to an empty folder:
        // neither is voice's to clear, so there is nothing to find.
        let dir = tempfile::tempdir().unwrap();
        let runtime = dir.path().join("runtime");
        let outside = dir.path().join("outside");
        let lock = outside.join("locks").join(WHISPER_REPO_DIR).join("x.lock");
        put(lock.clone(), 1);
        std::fs::create_dir_all(outside.join("empty")).unwrap();
        std::fs::create_dir_all(runtime.join("models")).unwrap();
        symlink(outside.join("locks"), runtime.join("models").join(".locks")).unwrap();
        symlink(outside.join("empty"), runtime.join("tmp")).unwrap();
        let mgr = SidecarManager::new(SidecarPaths {
            source_dir: dir.path().join("sidecar"),
            venv_dir: runtime.join("venv"),
        });
        assert!(!mgr.voice_leftovers_on_disk());
        let before = tree(dir.path());
        mgr.retire_voice();
        assert_eq!(tree(dir.path()), before);
        assert!(lock.is_file() && outside.join("empty").is_dir());
    }

    /// Take away everything voice left outside site-packages (the markers, the Whisper folders and
    /// the recording), so a test's only leftover is the one it puts in site-packages.
    #[cfg(unix)]
    fn clear_outside_site(runtime: &Path) {
        for path in [
            runtime.join("venv/.pm-voice"),
            runtime.join("venv/.pm-voice-kept"),
            runtime.join("models").join(WHISPER_REPO_DIR),
            runtime.join("models/.locks").join(WHISPER_REPO_DIR),
            runtime.join("tmp"),
        ] {
            remove_voice_leftover(&path);
        }
    }

    /// The venv, or any folder on the way down to its site-packages, that is a link leads out of
    /// PM's data dir: nothing through it is counted, pip is never run on it, and nothing in the
    /// folder it points to is touched, the voice marker beside a linked venv's engine included.
    #[cfg(unix)]
    #[test]
    fn a_linked_venv_or_site_packages_is_never_followed() {
        use std::os::unix::fs::symlink;
        for (linked, minor) in [
            ("venv", 12),
            ("venv/lib", 12),
            ("venv/lib/python3.11", 11),
            ("venv/lib/python3.12/site-packages", 12),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let runtime = dir.path().join("runtime");
            let log = dir.path().join("pip.log");
            // A whole venv outside, with the engine where its interpreter's site-packages is.
            let outside = dir.path().join("outside");
            fake_python(&outside.join("venv"), &log, minor);
            let site = outside
                .join("venv/lib")
                .join(format!("python3.{minor}"))
                .join("site-packages");
            put(site.join("av").join("__init__.py"), 3);
            put(site.join("ctranslate2-4.8.1.dist-info").join("RECORD"), 3);
            put(outside.join("venv/.pm-voice"), 1);
            // PM's own folders down to the link, its interpreter among them.
            if linked != "venv" {
                fake_python(&runtime.join("venv"), &log, minor);
            }
            let link = runtime.join(linked);
            std::fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink(outside.join(linked), &link).unwrap();

            let mgr = SidecarManager::new(SidecarPaths {
                source_dir: dir.path().join("sidecar"),
                venv_dir: runtime.join("venv"),
            });
            let before = tree(&outside);
            assert!(!mgr.voice_leftovers_on_disk(), "{linked}: found through it");
            mgr.retire_voice();
            assert_eq!(
                std::fs::read_to_string(&log).unwrap_or_default(),
                "",
                "{linked}: pip ran"
            );
            assert_eq!(
                tree(&outside),
                before,
                "{linked}: changed what it points to"
            );

            // With a real leftover beside it the whole clean-up runs, and still reaches nothing
            // through the link.
            let whisper = runtime.join("models").join(WHISPER_REPO_DIR);
            put(whisper.join("model.bin"), 3);
            mgr.retire_voice();
            assert!(!whisper.exists(), "{linked}: the real leftover stays");
            assert_eq!(
                std::fs::read_to_string(&log).unwrap_or_default(),
                "",
                "{linked}: pip ran in the full clean-up"
            );
            assert_eq!(
                tree(&outside),
                before,
                "{linked}: the full clean-up changed what it points to"
            );
        }
    }

    /// The interpreter's own `lib/python3.11` is a link to a folder outside holding an engine pip
    /// could remove: pip is never run through it, and that folder is left as it was, while the
    /// real engine beside it in `lib/python3.12` still goes by hand.
    #[cfg(unix)]
    #[test]
    fn pip_never_runs_on_a_linked_site_packages_of_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = dir.path().join("runtime");
        let venv = runtime.join("venv");
        let log = dir.path().join("pip.log");
        fake_python(&venv, &log, 11);
        let outside = dir.path().join("outside");
        put(outside.join("site-packages/av/__init__.py"), 3);
        put(outside.join("site-packages/av-18.1.0.dist-info/RECORD"), 3);
        std::fs::create_dir_all(venv.join("lib")).unwrap();
        std::os::unix::fs::symlink(&outside, venv.join("lib/python3.11")).unwrap();
        let real = venv.join("lib/python3.12/site-packages/av");
        put(real.join("__init__.py"), 3);
        let mgr = SidecarManager::new(SidecarPaths {
            source_dir: dir.path().join("sidecar"),
            venv_dir: venv,
        });

        let before = tree(&outside);
        mgr.retire_voice();
        assert_eq!(std::fs::read_to_string(&log).unwrap_or_default(), "");
        assert_eq!(tree(&outside), before, "the linked folder changed");
        assert!(!real.exists(), "the real engine is left");
        assert_gone_for_good(&mgr, dir.path(), &log);
    }

    /// A real removal leaves what else is in `runtime/tmp` and in huggingface_hub's lock folder:
    /// only voice's recording and the Whisper model's lock go, and a `tmp` that still holds
    /// something stays.
    #[cfg(unix)]
    #[test]
    fn what_else_is_in_tmp_and_the_locks_stays() {
        let dir = tempfile::tempdir().unwrap();
        let (mgr, log, _site, runtime) = fake_voice_venv(dir.path());
        let other = runtime.join("tmp").join("other.bin");
        let lock = runtime.join("models/.locks/models--Qdrant--bge-small-en-v1.5/x.lock");
        put(other.clone(), 1);
        put(lock.clone(), 1);
        mgr.retire_voice();
        assert!(!runtime.join("tmp").join("voice-a1b2c3.webm").exists());
        assert!(!runtime
            .join("models/.locks")
            .join(WHISPER_REPO_DIR)
            .exists());
        assert!(other.is_file(), "tmp keeps what isn't voice's");
        assert!(lock.is_file(), "the locks keep the search model's");
        assert_gone_for_good(&mgr, dir.path(), &log);
    }

    /// A venv on Python 3.11 with the engine only in a stale `lib/python3.12` beside its own
    /// folder: pip on 3.11 has nothing of the engine's to remove, so it is never run and never
    /// takes the venv's own setuptools. The stale engine goes by hand, its setuptools with it never.
    #[cfg(unix)]
    #[test]
    fn an_engine_beside_the_interpreters_own_folder_never_has_pip_take_its_setuptools() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = dir.path().join("runtime");
        let venv = runtime.join("venv");
        let log = dir.path().join("pip.log");
        fake_python(&venv, &log, 11);
        let own = venv.join("lib/python3.11/site-packages");
        let stale = venv.join("lib/python3.12/site-packages");
        for piece in [
            "setuptools/__init__.py",
            "setuptools-65.5.0.dist-info/RECORD",
            "fastembed/__init__.py",
        ] {
            put(own.join(piece), 3);
        }
        for piece in [
            "av/__init__.py",
            "av-18.1.0.dist-info/RECORD",
            "ctranslate2/__init__.py",
            "ctranslate2-4.8.1.dist-info/RECORD",
            "faster_whisper/__init__.py",
            "faster_whisper-1.2.1.dist-info/RECORD",
            "setuptools/__init__.py",
            "setuptools-80.9.0.dist-info/RECORD",
        ] {
            put(stale.join(piece), 3);
        }
        let mgr = SidecarManager::new(SidecarPaths {
            source_dir: dir.path().join("sidecar"),
            venv_dir: venv,
        });
        mgr.retire_voice();
        assert_eq!(std::fs::read_to_string(&log).unwrap_or_default(), "");
        for kept in ["setuptools", "setuptools-65.5.0.dist-info", "fastembed"] {
            assert!(own.join(kept).is_dir(), "{kept} is the venv's own");
        }
        for gone in [
            "av",
            "av-18.1.0.dist-info",
            "ctranslate2",
            "ctranslate2-4.8.1.dist-info",
            "faster_whisper",
            "faster_whisper-1.2.1.dist-info",
        ] {
            assert!(!stale.join(gone).exists(), "{gone} is left");
        }
        assert!(stale.join("setuptools").is_dir(), "never removed by hand");
        assert_gone_for_good(&mgr, dir.path(), &log);
    }

    /// The engine's only dist-info in the interpreter's own site-packages has no RECORD, so pip
    /// would refuse it: pip is never run (setuptools stays), and the package goes by hand.
    #[cfg(unix)]
    #[test]
    fn a_dist_info_pip_would_refuse_is_removed_without_pip() {
        let dir = tempfile::tempdir().unwrap();
        let (mgr, log, site, _runtime) = fake_voice_venv(dir.path());
        clear_engine(&site);
        put(site.join("av").join("__init__.py"), 3);
        put(site.join("av-18.1.0.dist-info").join("METADATA"), 3);
        put(site.join("setuptools").join("__init__.py"), 3);
        put(site.join("setuptools-80.9.0.dist-info").join("RECORD"), 3);
        mgr.retire_voice();
        assert_eq!(std::fs::read_to_string(&log).unwrap_or_default(), "");
        assert!(!site.join("av").exists());
        assert!(!site.join("av-18.1.0.dist-info").exists());
        assert!(site.join("setuptools").is_dir(), "pip never ran");
        assert_gone_for_good(&mgr, dir.path(), &log);
    }

    /// A stash pip left after the engine's last live entry was gone is a leftover on its own: the
    /// quick look finds it and it goes by hand, with no pip call. Another package's stash stays.
    #[cfg(unix)]
    #[test]
    fn a_stash_alone_is_removed_without_pip() {
        let dir = tempfile::tempdir().unwrap();
        let (mgr, log, site, runtime) = fake_voice_venv(dir.path());
        clear_engine(&site);
        clear_outside_site(&runtime);
        put(site.join("~v").join("__init__.py"), 3);
        put(site.join("~-.libs").join("libavcodec.so"), 3);
        put(
            site.join("~=ster_whisper-1.2.1.dist-info").join("RECORD"),
            3,
        );
        put(site.join("~-v").join("__init__.py"), 3);
        assert!(mgr.voice_leftovers_on_disk(), "a stash is found");
        mgr.retire_voice();
        assert_eq!(std::fs::read_to_string(&log).unwrap_or_default(), "");
        for gone in ["~v", "~-.libs", "~=ster_whisper-1.2.1.dist-info"] {
            assert!(!site.join(gone).exists(), "{gone} is left");
        }
        assert!(
            site.join("~-v").is_dir(),
            "pip never makes `~-v` of the engine"
        );
        assert_gone_for_good(&mgr, dir.path(), &log);
    }

    /// The engine in a venv whose interpreter is gone: no pip to run, so it all goes by hand, and
    /// setuptools stays.
    #[cfg(unix)]
    #[test]
    fn without_an_interpreter_the_engine_still_goes_by_hand() {
        let dir = tempfile::tempdir().unwrap();
        let (mgr, log, site, runtime) = fake_voice_venv(dir.path());
        std::fs::remove_file(runtime.join("venv/bin/python")).unwrap();
        mgr.retire_voice();
        assert_eq!(std::fs::read_to_string(&log).unwrap_or_default(), "");
        for gone in [
            "faster_whisper",
            "faster_whisper-1.2.1.dist-info",
            "ctranslate2",
            "ctranslate2.libs",
            "av",
            "av.libs",
            "av-18.1.0.dist-info",
        ] {
            assert!(!site.join(gone).exists(), "{gone} is left");
        }
        assert!(site.join("setuptools").is_dir(), "never removed by hand");
        assert_gone_for_good(&mgr, dir.path(), &log);
    }

    /// The shared optional-component flow end to end, on photo OCR and [`fake_python`]. An install
    /// over a current base venv first removes what the component's lock superseded, then installs
    /// the lock with the flags every lock install takes, and stamps the marker; a second install
    /// finds it current and runs nothing; a removal drops the marker and uninstalls exactly the
    /// component's own packages.
    #[cfg(unix)]
    #[test]
    fn an_optional_component_installs_once_and_removes_its_own_packages() {
        let dir = tempfile::tempdir().unwrap();
        let source_dir = dir.path().join("sidecar");
        std::fs::create_dir_all(&source_dir).unwrap();
        // Only their existence and hashes matter here.
        std::fs::write(
            source_dir.join("requirements.lock"),
            b"base==0 \\\n    --hash=sha256:aaa\n",
        )
        .unwrap();
        let ocr_lock = source_dir.join(OPTIONAL_OCR_COMPONENT.lock);
        std::fs::write(&ocr_lock, b"rapidocr==0 \\\n    --hash=sha256:bbb\n").unwrap();
        let venv = dir.path().join("runtime").join("venv");
        let log = dir.path().join("pip.log");
        fake_python(&venv, &log, 12);
        // The base venv as provision leaves it: its marker holds the base lock's hash.
        let base = crate::ingest::hex_digest(
            &std::fs::read(source_dir.join("requirements.lock")).unwrap(),
        );
        std::fs::write(venv.join(".pm-ready"), base).unwrap();
        let mgr = SidecarManager::new(SidecarPaths {
            source_dir,
            venv_dir: venv.clone(),
        });
        let logged = || std::fs::read_to_string(&log).unwrap_or_default();

        assert!(!mgr.optional_ocr_ready());
        mgr.install_optional_ocr(|_| {}).unwrap();
        let lock_arg = ocr_lock.to_string_lossy().into_owned();
        let mut install = vec!["-m", "pip", "install"];
        install.extend(LOCK_INSTALL_FLAGS);
        install.extend(["--progress-bar", "off", "-r", &lock_arg]);
        assert_eq!(
            logged(),
            format!(
                "-m pip uninstall -y --disable-pip-version-check opencv-python\n{}\n",
                install.join(" ")
            ),
            "the superseded package first, then the lock"
        );
        assert!(mgr.optional_ocr_ready());

        std::fs::remove_file(&log).unwrap();
        mgr.install_optional_ocr(|_| {}).unwrap();
        assert_eq!(logged(), "", "a current component installs nothing");

        mgr.uninstall_optional_ocr().unwrap();
        assert!(!venv.join(".pm-ocr").exists());
        assert!(!mgr.optional_ocr_ready());
        assert_eq!(
            logged(),
            format!(
                "-m pip uninstall -y --disable-pip-version-check {}\n",
                OPTIONAL_OCR_COMPONENT.uninstall.join(" ")
            )
        );
    }

    /// A computer that never had voice, or a fresh install with no venv yet: nothing is there, so
    /// the launch returns at once, without pip and without creating or removing anything.
    #[test]
    fn nothing_to_retire_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = dir.path().join("runtime");
        let mgr = SidecarManager::new(SidecarPaths {
            source_dir: dir.path().join("sidecar"),
            venv_dir: runtime.join("venv"),
        });
        assert!(!mgr.voice_leftovers_on_disk());
        mgr.retire_voice();
        assert!(!runtime.exists());

        // A real venv and models without any of voice's, and a `runtime/tmp` holding something
        // else: still nothing to do, and that file is left alone.
        let site = if cfg!(windows) {
            runtime.join("venv").join("Lib").join("site-packages")
        } else {
            runtime.join("venv/lib/python3.12/site-packages")
        };
        std::fs::create_dir_all(site.join("fastembed")).unwrap();
        std::fs::create_dir_all(site.join("avro")).unwrap();
        std::fs::create_dir_all(site.join("avro-1.12.0.dist-info")).unwrap();
        std::fs::create_dir_all(runtime.join("models/fastembed")).unwrap();
        std::fs::create_dir_all(runtime.join("tmp")).unwrap();
        std::fs::write(runtime.join("tmp/other.bin"), b"x").unwrap();
        assert!(!mgr.voice_leftovers_on_disk(), "avro is not av");
        mgr.retire_voice();
        assert!(runtime.join("tmp/other.bin").is_file());
    }

    /// Every sidecar file the app READS at runtime must be bundled, in BOTH manifests. The resource
    /// map is an explicit allow-list — a file is shipped by being named there and by nothing else —
    /// and `source_dir` is the repo's own `sidecar/` in dev, so a file missing from the manifests
    /// works perfectly here and is simply absent on a user's machine. That is the same shape as the
    /// vault-walk bug this codebase has shipped twice; the locks are new files, so they need it.
    #[test]
    fn every_sidecar_file_the_app_reads_is_bundled() {
        let mut expected = vec![
            "../sidecar/pm_sidecar.py".to_string(),
            "../sidecar/requirements.lock".to_string(),
        ];
        expected.extend(
            ALL_OPTIONAL_COMPONENTS
                .iter()
                .map(|c| format!("../sidecar/{}", c.lock)),
        );

        for manifest in ["tauri.conf.json", "tauri.linux.conf.json"] {
            let path = format!("{}/{manifest}", env!("CARGO_MANIFEST_DIR"));
            let json: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(&path).expect("the bundle manifest must exist"),
            )
            .expect("the bundle manifest must be valid JSON");
            let resources = json["bundle"]["resources"]
                .as_object()
                .unwrap_or_else(|| panic!("{manifest} has no bundle.resources map"));
            for key in &expected {
                assert!(
                    resources.contains_key(key),
                    "{manifest} does not bundle {key} - the installed app would not have that file",
                );
            }
        }
    }

    #[test]
    fn input_size_guard_rejects_over_cap_files() {
        // F-57: exactly at the cap is fine; one byte over is refused, so a 500 MB file is pre-flighted
        // out before it can balloon the Python child's memory.
        assert!(check_input_size(0, MAX_SIDECAR_INPUT_BYTES).is_ok());
        assert!(check_input_size(MAX_SIDECAR_INPUT_BYTES, MAX_SIDECAR_INPUT_BYTES).is_ok());
        assert!(check_input_size(MAX_SIDECAR_INPUT_BYTES + 1, MAX_SIDECAR_INPUT_BYTES).is_err());
    }

    #[test]
    fn the_text_family_cap_refuses_work_that_could_only_fail() {
        // A text-family file converts to roughly ITSELF, so a 60-128 MiB one cleared the 128 MiB
        // input cap and then produced a reply that could never fit under the 64 MiB line cap: a
        // guaranteed failure, after minutes of conversion, costing a child kill + respawn. The
        // guard's own promise is that oversized work is "refused before the child is even asked".
        assert_eq!(
            input_cap_for(Path::new("notes.txt")),
            MAX_SIDECAR_TEXT_INPUT_BYTES
        );
        assert!(check_input_size(60 * 1024 * 1024, input_cap_for(Path::new("big.md"))).is_err());

        // Case-insensitive, like every other extension check in the ingest path.
        assert_eq!(
            input_cap_for(Path::new("PAGE.HTML")),
            MAX_SIDECAR_TEXT_INPUT_BYTES
        );

        // A container format extracts to far LESS text than it occupies, so it keeps the full
        // allowance — capping those at 40 MiB would refuse ordinary scanned PDFs.
        assert_eq!(
            input_cap_for(Path::new("book.pdf")),
            MAX_SIDECAR_INPUT_BYTES
        );
        assert!(check_input_size(60 * 1024 * 1024, input_cap_for(Path::new("book.pdf"))).is_ok());

        // No extension, or one we don't classify: the generous cap. This guard exists to stop a
        // KNOWN-futile conversion, not to second-guess files it can't identify.
        assert_eq!(input_cap_for(Path::new("README")), MAX_SIDECAR_INPUT_BYTES);
    }

    #[test]
    fn the_text_cap_leaves_real_headroom_under_the_reply_cap() {
        // The reply is Markdown-wrapped and JSON-escaped, so it runs LARGER than the source. If the
        // text cap ever crept up to the reply cap this guard would go back to merely moving the
        // cliff rather than removing it.
        assert!(
            MAX_SIDECAR_TEXT_INPUT_BYTES < MAX_SIDECAR_LINE as u64,
            "a text file that clears the input cap must be able to fit in one reply line"
        );
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
        assert_eq!(secs("analyze_image"), 30 * 60);
        assert!(secs("count_tokens") < secs("embed"));
        assert!(secs("count_tokens") > 0);
        assert!(
            secs("something_new") > 0,
            "unknown methods get a safe default"
        );
    }

    #[test]
    fn only_the_formats_that_state_an_author_are_asked() {
        // The gate that keeps a folder of plain-text notes from paying a sidecar round trip per
        // file to be told nothing. Extension match is case-insensitive: Windows hands back
        // whatever case the file was created with.
        for name in [
            "plan.docx",
            "deck.PPTX",
            "budget.xlsx",
            "macro.xlsm",
            "scan.pdf",
        ] {
            assert!(carries_document_properties(Path::new(name)), "{name}");
        }
        for name in [
            "notes.md",
            "log.txt",
            "page.html",
            "rows.csv",
            "README",
            "photo.jpg",
        ] {
            assert!(!carries_document_properties(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn a_document_that_did_not_say_is_told_apart_from_one_that_said_nothing() {
        // Both halves of the wire contract with `pm_sidecar.py`. A null is the document declining to
        // state a fact; an empty string is Word's `<dc:creator/>`, which means the same thing — and
        // an empty author rendered under "Author" reads as PM having lost the value.
        assert_eq!(
            file_properties_from_reply(&json!({
                "author": "Jane Okafor",
                "last_modified_by": "  Sam Reyes  ",
                "created": "2025-11-04T09:12:00Z",
            })),
            FileProperties {
                author: Some("Jane Okafor".into()),
                last_modified_by: Some("Sam Reyes".into()),
                created_at: Some("2025-11-04T09:12:00Z".into()),
            }
        );
        for blank in [json!(null), json!(""), json!("   ")] {
            let props = file_properties_from_reply(
                &json!({ "author": blank, "last_modified_by": blank, "created": blank }),
            );
            assert_eq!(props, FileProperties::default(), "{blank}");
        }
        // A reply missing the keys altogether — an older worker, or one that gave up — is the same
        // answer, not a panic.
        assert_eq!(
            file_properties_from_reply(&json!({})),
            FileProperties::default()
        );
    }

    #[test]
    fn a_property_read_is_never_the_reason_a_file_fails_to_land() {
        // `file_properties` returns a value, not a Result, so no call site can make it fatal by
        // accident — a document's author is never worth failing an ingest over. Coercing it to this
        // exact fn pointer IS the assertion: give it a fallible return and this stops compiling.
        let _: fn(&SidecarManager, &Path) -> FileProperties = SidecarManager::file_properties;
    }

    #[test]
    fn model_not_cached_is_told_apart_from_a_real_failure() {
        // The offline worker's "please fetch this model" signal — the only reply that triggers a
        // fetch-and-retry (issue #286).
        assert!(is_model_not_cached(&json!({
            "ok": false, "error_kind": "model_not_cached", "error": "not in cache"
        })));
        // A genuine error is NOT a fetch trigger, even though it also has ok:false.
        assert!(!is_model_not_cached(&json!({
            "ok": false, "error": "onnx blew up"
        })));
        // A success is never a fetch trigger, whatever else it carries.
        assert!(!is_model_not_cached(&json!({ "ok": true, "result": {} })));
        // A stray error_kind on a successful reply must not trigger a pointless refetch.
        assert!(!is_model_not_cached(&json!({
            "ok": true, "error_kind": "model_not_cached"
        })));
    }

    #[test]
    fn the_fetcher_is_handed_the_model_and_nothing_of_the_users() {
        // The `--fetch` helper is the ONE child that keeps a socket (issue #286). Of an embed,
        // count_tokens or rerank request it gets the model id and a custom model's spec, which is
        // all it reads; the texts, the query and the passages stay behind.
        let spec = json!({ "hf": "someone/custom-embedder", "dim": 384 });
        let embed = json!({
            "model": "custom:abc", "custom": spec, "texts": ["a", "b"], "batch_size": 8
        });
        assert_eq!(
            fetch_params("embed", &embed),
            json!({ "model": "custom:abc", "custom": spec })
        );
        let count = json!({ "model": "bge-small", "texts": ["a private note"] });
        assert_eq!(
            fetch_params("count_tokens", &count),
            json!({ "model": "bge-small" })
        );
        let rerank = json!({ "model": "bge-reranker", "query": "q", "passages": ["p"] });
        assert_eq!(
            fetch_params("rerank", &rerank),
            json!({ "model": "bge-reranker" })
        );
        // Neither key is invented when the request lacks it: the helper's own default stands.
        assert_eq!(fetch_params("embed", &json!({ "texts": ["a"] })), json!({}));

        // analyze_image names no model, and its params carry the path of the image mid-ingest:
        // all the fetcher does is build the OCR engine. A method with nothing to fetch gets
        // nothing either.
        let image = json!({ "path": "/tmp/sandbox-in/receipt.png", "run_ocr": true });
        assert_eq!(fetch_params("analyze_image", &image), Value::Null);
        assert_eq!(
            fetch_params("convert", &json!({ "path": "/tmp/sandbox-in/a.pdf" })),
            Value::Null
        );

        for (method, params) in [
            ("embed", &embed),
            ("count_tokens", &count),
            ("rerank", &rerank),
            ("analyze_image", &image),
        ] {
            let handed = fetch_params(method, params);
            for key in ["texts", "query", "passages", "path"] {
                assert!(handed.get(key).is_none(), "{method} hands over {key}");
            }
        }
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
    fn appimage_detection_needs_a_nonempty_env_var() {
        use super::running_from_appimage;
        assert!(running_from_appimage(Some("/tmp/.mount_PMx1y2"), None));
        assert!(running_from_appimage(None, Some("/home/bobby/PM.AppImage")));
        assert!(!running_from_appimage(None, None), "rpm/dev install");
        // An empty var (e.g. exported blank by a wrapper script) is not an AppImage.
        assert!(!running_from_appimage(Some(""), Some("")));
    }

    #[test]
    fn stable_copy_stamp_comparison_treats_missing_or_empty_as_stale() {
        use super::stable_copy_current;
        let stamp = "3.12.13+20260610 c218f50b";
        assert!(stable_copy_current(Some(stamp), Some(stamp)));
        assert!(stable_copy_current(
            Some(stamp),
            Some("3.12.13+20260610 c218f50b\n")
        ));
        // Pin bump → re-copy.
        assert!(!stable_copy_current(
            Some("3.12.14+20270101 abc"),
            Some(stamp)
        ));
        // Half-finished or absent copies always redo.
        assert!(!stable_copy_current(Some(stamp), None));
        assert!(!stable_copy_current(None, Some(stamp)));
        assert!(!stable_copy_current(Some(""), Some("")));
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
        assert!(meets_min((3, 11)));
        assert!(meets_min((3, 14)));
        assert!(meets_min((4, 0)));
        // pillow 13 has no 3.10 wheels and no 3.10 in its Requires-Python.
        assert!(!meets_min((3, 10)));
        assert!(!meets_min((3, 9)));
        assert!(!meets_min((2, 7)));
    }

    #[test]
    fn every_lock_install_is_hash_checked_and_resolves_nothing() {
        // `--require-hashes`: an artifact whose digest is not in the lock is refused. `--no-deps`:
        // pip must not re-resolve: whenever the base lock overrides a dependency's bound (as it did
        // fastembed 0.9.0's `pillow<13`), pip's resolver refuses it (ResolutionImpossible) and
        // every install would fail.
        assert!(LOCK_INSTALL_FLAGS.contains(&"--require-hashes"));
        assert!(LOCK_INSTALL_FLAGS.contains(&"--no-deps"));
    }

    #[test]
    fn python_guidance_names_the_floor_pm_enforces() {
        // The messages used to spell the version out, so raising MIN_PYTHON would have left them
        // asking for a Python that PM then turns down.
        assert!(missing_message().contains("needs Python 3.11 or newer"));
        let too_old = too_old_message();
        assert!(too_old.contains("older than 3.11"));
        assert!(too_old.contains("Install Python 3.11 or newer"));
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
        // A venv an older PM built on 3.10, which the floor accepted until pillow 13: rebuilt once.
        assert!(should_rebuild_venv(true, true, Some((3, 10))));
        // Venv + marker + new enough → keep it.
        assert!(!should_rebuild_venv(true, true, Some((3, 11))));
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
        // What a 3.10 interpreter meets now: pillow 13 publishes no wheel it can install.
        assert_eq!(
            classify_pip_failure("ERROR: No matching distribution found for pillow==13.0.0"),
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
        // 3.10 is listed and dropped: it is below the floor now, like 3.9.
        assert_eq!(pick_framework_versions(&names), vec![(3, 12), (3, 11)]);

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

    /// The Stage-4 learned-reranker seam: a disk-resident model must carry its directory across to
    /// the sidecar as `local_path`, and must NOT name a real hub repo — if `specific_model_path`
    /// ever stopped short-circuiting, a placeholder 404s loudly instead of silently downloading
    /// somebody else's weights. Before this, `custom_spec` returned `None` for a local model, so
    /// one could be described in the registry and never loaded.
    #[test]
    fn a_local_path_model_carries_its_directory_to_the_sidecar() {
        use crate::registry::{Pooling, Role, Runtime, Source};
        use std::path::PathBuf;

        let entry = ModelEntry {
            id: "pm-local-reranker",
            role: Role::Reranker,
            dimension: 0,
            max_tokens: 512,
            tokenizer: "pm-local-reranker",
            runtime: Runtime::OnnxFastembed,
            source: Source::LocalPath(PathBuf::from("/models/reranker")),
            pooling: Pooling::None,
            query_prefix: None,
            passage_prefix: None,
            normalize: false,
            model_file: Some("model.onnx"),
            multilingual: false,
            label: "Local reranker",
        };

        let spec = custom_spec(&entry).expect("a local model still produces a registration spec");
        assert_eq!(spec["local_path"], "/models/reranker");
        assert_eq!(spec["model_file"], "model.onnx");
        assert_eq!(
            spec["hf"], LOCAL_MODEL_SOURCE,
            "registered under the deliberate placeholder, never a real repo"
        );
    }

    /// A hub model carries no `local_path`, so the sidecar passes no `specific_model_path` and the
    /// normal download path is untouched.
    #[test]
    fn a_hub_model_carries_no_local_path() {
        let entry = crate::registry::active_embedder();
        if let Some(spec) = custom_spec(&entry) {
            assert!(
                spec["local_path"].is_null(),
                "a hub model must not claim a disk location"
            );
        }
    }

    #[test]
    fn thread_budget_is_half_the_cores_inside_a_hard_clamp() {
        assert_eq!(sidecar_thread_budget(Some(24)), 8, "clamped at the ceiling");
        assert_eq!(sidecar_thread_budget(Some(16)), 8, "exactly the ceiling");
        assert_eq!(sidecar_thread_budget(Some(12)), 6);
        assert_eq!(sidecar_thread_budget(Some(8)), 4);
        assert_eq!(sidecar_thread_budget(Some(4)), 2);
        // A single-core VM must still get two, or a pool of one serializes the whole pass.
        assert_eq!(sidecar_thread_budget(Some(1)), 2, "clamped at the floor");
        assert_eq!(
            sidecar_thread_budget(None),
            2,
            "unknown core count is treated as small"
        );
    }

    #[test]
    fn the_worker_never_inherits_an_unbounded_thread_pool() {
        // Three native pools inside the sidecar each size themselves to the core count unless told
        // otherwise, so this variable is what stops a 24-core box spawning ~94 threads for one
        // embedding pass. The sidecar re-derives the same number when it is absent, but a worker
        // that never receives it would be relying on that fallback rather than on Rust's answer.
        let budget =
            sidecar_thread_budget(std::thread::available_parallelism().map(|n| n.get()).ok());
        assert!(
            (2..=8).contains(&budget),
            "thread budget {budget} escaped the clamp on this machine"
        );
    }
}
