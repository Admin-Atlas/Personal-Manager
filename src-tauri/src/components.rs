// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! On-device storage inventory + a reference-counted, guarded teardown of the large, regenerable
//! components PM downloads: the optional t-SNE stack (`openTSNE` → `scikit-learn`/`scipy`), the
//! optional photo-OCR stack (`rapidocr`+`pi-heif` → OpenCV/`shapely`/`pyclipper`), voice input's
//! speech engine, the Whisper speech model, and a **read-only** view of the active embedder.
//!
//! The photo-OCR stack is also **installable** from this inventory (the one place it's enabled or
//! disabled): when absent it shows an Install action, when present it's removable with its cascade.
//! Voice input is the same, with its switch on the row (Mods spec D8): turning it on installs the
//! engine, turning it off offers to remove it.
//!
//! The teardown is a cascade: a heavy shared library can only be removed once nothing still needs it
//! (`scikit-learn` after `openTSNE`; `scipy` after both; the OCR image libs after `rapidocr`). `numpy`
//! is shared with the embedder and is never offered or removed. The dependency guard is enforced
//! **server-side** here, not just in the UI, and a final backstop in
//! [`crate::sidecar::SidecarManager::pip_uninstall`] refuses `numpy` outright. No database is involved
//! — this is pure filesystem inventory + `pip uninstall`.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::error::{Error, Result};
use crate::registry::ModelEntry;
// The Speech-model row measures and removes the Whisper model's own folders only, never
// `runtime/models` itself, which also holds the embedder, reranker and OCR weights (#1046). The
// helpers live in `sidecar`, which also reads the folder as D8's evidence that voice was used.
use crate::sidecar::{remove_whisper, whisper_paths};
use crate::{db, paths, AppState};

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Status {
    /// Can't be removed (the base venv).
    Required,
    /// Active and required, but managed elsewhere (the embedder) — read-only here.
    InUse,
    /// Removable now.
    Removable,
    /// Removable only after a dependent is removed first (carries `blockers`).
    Blocked,
    /// An optional component that isn't installed yet — offers an Install action (photo OCR). The
    /// `size_bytes` on such a row is an estimate of what installing it will use, not on-disk usage.
    Installable,
    /// An optional component that is on disk but not working (photo OCR whose marker a moved lock
    /// left stale) — offers Install, to repair it, AND Remove, so the space can be had back without
    /// first downloading it all again. `size_bytes` is real on-disk usage.
    Stale,
}

/// Rough on-disk footprint of the optional photo-OCR stack once installed: measured at 204 MiB with
/// rapidocr 3.10.0 (OpenCV's headless build is the bulk; the old estimate of 150 MiB undercounted
/// even the 239 MiB the full opencv-python build used). Shown on the not-yet-installed row so the
/// cost is visible before choosing to install; never added to the on-disk total (nothing is on disk
/// yet). The ~31 MB of OCR models download separately, on the first photo.
const OCR_INSTALL_ESTIMATE_BYTES: u64 = 205 * 1024 * 1024;

/// Rough on-disk footprint of voice input's speech engine once installed: measured at 243 MiB on
/// Linux with faster-whisper 1.2.1, CTranslate2 4.8.1 and PyAV 18.1.0 (the two native libraries are
/// nearly all of it). Shown, like the OCR estimate, on a row whose engine isn't on disk. The speech
/// model (~145 MB) downloads separately, on the first recording.
const VOICE_INSTALL_ESTIMATE_BYTES: u64 = 245 * 1024 * 1024;

/// Every site-packages entry the speech engine installs: the voice lock's own packages
/// (`OPTIONAL_VOICE_COMPONENT.uninstall` in sidecar.rs), each import dir with its dist-info and
/// bundled-library folder. setuptools brings `_distutils_hack` and its `.pth`, and older releases
/// `pkg_resources`.
const VOICE_GLOBS: &[&str] = &[
    "faster_whisper",
    "faster_whisper-*",
    "ctranslate2",
    "ctranslate2-*",
    "ctranslate2.libs",
    "av",
    "av-*",
    "av.libs",
    "setuptools",
    "setuptools-*",
    "_distutils_hack",
    "distutils-precedence.pth",
    "pkg_resources",
];

/// A dependent that must be removed first; `anchor` is the component id to scroll to in this tab.
#[derive(Serialize)]
struct Blocker {
    label: String,
    anchor: &'static str,
}

/// A "manage elsewhere" link (a pill that switches Settings tab) for components not removed here.
#[derive(Serialize)]
struct Manage {
    label: String,
    tab: &'static str,
}

