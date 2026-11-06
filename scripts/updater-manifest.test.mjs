// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The updater feed (latest.json): what build-updater-manifest.mjs writes from a release's
// artifacts, and what verify-updater-manifest.mjs lets through to publishing.
//
// The property that matters most: PM on Mac is Apple Silicon only, macOS 14 or newer. Tauri's
// updater looks up `darwin-<the running binary's arch>`, so the Mac bundle must be listed under
// `darwin-aarch64` alone (an Intel Mac on an older, universal PM then finds nothing and keeps its
// version), and the manifest must state the minimum macOS that PM's own update check reads before
// it downloads (src/lib/updateGate.ts). Both scripts are also run for real, as the publish job
// runs them, against a fake artifacts folder.

import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";

import {
  buildManifest,
  MAC_KEY,
  MACOS_MINIMUM_FIELD,
  minimumMacosOf,
} from "./build-updater-manifest.mjs";
import { manifestProblems } from "./verify-updater-manifest.mjs";

const scriptsDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(scriptsDir, "..");
const conf = JSON.parse(readFileSync(join(repoRoot, "src-tauri/tauri.conf.json"), "utf8"));

const REPO = "itsAtlas-org/Personal-Manager";
const TAG = "v3.175.0-alpha";
const VERSION = "3.175.0-alpha";
const PREFIX = `https://github.com/${REPO}/releases/download/${TAG}/`;

const dirs = [];
afterEach(() => {
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true });
});

/** A fake publish-job artifacts folder holding `names`, each `.sig` holding a signature line. */
function artifacts(names) {
  const dir = mkdtempSync(join(tmpdir(), "pm-updater-manifest-"));
  dirs.push(dir);
  for (const name of names) {
    writeFileSync(join(dir, name), name.endsWith(".sig") ? `sig-of-${name}\n` : "bytes");
  }
  return dir;
}

/** Everything a real release stages: the three updater artifacts, their sigs and the plain assets. */
const RELEASE_FILES = [
  "PM_3.175.0-alpha_x64-setup.exe",
  "PM_3.175.0-alpha_x64-setup.exe.sig",
  "PM.app.tar.gz",
  "PM.app.tar.gz.sig",
  "PM_3.175.0-alpha_aarch64.dmg",
  "PM_3.175.0-alpha_amd64.AppImage",
  "PM_3.175.0-alpha_amd64.AppImage.sig",
  "PM-3.175.0-alpha-1.x86_64.rpm",
  "PM_3.175.0-alpha_amd64.deb",
];

/** `minimumMacos` defaults to "14.0" only when it isn't passed, so `undefined` can be tested. */
const build = (artifactsDir, ...minimum) =>
  buildManifest({
    artifactsDir,
    releasesRepo: REPO,
    tag: TAG,
    version: VERSION,
    notes: `PM ${TAG}`,
    minimumMacos: minimum.length ? minimum[0] : "14.0",
    pubDate: "2026-11-06T00:00:00.000Z",
  });

const check = (manifest, minimumMacos = "14.0") =>
  manifestProblems({ manifest, version: VERSION, tag: TAG, releasesRepo: REPO, minimumMacos });

describe("build-updater-manifest", () => {
  it("lists the Mac bundle under darwin-aarch64 alone, beside the minimum macOS", () => {
    const m = build(artifacts(RELEASE_FILES));
    expect(Object.keys(m.platforms).sort()).toEqual([
      "darwin-aarch64",
      "linux-x86_64",
      "windows-x86_64",
    ]);
    expect(MAC_KEY).toBe("darwin-aarch64");
    // What an Intel Mac's updater looks up, with and without the bundle-type suffix.
    expect(m.platforms["darwin-x86_64"]).toBeUndefined();
    expect(m.platforms["darwin-x86_64-app"]).toBeUndefined();
    expect(m.platforms["darwin-aarch64"]).toEqual({
      url: `${PREFIX}PM.app.tar.gz`,
      signature: "sig-of-PM.app.tar.gz.sig",
    });
    expect(MACOS_MINIMUM_FIELD).toBe("minimum_macos_version");
    expect(m.minimum_macos_version).toBe("14.0");
    expect(check(m)).toEqual([]);
  });

  it("states no minimum macOS when the release has no Mac bundle", () => {
    const m = build(
      artifacts([
        "PM_3.175.0-alpha_x64-setup.exe",
        "PM_3.175.0-alpha_x64-setup.exe.sig",
        "PM_3.175.0-alpha_amd64.AppImage",
        "PM_3.175.0-alpha_amd64.AppImage.sig",
      ]),
    );
    expect(m).not.toHaveProperty("minimum_macos_version");
    expect(check(m)).toEqual([]);
  });

  it("refuses a Mac bundle without a readable minimum macOS", () => {
    const dir = artifacts(["PM.app.tar.gz", "PM.app.tar.gz.sig"]);
    for (const bad of [undefined, "", "14.0 ", "fourteen", "14.0-beta"]) {
      expect(() => build(dir, bad)).toThrow(/needs the minimum macOS/);
    }
  });

  it("still refuses an ambiguous updater artifact and a bundle without its .sig", () => {
    expect(() => build(artifacts(["a.app.tar.gz", "a.app.tar.gz.sig", "b.app.tar.gz"]))).toThrow(
      /Ambiguous updater artifact/,
    );
    expect(() => build(artifacts(["PM.app.tar.gz"]))).toThrow(/no matching \.sig/);
  });

  it("reads PM's minimum macOS out of tauri.conf.json: 14.0", () => {
    expect(minimumMacosOf(conf)).toBe("14.0");
  });

  it("refuses a minimumSystemVersion PM's update check couldn't parse", () => {
    for (const v of [undefined, 14, "", " 14.0", "14.0.0.1", "14.x", "v14"]) {
      expect(() => minimumMacosOf({ bundle: { macOS: { minimumSystemVersion: v } } })).toThrow(
        /minimumSystemVersion/,
      );
    }
    for (const v of ["14", "14.0", "15.2.1"]) {
      expect(minimumMacosOf({ bundle: { macOS: { minimumSystemVersion: v } } })).toBe(v);
    }
  });
});

