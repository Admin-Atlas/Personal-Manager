// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The shared importance toggle (Review tab + the project focus panel). Four explicit levels;
// untriaged (`null`) renders as no active segment — it's a distinct state, not a level.

import type { Importance } from "../lib/types";
import { SegmentedControl, type SegOption } from "./ui";

/** The selectable importance levels, highest first. `archive` is the explicit "shelved" level —
 *  distinct from untriaged `null` — so a deliberately archived document is hidden from the Map and
 *  sinks to the bottom of importance-sorted lists, while a brand-new untriaged one still shows. */
const IMPORTANCE_LEVELS = ["high", "medium", "low", "archive"] as const;

const IMPORTANCE_OPTIONS: ReadonlyArray<SegOption<string>> = IMPORTANCE_LEVELS.map((level) => ({
  value: level,
  label: level,
}));

/** A segmented High / Medium / Low / Archive picker. An untriaged document (`value === null`)
 *  shows no active segment; choosing any level (incl. Archive) sets it explicitly. */
export function ImportancePicker({
  value,
  onChange,
}: {
  value: Importance;
  onChange: (value: Importance) => void;
}) {
  return (
    <SegmentedControl
      options={IMPORTANCE_OPTIONS}
      value={value ?? ""}
      onChange={(key) => onChange(key as Importance)}
      className="capitalize"
    />
  );
}
