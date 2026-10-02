// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { createContext, useContext } from "react";

import type { SectionId } from "./sections";

/**
 * Where a pointer in the Local AI tab can send someone: a section; the "Settings PM's numbers
 * assume" fold inside Model server (`tuning`); the "Show all … models" fold under All models
 * (`catalog`); or one model's card in that fold (`rec:<repo>`). The tab opens whichever fold the
 * target sits in before it scrolls there.
 */
export type LocalAiTarget = SectionId | "tuning" | "catalog" | `rec:${string}`;

/** The anchor of the "Settings PM's numbers assume" fold inside Model server — where the "tuning"
 *  target scrolls once the tab has opened it. */
export const TUNING_ID = "localai-tuning";

/** That fold's title, the one spelling every pointer to it uses — so a pointer can't go on naming a
 *  fold that has been renamed. */
export const TUNING_TITLE = "Settings PM's numbers assume";

/** The anchor of one model's card under All models: its repo, lower-cased, with every run of
 *  anything that isn't a letter or a digit as one hyphen — so `bartowski/Qwen2.5-7B-Instruct-GGUF`
 *  is `localai-rec-bartowski-qwen2-5-7b-instruct-gguf`. */
export function recCardId(repo: string): string {
  return `localai-rec-${repo.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`;
}

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
