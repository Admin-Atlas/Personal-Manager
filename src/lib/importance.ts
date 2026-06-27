// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Shared sort ranking for document importance (Documents table, Review queue, focus-view Files).
// Higher = more important. Archive sits below everything — below an untriaged document (null → 0) —
// so deliberately shelved files sink to the bottom when sorting by importance.

import type { Importance } from "./types";

export const IMPORTANCE_RANK: Record<string, number> = {
  high: 3,
  medium: 2,
  low: 1,
  archive: -1,
};

/** Sort weight for an importance value; untriaged (`null`) is 0, above archive. */
export const rankImportance = (imp: Importance): number => (imp ? (IMPORTANCE_RANK[imp] ?? 0) : 0);
