// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { createContext, useContext } from "react";

import type { SectionId } from "./sections";

/**
 * Where a pointer in the Local AI tab can send someone: a section, or the "Settings PM's numbers
 * assume" fold inside Model server, which the tab opens before it scrolls there.
 */
export type LocalAiTarget = SectionId | "tuning";

/** The anchor of the "Settings PM's numbers assume" fold inside Model server — where the "tuning"
 *  target scrolls once the tab has opened it. */
export const TUNING_ID = "localai-tuning";

/** Take the reader to `target`, opening whatever fold it sits in first. */
export type Locate = (target: LocalAiTarget) => void;

/**
 * The tab's `locate`, for any pointer rendered inside it. null outside the tab — a section rendered
 * on its own, as its tests do — where a pointer has nowhere to go and renders as plain text.
 *
 * Kept apart from `SectionLink.tsx` so that file exports only components, which is what fast
 * refresh needs.
 */
export const LocateContext = createContext<Locate | null>(null);

/** The tab's `locate`, or null when there is no tab around this component. */
export function useLocate(): Locate | null {
  return useContext(LocateContext);
}
