// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Pure decision for the auto-updater's "did a previous install attempt silently fail?" marker.
//
// On Windows the updater plugin applies an update by launching the installer and exiting the
// process without observing whether it launched, so an OS-level block (Smart App Control, or a
// SmartScreen "Don't run") tears the app down with no error and it reopens on the OLD version.
// The download-and-fail then repeats every launch with no signal. To break that, `useUpdater`
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