#[derive(Serialize)]
struct Component {
    id: &'static str,
    label: String,
    detail: String,
    size_bytes: u64,
    /// The size is an estimate, not a real on-disk measurement (the embedder's shared cache).
    approximate: bool,
    /// Indented under the component above it (the t-SNE libraries under the enhanced layout).
    child: bool,
    status: Status,
    blockers: Vec<Blocker>,
    manage: Option<Manage>,
    note: Option<String>,
    /// A feature switch that lives on this row, and where it stands: voice input's. `None` for
    /// every other row. With it, `status` describes the engine on disk, and the switch decides
    /// whether the row offers Turn on or Turn off beside it.
    switch: Option<bool>,
}

/// The voice switch and whether its engine is ready, read before the walk (a DB row and a marker).
#[derive(Clone, Copy, Default)]
struct VoiceState {
    on: bool,
    ready: bool,
}

#[derive(Serialize)]
pub struct StorageReport {
    total_bytes: u64,
    components: Vec<Component>,
}

// ---- the dependency cascade (pure, unit-tested) ---------------------------

/// Which component (if any) must be removed before `id` can be. `None` = removable now. Used both to
/// label the UI and to guard the actual removal, so the two can never disagree.
fn blocker_for(
    id: &str,
    tsne_present: bool,
    sklearn_present: bool,
    ocr_present: bool,
) -> Option<&'static str> {
    match id {
        "scikit-learn" if tsne_present => Some("openTSNE"),
        "scipy" if tsne_present => Some("openTSNE"),
        "scipy" if sklearn_present => Some("scikit-learn"),
        "opencv-python" if ocr_present => Some("ocr"),
        "shapely" if ocr_present => Some("ocr"),
        "pyclipper" if ocr_present => Some("ocr"),
        _ => None,
    }
}

fn blocker_label(anchor: &str) -> String {
    match anchor {
        "openTSNE" => "Remove the enhanced layout first".into(),
        "scikit-learn" => "Remove scikit-learn first".into(),
        "ocr" => "Remove photo text recognition first".into(),
        other => format!("Remove {other} first"),
    }
}

// ---- filesystem inventory -------------------------------------------------

/// Recursively sum the sizes of the regular files under `path` (symlinks skipped). Best-effort: an
/// unreadable entry contributes 0 rather than failing the whole scan.
fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let Ok(rd) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in rd.flatten() {
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_dir() {
            total += dir_size(&entry.path());
        } else if ft.is_file() {
            total += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    total
}

fn dir_nonempty(path: &Path) -> bool {
    std::fs::read_dir(path)
        .map(|mut rd| rd.next().is_some())
        .unwrap_or(false)
}

/// The venv's `site-packages` (per-OS layout), if it exists.
fn site_packages_dir(venv: &Path) -> Option<PathBuf> {
    if cfg!(windows) {
        let p = venv.join("Lib").join("site-packages");
        return p.is_dir().then_some(p);
    }
    let rd = std::fs::read_dir(venv.join("lib")).ok()?;
    for e in rd.flatten() {
        if e.file_name().to_string_lossy().starts_with("python") {
            let sp = e.path().join("site-packages");
            if sp.is_dir() {
                return Some(sp);
            }
        }
    }
    None
}

fn match_pat(name: &str, pat: &str) -> bool {
    match pat.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == pat,
    }
}

/// `(present, size)` for a package: present iff its primary import dir exists; size sums every
/// site-packages entry matching `patterns` (the import dir, its `*.dist-info`, any `*.libs`, and
/// bundled siblings removed alongside it).
fn pkg(site: Option<&PathBuf>, primary: &str, patterns: &[&str]) -> (bool, u64) {
    let Some(site) = site else {
        return (false, 0);
    };
    let present = site.join(primary).is_dir();
    let mut size = 0;
    if let Ok(rd) = std::fs::read_dir(site) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if patterns.iter().any(|p| match_pat(&name, p)) {
                let path = e.path();
                size += if path.is_dir() {
                    dir_size(&path)
                } else {
                    e.metadata().map(|m| m.len()).unwrap_or(0)
                };
            }
        }
    }
    (present, size)
}

/// Like [`pkg`], but presence is detected from a `*.dist-info` directory rather than an import dir —
/// for single-file extension modules (e.g. `pyclipper`, which installs `pyclipper.<abi>.pyd`/`.so`
/// with no package directory). `dist_prefix` is the normalised distribution name (`"pyclipper-"`).
fn pkg_dist(site: Option<&PathBuf>, dist_prefix: &str, patterns: &[&str]) -> (bool, u64) {
    let Some(site) = site else {
        return (false, 0);
    };
    let mut present = false;
    let mut size = 0;
    if let Ok(rd) = std::fs::read_dir(site) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with(dist_prefix) && name.ends_with(".dist-info") {
                present = true;
            }
            if patterns.iter().any(|p| match_pat(&name, p)) {
                let path = e.path();
                size += if path.is_dir() {
                    dir_size(&path)
                } else {
                    e.metadata().map(|m| m.len()).unwrap_or(0)
                };
            }
        }
    }
    (present, size)
}

