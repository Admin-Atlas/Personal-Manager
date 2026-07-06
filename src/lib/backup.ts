// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { BackupPhase } from "./types";

// Small, UI-free helpers for the Backup panel — pulled out of the component so the pure logic
// (phase classification, the partial-failure banner copy) is unit-tested on its own.

/**
 * Phases whose progress fraction isn't a true measure of the work done, so the bar should show an
 * indeterminate shimmer rather than a percentage (F-45). The upload/download fraction is a coarse
 * per-destination fan-out step (0 then 1 for a single destination) — the Proton CLI has no byte
 * callback and the Google resumable upload isn't metered — so a percent bar sits frozen at 0%
 * through a minutes-long transfer. Matches the design system's "opaque phases always shimmer" rule.
 */
export function isOpaquePhase(phase: BackupPhase | null): boolean {
  return phase === "upload" || phase === "download";
}

/**
 * Copy for the non-blocking "backed up, but some destinations failed" banner (F-22). A backup run
 * fans out to every enabled destination and stamps success per-destination; when at least one
 * succeeds but others fail, the backend returns the failures as `"<label>: <error>"` strings. Null
 * when nothing failed (the clean-run case), so the caller can `setWarning(describeFailures(...))`
 * directly.
 */
export function describeFailures(failed: readonly string[]): string | null {
  if (failed.length === 0) return null;
  const noun = failed.length === 1 ? "destination" : "destinations";
  return `Backed up, but ${failed.length} ${noun} failed — ${failed.join("; ")}. The archive did reach the destinations that succeeded.`;
}
