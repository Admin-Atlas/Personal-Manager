// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Find local models that are **on disk but not currently served** (#449).
//!
//! The Workbench's "installed" list is derived from the configured endpoint's `/v1/models`, so a
//! model you downloaded but haven't loaded is invisible to the fit scorer. This module walks the
//! places the runners PM supports actually keep their weights, so the Workbench can say "you already
//! have this, and here is how it fits" without you loading it first.
//!
//! **Read-only, and only metadata.** Filenames, byte sizes, and — for Ollama — the small JSON
//! manifest and config that sit beside the weights. No model file is ever opened for its contents,
//! nothing is written (notably: LM Studio's own resolver creates its home-pointer file as a side
//! effect of resolving it; we never do), and nothing leaves the machine.
//!
//! Each runner is walked the way its own tooling does, not with a blind recursive sweep:
//!
//! | Source | Where | How |
//! |---|---|---|
//! | Ollama | `$OLLAMA_MODELS`, else `~/.ollama/models` (+ the Linux service home) | parse `manifests/*/*/*/*` — the JSON carries exact byte sizes, so the multi-GB blobs are never touched |
//! | Hugging Face | `$HF_HUB_CACHE`/`$HF_HOME`/`$XDG_CACHE_HOME`, else `~/.cache/huggingface/hub` | `models--*/snapshots/<rev>/**.gguf` |
//! | LM Studio | `~/.lmstudio-home-pointer` → `~/.cache/lm-studio` → `~/.lmstudio`, then `settings.json`'s `downloadsFolder` | `<publisher>/<repo>/*.gguf` |
//! | A folder you choose | the `local_model_scan_dir` setting | bounded walk for `*.gguf` |
//!
//! Everything is best-effort in the same way [`crate::hardware`] is: a missing directory, an
//! unreadable file, a half-written manifest or a permission denial skips that one entry and the scan
//! carries on. The scan is bounded by [`MAX_MODELS`] and [`MAX_WALK_DEPTH`] so it can't wander into a
//! junctioned media drive, and it reports when it hit those limits rather than implying it saw
//! everything.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Stop after this many distinct models. Far above any real install; a backstop against a pathological
/// tree, not a product limit.
const MAX_MODELS: usize = 512;
/// How deep to descend below a source's root. HF snapshots nest two levels (a per-quant subdirectory),
/// LM Studio exactly two; the rest is headroom for a user-chosen folder.
const MAX_WALK_DEPTH: usize = 6;
/// Directory names that are never worth descending into — huge, and never holding an enumerable model.
/// `blobs`/`.locks` in the HF cache alone can hold tens of thousands of entries.
const SKIP_DIRS: &[&str] = &[
    ".locks",
    "blobs",
    "trees",
    ".no_exist",
    "xet",
    ".git",
    "node_modules",
];

/// Which runner a model on disk belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskSource {
    Ollama,
    HuggingFace,
    LmStudio,
    /// A folder the user pointed PM at.
    Folder,
}

/// One model found on disk.
#[derive(Debug, Clone, Serialize)]
pub struct DiskModel {
    /// The runner's own name for it — an Ollama tag (`llama3.2:1b`), or `owner/repo` plus the file
    /// for the file-based runners. This is what catalog matching is tried against.
    pub name: String,
    pub source: DiskSource,
    /// Where it lives, for display. A directory for a sharded set, else the file.
    pub path: String,
    /// WEIGHTS only, in GB — real bytes on disk, all shards summed. Deliberately excludes the
    /// projector, so this is the same base the catalog's per-quant `file_gb` uses and the two are
    /// substitutable. The projector is carried separately in [`DiskModel::sidecar_gb`]; folding it
    /// in here made the on-disk footprint count it twice (once in the weight term, once from the
    /// catalog's `projector_gb`) and made this field's "same base as the catalog" claim false.
    pub size_gb: f64,
    /// The vision/audio projector (or MTP head) that loads WITH this model, in GB, measured on this
    /// disk — `0.0` when there is none. Resident cost, but not weights: it maps to
    /// `fit::ModelSpec::projector_gb`, never to a quant candidate's weight.
    pub sidecar_gb: f64,
    /// The quantization label when it could be read from the name (or, for Ollama, its config), else
    /// `None` — which makes the fit `unknown` rather than guessed.
    pub quant: Option<String>,
    /// How many files make up the weights: 1, or the shard count for a split GGUF.
    pub shards: u32,
}

/// What a scan found, plus whether it was complete.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DiskScan {
    pub models: Vec<DiskModel>,
    /// The scan stopped at [`MAX_MODELS`], so the list is a prefix rather than everything on disk.
    pub truncated: bool,
    /// Sources whose root exists on this machine — so the UI can distinguish "Ollama is here and has
    /// nothing downloaded" from "Ollama isn't installed".
    pub sources_present: Vec<DiskSource>,
}

