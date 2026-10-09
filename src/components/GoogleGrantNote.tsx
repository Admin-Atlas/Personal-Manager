// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { GOOGLE_PERMISSIONS_URL, googleGrantNote } from "../lib/googleGrantNote";
import type { GoogleGrantOutcome } from "../lib/googleGrantNote";

/** After a Google disconnect that kept PM's access alive: which features kept it, and where to remove
 *  it by hand. A status readout, so never folded. The live region stays mounted while empty — a
 *  region that mounts already holding its message is never announced — and only its contents come and
 *  go (the SavedTick pattern). */
export function GoogleGrantNote({ outcome }: { outcome: GoogleGrantOutcome | null }) {
  const text = outcome ? googleGrantNote(outcome) : null;
  return (
    <p role="status" className={text ? "mt-2 text-xs text-ink3" : undefined}>
      {text && (
        <>
          {text}{" "}
          <a
            href={GOOGLE_PERMISSIONS_URL}
            target="_blank"
            rel="noreferrer"
            className="underline hover:text-ink2"
          >
            myaccount.google.com
          </a>
          .
        </>
      )}
    </p>
  );
}