describe("verify-updater-manifest", () => {
  const good = () => build(artifacts(RELEASE_FILES));

  it("refuses a manifest that offers an update to Intel Macs", () => {
    const m = good();
    m.platforms["darwin-x86_64"] = m.platforms["darwin-aarch64"];
    expect(check(m)).toEqual([expect.stringMatching(/^darwin-x86_64: PM on Mac is Apple Silicon/)]);
  });

  it("refuses a platform PM doesn't ship", () => {
    const m = good();
    m.platforms["darwin-aarch64-app"] = m.platforms["darwin-aarch64"];
    expect(check(m)).toEqual([
      expect.stringMatching(/^darwin-aarch64-app: not a platform PM ships/),
    ]);
  });

  it("refuses a Mac entry whose minimum macOS is missing or isn't tauri.conf.json's", () => {
    const missing = good();
    delete missing.minimum_macos_version;
    expect(check(missing)).toEqual([expect.stringMatching(/minimum_macos_version is undefined/)]);

    const wrong = good();
    wrong.minimum_macos_version = "13.0";
    expect(check(wrong)).toEqual([expect.stringMatching(/minimum_macos_version is "13.0"/)]);

    expect(check(good(), "15.0")).toEqual([
      expect.stringMatching(/minimumSystemVersion is "15.0"/),
    ]);
    expect(check(good(), "14.0-beta")).toEqual([
      expect.stringMatching(/expected a version like "14.0"/),
    ]);
  });

  it("refuses a minimum macOS on a manifest with no Mac entry", () => {
    const m = good();
    delete m.platforms["darwin-aarch64"];
    expect(check(m)).toEqual(["minimum_macos_version is set, but the manifest has no Mac entry"]);
  });

  it("keeps its earlier checks: version, url and signature", () => {
    const m = good();
    m.version = "3.174.2-alpha";
    m.platforms["windows-x86_64"].url = "https://example.invalid/PM-setup.exe";
    m.platforms["linux-x86_64"].signature = "";
    expect(check(m)).toEqual([
      'manifest.version "3.174.2-alpha" ≠ release version "3.175.0-alpha"',
      expect.stringMatching(/^windows-x86_64: url does not point at this release/),
      "linux-x86_64: missing signature",
    ]);
    expect(check({ version: VERSION, platforms: {} })).toEqual(["no platforms in the manifest"]);
  });
});

describe("the two scripts, run as the publish job runs them", () => {
  it("build a manifest from an artifacts folder that the verifier then accepts", () => {
    const dir = artifacts(RELEASE_FILES);
    const out = join(dir, "latest.json");
    const env = {
      PATH: process.env.PATH,
      ARTIFACTS_DIR: dir,
      RELEASES_REPO: REPO,
      TAG,
      VERSION,
      NOTES: `PM ${TAG}`,
      OUT: out,
    };
    execFileSync(process.execPath, [join(scriptsDir, "build-updater-manifest.mjs")], {
      cwd: repoRoot,
      env,
      encoding: "utf8",
    });
    const written = JSON.parse(readFileSync(out, "utf8"));
    expect(Object.keys(written.platforms).sort()).toEqual([
      "darwin-aarch64",
      "linux-x86_64",
      "windows-x86_64",
    ]);
    expect(written.minimum_macos_version).toBe(conf.bundle.macOS.minimumSystemVersion);

    const ok = execFileSync(process.execPath, [join(scriptsDir, "verify-updater-manifest.mjs")], {
      cwd: repoRoot,
      env,
      encoding: "utf8",
    });
    expect(ok).toMatch(/✓ updater manifest ok: .* \(macOS 14\.0 or newer\)/);

    // And the verifier fails the publish when the Intel key comes back.
    written.platforms["darwin-x86_64"] = written.platforms["darwin-aarch64"];
    writeFileSync(out, JSON.stringify(written));
    expect(() =>
      execFileSync(process.execPath, [join(scriptsDir, "verify-updater-manifest.mjs")], {
        cwd: repoRoot,
        env,
        encoding: "utf8",
        stdio: "pipe",
      }),
    ).toThrow(/darwin-x86_64: PM on Mac is Apple Silicon only/);
  });
});
