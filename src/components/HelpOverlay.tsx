// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from "react";
import { HELP, useHelp, type HelpEntry } from "../lib/help";

/**
 * When help mode is on (Step 4b), this listens for the section the user is
 * hovering and shows its plain-language explanation in a fixed panel, plus a
 * small banner reminding them help mode is active. Sections opt in with a
 * `data-help="<id>"` attribute that maps to an entry in `HELP`.
 */
export function HelpOverlay() {
  const { enabled, setEnabled } = useHelp();
  const [entry, setEntry] = useState<HelpEntry | null>(null);

  useEffect(() => {
    if (!enabled) {
      setEntry(null);
      return;
    }
    function onOver(e: MouseEvent) {
      const target = e.target;
      if (!(target instanceof Element)) return;
      const node = target.closest<HTMLElement>("[data-help]");
      const id = node?.dataset.help;
      setEntry(id && HELP[id] ? HELP[id] : null);
    }
    document.addEventListener("mouseover", onOver);
    return () => document.removeEventListener("mouseover", onOver);
  }, [enabled]);

  if (!enabled) return null;

  return (
    <>
      <div className="pointer-events-none fixed left-1/2 top-3 z-40 -translate-x-1/2 rounded-full border border-amber-700/60 bg-amber-950/70 px-3 py-1 text-xs text-amber-200 shadow-lg">
        Help mode on — hover a highlighted section. Turn off in Settings.
      </div>

      {entry && (
        <div className="pointer-events-none fixed bottom-4 left-1/2 z-40 w-[28rem] max-w-[calc(100vw-2rem)] -translate-x-1/2 rounded-xl border border-amber-800/60 bg-neutral-900/95 p-4 shadow-2xl backdrop-blur">
          <p className="text-sm font-semibold text-amber-200">{entry.title}</p>
          <p className="mt-1 text-sm leading-relaxed text-neutral-300">{entry.body}</p>
        </div>
      )}

      {/* An always-available way out, in case the user lands somewhere with no
          Settings button in view. */}
      <button
        onClick={() => setEnabled(false)}
        className="fixed bottom-4 right-4 z-40 rounded-lg border border-neutral-700 bg-neutral-900/90 px-3 py-1.5 text-xs text-neutral-300 shadow-lg hover:bg-neutral-800"
      >
        Exit help mode
      </button>
    </>
  );
}
