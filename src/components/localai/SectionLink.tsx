// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { ReactNode } from "react";

import { LocateContext, useLocate, type Locate } from "./locate";
import { sectionLabel, type SectionId } from "./sections";

/** Hands the tab's `locate` to every `SectionLink` inside it. */
export function LocateProvider({ locate, children }: { locate: Locate; children: ReactNode }) {
  return <LocateContext.Provider value={locate}>{children}</LocateContext.Provider>;
}

/**
 * A section's name inside a sentence, as a way to get there.
 *
 * Copy that sends someone somewhere names the section rather than pointing "above" or "below": the
 * tab's order is the rail's, and a direction word is wrong the moment a section moves. The name comes
 * from `sections.ts`, so renaming a section renames every pointer to it.
 *
 * Inside the tab it is a button that scrolls there. Outside one — a section rendered on its own — it
 * is the name in plain text, because a control that does nothing is worse than none.
 */
export function SectionLink({ to }: { to: SectionId }) {
  const locate = useLocate();
  if (!locate) return <span className="text-ink2">{sectionLabel(to)}</span>;
  return (
    <button
      type="button"
      onClick={() => locate(to)}
      className="text-accent-text underline decoration-dotted underline-offset-2"
    >
      {sectionLabel(to)}
    </button>
  );
}
