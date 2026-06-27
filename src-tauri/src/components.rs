// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! On-device storage inventory + a reference-counted, guarded teardown of the large, regenerable
//! components PM downloads: the optional t-SNE stack (`openTSNE` → `scikit-learn`/`scipy`), the
//! Whisper speech model, and a **read-only** view of the active embedder.
//!
//! The teardown is a cascade: a heavy shared library can only be removed once nothing still needs it
//! (`scikit-learn` after `openTSNE`; `scipy` after both). `numpy` is shared with the embedder and is
//! never offered or removed. The dependency guard is enforced **server-side** here, not just in the
//! UI, and a final backstop in [`crate::sidecar::SidecarManager::pip_uninstall`] refuses `numpy`
//! outright. No database is involved — this is pure filesystem inventory + `pip uninstall`.

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
}

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
fn blocker_for(id: &str, tsne_present: bool, sklearn_present: bool) -> Option<&'static str> {
    match id {
        "scikit-learn" if tsne_present => Some("openTSNE"),
        "scipy" if tsne_present => Some("openTSNE"),
        "scipy" if sklearn_present => Some("scikit-learn"),
        _ => None,
    }
}

fn blocker_label(anchor: &str) -> String {
    match anchor {
        "openTSNE" => "Remove the enhanced layout first".into(),
        "scikit-learn" => "Remove scikit-learn first".into(),
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

fn build_report(venv: &Path, data: &Path, embedder: &ModelEntry) -> StorageReport {
    let site = site_packages_dir(venv);
    let (tsne_present, tsne_size) = pkg(site.as_ref(), "openTSNE", &["openTSNE", "openTSNE-*"]);
    let (sklearn_present, sklearn_size) = pkg(
        site.as_ref(),
        "sklearn",
        &["sklearn", "scikit_learn*", "joblib*", "threadpoolctl*"],
    );
    let (scipy_present, scipy_size) = pkg(site.as_ref(), "scipy", &["scipy", "scipy*"]);

    let venv_total = dir_size(venv);
    // The base engine is the venv minus the optional t-SNE libraries (listed separately below), so the
    // total never double-counts them.
    let base = venv_total.saturating_sub(tsne_size + sklearn_size + scipy_size);

    let models = data.join("runtime").join("models");
    let whisper_present = dir_nonempty(&models);
    let whisper_size = if whisper_present {
        dir_size(&models)
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

    let mut lib = |id: &'static str, label: &str, detail: &str, size: u64| {
        let blk = blocker_for(id, tsne_present, sklearn_present);
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
            "scikit-learn",
            "scikit-learn",
            "Used by the enhanced layout. Includes joblib and threadpoolctl.",
            sklearn_size,
        );
    }
    if scipy_present {
        lib(
            "scipy",
            "scipy",
            "Used by the enhanced layout and scikit-learn.",
            scipy_size,
        );
    }

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
    });

    let total = base + tsne_size + sklearn_size + scipy_size + whisper_size + embedder_size;
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

    // Re-check the cascade server-side (defence in depth — the UI greys blocked buttons, but never
    // trust the webview for a destructive uninstall).
    if let Some(anchor) = blocker_for(id, tsne, sklearn) {
        return Err(Error::Other(blocker_label(anchor)));
    }

    let state = app.state::<AppState>();
    let sidecar = &state.sidecar;
    match id {
        "openTSNE" => sidecar.uninstall_optional_tsne()?,
        "scikit-learn" => sidecar.pip_uninstall(&["scikit-learn", "joblib", "threadpoolctl"])?,
        "scipy" => sidecar.pip_uninstall(&["scipy"])?,
        "whisper" => {
            let m = data.join("runtime").join("models");
            if m.exists() {
                std::fs::remove_dir_all(&m)?;
            }
        }
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
    tauri::async_runtime::spawn_blocking(move || build_report(&venv, &data, &embedder))
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

    #[test]
    fn cascade_blocks_then_unblocks_in_order() {
        // While openTSNE is present, neither heavy library can go.
        assert_eq!(blocker_for("scikit-learn", true, true), Some("openTSNE"));
        assert_eq!(blocker_for("scipy", true, true), Some("openTSNE"));
        // openTSNE removed → scikit-learn is free, but scipy still waits on scikit-learn.
        assert_eq!(blocker_for("scikit-learn", false, true), None);
        assert_eq!(blocker_for("scipy", false, true), Some("scikit-learn"));
        // Everything above gone → scipy is free.
        assert_eq!(blocker_for("scipy", false, false), None);
        // openTSNE itself, and unrelated ids, are never blocked.
        assert_eq!(blocker_for("openTSNE", true, true), None);
        assert_eq!(blocker_for("whisper", true, true), None);
    }

    #[test]
    fn match_pat_handles_globs_and_exact() {
        assert!(match_pat("scikit_learn.libs", "scikit_learn*"));
        assert!(match_pat("scipy", "scipy"));
        assert!(!match_pat("scipython", "scipy"));
        assert!(match_pat("openTSNE-1.0.4.dist-info", "openTSNE-*"));
    }
}