/// Scan every known runner plus an optional user-chosen folder.
///
/// `home` is the user's home directory; passing it in (rather than reading it here) keeps the whole
/// walk pointable at a fixture tree in tests. Blocking — call it off the async runtime.
pub fn scan(home: &Path, extra_folder: Option<&Path>) -> DiskScan {
    let mut out = DiskScan::default();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    for root in ollama_roots(home, env_path("OLLAMA_MODELS")) {
        if root.join("manifests").is_dir() {
            mark_present(&mut out, DiskSource::Ollama);
            scan_ollama(&root, &mut out, &mut seen);
        }
    }
    for root in huggingface_roots(
        home,
        env_path("HF_HUB_CACHE"),
        env_path("HUGGINGFACE_HUB_CACHE"),
        env_path("HF_HOME"),
        env_path("XDG_CACHE_HOME"),
    ) {
        if root.is_dir() {
            mark_present(&mut out, DiskSource::HuggingFace);
            scan_huggingface(&root, &mut out, &mut seen);
        }
    }
    for root in lmstudio_roots(home) {
        if root.is_dir() {
            mark_present(&mut out, DiskSource::LmStudio);
            scan_lmstudio(&root, &mut out, &mut seen);
        }
    }
    if let Some(folder) = extra_folder {
        if folder.is_dir() {
            mark_present(&mut out, DiskSource::Folder);
            collect_gguf_models(folder, DiskSource::Folder, "", &mut out, &mut seen, 0);
        }
    }

    out.models.sort_by(|a, b| {
        b.size_gb
            .partial_cmp(&a.size_gb)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

fn mark_present(out: &mut DiskScan, source: DiskSource) {
    if !out.sources_present.contains(&source) {
        out.sources_present.push(source);
    }
}

/// An environment variable as a path string, treating empty as unset (which is how the runners read
/// theirs too).
fn env_path(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.trim().is_empty())
}

// --- root resolution (pure, so each runner's documented precedence is unit-tested) ---------------

/// Where Ollama keeps its store. `OLLAMA_MODELS` wins; otherwise `~/.ollama/models`, plus — on Linux
/// only — the system-installer path, since the packaged service runs as its own user whose home is
/// `/usr/share/ollama`.
///
/// A caller must still require a `manifests` subdirectory before accepting a root: the env var is
/// commonly set out-of-band (`launchctl setenv`, a systemd `Environment=`), so it is frequently
/// *absent* from PM's own environment even though the store has moved.
pub fn ollama_roots(home: &Path, env_models: Option<String>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(raw) = env_models {
        roots.push(expand_path(&raw, home));
    }
    roots.push(home.join(".ollama").join("models"));
    if cfg!(target_os = "linux") {
        roots.push(PathBuf::from("/usr/share/ollama/.ollama/models"));
    }
    dedupe_paths(roots)
}

/// Where the Hugging Face hub cache lives, in `huggingface_hub`'s documented precedence:
/// `HF_HUB_CACHE` → `HUGGINGFACE_HUB_CACHE` (legacy) → `$HF_HOME/hub` → `$XDG_CACHE_HOME/huggingface/hub`
/// → `~/.cache/huggingface/hub`.
///
/// Two details that are easy to get wrong and both come straight from the Python: the values are
/// shell-expanded, so `~/models` and `%LOCALAPPDATA%\hf` are legal; and `XDG_CACHE_HOME` is honoured
/// on **every** OS, not just Linux — the reference implementation never checks the platform.
pub fn huggingface_roots(
    home: &Path,
    hub_cache: Option<String>,
    legacy_hub_cache: Option<String>,
    hf_home: Option<String>,
    xdg_cache: Option<String>,
) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for raw in [hub_cache, legacy_hub_cache].into_iter().flatten() {
        roots.push(expand_path(&raw, home));
    }
    if let Some(raw) = hf_home {
        roots.push(expand_path(&raw, home).join("hub"));
    }
    if let Some(raw) = xdg_cache {
        roots.push(expand_path(&raw, home).join("huggingface").join("hub"));
    }
    roots.push(home.join(".cache").join("huggingface").join("hub"));
    dedupe_paths(roots)
}

/// Where LM Studio keeps models. Its home resolution is a documented chain: a `.lmstudio-home-pointer`
/// file **short-circuits everything** (this is how every portable / D-drive install works, so probing
/// the defaults first would read the wrong tree), else the legacy `~/.cache/lm-studio` if it exists,
/// else `~/.lmstudio`. Within a home, `settings.json`'s `downloadsFolder` can relocate the user models
/// directory alone.
///
/// Returns every models root worth scanning: the user one, the hub one, and the bundled one that
/// ships with the app. The legacy home is probed as well as the resolved one, because dual installs
/// (a Snap or Docker image beside a desktop install) really do happen.
pub fn lmstudio_roots(home: &Path) -> Vec<PathBuf> {
    let mut homes = Vec::new();
    if let Some(pointed) = std::fs::read_to_string(home.join(".lmstudio-home-pointer"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        homes.push(expand_path(&pointed, home));
    }
    homes.push(home.join(".cache").join("lm-studio"));
    homes.push(home.join(".lmstudio"));

    let mut roots = Vec::new();
    for h in dedupe_paths(homes) {
        // `downloadsFolder` moves the user models dir only; the hub and bundled roots stay put.
        match lmstudio_downloads_folder(
            &std::fs::read_to_string(h.join("settings.json")).unwrap_or_default(),
        ) {
            Some(folder) => roots.push(expand_path(&folder, home)),
            None => roots.push(h.join("models")),
        }
        roots.push(h.join("hub").join("models"));
        roots.push(h.join(".internal").join("bundled-models"));
    }
    dedupe_paths(roots)
}

/// The `downloadsFolder` key of an LM Studio `settings.json`, if it's set to a non-empty string. The
/// value is a literal native path (Windows backslashes, JSON-escaped) and is **not** `~`-expanded by
/// LM Studio — but users hand-edit this file, so the caller expands defensively.
pub fn lmstudio_downloads_folder(settings_json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(settings_json)
        .ok()?
        .get("downloadsFolder")?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Expand a configured path the way the runners' own resolvers do: a leading `~` becomes the home
/// directory, and `%VAR%` references are substituted from the environment. Anything else is taken
/// literally.
fn expand_path(raw: &str, home: &Path) -> PathBuf {
    let raw = raw.trim();
    let expanded = expand_percent_vars(raw);
    if let Some(rest) = expanded
        .strip_prefix("~/")
        .or_else(|| expanded.strip_prefix("~\\"))
    {
        return home.join(rest);
    }
    if expanded == "~" {
        return home.to_path_buf();
    }
    PathBuf::from(expanded)
}

/// Substitute `%NAME%` references from the environment, leaving unknown names untouched (so a literal
/// percent in a path can't silently swallow the rest of it).
fn expand_percent_vars(raw: &str) -> String {
    if !raw.contains('%') {
        return raw.to_string();
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                match std::env::var(name) {
                    Ok(v) if !name.is_empty() => out.push_str(&v),
                    _ => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push('%');
                out.push_str(after);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

// --- Ollama -------------------------------------------------------------------------------------

/// One layer of an Ollama manifest.
#[derive(Debug, Deserialize)]
struct OllamaLayer {
    #[serde(rename = "mediaType")]
    media_type: String,
    #[serde(default)]
    digest: String,
    #[serde(default)]
    size: i64,
}

/// An Ollama manifest. `layers` is `Option` deliberately: a cloud model serialises it as literal
/// `null`, and a plain `Vec` would make serde reject the whole model rather than skip it.
#[derive(Debug, Deserialize)]
struct OllamaManifest {
    #[serde(default)]
    config: Option<OllamaLayer>,
    #[serde(default)]
    layers: Option<Vec<OllamaLayer>>,
}

/// Walk Ollama's store. The manifests are a few hundred bytes each and carry exact byte counts, so
/// this reads no weights at all — one small JSON per model, and one small config blob for its quant.
fn scan_ollama(root: &Path, out: &mut DiskScan, seen: &mut HashSet<PathBuf>) {
    // Ollama's own enumerator globs exactly `manifests/*/*/*/*` — host / namespace / model / tag,
    // where the tag is a file. Anything shallower or deeper is not a model.
    let manifests = root.join("manifests");
    for host in read_dir_sorted(&manifests) {
        for ns in read_dir_sorted(&host) {
            for model in read_dir_sorted(&ns) {
                for tag in read_dir_sorted(&model) {
                    if out.models.len() >= MAX_MODELS {
                        out.truncated = true;
                        return;
                    }
                    if tag.is_dir() {
                        continue;
                    }
                    if let Some(found) = ollama_model_from_manifest(root, &manifests, &tag) {
                        if seen.insert(tag.clone()) {
                            out.models.push(found);
                        }
                    }
                }
            }
        }
    }
}

fn ollama_model_from_manifest(root: &Path, manifests: &Path, tag: &Path) -> Option<DiskModel> {
    let rel = tag.strip_prefix(manifests).ok()?;
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.len() != 4 {
        return None;
    }
    // A truncated manifest (a concurrent pull) is skipped, never fatal.
    let raw = std::fs::read_to_string(tag).ok()?;
    let manifest: OllamaManifest = serde_json::from_str(&raw).ok()?;
    let bytes = ollama_weight_bytes(&manifest)?;
    let quant = manifest
        .config
        .as_ref()
        .and_then(|c| ollama_blob_path(root, &c.digest))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| ollama_quant_from_config(&s));
    Some(DiskModel {
        name: ollama_display_name(&parts[0], &parts[1], &parts[2], &parts[3]),
        source: DiskSource::Ollama,
        path: tag.display().to_string(),
        size_gb: bytes_to_gb(bytes),
        sidecar_gb: bytes_to_gb(ollama_projector_bytes(&manifest)),
        quant,
        shards: 1,
    })
}

/// Total WEIGHT bytes in a manifest: the single GGUF `model` layer, or every `tensor` layer for a
/// safetensors model. The projector is deliberately NOT included — it is resident cost but not
/// weights, and [`ollama_projector_bytes`] carries it separately so the fit scorer can put each in
/// the right term. Returns `None` for a manifest with no weights at all — which is exactly a
/// **cloud** model (`"layers": null`), and correctly means "nothing is on this disk" rather than
/// "a zero-byte model".
fn ollama_weight_bytes(manifest: &OllamaManifest) -> Option<u64> {
    let layers = manifest.layers.as_ref()?;
    let total: i64 = layers
        .iter()
        .filter(|l| {
            matches!(
                l.media_type.as_str(),
                "application/vnd.ollama.image.model" | "application/vnd.ollama.image.tensor"
            )
        })
        .map(|l| l.size.max(0))
        .sum();
    u64::try_from(total).ok().filter(|&b| b > 0)
}

/// The projector bytes a manifest declares, or `0`. Summed rather than picked, unlike the file-based
/// runners: a manifest is an explicit list of what THIS tag loads, so a second projector layer in it
/// would be a second projector actually loaded — not a spare precision sitting in a folder.
fn ollama_projector_bytes(manifest: &OllamaManifest) -> u64 {
    let Some(layers) = manifest.layers.as_ref() else {
        return 0;
    };
    let total: i64 = layers
        .iter()
        .filter(|l| l.media_type.as_str() == "application/vnd.ollama.image.projector")
        .map(|l| l.size.max(0))
        .sum();
    u64::try_from(total).unwrap_or(0)
}

/// The blob file for a digest. Ollama stores `sha256:<hex>` as `sha256-<hex>` because `:` is illegal
/// on NTFS; a store written before that change and not yet migrated can still hold the colon form, so
/// both are accepted (we only ever read).
fn ollama_blob_path(root: &Path, digest: &str) -> Option<PathBuf> {
    let (algo, hex) = digest.split_once([':', '-'])?;
    if algo != "sha256" || hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(root.join("blobs").join(format!("sha256-{hex}")))
}

/// The quantization from an Ollama config blob — a few hundred bytes of JSON beside the weights, and
/// the only place an Ollama model records it (a tag like `llama3.2:1b` names a size, never a quant).
fn ollama_quant_from_config(config_json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(config_json).ok()?;
    let raw = value.get("file_type")?.as_str()?.trim();
    (!raw.is_empty()).then(|| raw.to_string())
}

/// An Ollama model's name as `ollama list` prints it: the default registry and `library` namespace
/// are elided, everything else is spelled out, and the tag is always present.
pub fn ollama_display_name(host: &str, namespace: &str, model: &str, tag: &str) -> String {
    let mut s = String::new();
    if !host.eq_ignore_ascii_case("registry.ollama.ai") {
        s.push_str(host);
        s.push('/');
        s.push_str(namespace);
        s.push('/');
    } else if !namespace.eq_ignore_ascii_case("library") {
        s.push_str(namespace);
        s.push('/');
    }
    s.push_str(model);
    s.push(':');
    s.push_str(tag);
    s
}

// --- Hugging Face hub cache ---------------------------------------------------------------------

/// Walk `models--*/snapshots/<rev>/**` for GGUF files. `blobs/` and `.locks/` are never descended —
/// the former holds the same bytes we stat through the snapshot, the latter can hold tens of
/// thousands of tiny files.
fn scan_huggingface(root: &Path, out: &mut DiskScan, seen: &mut HashSet<PathBuf>) {
    for repo_dir in read_dir_sorted(root) {
        if out.models.len() >= MAX_MODELS {
            out.truncated = true;
            return;
        }
        let Some(folder) = repo_dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(repo_id) = hf_repo_id_from_folder(folder) else {
            continue;
        };
        // Several cached revisions hold the same model; take the one `refs/main` names, else the
        // newest, so one model shows up once.
        let snapshots = repo_dir.join("snapshots");
        let Some(revision) = hf_preferred_revision(&repo_dir, &snapshots) else {
            continue;
        };
        // The repo id is the catalog-matchable identity — a bare GGUF filename often isn't.
        collect_gguf_models(
            &revision,
            DiskSource::HuggingFace,
            &format!("{repo_id}/"),
            out,
            seen,
            0,
        );
    }
}

/// Decode a `models--org--repo` cache folder back into `org/repo`. The separator can't occur inside a
/// repo id (the Hub forbids it), so the replace is unambiguous. Canonical single-segment models
/// (`models--roberta-base`) yield a repo id with no slash, which is valid.
pub fn hf_repo_id_from_folder(folder: &str) -> Option<String> {
    let (kind, rest) = folder.split_once("--")?;
    if kind != "models" || rest.is_empty() {
        return None;
    }
    Some(rest.replace("--", "/"))
}

/// The snapshot directory to read for a repo: whatever `refs/main` points at, else the most recently
/// modified one.
fn hf_preferred_revision(repo_dir: &Path, snapshots: &Path) -> Option<PathBuf> {
    if let Some(head) = std::fs::read_to_string(repo_dir.join("refs").join("main"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        let candidate = snapshots.join(head);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    read_dir_sorted(snapshots)
        .into_iter()
        .filter(|p| p.is_dir())
        .max_by_key(|p| {
            std::fs::metadata(p)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        })
}

// --- LM Studio ----------------------------------------------------------------------------------

/// Walk `<root>/<publisher>/<repo>/*.gguf`. Recognised models sit at exactly that depth, so the walk
/// is capped there rather than let loose on a junctioned drive.
fn scan_lmstudio(root: &Path, out: &mut DiskScan, seen: &mut HashSet<PathBuf>) {
    for publisher in read_dir_sorted(root) {
        if !publisher.is_dir() {
            continue;
        }
        for repo in read_dir_sorted(&publisher) {
            if out.models.len() >= MAX_MODELS {
                out.truncated = true;
                return;
            }
            if !repo.is_dir() {
                continue;
            }
            let pub_name = publisher.file_name().unwrap_or_default().to_string_lossy();
            let repo_name = repo.file_name().unwrap_or_default().to_string_lossy();
            // Recognised LM Studio models sit at exactly `<publisher>/<repo>/<file>.gguf`, so this
            // reads that one directory rather than recursing (an MLX model, for instance, is a
            // directory of safetensors at the same depth and is deliberately not descended into).
            for model in gguf_models_in_dir(&repo, DiskSource::LmStudio, seen) {
                if out.models.len() >= MAX_MODELS {
                    out.truncated = true;
                    return;
                }
                out.models.push(DiskModel {
                    name: format!("{pub_name}/{repo_name}/{}", model.name),
                    ..model
                });
            }
        }
    }
}

// --- shared GGUF collection ---------------------------------------------------------------------

/// Recursively collect GGUF models under `dir`, prefixing each name with `name_prefix` (the repo id
/// for a Hugging Face snapshot, empty for a user folder). Bounded by depth and the model cap.
fn collect_gguf_models(
    dir: &Path,
    source: DiskSource,
    name_prefix: &str,
    out: &mut DiskScan,
    seen: &mut HashSet<PathBuf>,
    depth: usize,
) {
    if depth > MAX_WALK_DEPTH {
        return;
    }
    for model in gguf_models_in_dir(dir, source, seen) {
        if out.models.len() >= MAX_MODELS {
            out.truncated = true;
            return;
        }
        out.models.push(DiskModel {
            name: format!("{name_prefix}{}", model.name),
            ..model
        });
    }
    for child in read_dir_sorted(dir) {
        if out.models.len() >= MAX_MODELS {
            out.truncated = true;
            return;
        }
        if !child.is_dir() {
            continue;
        }
        let skip = child
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| SKIP_DIRS.contains(&n));
        if skip {
            continue;
        }
        collect_gguf_models(&child, source, name_prefix, out, seen, depth + 1);
    }
}

/// The GGUF models directly inside one directory, with shard sets folded into a single entry and
/// projector sidecars folded into the model they belong to.
fn gguf_models_in_dir(
    dir: &Path,
    source: DiskSource,
    seen: &mut HashSet<PathBuf>,
) -> Vec<DiskModel> {
    let mut singles: Vec<(String, PathBuf, u64)> = Vec::new();
    // (shard prefix) → (total declared, files found, bytes)
    let mut shard_sets: Vec<(String, u32, u32, u64, PathBuf)> = Vec::new();
    // Projector sidecars are part of a model's footprint but are not models themselves. Collected
    // as CANDIDATES, not summed: a snapshot can hold several precisions of the same projector and a
    // model loads exactly one of them. See `pick_sidecar_bytes`.
    let mut sidecars: Vec<(String, u64)> = Vec::new();

    for path in read_dir_sorted(dir) {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !is_gguf_file(name) {
            continue;
        }
        // Follow the link: on a Hugging Face cache the snapshot entry is usually a symlink into
        // `blobs/`, and a directory-entry stat would report the reparse point (0 bytes on Windows)
        // instead of the file. `fs::metadata` follows; `DirEntry::metadata` would not.
        let Ok(meta) = std::fs::metadata(&path) else {
            continue; // a broken link — the blob was pruned. Skip the file, keep scanning.
        };
        let bytes = meta.len();
        if bytes == 0 {
            continue;
        }
        if is_sidecar_gguf(name) {
            sidecars.push((name.to_string(), bytes));
            continue;
        }
        match split_shard(name) {
            Some((prefix, _idx, total)) => {
                match shard_sets.iter_mut().find(|(p, ..)| *p == prefix) {
                    Some(entry) => {
                        entry.2 += 1;
                        entry.3 = entry.3.saturating_add(bytes);
                    }
                    None => shard_sets.push((prefix, total, 1, bytes, dir.to_path_buf())),
                }
            }
            None => singles.push((name.to_string(), path, bytes)),
        }
    }

    // One choice per directory, applied to every model built from it — correct for a Hugging Face
    // snapshot (one repo per directory), and the same scope the previous sum had.
    let sidecar_gb = bytes_to_gb(pick_sidecar_bytes(&sidecars));

    let mut models = Vec::new();
    for (name, path, bytes) in singles {
        // Dedupe on the resolved target so one physical file shared between runners (a junctioned
        // model library is a real setup) is reported once.
        let key = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !seen.insert(key) {
            continue;
        }
        let stem = name.trim_end_matches(".gguf").trim_end_matches(".GGUF");
        models.push(DiskModel {
            name: name.clone(),
            source,
            path: path.display().to_string(),
            size_gb: bytes_to_gb(bytes),
            sidecar_gb,
            quant: quant_from_name(stem),
            shards: 1,
        });
    }
    for (prefix, declared, found, bytes, dir) in shard_sets {
        // An incomplete set is a half-finished download, not a model you can run.
        if found != declared {
            continue;
        }
        let key = dir.join(&prefix);
        if !seen.insert(key.clone()) {
            continue;
        }
        models.push(DiskModel {
            name: format!("{prefix}.gguf"),
            source,
            path: dir.display().to_string(),
            size_gb: bytes_to_gb(bytes),
            sidecar_gb,
            quant: quant_from_name(&prefix),
            shards: declared,
        });
    }
    models
}

/// Whether a filename is a GGUF weight file. Deliberately strict: the in-flight and legacy-split
/// forms (`.gguf.incomplete`, `.gguf-split-a`, `.gguf.part1of4`) do **not** end in `.gguf`, and none
/// of them is loadable as-is.
pub fn is_gguf_file(name: &str) -> bool {
    name.len() > 5 && name.to_ascii_lowercase().ends_with(".gguf")
}

/// Whether a GGUF file is a sidecar rather than a model: a vision/audio projector (`mmproj-…`) or a
/// multi-token-prediction head (`mtp-…`). They live beside the weights, are hundreds of MB, and are
/// not loadable on their own — counted toward the model's footprint, never listed as models.
pub fn is_sidecar_gguf(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("mmproj") || n.starts_with("mtp-")
}

/// Pick the ONE sidecar a model actually loads, out of everything found beside it. `0` when there
/// is none.
///
/// This mirrors the catalog generator's `pickProjector` deliberately, because the two numbers are
/// compared against each other: prefer an f16 `mmproj`, else take the **smallest** candidate.
/// **Never the sum** — a Hugging Face snapshot commonly ships `mmproj-F16` *and* `mmproj-F32` of the
/// same projector, a model loads exactly one, and summing them was inflating every model in that
/// directory by roughly the size of a spare projector.
///
/// The f16 test is `mmproj` + an optional `-`/`.`/`_` + `f16`, matching the generator's regex
/// character for character. Note it does NOT match the common `mmproj-model-f16.gguf` form (there is
/// a `-model` in between) — such a file falls through to the smallest-candidate rule, which picks
/// the f16 anyway whenever an f32 is its only rival. Kept strict so the two sides cannot disagree.
pub fn pick_sidecar_bytes(candidates: &[(String, u64)]) -> u64 {
    let is_f16 = |name: &str| {
        let n = name.to_ascii_lowercase();
        n.split_once("mmproj").is_some_and(|(_, rest)| {
            rest.strip_prefix(['-', '.', '_'])
                .unwrap_or(rest)
                .starts_with("f16")
        })
    };
    if let Some((_, bytes)) = candidates.iter().find(|(name, _)| is_f16(name)) {
        return *bytes;
    }
    candidates
        .iter()
        .map(|(_, bytes)| *bytes)
        .min()
        .unwrap_or(0)
}

/// Split a `gguf-split` shard filename into `(prefix, index, total)`. The convention is llama.cpp's
/// `%s-%05d-of-%05d.gguf`, always five zero-padded digits and 1-based. Matching is anchored at the
/// end because real base names contain digits and hyphens.
pub fn split_shard(name: &str) -> Option<(String, u32, u32)> {
    let stem = name
        .strip_suffix(".gguf")
        .or_else(|| name.strip_suffix(".GGUF"))?;
    let (rest, total) = stem.rsplit_once("-of-")?;
    let (prefix, index) = rest.rsplit_once('-')?;
    if index.len() != 5 || total.len() != 5 {
        return None;
    }
    let index = index.parse::<u32>().ok()?;
    let total = total.parse::<u32>().ok()?;
    if index == 0 || total == 0 || index > total || prefix.is_empty() {
        return None;
    }
    // A literal `split` token appears in some publishers' names; it belongs to the shard suffix.
    let prefix = prefix.strip_suffix("-split").unwrap_or(prefix);
    Some((prefix.to_string(), index, total))
}

/// The quantization label at the end of a GGUF file stem, e.g. `…-Q4_K_M` or `….Q8_0`. Both the
/// hyphen and dot separators occur in the wild. `None` when the trailing token isn't a quant PM
/// knows — which makes the fit `unknown` rather than a guess.
pub fn quant_from_name(stem: &str) -> Option<String> {
    // Split on `-` and `.` only, never `_` — a quant label contains underscores (`Q4_K_M`), so the
    // trailing segment after those two separators IS the label when there is one.
    let token = stem.rsplit(['-', '.']).next()?;
    crate::fit::Quant::from_label(token).map(|_| token.to_ascii_uppercase())
}

// --- small helpers ------------------------------------------------------------------------------

/// Directory entries, sorted so a scan is deterministic. An unreadable directory yields nothing.
fn read_dir_sorted(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    paths.sort();
    paths
}

/// Bytes → GB in the same base the catalog and the hardware scan use, so fit-scoring compares like
/// with like.
fn bytes_to_gb(bytes: u64) -> f64 {
    let gb = bytes as f64 / 1_073_741_824.0;
    (gb * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ollama_names_match_what_ollama_list_prints() {
        // The default registry and the `library` namespace are elided; everything else is spelled out.
        assert_eq!(
            ollama_display_name("registry.ollama.ai", "library", "llama3.2", "1b"),
            "llama3.2:1b"
        );
        assert_eq!(
            ollama_display_name("registry.ollama.ai", "mycorp", "tuned", "v2"),
            "mycorp/tuned:v2"
        );
        assert_eq!(
            ollama_display_name("hf.co", "bartowski", "Qwen2.5-7B-Instruct-GGUF", "Q4_K_M"),
            "hf.co/bartowski/Qwen2.5-7B-Instruct-GGUF:Q4_K_M"
        );
        // Ollama matches its own store case-insensitively, so casing must not change the elision.
        assert_eq!(
            ollama_display_name("Registry.Ollama.AI", "Library", "phi4", "latest"),
            "phi4:latest"
        );
    }

    #[test]
    fn ollama_weights_exclude_metadata_layers_and_cloud_models() {
        let local = r#"{"config":{"mediaType":"c","digest":"sha256:aa","size":485},
          "layers":[{"mediaType":"application/vnd.ollama.image.model","digest":"sha256:bb","size":1321082688},
                    {"mediaType":"application/vnd.ollama.image.template","digest":"sha256:cc","size":1429},
                    {"mediaType":"application/vnd.ollama.image.license","digest":"sha256:dd","size":7711}]}"#;
        let m: OllamaManifest = serde_json::from_str(local).unwrap();
        assert_eq!(ollama_weight_bytes(&m), Some(1_321_082_688));

        // A vision projector is part of the resident footprint, but it is NOT weights: it comes back
        // in its own term so the fit scorer can put it where the catalog puts `projector_gb`.
        // Folding it into the weight total double-counted it against a catalog-matched model.
        let vision = r#"{"layers":[{"mediaType":"application/vnd.ollama.image.model","digest":"","size":4108916992},
                                   {"mediaType":"application/vnd.ollama.image.projector","digest":"","size":624434368}]}"#;
        let m: OllamaManifest = serde_json::from_str(vision).unwrap();
        assert_eq!(ollama_weight_bytes(&m), Some(4_108_916_992));
        assert_eq!(ollama_projector_bytes(&m), 624_434_368);

        // No projector layer is 0, not None — "this model has no projector" is a fact, not a gap.
        let m: OllamaManifest = serde_json::from_str(local).unwrap();
        assert_eq!(ollama_projector_bytes(&m), 0);

        // A cloud model has `"layers": null` — it must PARSE (a plain Vec would reject the whole
        // model) and report no local weights, because there are none on this disk.
        let cloud = r#"{"config":{"mediaType":"c","digest":"sha256:aa","size":384},"layers":null}"#;
        let m: OllamaManifest = serde_json::from_str(cloud).unwrap();
        assert_eq!(ollama_weight_bytes(&m), None);
    }

    #[test]
    fn ollama_blob_paths_accept_both_digest_separators() {
        let root = Path::new("/models");
        let hex = "74701a8c35f6c8d9a4b91f3f3497643001d63e0c7a84e085bed452548fa88d45";
        // The modern form, and the pre-migration colon form a store may still hold.
        let want = root.join("blobs").join(format!("sha256-{hex}"));
        assert_eq!(
            ollama_blob_path(root, &format!("sha256:{hex}")),
            Some(want.clone())
        );
        assert_eq!(ollama_blob_path(root, &format!("sha256-{hex}")), Some(want));
        // Anything that isn't a real digest is refused — `os.CreateTemp` leaves `sha256-<decimal>`
        // files in the blobs dir while a layer is being written.
        assert_eq!(ollama_blob_path(root, "sha256-3847162049"), None);
        assert_eq!(ollama_blob_path(root, "md5-abc"), None);
        assert_eq!(ollama_blob_path(root, ""), None);
    }

    #[test]
    fn ollama_quant_comes_from_the_config_blob() {
        // A tag like `llama3.2:1b` names a size, never a quant — the config blob is the only source.
        assert_eq!(
            ollama_quant_from_config(r#"{"model_family":"llama","file_type":"Q4_K_M"}"#).as_deref(),
            Some("Q4_K_M")
        );
        assert_eq!(ollama_quant_from_config(r#"{"file_type":""}"#), None);
        assert_eq!(
            ollama_quant_from_config(r#"{"model_family":"llama"}"#),
            None
        );
        assert_eq!(ollama_quant_from_config("not json"), None);
    }

    #[test]
    fn hf_cache_folders_decode_back_to_repo_ids() {
        assert_eq!(
            hf_repo_id_from_folder("models--bartowski--Meta-Llama-3.1-8B-Instruct-GGUF").as_deref(),
            Some("bartowski/Meta-Llama-3.1-8B-Instruct-GGUF")
        );
        // Canonical single-segment models have no org and must not panic or produce garbage.
        assert_eq!(
            hf_repo_id_from_folder("models--roberta-base").as_deref(),
            Some("roberta-base")
        );
        // Datasets and spaces share the cache but are not models.
        assert_eq!(hf_repo_id_from_folder("datasets--glue"), None);
        assert_eq!(
            hf_repo_id_from_folder("spaces--dalle-mini--dalle-mini"),
            None
        );
        assert_eq!(hf_repo_id_from_folder("CACHEDIR.TAG"), None);
        assert_eq!(hf_repo_id_from_folder(".locks"), None);
    }

    #[test]
    fn shard_sets_are_recognised_and_grouped_by_prefix() {
        assert_eq!(
            split_shard("Meta-Llama-3.1-70B-Instruct-Q6_K-00001-of-00002.gguf"),
            Some(("Meta-Llama-3.1-70B-Instruct-Q6_K".to_string(), 1, 2))
        );
        assert_eq!(
            split_shard("DeepSeek-R1.BF16-00030-of-00030.gguf"),
            Some(("DeepSeek-R1.BF16".to_string(), 30, 30))
        );
        // Some publishers insert a literal `split` token; it belongs to the suffix, not the name.
        assert_eq!(
            split_shard("grok-1-Q2_K-split-00001-of-00009.gguf"),
            Some(("grok-1-Q2_K".to_string(), 1, 9))
        );
        // Not shards.
        assert_eq!(split_shard("qwen2.5-7b-instruct-q4_k_m.gguf"), None);
        assert_eq!(split_shard("model-1-of-2.gguf"), None); // not zero-padded to five
        assert_eq!(split_shard("model-00000-of-00000.gguf"), None);
        assert_eq!(split_shard("model-00003-of-00002.gguf"), None); // index past the total
    }

    #[test]
    fn gguf_files_and_sidecars_are_told_apart() {
        assert!(is_gguf_file("Qwen3.5-9B-Q4_K_M.gguf"));
        assert!(is_gguf_file("MODEL.GGUF"));
        assert!(!is_gguf_file(".gguf"));
        // The in-flight and legacy-split forms don't end in `.gguf` and aren't loadable as they are.
        assert!(!is_gguf_file("model.gguf.incomplete"));
        assert!(!is_gguf_file("goliath-120b.Q4_K_M.gguf-split-a"));
        assert!(!is_gguf_file("Llama-405B.IQ3_M.gguf.part1of4"));

        // Projectors sit beside the weights and are large, but they are not models.
        assert!(is_sidecar_gguf("mmproj-model-f16.gguf"));
        assert!(is_sidecar_gguf("mmproj-Qwen3.5-9B-BF16.gguf"));
        assert!(is_sidecar_gguf("mtp-head.gguf"));
        assert!(!is_sidecar_gguf("gemma-3-4b-it-Q4_K_M.gguf"));
    }

    #[test]
    fn one_projector_is_chosen_and_precisions_are_never_summed() {
        // The bug this pins: a snapshot shipping two precisions of the SAME projector had both
        // folded into the model's size, inflating it by roughly a spare projector.
        let two = [
            ("mmproj-F32.gguf".to_string(), 2_000_000_000u64),
            ("mmproj-F16.gguf".to_string(), 1_000_000_000u64),
        ];
        assert_eq!(pick_sidecar_bytes(&two), 1_000_000_000);

        // f16 wins even when it is not the smallest, matching the generator's preference order.
        let f16_is_bigger = [
            ("mmproj-Q8_0.gguf".to_string(), 500_000_000u64),
            ("mmproj.f16.gguf".to_string(), 900_000_000u64),
        ];
        assert_eq!(pick_sidecar_bytes(&f16_is_bigger), 900_000_000);

        // The common real-world name has `-model-` in the middle, so it misses the strict f16 test
        // (as it does in the generator) and falls to smallest — which is still the f16 here.
        let real = [
            ("mmproj-model-f16.gguf".to_string(), 800_000_000u64),
            ("mmproj-model-f32.gguf".to_string(), 1_600_000_000u64),
        ];
        assert_eq!(pick_sidecar_bytes(&real), 800_000_000);

        let one = [("mmproj-model-f16.gguf".to_string(), 42u64)];
        assert_eq!(pick_sidecar_bytes(&one), 42);
        assert_eq!(pick_sidecar_bytes(&[]), 0);
    }

    #[test]
    fn a_projector_is_reported_beside_the_weights_not_inside_them() {
        // GiB-scale fixtures on purpose: `bytes_to_gb` rounds to 2dp, so KiB-scale files would both
        // round to 0.00 and the assertion would pass whatever the code did.
        let gib = 1_073_741_824u64;
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let folder = tmp.path().join("models");
        std::fs::create_dir_all(home.join("empty")).unwrap();
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("gemma-3-4b-it-Q4_K_M.gguf"),
            vec![0u8; (2 * gib) as usize],
        )
        .unwrap();
        std::fs::write(
            folder.join("mmproj-model-f16.gguf"),
            vec![0u8; gib as usize],
        )
        .unwrap();
        // A second precision of the same projector: present on disk, never loaded alongside.
        std::fs::write(
            folder.join("mmproj-model-f32.gguf"),
            vec![0u8; (2 * gib) as usize],
        )
        .unwrap();

        let scan = scan(&home, Some(&folder));
        assert_eq!(scan.models.len(), 1, "projectors are not models");
        let m = &scan.models[0];
        assert_eq!(m.quant.as_deref(), Some("Q4_K_M"));
        // Weights only — the projector is NOT in here, so this is the same base as the catalog's
        // per-quant `file_gb` and the two are substitutable in `fit::QuantCandidate`.
        assert_eq!(m.size_gb, 2.0);
        // Exactly ONE projector, the smaller of the two. 3.0 would mean they were summed.
        assert_eq!(m.sidecar_gb, 1.0);
    }

    #[test]
    fn quant_labels_are_read_off_the_filename_or_left_unknown() {
        // Both separators occur in the wild, and casing varies by publisher.
        assert_eq!(
            quant_from_name("gemma-3-4b-it-Q4_K_M").as_deref(),
            Some("Q4_K_M")
        );
        assert_eq!(
            quant_from_name("MobileLLM-125M-HF.Q8_0").as_deref(),
            Some("Q8_0")
        );
        assert_eq!(
            quant_from_name("qwen2.5-7b-instruct-q4_k_m").as_deref(),
            Some("Q4_K_M")
        );
        assert_eq!(quant_from_name("Model-IQ4_XS").as_deref(), Some("IQ4_XS"));
        // Legacy and full-precision labels read too. This gate is `Quant::from_label`, so widening
        // that table is what makes these files scoreable off a measured size instead of `unknown`.
        assert_eq!(quant_from_name("some-model-F16").as_deref(), Some("F16"));
        assert_eq!(quant_from_name("llama-2-7b.Q4_0").as_deref(), Some("Q4_0"));
        assert_eq!(
            quant_from_name("Model-bf16").as_deref(),
            Some("BF16"),
            "the file's own label is reported, even though it scores as F16"
        );
        // An unrecognised or absent label is still never guessed — a trailing word that merely looks
        // like a token must not become a quant, so this stays gated on the table.
        assert_eq!(quant_from_name("some-model-TQ1_0"), None);
        assert_eq!(quant_from_name("qwen3-4b-instruct"), None);
        assert_eq!(quant_from_name("plain-model"), None);
        assert_eq!(quant_from_name(""), None);
    }

    #[test]
    fn lmstudio_settings_relocate_the_models_folder() {
        // A real settings.json value: a fully-qualified native path with escaped backslashes.
        let json = r#"{"downloadsFolder":"C:\\Users\\drago\\.lmstudio\\models","other":1}"#;
        assert_eq!(
            lmstudio_downloads_folder(json).as_deref(),
            Some(r"C:\Users\drago\.lmstudio\models")
        );
        assert_eq!(
            lmstudio_downloads_folder(r#"{"downloadsFolder":"  "}"#),
            None
        );
        assert_eq!(lmstudio_downloads_folder("{}"), None);
        assert_eq!(lmstudio_downloads_folder("garbage"), None);
    }

    #[test]
    fn roots_follow_each_runners_documented_precedence() {
        let home = Path::new("/home/bobby");

        // Ollama: the env var wins, the default always follows as a fallback.
        let roots = ollama_roots(home, Some("/mnt/big/models".into()));
        assert_eq!(roots[0], PathBuf::from("/mnt/big/models"));
        assert!(roots.contains(&home.join(".ollama").join("models")));
        // Unset → just the defaults, no empty entry.
        let roots = ollama_roots(home, None);
        assert_eq!(roots[0], home.join(".ollama").join("models"));

        // Hugging Face: HF_HUB_CACHE beats the legacy name, which beats HF_HOME, which beats XDG.
        let roots = huggingface_roots(
            home,
            Some("/a".into()),
            Some("/b".into()),
            Some("/c".into()),
            Some("/d".into()),
        );
        assert_eq!(roots[0], PathBuf::from("/a"));
        assert_eq!(roots[1], PathBuf::from("/b"));
        assert_eq!(roots[2], PathBuf::from("/c/hub"));
        assert_eq!(roots[3], PathBuf::from("/d/huggingface/hub"));
        assert_eq!(
            *roots.last().unwrap(),
            home.join(".cache").join("huggingface").join("hub")
        );
        // XDG is honoured on every OS — the reference implementation never checks the platform.
        let roots = huggingface_roots(home, None, None, None, Some("/xdg".into()));
        assert_eq!(roots[0], PathBuf::from("/xdg/huggingface/hub"));
    }

    #[test]
    fn configured_paths_expand_a_leading_tilde() {
        let home = Path::new("/home/bobby");
        assert_eq!(expand_path("~/models", home), home.join("models"));
        assert_eq!(expand_path("~", home), home);
        assert_eq!(
            expand_path("  /abs/path  ", home),
            PathBuf::from("/abs/path")
        );
        // An unknown %VAR% is left alone rather than swallowing the rest of the path.
        assert_eq!(
            expand_path("%NOT_A_REAL_VAR_XYZ%/models", home),
            PathBuf::from("%NOT_A_REAL_VAR_XYZ%/models")
        );
    }

    #[test]
    fn a_scan_of_a_fixture_tree_finds_each_runners_models() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();

        // --- Ollama: a manifest tree whose JSON carries the sizes, plus a config blob for the quant.
        let tag_dir = home.join(".ollama/models/manifests/registry.ollama.ai/library/llama3.2");
        std::fs::create_dir_all(&tag_dir).unwrap();
        let cfg_hex = "a".repeat(64);
        std::fs::write(
            tag_dir.join("1b"),
            format!(
                r#"{{"config":{{"mediaType":"c","digest":"sha256:{cfg_hex}","size":485}},
                    "layers":[{{"mediaType":"application/vnd.ollama.image.model","digest":"sha256:{}","size":2147483648}},
                              {{"mediaType":"application/vnd.ollama.image.license","digest":"sha256:{}","size":7711}}]}}"#,
                "b".repeat(64),
                "c".repeat(64)
            ),
        )
        .unwrap();
        let blobs = home.join(".ollama/models/blobs");
        std::fs::create_dir_all(&blobs).unwrap();
        std::fs::write(
            blobs.join(format!("sha256-{cfg_hex}")),
            r#"{"model_family":"llama","model_type":"1B","file_type":"Q4_K_M"}"#,
        )
        .unwrap();
        // A cloud model: listed by Ollama, but nothing of it is on this disk.
        let cloud_dir = home.join(".ollama/models/manifests/registry.ollama.ai/library/gpt-oss");
        std::fs::create_dir_all(&cloud_dir).unwrap();
        std::fs::write(cloud_dir.join("120b-cloud"), r#"{"layers":null}"#).unwrap();

        // --- Hugging Face: a snapshot with a real quant, a projector sidecar, and an incomplete blob.
        let snap = home.join(
            ".cache/huggingface/hub/models--bartowski--Qwen2.5-7B-Instruct-GGUF/snapshots/abc123",
        );
        std::fs::create_dir_all(&snap).unwrap();
        std::fs::write(
            snap.join("Qwen2.5-7B-Instruct-Q4_K_M.gguf"),
            vec![7u8; 4096],
        )
        .unwrap();
        std::fs::write(snap.join("mmproj-model-f16.gguf"), vec![1u8; 1024]).unwrap();
        std::fs::write(snap.join("README.md.incomplete"), b"x").unwrap();
        // `blobs/` must never be descended — the same bytes, and we already stat them via the snapshot.
        let hf_blobs =
            home.join(".cache/huggingface/hub/models--bartowski--Qwen2.5-7B-Instruct-GGUF/blobs");
        std::fs::create_dir_all(&hf_blobs).unwrap();
        std::fs::write(hf_blobs.join("deadbeef.gguf"), vec![9u8; 8192]).unwrap();

        // --- LM Studio: publisher/repo/file.gguf, plus a complete shard set.
        let lms = home.join(".lmstudio/models/lmstudio-community/Gemma-3-4B-GGUF");
        std::fs::create_dir_all(&lms).unwrap();
        std::fs::write(lms.join("gemma-3-4b-it-Q8_0.gguf"), vec![2u8; 2048]).unwrap();
        let big = home.join(".lmstudio/models/unsloth/Big-GGUF");
        std::fs::create_dir_all(&big).unwrap();
        std::fs::write(big.join("Big-Q4_K_M-00001-of-00002.gguf"), vec![3u8; 1024]).unwrap();
        std::fs::write(big.join("Big-Q4_K_M-00002-of-00002.gguf"), vec![3u8; 1024]).unwrap();
        // An interrupted download: shard 1 of 3 with siblings missing is not a runnable model.
        let partial = home.join(".lmstudio/models/someone/Partial-GGUF");
        std::fs::create_dir_all(&partial).unwrap();
        std::fs::write(
            partial.join("Partial-Q4_K_M-00001-of-00003.gguf"),
            vec![4u8; 512],
        )
        .unwrap();

        let scan = scan(home, None);
        let names: Vec<&str> = scan.models.iter().map(|m| m.name.as_str()).collect();

        // Ollama: named as `ollama list` would, sized from the manifest, quant from the config blob.
        let ollama = scan
            .models
            .iter()
            .find(|m| m.name == "llama3.2:1b")
            .unwrap();
        assert_eq!(ollama.source, DiskSource::Ollama);
        assert_eq!(ollama.quant.as_deref(), Some("Q4_K_M"));
        assert!(
            (ollama.size_gb - 2.0).abs() < 0.01,
            "manifest bytes, not blob stats"
        );
        // The cloud model has no weights here, so it is not an on-disk model.
        assert!(!names.iter().any(|n| n.contains("gpt-oss")));

        // Hugging Face: identified by repo id + filename, with the projector folded into its size and
        // never listed on its own. Nothing from `blobs/` leaks in.
        let hf = scan
            .models
            .iter()
            .find(|m| m.name.starts_with("bartowski/Qwen2.5-7B-Instruct-GGUF/"))
            .expect("hugging face model");
        assert_eq!(hf.quant.as_deref(), Some("Q4_K_M"));
        assert!(!names.iter().any(|n| n.contains("mmproj")));
        assert!(!names.iter().any(|n| n.contains("deadbeef")));

        // LM Studio: publisher/repo/file, and a complete shard set collapsed into ONE model.
        assert!(names.contains(&"lmstudio-community/Gemma-3-4B-GGUF/gemma-3-4b-it-Q8_0.gguf"));
        let sharded = scan
            .models
            .iter()
            .find(|m| m.name.contains("Big-Q4_K_M"))
            .expect("sharded model");
        assert_eq!(sharded.shards, 2);
        // An incomplete set is not offered.
        assert!(!names.iter().any(|n| n.contains("Partial")));

        assert!(!scan.truncated);
        assert!(scan.sources_present.contains(&DiskSource::Ollama));
        assert!(scan.sources_present.contains(&DiskSource::HuggingFace));
        assert!(scan.sources_present.contains(&DiskSource::LmStudio));
        assert!(!scan.sources_present.contains(&DiskSource::Folder));
    }

    #[test]
    fn a_scan_of_an_empty_home_finds_nothing_and_does_not_fail() {
        let tmp = tempfile::tempdir().unwrap();
        let scan = scan(tmp.path(), None);
        assert!(scan.models.is_empty());
        assert!(scan.sources_present.is_empty());
        assert!(!scan.truncated);
    }

    #[test]
    fn a_user_folder_is_walked_and_never_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let folder = tmp.path().join("my-models");
        std::fs::create_dir_all(home.join("empty")).unwrap();
        std::fs::create_dir_all(folder.join("nested")).unwrap();
        std::fs::write(folder.join("nested/Solo-Q6_K.gguf"), vec![5u8; 4096]).unwrap();

        let scan = scan(&home, Some(&folder));
        assert_eq!(scan.models.len(), 1);
        assert_eq!(scan.models[0].source, DiskSource::Folder);
        assert_eq!(scan.models[0].quant.as_deref(), Some("Q6_K"));

        // Read-only: LM Studio's own resolver writes a home-pointer file as a side effect of
        // resolving the default. PM must never do that.
        assert!(!home.join(".lmstudio-home-pointer").exists());
        assert!(!home.join(".lmstudio").exists());
    }
}