/// The import dirs of the speech engine's three packages. Any one of them is an engine on disk: pip
/// removes packages one at a time and stops at the first it can't, so a removal that failed
/// part-way can leave CTranslate2 or PyAV (nearly all of the ~240 MB) without faster-whisper.
const VOICE_IMPORT_DIRS: &[&str] = &["faster_whisper", "ctranslate2", "av"];

/// Whether voice input's speech engine, or any part of it, is in the venv. The Storage row and the
/// sidecar's one-off clean-up of an older PM's engine (`sidecar::legacy_voice_action`) both ask
/// this, so they can never disagree about what is installed, and what a failed removal left is
/// still shown (with its Remove) and still cleaned up.
pub(crate) fn voice_engine_on_disk(venv: &Path) -> bool {
    site_packages_dir(venv)
        .is_some_and(|site| VOICE_IMPORT_DIRS.iter().any(|d| site.join(d).is_dir()))
}

/// Approximate on-disk size of an embedder's weights (its shared fastembed/HF cache is outside the
/// data dir and unmanaged, so we estimate rather than scan).
fn embedder_estimate_bytes(id: &str) -> u64 {
    let mb: u64 = match id {
        "BAAI/bge-small-en-v1.5" => 90,
        "intfloat/multilingual-e5-large" => 1100,
        _ => 0,
    };
    mb * 1024 * 1024
}

/// The photo-OCR row. The feature is a first-class row whether or not it's installed: when working
/// it's Removable (the image libraries cascade-remove under it), and otherwise it offers an Install
/// action right here, so the whole feature is enabled/disabled in one place, like the others.
///
/// "Working" is the sidecar's marker (`ocr_ready`, what ingest reads), not the packages being on
/// disk. They part ways when a lock moves and the reinstall at the next launch fails (offline, a
/// dropped download): the old packages stay, every photo is indexed without its text, and a row
/// keyed on presence alone went on showing OCR installed with nothing to press. Installing over
/// the stale copy is what repairs it; Remove stays on offer beside it (`Status::Stale`), because
/// the image libraries under it stay cascade-blocked while it is on disk, and an offline user who
/// wants the space back must not have to download it all first.
fn ocr_row(present: bool, ready: bool, size: u64) -> Component {
    let label = "Photo text recognition (rapidocr)".into();
    match (present, ready) {
        (true, true) => Component {
            id: "ocr",
            label,
            detail: "Reads text out of dropped photos and screenshots (OCR), on-device.".into(),
            size_bytes: size,
            approximate: false,
            child: false,
            status: Status::Removable,
            blockers: Vec::new(),
            manage: None,
            note: Some(
                "Removing it indexes new photos by date and location only, with no text. \
                 You can reinstall it any time."
                    .into(),
            ),
            switch: None,
        },
        (true, false) => Component {
            id: "ocr",
            label,
            detail: "Reads text out of dropped photos and screenshots (OCR), on-device. It is on \
                     this computer but needs installing again after an update before it works."
                .into(),
            size_bytes: size,
            approximate: false,
            child: false,
            status: Status::Stale,
            blockers: Vec::new(),
            manage: None,
            note: Some("Until then, photos are indexed by their date and location only.".into()),
            switch: None,
        },
        (false, _) => Component {
            id: "ocr",
            label,
            detail: "Reads text out of dropped photos and screenshots (OCR) so it's searchable — \
                     all on-device. Optional, a one-time download."
                .into(),
            size_bytes: OCR_INSTALL_ESTIMATE_BYTES,
            approximate: true,
            child: false,
            status: Status::Installable,
            blockers: Vec::new(),
            manage: None,
            note: Some("Without it, photos are indexed by their date and location only.".into()),
            switch: None,
        },
    }
}

/// The voice-input row (Mods spec D8): the switch and its speech engine in one place, the way
/// photo OCR's row holds that whole feature, so voice is turned on, turned off and removed here.
/// `status` describes the engine on disk, by the same rules as OCR's (a marker a moved lock left
/// stale is `Stale`), and `switch` says which of Turn on / Turn off goes beside it. Pure, so the
/// six cases are tested. The speech model's weights keep their own row.
fn voice_row(on: bool, present: bool, ready: bool, size: u64) -> Component {
    const WHAT: &str = "Dictate into the chat box with the mic. PM turns your speech into text on \
                        this computer; the recording never leaves it.";
    let (detail, note, status, installed) = match (on, present, ready) {
        (true, true, true) => (
            WHAT.to_string(),
            "Turning it off offers to remove the engine and the speech model too.",
            Status::Removable,
            true,
        ),
        (true, true, false) => (
            format!("{WHAT} It is on, but its engine needs installing again after an update."),
            "Until then, the mic installs it the first time you record.",
            Status::Stale,
            true,
        ),
        (true, false, _) => (
            format!("{WHAT} It is on, but its engine isn't on this computer."),
            "Install it here, or the mic installs it the first time you record.",
            Status::Installable,
            false,
        ),
        (false, true, true) => (
            format!("{WHAT} Off. Its engine is still on this computer, so turning it on is quick."),
            "Remove it to free the space; turning voice on again downloads it.",
            Status::Removable,
            true,
        ),
        (false, true, false) => (
            format!("{WHAT} Off. Its engine is still on this computer, from before an update."),
            "Remove it to free the space; turning voice on installs it again.",
            Status::Stale,
            true,
        ),
        (false, false, _) => (
            format!("{WHAT} Off: turning it on downloads the engine once."),
            "The speech model (about 145 MB) downloads the first time you record.",
            Status::Installable,
            false,
        ),
    };
    Component {
        id: "voice",
        label: "Voice input (speech engine)".into(),
        detail,
        // What is on disk once installed; before then, what installing it will use.
        size_bytes: if installed {
            size
        } else {
            VOICE_INSTALL_ESTIMATE_BYTES
        },
        approximate: !installed,
        child: false,
        status,
        blockers: Vec::new(),
        manage: None,
        note: Some(note.into()),
        switch: Some(on),
    }
}

