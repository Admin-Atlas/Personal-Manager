// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Settings footer's save state, in one line.
//
// It replaces a standing sentence ("Changes are saved as you make them") that was true but said the
// same thing on every render, so it never actually confirmed anything. Three states:
//
//   • something uncommitted on this tab  → name it, so leaving isn't a surprise
//   • a write landed in the last moments → "Saved ✓"
//   • otherwise                          → the standing explanation, still true and still useful
//
// The tick is deliberately NOT sticky. A permanent tick is the same non-signal as the sentence was;
// it has to appear in response to something to mean anything.

import { useEffect, useState } from "react";
import { onSettingSaved } from "../../lib/settingsSaved";

/** How long the acknowledgement stays up. Long enough to notice out of the corner of an eye after
 *  looking away from the footer to the control you just changed, short enough that it is clearly
 *  about the thing you just did. */
const HOLD_MS = 2200;

interface Props {
  /** Names of settings edited on this tab but not yet committed. */
  pendingLabels: string[];
  /** Bumped by the parent when the explicit Save runs, so the tick fires even when that save had
   *  nothing to commit and therefore triggered no write of its own. */
  savedAt: number | null;
}

export function SavedTick({ pendingLabels, savedAt }: Props) {
  const [flashedAt, setFlashedAt] = useState<number | null>(null);

  // Any settings write anywhere in the app lights this while Settings is open. `Date.now()` in the
  // handler rather than a boolean, so a second save during the hold restarts it instead of the
  // first one's timer cutting the second one short.
  useEffect(() => onSettingSaved(() => setFlashedAt(Date.now())), []);
  useEffect(() => {
    if (savedAt) setFlashedAt(savedAt);
  }, [savedAt]);

  useEffect(() => {
    if (flashedAt === null) return;
    const id = setTimeout(() => setFlashedAt(null), HOLD_MS);
    return () => clearTimeout(id);
  }, [flashedAt]);

  if (pendingLabels.length > 0) {
    return (
      <p className="min-w-0 text-xs text-st-due">
        Not saved yet: <span className="text-ink3">{pendingLabels.join(", ")}</span>
      </p>
    );
  }

  // `role="status"` + aria-live on the WRAPPER, which is always mounted: an aria-live region only
  // announces changes to a region that already existed, so putting it on the tick itself would mean
  // a screen reader hears nothing the first time.
  return (
    <p className="min-w-0 text-xs text-ink4" role="status" aria-live="polite">
      {flashedAt !== null ? (
        <span className="text-accent-text">Saved ✓</span>
      ) : (
        "Changes are saved as you make them."
      )}
    </p>
  );
}
