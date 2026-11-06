// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! On-device storage inventory + a reference-counted, guarded teardown of the large, regenerable
//! components PM downloads: the optional t-SNE stack (`openTSNE` → `scikit-learn`/`scipy`), the
//! optional photo-OCR stack (`rapidocr`+`pi-heif` → OpenCV/`shapely`/`pyclipper`), and a
//! **read-only** view of the active embedder.
//!
//! The photo-OCR stack is also **installable** from this inventory (the one place it's enabled or
//! disabled): when absent it shows an Install action, when present it's removable with its cascade.
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

/// Approximate on-disk size of an embedder's weights. They live in `runtime/models` inside the data
/// dir (since #286), so they go with `runtime/` and download again on next use; but that folder
/// also holds the reranker, the hf and xet caches and the photo-OCR weights, so the active
/// embedder's share is estimated rather than scanned.
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
        },
    }
}

fn build_report(venv: &Path, embedder: &ModelEntry, ocr_ready: bool) -> StorageReport {
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

    let venv_total = dir_size(venv);
    // The base engine is the venv minus every optional library (each listed separately below), so the
    // total never double-counts them.
    let optional_size =
        tsne_size + sklearn_size + scipy_size + ocr_size + cv2_size + shapely_size + pyclipper_size;
    let base = venv_total.saturating_sub(optional_size);

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
    });

    let total = base + optional_size + embedder_size;
    StorageReport {
        total_bytes: total,
        components,
    }
}

// ---- removal --------------------------------------------------------------

fn do_remove(app: &AppHandle, id: &str, venv: &Path) -> Result<()> {
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
        // The row keeps its old id (the frontend keys on it), but what is installed changed name:
        // rapidocr 3.10 depends on `opencv-python-headless`, and a venv that had OCR before still
        // holds `opencv-python`. Both own the same `cv2/` folder, so removing only one leaves either
        // nothing reclaimed (a new install) or a dist-info pointing at deleted files (an old one).
        // `pip uninstall -y` skips whichever is absent.
        "opencv-python" => sidecar.pip_uninstall(&["opencv-python-headless", "opencv-python"])?,
        "shapely" => sidecar.pip_uninstall(&["shapely"])?,
        "pyclipper" => sidecar.pip_uninstall(&["pyclipper"])?,
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
    // A marker read and a hash of the small OCR lock: cheap enough to take before the walk.
    let ocr_ready = app.state::<AppState>().sidecar.optional_ocr_ready();
    tauri::async_runtime::spawn_blocking(move || build_report(&venv, &embedder, ocr_ready))
        .await
        .map_err(|e| Error::Other(format!("storage scan task panicked: {e}")))
}

/// Remove a component, enforcing the dependency cascade. For `openTSNE` it also recomputes the
/// semantic Map with PCA in the background. Rejects anything not on the allow-list (numpy / venv /
/// embedder).
#[tauri::command]
pub async fn remove_storage_component(app: AppHandle, id: String) -> Result<()> {
    let venv = paths::venv_dir(&app)?;
    let app2 = app.clone();
    let id2 = id.clone();
    tauri::async_runtime::spawn_blocking(move || do_remove(&app2, &id2, &venv))
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
        assert_eq!(blocker_for("ocr", true, true, false), None);
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

    /// The whole report on a venv holding every optional library: each row measures its own
    /// packages, the document engine is what is left of the venv, the cascade blocks each library
    /// under the feature that needs it, and the total counts the venv once plus the embedder's
    /// estimate.
    #[test]
    fn the_report_measures_each_component_once() {
        let dir = tempfile::tempdir().unwrap();
        let venv = dir.path().join("runtime").join("venv");
        let site = if cfg!(windows) {
            venv.join("Lib").join("site-packages")
        } else {
            venv.join("lib").join("python3.12").join("site-packages")
        };
        let put = |path: PathBuf, bytes: usize| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, vec![0u8; bytes]).unwrap();
        };
        put(venv.join("pyvenv.cfg"), 5);
        put(site.join("fastembed").join("__init__.py"), 1000);
        put(site.join("openTSNE").join("__init__.py"), 10);
        put(site.join("openTSNE-1.0.4.dist-info").join("RECORD"), 1);
        put(site.join("sklearn").join("__init__.py"), 20);
        put(site.join("scipy").join("__init__.py"), 40);
        put(site.join("rapidocr").join("__init__.py"), 80);
        put(site.join("cv2").join("__init__.py"), 160);
        put(site.join("shapely").join("__init__.py"), 320);
        // pyclipper installs a single extension module and no import dir, so its dist-info is
        // what says it is there.
        put(site.join("pyclipper-1.3.0.dist-info").join("RECORD"), 640);

        let embedder = crate::registry::active_embedder();
        let estimate = embedder_estimate_bytes(embedder.id);
        assert!(estimate > 0, "the default embedder has an estimate");
        let report = build_report(&venv, &embedder, true);

        let ids: Vec<&str> = report.components.iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            [
                "venv",
                "openTSNE",
                "scikit-learn",
                "scipy",
                "ocr",
                "opencv-python",
                "shapely",
                "pyclipper",
                "embedder"
            ]
        );
        let row = |id: &str| report.components.iter().find(|c| c.id == id).unwrap();
        let blocked_by = |id: &str| {
            let c = row(id);
            assert!(matches!(c.status, Status::Blocked), "{id} is not blocked");
            c.blockers.iter().map(|b| b.anchor).collect::<Vec<_>>()
        };

        assert_eq!(
            row("venv").size_bytes,
            1005,
            "the base alone: pyvenv.cfg and fastembed"
        );
        assert!(matches!(row("venv").status, Status::Required));
        assert_eq!(
            row("openTSNE").size_bytes,
            11,
            "its import dir and dist-info"
        );
        assert!(matches!(row("openTSNE").status, Status::Removable));
        assert_eq!(row("scikit-learn").size_bytes, 20);
        assert_eq!(blocked_by("scikit-learn"), ["openTSNE"]);
        assert_eq!(row("scipy").size_bytes, 40);
        assert_eq!(blocked_by("scipy"), ["openTSNE"]);
        assert_eq!(row("ocr").size_bytes, 80);
        assert!(matches!(row("ocr").status, Status::Removable));
        assert_eq!(row("opencv-python").size_bytes, 160);
        assert_eq!(blocked_by("opencv-python"), ["ocr"]);
        assert_eq!(row("shapely").size_bytes, 320);
        assert_eq!(blocked_by("shapely"), ["ocr"]);
        assert_eq!(row("pyclipper").size_bytes, 640);
        assert_eq!(blocked_by("pyclipper"), ["ocr"]);
        let model = row("embedder");
        assert_eq!((model.size_bytes, model.approximate), (estimate, true));
        assert!(matches!(model.status, Status::InUse));

        // The rows split the venv between them, so the total is the venv once, plus the
        // embedder's estimate once.
        assert_eq!(dir_size(&venv), 1005 + 11 + 20 + 40 + 80 + 160 + 320 + 640);
        assert_eq!(report.total_bytes, dir_size(&venv) + estimate);
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
