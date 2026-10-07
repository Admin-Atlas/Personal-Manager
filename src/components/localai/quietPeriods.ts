// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

/** The quiet periods offered, shared by Model memory's "Quiet period" and On battery's "On battery,
 *  hand the memory back" — two pickers for the same kind of wait, so they offer the same waits. */
export const QUIET_MINUTES = [1, 2, 5, 10, 15, 30, 60];

/** "1 minute" / "5 minutes". */
export function minutes(n: number): string {
  return n === 1 ? "1 minute" : `${n} minutes`;
}

/** The quiet periods plus a stored one that isn't among them (in order), so the Select shows what
 *  is really stored rather than snapping to a neighbour. */
export function withStored(options: readonly number[], stored: number | null): number[] {
  const out = [...options];
  if (stored != null && !out.includes(stored)) {
    out.push(stored);
    out.sort((a, b) => a - b);
  }
  return out;
}
