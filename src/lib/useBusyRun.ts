// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useState } from "react";
import { runMutation } from "./runMutation";

/**
 * The busy/error latch shared by the connector blocks (credentials, calendar accounts, iCal
 * feeds): `run(label, fn)` holds `busy` at `label` while `fn` runs, routes a rejection into
 * `error` via {@link runMutation}, and always releases the latch. `busy` doubles as the
 * disable-everything flag and the per-action spinner discriminator ("save"/"sync"/…); `setError`
 * is exposed because the host components also surface refetch failures through the same line.
 * Deliberately the thin variant — the Teach view and the vault surfaces keep their own fuller
 * `run` shapes (extra reload/status steps).
 */
export function useBusyRun() {
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const run = useCallback(async (label: string, fn: () => Promise<void>): Promise<void> => {
    setBusy(label);
    await runMutation(fn, setError); // never rejects, so the latch always releases
    setBusy(null);
  }, []);

  return { busy, error, setError, run };
}
