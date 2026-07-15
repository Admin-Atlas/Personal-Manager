// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from "react";
import { HELP, useHelp, type HelpEntry } from "../lib/help";
import { Button } from "./ui";

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
      {/* Above Modal's z-50 scrim: help mode is usable *inside* a dialog too, and at z-40 its
          banner and card were painted behind one — invisible against an opaque dialog. */}
      <div className="pointer-events-none fixed left-1/2 top-3 z-[60] -translate-x-1/2 rounded-full border border-border2 bg-surface px-3 py-1 text-xs text-ink2 shadow-lg">
        Help mode on — hover a highlighted section. Turn off in Settings.
      </div>

      {entry && (
        <div className="pointer-events-none fixed bottom-4 left-1/2 z-[60] w-[28rem] max-w-[calc(100vw-2rem)] -translate-x-1/2 rounded-[var(--radius)] border border-border2 bg-surface p-4 shadow-2xl backdrop-blur">
          <p className="text-sm font-semibold text-accent-text">{entry.title}</p>
          <p className="mt-1 text-sm leading-relaxed text-ink2">{entry.body}</p>
        </div>
      )}

      {/* An always-available way out, in case the user lands somewhere with no
          Settings button in view. */}
      <Button
        variant="secondary"
        onClick={() => setEnabled(false)}
        className="fixed bottom-4 right-4 z-[60] text-xs shadow-lg"
      >
        Exit help mode
      </Button>
    </>
  );
}
