// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The regression app measures PM's mitigations on PM's engine, so it refuses to build on any other:
//! every crate that decides how a webview behaves must be at exactly the version PM's lock pins, and
//! the page's CSP must be PM's. When PM moves Tauri, this fails until the app's lock follows
//! (`cargo update -p <crate> --precise <version>` here), so the job can never go green measuring an
//! engine PM no longer ships.
//!
//! The frame's shape is pinned the same way: the harness mounts each probe frame with the `sandbox`
//! and `allow` values it declares as `FRAME_SANDBOX` and `FRAME_ALLOW`, and those must equal the
//! literals on the one `<iframe>` in PM's `ModFrame.tsx` (INVARIANTS.md I-24). Otherwise a change to
//! ModFrame's frame would leave this job measuring the old one.
//!
//! A bare `tauri_build::build()` after that, like PM's: no app ACL manifest, so custom commands are
//! reachable from any local origin, which is the condition the test exists to check.

use std::collections::BTreeMap;
use std::path::Path;

/// The crates whose version changes what the webview does.
const ENGINE_CRATES: &[&str] = &[
    "tauri",
    "tauri-build",
    "tauri-codegen",
    "tauri-macros",
    "tauri-runtime",
    "tauri-runtime-wry",
    "tauri-utils",
    "wry",
    "tao",
    "webkit2gtk",
    "webkit2gtk-sys",
    "javascriptcore-rs",
    "soup3",
    "webview2-com",
    "webview2-com-sys",
    "objc2",
    "objc2-web-kit",
    "objc2-foundation",
    "objc2-app-kit",
    "block2",
];

const PM_LOCK: &str = "../../../src-tauri/Cargo.lock";
const PM_CONFIG: &str = "../../../src-tauri/tauri.conf.json";
const PM_MOD_FRAME: &str = "../../../src/components/mods/ModFrame.tsx";
const HARNESS: &str = "../frontend/harness.js";

fn main() {
    for path in [
        PM_LOCK,
        "Cargo.lock",
        PM_CONFIG,
        "tauri.conf.json",
        PM_MOD_FRAME,
        HARNESS,
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    let mut problems = Vec::new();

    let pm = lock_versions(&read(PM_LOCK));
    let ours = lock_versions(&read("Cargo.lock"));
    for name in ENGINE_CRATES {
        let (want, have) = (pm.get(*name), ours.get(*name));
        if want != have {
            problems.push(format!(
                "{name}: PM's lock has {}, this app's has {}",
                shown(want),
                shown(have)
            ));
        }
    }
    if !pm.contains_key("tauri") {
        problems.push(format!(
            "{PM_LOCK} names no tauri: the lock check is reading the wrong file"
        ));
    }

    let (want, have) = (csp(&read(PM_CONFIG)), csp(&read("tauri.conf.json")));
    if want.is_none() || want != have {
        problems.push(format!(
            "the page's CSP isn't PM's: PM has {want:?}, this app has {have:?}"
        ));
    }

    match mod_frame_shape(&read(PM_MOD_FRAME)) {
        Ok(pm) => {
            let harness = read(HARNESS);
            for (name, attr, want) in [
                ("FRAME_SANDBOX", "sandbox", &pm.sandbox),
                ("FRAME_ALLOW", "allow", &pm.allow),
            ] {
                let have = declared(&harness, name);
                if have.as_ref() != Some(want) {
                    problems.push(format!(
                        "the harness mounts frames with {name} = {have:?}, but ModFrame's iframe has \
                         {attr}={want:?}"
                    ));
                }
            }
        }
        Err(why) => problems.push(format!("{PM_MOD_FRAME}: {why}")),
    }

    if !problems.is_empty() {
        panic!(
            "the sealed-frame app must run on PM's engine, PM's CSP and PM's frame:\n  {}\n\
             Bring tests/sealed-frame/src-tauri/Cargo.lock, tauri.conf.json and \
             frontend/harness.js in line with PM's (`cargo update -p <crate> --precise <version>`).",
            problems.join("\n  ")
        );
    }

    tauri_build::build()
}

/// The two attributes that decide what a mod frame may do.
struct FrameShape {
    sandbox: String,
    allow: String,
}

/// The `sandbox` and `allow` literals on the one `<iframe>` in ModFrame.tsx's code (its comments,
/// which name the element too, are skipped). The ESLint fence (I-24) already holds that element to
/// literal attributes, so a plain scan of its text is exact.
fn mod_frame_shape(source: &str) -> Result<FrameShape, String> {
    let code: String = source
        .lines()
        .filter(|line| {
            let line = line.trim_start();
            !(line.starts_with("//") || line.starts_with("/*") || line.starts_with('*'))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut frames = code.match_indices("<iframe");
    let (Some((at, _)), None) = (frames.next(), frames.next()) else {
        return Err("expected exactly one <iframe in its code".into());
    };
    let element = &code[at..];
    let element = &element[..element.find("/>").ok_or("the <iframe never closes")?];
    let literal = |attr: &str| {
        let key = format!("{attr}=\"");
        let mut found = element
            .match_indices(key.as_str())
            .filter(|(i, _)| element[..*i].ends_with(char::is_whitespace))
            .map(|(i, _)| {
                let rest = &element[i + key.len()..];
                rest.find('"').map(|end| rest[..end].to_owned())
            });
        match (found.next(), found.next()) {
            (Some(Some(value)), None) => Ok(value),
            _ => Err(format!("expected one literal {attr}=\"…\" on the <iframe")),
        }
    };
    Ok(FrameShape {
        sandbox: literal("sandbox")?,
        allow: literal("allow")?,
    })
}

/// A `var NAME = "value";` line in the harness.
fn declared(source: &str, name: &str) -> Option<String> {
    let prefix = format!("var {name} = \"");
    let mut found = source.lines().filter_map(|line| {
        line.trim()
            .strip_prefix(prefix.as_str())?
            .strip_suffix("\";")
    });
    match (found.next(), found.next()) {
        (Some(value), None) => Some(value.to_owned()),
        _ => None,
    }
}

fn read(path: &str) -> String {
    std::fs::read_to_string(Path::new(path)).unwrap_or_else(|e| panic!("can't read {path}: {e}"))
}

/// Every version of every package in a Cargo.lock: each `[[package]]` names itself on a
/// `name = "…"` line with its `version = "…"` on the next.
fn lock_versions(lock: &str) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut lines = lock.lines().map(str::trim);
    while let Some(line) = lines.next() {
        let Some(name) = quoted(line, "name = ") else {
            continue;
        };
        if let Some(version) = lines.next().and_then(|l| quoted(l, "version = ")) {
            out.entry(name.to_owned())
                .or_default()
                .push(version.to_owned());
        }
    }
    for versions in out.values_mut() {
        versions.sort();
    }
    out
}

fn quoted<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.strip_prefix(key)?.strip_prefix('"')?.strip_suffix('"')
}

fn shown(versions: Option<&Vec<String>>) -> String {
    versions.map_or_else(|| "none".to_owned(), |v| v.join(", "))
}

fn csp(config: &str) -> Option<String> {
    let config: serde_json::Value = serde_json::from_str(config).ok()?;
    config["app"]["security"]["csp"].as_str().map(str::to_owned)
}