fn build_report(
    venv: &Path,
    data: &Path,
    embedder: &ModelEntry,
    ocr_ready: bool,
    voice: VoiceState,
) -> StorageReport {
    let site = site_packages_dir(venv);
    let (tsne_present, tsne_size) = pkg(site.as_ref(), "openTSNE", &["openTSNE", "openTSNE-*"]);
    let (sklearn_present, sklearn_size) = pkg(
        site.as_ref(),
        "sklearn",
        &["sklearn", "scikit_learn*", "joblib*", "threadpoolctl*"],
    );
    let (scipy_present, scipy_size) = pkg(site.as_ref(), "scipy", &["scipy", "scipy*"]);

    // Optional photo-OCR stack. The top-level component bundles rapidocr + pi-heif (the two pins
    // `uninstall_optional_ocr` removes) plus its tiny pure-Python siblings; the heavy image libraries
    // (OpenCV / shapely / pyclipper) are separate, cascade-removable children. numpy is shared
    // with the embedder and never listed. pyclipper installs no import dir, so it needs `pkg_dist`.
    let (ocr_present, ocr_size) = pkg(
        site.as_ref(),
        "rapidocr",
        &[
            "rapidocr",
            "rapidocr-*",
            "rapidocr_*",
            "pi_heif",
            "pi_heif-*",
            "pi_heif.libs",
            // pillow_heif is what the component installed before the pi-heif swap. Its globs stay so
            // an existing install is still MEASURED (and so the cascade still reclaims it) — a
            // package that stops being listed here does not stop taking up disk, it just stops being
            // counted, and the Storage tab quietly under-reports by ~28 MB.
            "pillow_heif",
            "pillow_heif-*",
            "pillow_heif.libs",
            "omegaconf",
            "omegaconf-*",
            "colorlog",
            "colorlog-*",
        ],
    );
    let (cv2_present, cv2_size) = pkg(site.as_ref(), "cv2", &["cv2", "opencv*", "opencv_python*"]);
    let (shapely_present, shapely_size) = pkg(
        site.as_ref(),
        "shapely",
        &["shapely", "shapely*", "Shapely*"],
    );
    let (pyclipper_present, pyclipper_size) =
        pkg_dist(site.as_ref(), "pyclipper-", &["pyclipper*"]);
    // Voice input's speech engine. Counted only with one of its own packages there: setuptools is
    // in its set, and a venv built on Python 3.11 has its own copy from `venv` whether or not voice
    // is installed, which then belongs to the document engine.
    let (_, voice_size) = pkg(site.as_ref(), "faster_whisper", VOICE_GLOBS);
    let voice_present = voice_engine_on_disk(venv);
    let voice_size = if voice_present { voice_size } else { 0 };

    let venv_total = dir_size(venv);
    // The base engine is the venv minus every optional library (each listed separately below), so the
    // total never double-counts them.
    let optional_size = tsne_size
        + sklearn_size
        + scipy_size
        + ocr_size
        + cv2_size
        + shapely_size
        + pyclipper_size
        + voice_size;
    let base = venv_total.saturating_sub(optional_size);

    // The Whisper model's own folders, never the whole of `runtime/models`: the embedder, the
    // reranker, the hf and xet caches and the photo-OCR weights live there too. Measured that way,
    // the row appeared for someone who never dictated, the total counted the embedder twice, and the
    // voice-off dialog offered to free all of it.
    let models = data.join("runtime").join("models");
    let [whisper_dir, whisper_locks] = whisper_paths(&models);
    let whisper_present = dir_nonempty(&whisper_dir);
    let whisper_size = if whisper_present {
        dir_size(&whisper_dir) + dir_size(&whisper_locks)
    } else {
        0
    };

    let embedder_size = embedder_estimate_bytes(embedder.id);

    let mut components = Vec::new();

    components.push(Component {
        id: "venv",
        label: "Document engine (Python)".into(),
        detail: "Converts and indexes your files. Always needed.".into(),
        size_bytes: base,
        approximate: false,
        child: false,
        status: Status::Required,
        blockers: Vec::new(),
        manage: None,
        note: None,
        switch: None,
    });

    if tsne_present {
        components.push(Component {
            id: "openTSNE",
            label: "Enhanced map layout (openTSNE)".into(),
            detail: "Sharper, tighter clusters for the semantic Map (t-SNE).".into(),
            size_bytes: tsne_size,
            approximate: false,
            child: false,
            status: Status::Removable,
            blockers: Vec::new(),
            manage: Some(Manage {
                label: "Turn on/off in Memory map".into(),
                tab: "general",
            }),
            note: Some("Removing it returns the Map to the basic (PCA) layout.".into()),
            switch: None,
        });
    }

    // A cascade-removable child library row (indented under its parent feature). A free fn rather than
    // a closure so it never holds a long-lived mutable borrow of `components` across the pushes below.
    let lib = |components: &mut Vec<Component>,
               id: &'static str,
               label: &str,
               detail: &str,
               size: u64| {
        let blk = blocker_for(id, tsne_present, sklearn_present, ocr_present);
        components.push(Component {
            id,
            label: label.into(),
            detail: detail.into(),
            size_bytes: size,
            approximate: false,
            child: true,
            status: if blk.is_some() {
                Status::Blocked
            } else {
                Status::Removable
            },
            blockers: blk
                .map(|a| {
                    vec![Blocker {
                        label: blocker_label(a),
                        anchor: a,
                    }]
                })
                .unwrap_or_default(),
            manage: None,
            note: None,
            switch: None,
        });
    };
    if sklearn_present {
        lib(
            &mut components,
            "scikit-learn",
            "scikit-learn",
            "Used by the enhanced layout. Includes joblib and threadpoolctl.",
            sklearn_size,
        );
    }
    if scipy_present {
        lib(
            &mut components,
            "scipy",
            "scipy",
            "Used by the enhanced layout and scikit-learn.",
            scipy_size,
        );
    }

    components.push(ocr_row(ocr_present, ocr_ready, ocr_size));
    if cv2_present {
        lib(
            &mut components,
            "opencv-python",
            "OpenCV (opencv-python-headless)",
            "Image processing used by photo text recognition.",
            cv2_size,
        );
    }
    if shapely_present {
        lib(
            &mut components,
            "shapely",
            "shapely",
            "Geometry used by photo text recognition.",
            shapely_size,
        );
    }
    if pyclipper_present {
        lib(
            &mut components,
            "pyclipper",
            "pyclipper",
            "Polygon clipping used by photo text recognition.",
            pyclipper_size,
        );
    }

    components.push(voice_row(voice.on, voice_present, voice.ready, voice_size));

    if whisper_present {
        components.push(Component {
            id: "whisper",
            label: "Speech model (Whisper)".into(),
            detail: "Used by voice notes / dictation.".into(),
            size_bytes: whisper_size,
            approximate: false,
            child: false,
            status: Status::Removable,
            blockers: Vec::new(),
            manage: None,
            note: Some("Re-downloads automatically the next time you record.".into()),
            switch: None,
        });
    }

    components.push(Component {
        id: "embedder",
        label: format!("Search model ({})", embedder.label),
        detail: "Powers search and indexing. Always needed.".into(),
        size_bytes: embedder_size,
        approximate: true,
        child: false,
        status: Status::InUse,
        blockers: Vec::new(),
        manage: Some(Manage {
            label: "Change under Search → Language".into(),
            tab: "search",
        }),
        note: Some("Stored in a shared model cache; can't be removed here.".into()),
        switch: None,
    });

    let total = base + optional_size + whisper_size + embedder_size;
    StorageReport {
        total_bytes: total,
        components,
    }
}

