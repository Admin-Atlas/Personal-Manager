// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A section's explanatory prose, folded behind one caret so the controls people came for
// sit together instead of being pushed apart by rationale.
//
// This is the single place that decides such prose starts CLOSED, and it starts closed at
// every depth — which is why there's no `defaultOpen` to pass. Depth reveals *features*
// (`showPower` = cost, token counts, timestamps); explanation isn't a feature, it's ambient
// text that clogged the view for everyone, power users included. Earlier call sites keyed
// these disclosures to `defaultOpen={showPower}`, which had it exactly backwards: the reader
// most likely to already know how backups work was the one who got the essay unfolded.
// Routing every info block through here keeps that from being re-litigated per call site.
// Help mode (`data-help` + the HELP registry in lib/help.ts) stays the on-demand channel for
// the same material, so nothing here is the only copy of anything.
//
// What does NOT belong in here — it reads as commentary but is part of the control:
//   * live status readouts ("Following this device: Europe/London")
//   * gating hints that say why a control is dead ("Enter a strong passphrase to enable")
//   * irreversible-loss warnings ("there's no recovery if you lose it")
// Fold one of those away and the user meets the consequence instead of the warning.

import type { ReactNode } from "react";
import { Collapsible } from "./Collapsible";
import { cn } from "./cn";

export interface SectionInfoProps {
  /** Caret label. Phrase it as the question the reader is actually asking. */
  title?: ReactNode;
  /** Registry id for help mode, mirroring the `data-help` convention elsewhere. */
  helpId?: string;
  children: ReactNode;
  className?: string;
}

export function SectionInfo({
  title = "What this does",
  helpId,
  children,
  className,
}: SectionInfoProps) {
  return (
    <Collapsible
      className={cn("mt-2", className)}
      defaultOpen={false}
      title={<span className="text-xs text-ink3">{title}</span>}
    >
      <div
        className="mt-1.5 space-y-1.5 rounded-[var(--radius)] bg-surface px-3 py-2 text-xs text-ink3"
        data-help={helpId}
      >
        {children}
      </div>
    </Collapsible>
  );
}
