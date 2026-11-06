// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The updater feed (latest.json): what build-updater-manifest.mjs writes from a release's
// artifacts, and what verify-updater-manifest.mjs lets through to publishing.
//
// The property that matters most: the manifest states the minimum macOS that tauri.conf.json
// sets, which PM's own update check reads before it downloads (src/lib/updateGate.ts), so a Mac
// whose macOS is too old for an update is never handed one it can't open. Tauri's updater looks
// up `darwin-<the running binary's arch>`, and the one universal Mac bundle is listed under both
// Mac keys. The tests read the configured minimum rather than pinning one, so they keep holding
// when it moves. Both scripts are also run for real, as the publish job runs them, against a fake
// artifacts folder.

import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";

import {
  buildManifest,
  MAC_KEYS,
  MACOS_MINIMUM_FIELD,
  minimumMacosOf,
} from "./build-updater-manifest.mjs";
import { manifestProblems } from "./verify-updater-manifest.mjs";

const scriptsDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(scriptsDir, "..");
const conf = JSON.parse(readFileSync(join(repoRoot, "src-tauri/tauri.conf.json"), "utf8"));

/** The minimum macOS tauri.conf.json sets: what every Mac entry must state. */
const MINIMUM = conf.bundle.macOS.minimumSystemVersion;
/** A well-formed minimum that isn't the configured one. */
const OTHER = MINIMUM === "99.0" ? "98.0" : "99.0";

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
  "PM_3.175.0-alpha_universal.dmg",
  "PM_3.175.0-alpha_amd64.AppImage",
  "PM_3.175.0-alpha_amd64.AppImage.sig",
  "PM-3.175.0-alpha-1.x86_64.rpm",
  "PM_3.175.0-alpha_amd64.deb",
];

/** `minimumMacos` defaults to the configured one only when it isn't passed, so `undefined` can be tested. */
const build = (artifactsDir, ...minimum) =>
  buildManifest({
    artifactsDir,
    releasesRepo: REPO,
    tag: TAG,
    version: VERSION,
    notes: `PM ${TAG}`,
    minimumMacos: minimum.length ? minimum[0] : MINIMUM,
    pubDate: "2026-11-06T00:00:00.000Z",
  });

const check = (manifest, minimumMacos = MINIMUM) =>
  manifestProblems({ manifest, version: VERSION, tag: TAG, releasesRepo: REPO, minimumMacos });

const ALL_KEYS = ["darwin-aarch64", "darwin-x86_64", "linux-x86_64", "windows-x86_64"];

describe("build-updater-manifest", () => {
  it("lists the universal Mac bundle under both Mac keys, beside the minimum macOS", () => {
    const m = build(artifacts(RELEASE_FILES));
    expect(Object.keys(m.platforms).sort()).toEqual(ALL_KEYS);
    expect([...MAC_KEYS].sort()).toEqual(["darwin-aarch64", "darwin-x86_64"]);
    for (const key of MAC_KEYS) {
      expect(m.platforms[key]).toEqual({
        url: `${PREFIX}PM.app.tar.gz`,
        signature: "sig-of-PM.app.tar.gz.sig",
      });
    }
    expect(MACOS_MINIMUM_FIELD).toBe("minimum_macos_version");
    expect(m.minimum_macos_version).toBe(MINIMUM);
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

  it("reads PM's minimum macOS out of tauri.conf.json, in a shape the update check parses", () => {
    expect(minimumMacosOf(conf)).toBe(MINIMUM);
  });

  it("refuses a minimumSystemVersion PM's update check couldn't parse", () => {
    for (const v of [undefined, 14, "", " 14.0", "14.0.0.1", "14.x", "v14"]) {
      expect(() => minimumMacosOf({ bundle: { macOS: { minimumSystemVersion: v } } })).toThrow(
        /minimumSystemVersion/,
      );
    }
    for (const v of ["10.15", "14", "14.0", "15.2.1"]) {
      expect(minimumMacosOf({ bundle: { macOS: { minimumSystemVersion: v } } })).toBe(v);
    }
  });
});

describe("verify-updater-manifest", () => {
  const good = () => build(artifacts(RELEASE_FILES));

  it("asks for the minimum macOS beside either Mac key", () => {
    for (const key of MAC_KEYS) {
      const m = good();
      for (const other of MAC_KEYS) if (other !== key) delete m.platforms[other];
      expect(check(m)).toEqual([]);
      delete m.minimum_macos_version;
      expect(check(m)).toEqual([expect.stringMatching(/minimum_macos_version is undefined/)]);
    }
  });

  it("refuses a platform PM doesn't ship", () => {
    const m = good();
    m.platforms["darwin-aarch64-app"] = m.platforms["darwin-aarch64"];
    expect(check(m)).toEqual([
      expect.stringMatching(/^darwin-aarch64-app: not a platform PM ships/),
    ]);
  });

  it("refuses a Mac entry whose minimum macOS is missing or isn't tauri.conf.json's", () => {
    expect(OTHER).not.toBe(MINIMUM);

    const missing = good();
    delete missing.minimum_macos_version;
    expect(check(missing)).toEqual([expect.stringMatching(/minimum_macos_version is undefined/)]);

    const wrong = good();
    wrong.minimum_macos_version = OTHER;
    expect(check(wrong)).toEqual([
      expect.stringContaining(`minimum_macos_version is ${JSON.stringify(OTHER)}`),
    ]);

    expect(check(good(), OTHER)).toEqual([
      expect.stringContaining(`minimumSystemVersion is ${JSON.stringify(OTHER)}`),
    ]);
    expect(check(good(), "14.0-beta")).toEqual([
      expect.stringMatching(/expected a version like "14.0"/),
    ]);
  });

  it("refuses a minimum macOS on a manifest with no Mac entry", () => {
    const m = good();
    for (const key of MAC_KEYS) delete m.platforms[key];
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
  const run = (script, env, opts = {}) =>
    execFileSync(process.execPath, [join(scriptsDir, script)], {
      cwd: repoRoot,
      env,
      encoding: "utf8",
      ...opts,
    });

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
    run("build-updater-manifest.mjs", env);
    const written = JSON.parse(readFileSync(out, "utf8"));
    expect(Object.keys(written.platforms).sort()).toEqual(ALL_KEYS);
    expect(written.minimum_macos_version).toBe(MINIMUM);

    const ok = run("verify-updater-manifest.mjs", env);
    expect(ok).toContain(`(macOS ${MINIMUM} or newer)`);

    // The verifier checks against the tauri.conf.json it is pointed at: one that sets another
    // minimum fails the publish.
    const otherConf = join(dir, "tauri.conf.json");
    writeFileSync(
      otherConf,
      JSON.stringify({
        ...conf,
        bundle: { ...conf.bundle, macOS: { minimumSystemVersion: OTHER } },
      }),
    );
    expect(() =>
      run("verify-updater-manifest.mjs", { ...env, TAURI_CONF: otherConf }, { stdio: "pipe" }),
    ).toThrow(/minimum_macos_version is .*, but tauri\.conf\.json's minimumSystemVersion is/);

    // And it fails the publish when the minimum is dropped.
    delete written.minimum_macos_version;
    writeFileSync(out, JSON.stringify(written));
    expect(() => run("verify-updater-manifest.mjs", env, { stdio: "pipe" })).toThrow(
      /minimum_macos_version is undefined/,
    );
  });
});