// ---- removal --------------------------------------------------------------

fn do_remove(app: &AppHandle, id: &str, venv: &Path, data: &Path) -> Result<()> {
    let site = site_packages_dir(venv);
    let present = |p: &str| site.as_ref().map(|s| s.join(p).is_dir()).unwrap_or(false);
    let tsne = present("openTSNE");
    let sklearn = present("sklearn");
    let ocr = present("rapidocr");

    // Re-check the cascade server-side (defence in depth — the UI greys blocked buttons, but never
    // trust the webview for a destructive uninstall).
    if let Some(anchor) = blocker_for(id, tsne, sklearn, ocr) {
        return Err(Error::Other(blocker_label(anchor)));
    }

    let state = app.state::<AppState>();
    let sidecar = &state.sidecar;
    match id {
        "openTSNE" => sidecar.uninstall_optional_tsne()?,
        "scikit-learn" => sidecar.pip_uninstall(&["scikit-learn", "joblib", "threadpoolctl"])?,
        "scipy" => sidecar.pip_uninstall(&["scipy"])?,
        "ocr" => sidecar.uninstall_optional_ocr()?,
        // Only while voice is off: the row offers Remove only then (Turn off is how an engine in
        // use goes, and it offers this removal), and with voice on the next recording would just
        // download it again.
        "voice" => {
            if crate::voice_switch::is_on(&state)? {
                return Err(Error::Other(
                    "Voice input is on. Turn it off first; that offers to remove its engine."
                        .into(),
                ));
            }
            sidecar.uninstall_optional_voice()?
        }
        // The row keeps its old id (the frontend keys on it), but what is installed changed name:
        // rapidocr 3.10 depends on `opencv-python-headless`, and a venv that had OCR before still
        // holds `opencv-python`. Both own the same `cv2/` folder, so removing only one leaves either
        // nothing reclaimed (a new install) or a dist-info pointing at deleted files (an old one).
        // `pip uninstall -y` skips whichever is absent.
        "opencv-python" => sidecar.pip_uninstall(&["opencv-python-headless", "opencv-python"])?,
        "shapely" => sidecar.pip_uninstall(&["shapely"])?,
        "pyclipper" => sidecar.pip_uninstall(&["pyclipper"])?,
        // The Whisper model's folders only: removing all of `runtime/models` took the search and
        // photo-OCR models with it.
        "whisper" => remove_whisper(&data.join("runtime").join("models"))?,
        other => {
            return Err(Error::Other(format!("'{other}' can't be removed here.")));
        }
    }
    Ok(())
}

