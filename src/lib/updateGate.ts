// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Pure decision for the auto-updater's "did a previous install attempt silently fail?" marker.
//
// On Windows the updater plugin applies an update by launching the installer and exiting the
// process. It does notice a launch that fails outright (install() throws, and `useUpdater`
// handles it), but anything that stops the installer once it is running happens after PM has
// exited: the app reopens on the OLD version with no error, and the download-and-fail then
// repeats every launch with no signal. To break that, `useUpdater`
// records the version it is about to install, and on the next launch compares that marker
// against the version now running and the version the feed is offering. This module is that
// comparison, kept pure so it can be unit-tested without Tauri/localStorage.

export interface AttemptMarkerInput {
  /** The version we last recorded an install attempt for, or null if none is pending. */
  attempted: string | null;
  /** The version currently running (from the Tauri app API). */
  running: string;
  /** The version the update feed is offering now, or null if the feed offers nothing. */
  offered: string | null;
}

export interface AttemptMarkerDecision {
  /** The offered update equals one we already tried and we are still not on it → likely a
   *  silent OS block; the banner should warn instead of offering a plain restart. */
  blocked: boolean;
  /** The marker is stale (the update applied, or a newer version supersedes it) and should
   *  be cleared so it can't produce a false "blocked" later. */
  clearMarker: boolean;
}

/**
 * Decide what a pending install-attempt marker means this launch.
 *
 * - No marker → nothing to say.
 * - We are now running the attempted version → it applied; clear the marker.
 * - The feed offers the same version we attempted (and we're not on it) → it was blocked.
 * - The feed offers a *different* version → the old attempt is moot; clear the marker.
 * - The feed offers nothing (offline / no update) → inconclusive; keep the marker for later.
 */
export function evaluateAttemptMarker(input: AttemptMarkerInput): AttemptMarkerDecision {
  const { attempted, running, offered } = input;
  if (!attempted) return { blocked: false, clearMarker: false };
  if (attempted === running) return { blocked: false, clearMarker: true };
  if (offered !== null && offered === attempted) return { blocked: true, clearMarker: false };
  if (offered !== null && offered !== attempted) return { blocked: false, clearMarker: true };
  return { blocked: false, clearMarker: false };
}

// ---- macOS minimum ------------------------------------------------------------------------------
//
// An update can need a newer macOS than this Mac has. macOS refuses to open an app whose minimum
// (Info.plist's LSMinimumSystemVersion) is above the running version, and the updater replaces PM
// in place, so installing that update would leave a PM that doesn't open. The release states the
// minimum in latest.json (`minimum_macos_version`, written from tauri.conf.json by
// scripts/build-updater-manifest.mjs and checked by verify-updater-manifest.mjs), and `useUpdater`
// asks this before it downloads anything.

/** The latest.json field stating the oldest macOS the offered release opens on. */
export const MACOS_MINIMUM_FIELD = "minimum_macos_version";

/** The minimum as the release states it: "14", "14.0" or "14.0.1" (the verifier's own shape). */
const STATED_MINIMUM = /^\d{1,3}(\.\d{1,3}){0,2}$/;

/** The numeric start of the running version, so "13.4.1 (a)" still reads as 13.4.1. */
const RUNNING_VERSION = /^(\d+)(?:\.(\d+))?(?:\.(\d+))?/;

/** Compare dotted versions part by part, a missing part counting as 0. */
function compareVersions(a: number[], b: number[]): number {
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    const d = (a[i] ?? 0) - (b[i] ?? 0);
    if (d !== 0) return d;
  }
  return 0;
}

/** "14.0" → "14", "14.2.0" → "14.2": how a person names a macOS version. */
function displayVersion(parts: number[]): string {
  const shown = [...parts];
  while (shown.length > 1 && shown[shown.length - 1] === 0) shown.pop();
  return shown.join(".");
}

export interface MacosBlock {
  /** The macOS the offered update needs, as a person names it ("14"). */
  required: string;
  /** This Mac's macOS, as the system states it ("13.6.1"). */
  running: string;
}

/**
 * Whether the offered update must be held back because it needs a newer macOS than this Mac has.
 *
 * `manifest` is the release feed as the updater received it (the plugin's `rawJson`), and
 * `runningMacos` is this Mac's version, null off macOS. Null (go ahead) when this isn't a Mac or
 * its version can't be read, when the feed states no minimum (every release before 3.175.0), or
 * when the stated minimum isn't one the release verifier would have let through — the verifier is
 * what guarantees the shape, and a feed that broke it says nothing reliable.
 */
export function macosUpdateBlock(
  manifest: unknown,
  runningMacos: string | null,
): MacosBlock | null {
  if (runningMacos === null) return null;
  const running = RUNNING_VERSION.exec(runningMacos.trim());
  if (!running) return null;
  const stated =
    typeof manifest === "object" && manifest !== null
      ? (manifest as Record<string, unknown>)[MACOS_MINIMUM_FIELD]
      : undefined;
  if (typeof stated !== "string" || !STATED_MINIMUM.test(stated)) return null;

  const required = stated.split(".").map(Number);
  const have = running
    .slice(1)
    .filter((p) => p !== undefined)
    .map(Number);
  if (compareVersions(have, required) >= 0) return null;
  return { required: displayVersion(required), running: runningMacos.trim() };
}