// ---- commands -------------------------------------------------------------

/// Inventory the large on-device components with their sizes and the dependency cascade. The
/// directory walk runs off the async runtime.
#[tauri::command]
pub async fn list_storage_components(app: AppHandle) -> Result<StorageReport> {
    let embedder = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        db::selected_embedder(&conn)?
    };
    let venv = paths::venv_dir(&app)?;
    let data = paths::data_dir(&app)?;
    // Marker reads and hashes of the small optional locks: cheap enough to take before the walk.
    let (ocr_ready, voice) = {
        let state = app.state::<AppState>();
        let voice = VoiceState {
            on: crate::voice_switch::is_on(&state)?,
            ready: state.sidecar.optional_voice_ready(),
        };
        (state.sidecar.optional_ocr_ready(), voice)
    };
    tauri::async_runtime::spawn_blocking(move || {
        build_report(&venv, &data, &embedder, ocr_ready, voice)
    })
    .await
    .map_err(|e| Error::Other(format!("storage scan task panicked: {e}")))
}

/// Remove a component, enforcing the dependency cascade. For `openTSNE` it also recomputes the
/// semantic Map with PCA in the background. Rejects anything not on the allow-list (numpy / venv /
/// embedder).
#[tauri::command]
pub async fn remove_storage_component(app: AppHandle, id: String) -> Result<()> {
    let venv = paths::venv_dir(&app)?;
    let data = paths::data_dir(&app)?;
    let app2 = app.clone();
    let id2 = id.clone();
    tauri::async_runtime::spawn_blocking(move || do_remove(&app2, &id2, &venv, &data))
        .await
        .map_err(|e| Error::Other(format!("storage remove task panicked: {e}")))??;

    if id == "openTSNE" {
        let app3 = app.clone();
        tauri::async_runtime::spawn(async move {
            let _ = crate::layout::precompute_semantic_layout(&app3, true, true).await;
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidecar::WHISPER_REPO_DIR;

    #[test]
    fn cascade_blocks_then_unblocks_in_order() {
        // While openTSNE is present, neither heavy library can go.
        assert_eq!(
            blocker_for("scikit-learn", true, true, false),
            Some("openTSNE")
        );
        assert_eq!(blocker_for("scipy", true, true, false), Some("openTSNE"));
        // openTSNE removed → scikit-learn is free, but scipy still waits on scikit-learn.
        assert_eq!(blocker_for("scikit-learn", false, true, false), None);
        assert_eq!(
            blocker_for("scipy", false, true, false),
            Some("scikit-learn")
        );
        // Everything above gone → scipy is free.
        assert_eq!(blocker_for("scipy", false, false, false), None);
        // openTSNE itself, and unrelated ids, are never blocked.
        assert_eq!(blocker_for("openTSNE", true, true, false), None);
        assert_eq!(blocker_for("whisper", true, true, false), None);
    }

    #[test]
    fn ocr_cascade_blocks_then_unblocks() {
        // The OCR image libraries wait on rapidocr while it's present...
        assert_eq!(
            blocker_for("opencv-python", false, false, true),
            Some("ocr")
        );
        assert_eq!(blocker_for("shapely", false, false, true), Some("ocr"));
        assert_eq!(blocker_for("pyclipper", false, false, true), Some("ocr"));
        // ...and free up once rapidocr is gone.
        assert_eq!(blocker_for("opencv-python", false, false, false), None);
        assert_eq!(blocker_for("shapely", false, false, false), None);
        // rapidocr itself is never blocked; the two cascades don't interfere.
        assert_eq!(blocker_for("ocr", true, true, true), None);
        assert_eq!(blocker_for("opencv-python", true, true, false), None);
        assert_eq!(blocker_for("scipy", false, false, true), None);
    }

    #[test]
    fn ocr_on_disk_but_not_working_offers_install() {
        let row = |present, ready| ocr_row(present, ready, 7);
        assert!(matches!(row(true, true).status, Status::Removable));
        // The stale copy a failed reinstall leaves: Install AND Remove, sized as what is really on
        // disk. Not plain Installable, which would leave it (and the libraries it blocks) with no
        // way off the disk short of downloading it all again.
        let stale = row(true, false);
        assert!(matches!(stale.status, Status::Stale));
        assert_eq!((stale.size_bytes, stale.approximate), (7, false));
        let absent = row(false, false);
        assert!(matches!(absent.status, Status::Installable));
        assert_eq!(
            (absent.size_bytes, absent.approximate),
            (OCR_INSTALL_ESTIMATE_BYTES, true)
        );
    }

    /// A `runtime/models` laid out the way a real install fills it: the embedder and reranker, the hf
    /// and xet caches, the photo-OCR weights, and the Whisper repo with its lock folder.
    fn seeded_models(data: &Path) -> PathBuf {
        let models = data.join("runtime").join("models");
        let put = |rel: &str, bytes: usize| {
            let p = models.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, vec![0u8; bytes]).unwrap();
        };
        put(
            "fastembed/models--Qdrant--bge-small-en-v1.5/model.onnx",
            1000,
        );
        put("hf/xet/chunk", 100);
        put("rapidocr/det.onnx", 300);
        put(&format!("{WHISPER_REPO_DIR}/snapshots/abc/model.bin"), 50);
        put(&format!(".locks/{WHISPER_REPO_DIR}/x.lock"), 7);
        models
    }

    #[test]
    fn removing_the_speech_model_leaves_the_search_and_ocr_models() {
        let dir = tempfile::tempdir().unwrap();
        let models = seeded_models(dir.path());
        remove_whisper(&models).unwrap();
        assert!(!models.join(WHISPER_REPO_DIR).exists());
        assert!(!models.join(".locks").join(WHISPER_REPO_DIR).exists());
        assert!(models
            .join("fastembed/models--Qdrant--bge-small-en-v1.5/model.onnx")
            .is_file());
        assert!(models.join("hf/xet/chunk").is_file());
        assert!(models.join("rapidocr/det.onnx").is_file());
        // Removing again (nothing left) is not an error.
        remove_whisper(&models).unwrap();
    }

    #[test]
    fn the_speech_model_row_measures_only_its_own_folder() {
        let dir = tempfile::tempdir().unwrap();
        seeded_models(dir.path());
        let venv = dir.path().join("runtime").join("venv");
        let embedder = crate::registry::active_embedder();
        let report = build_report(&venv, dir.path(), &embedder, false, VoiceState::default());
        let whisper = report
            .components
            .iter()
            .find(|c| c.id == "whisper")
            .expect("a Whisper row while its folder is present");
        assert_eq!(whisper.size_bytes, 57);
        // The total adds the embedder's estimate once; the seeded embedder bytes are not counted
        // a second time through the Whisper row.
        assert_eq!(
            report.total_bytes,
            57 + embedder_estimate_bytes(embedder.id)
        );
    }

    #[test]
    fn no_speech_model_row_when_only_other_models_are_present() {
        let dir = tempfile::tempdir().unwrap();
        let models = seeded_models(dir.path());
        remove_whisper(&models).unwrap();
        assert!(models.join("fastembed").is_dir());
        let venv = dir.path().join("runtime").join("venv");
        let report = build_report(
            &venv,
            dir.path(),
            &crate::registry::active_embedder(),
            false,
            VoiceState::default(),
        );
        assert!(report.components.iter().all(|c| c.id != "whisper"));
    }

    // That `WHISPER_REPO_DIR` matches the sidecar's model name is tested where it lives
    // (`sidecar::tests::whisper_folder_name_matches_the_sidecar_model`).

    /// The voice row's six cases: the switch always rides on it, and the engine's state is told by
    /// OCR's rules, so the frontend can offer Turn on / Turn off beside Install and Remove.
    #[test]
    fn the_voice_row_carries_the_switch_and_the_engine_state() {
        let row = |on, present, ready| voice_row(on, present, ready, 9);
        let shape = |c: &Component| {
            (
                c.id,
                c.switch,
                matches!(c.status, Status::Removable),
                matches!(c.status, Status::Stale),
                matches!(c.status, Status::Installable),
                c.size_bytes,
                c.approximate,
            )
        };
        let est = VOICE_INSTALL_ESTIMATE_BYTES;
        // On.
        assert_eq!(
            shape(&row(true, true, true)),
            ("voice", Some(true), true, false, false, 9, false)
        );
        assert_eq!(
            shape(&row(true, true, false)),
            ("voice", Some(true), false, true, false, 9, false)
        );
        assert_eq!(
            shape(&row(true, false, false)),
            ("voice", Some(true), false, false, true, est, true)
        );
        // Off: the engine left behind is Remove-able, and an absent one shows what Turn on costs.
        assert_eq!(
            shape(&row(false, true, true)),
            ("voice", Some(false), true, false, false, 9, false)
        );
        assert_eq!(
            shape(&row(false, true, false)),
            ("voice", Some(false), false, true, false, 9, false)
        );
        assert_eq!(
            shape(&row(false, false, false)),
            ("voice", Some(false), false, false, true, est, true)
        );
        // No other row carries a switch.
        assert_eq!(ocr_row(true, true, 1).switch, None);
    }

    /// A venv laid out with the speech engine in it (as an older PM's base install, or the switch,
    /// leaves it): the voice row measures the engine's globs, and the document engine no longer
    /// counts them, so the total adds them once.
    #[test]
    fn the_voice_engine_is_measured_on_its_own_row_and_out_of_the_base() {
        let dir = tempfile::tempdir().unwrap();
        let venv = dir.path().join("runtime").join("venv");
        let site = if cfg!(windows) {
            venv.join("Lib").join("site-packages")
        } else {
            venv.join("lib").join("python3.12").join("site-packages")
        };
        let put = |rel: &str, bytes: usize| {
            let p = site.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, vec![0u8; bytes]).unwrap();
        };
        put("faster_whisper/__init__.py", 10);
        put("faster_whisper-1.2.1.dist-info/RECORD", 1);
        put("ctranslate2/__init__.py", 20);
        put("ctranslate2.libs/libct2.so", 200);
        put("av/__init__.py", 30);
        put("av.libs/libavcodec.so", 300);
        put("setuptools/__init__.py", 4);
        put("fastembed/__init__.py", 1000);

        assert!(voice_engine_on_disk(&venv));
        let embedder = crate::registry::active_embedder();
        let on = VoiceState {
            on: true,
            ready: true,
        };
        let report = build_report(&venv, dir.path(), &embedder, false, on);
        let row = |id: &str| report.components.iter().find(|c| c.id == id).unwrap();
        assert_eq!(row("voice").size_bytes, 565);
        assert_eq!(row("voice").switch, Some(true));
        assert_eq!(row("venv").size_bytes, 1000, "the base excludes the engine");
        assert_eq!(
            report.total_bytes,
            1565 + embedder_estimate_bytes(embedder.id)
        );

        // A removal pip stopped part-way: faster-whisper gone, CTranslate2 and PyAV left. Still the
        // engine, on its own row with a Remove (voice is off), never folded into the base.
        std::fs::remove_dir_all(site.join("faster_whisper")).unwrap();
        assert!(voice_engine_on_disk(&venv));
        let report = build_report(&venv, dir.path(), &embedder, false, VoiceState::default());
        let row = |id: &str| report.components.iter().find(|c| c.id == id).unwrap();
        assert_eq!(row("voice").size_bytes, 1 + 20 + 200 + 30 + 300 + 4);
        assert!(matches!(row("voice").status, Status::Stale));
        assert_eq!(row("venv").size_bytes, 1000);

        // With none of its packages left there is no engine: an off row with the install estimate,
        // and what is left (here setuptools, as a 3.11 venv has from `venv` itself) is the base's.
        for d in ["ctranslate2", "ctranslate2.libs", "av", "av.libs"] {
            std::fs::remove_dir_all(site.join(d)).unwrap();
        }
        assert!(!voice_engine_on_disk(&venv));
        let report = build_report(&venv, dir.path(), &embedder, false, VoiceState::default());
        let voice = report.components.iter().find(|c| c.id == "voice").unwrap();
        assert!(matches!(voice.status, Status::Installable));
        assert_eq!(voice.switch, Some(false));
        let base = report.components.iter().find(|c| c.id == "venv").unwrap();
        assert_eq!(base.size_bytes, 1000 + 1 + 4);
    }

    #[test]
    fn match_pat_handles_globs_and_exact() {
        assert!(match_pat("scikit_learn.libs", "scikit_learn*"));
        assert!(match_pat("scipy", "scipy"));
        assert!(!match_pat("scipython", "scipy"));
        assert!(match_pat("openTSNE-1.0.4.dist-info", "openTSNE-*"));
        assert!(match_pat("pyclipper-1.3.0.dist-info", "pyclipper*"));
        assert!(match_pat(
            "opencv_python-4.10.0.84.dist-info",
            "opencv_python*"
        ));
    }
}
