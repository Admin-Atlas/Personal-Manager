// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The register of settings a user has EDITED BUT NOT COMMITTED, so Settings can refuse to walk away
// from them silently.
//
// Almost every control in Settings writes on change and has nothing to register. The exceptions are
// the controls where writing per keystroke would be actively wrong — the backup schedule is the
// standing example: committing a retention of "1" on the way to typing "15" would briefly make it
// the live schedule and could delete backups. Those keep an explicit Save, and register here so that
// leaving the tab (or closing Settings) asks first, and names what is at stake rather than showing a
// bare "unsaved changes" scare.
//
// Deliberately a registry rather than a prop chain: the tabs are independently-mounted components
// several levels down, and threading a dirty flag up through each one would mean every future
// deferred control has to remember to wire it. Registering is one hook call at the control.

import { createContext, useContext, useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";

/** One tab's uncommitted edit. `labels` names the affected settings in the user's own words —
 *  they're read back in the confirm dialog, so "Backup frequency", not "freqDraft". */
export interface PendingChange {
  tab: string;
  labels: string[];
  save: () => Promise<void>;
}

interface Registry {
  /** Register/replace/clear (null) the entry for `id`. */
  set: (id: string, entry: PendingChange | null) => void;
  /** Every uncommitted entry for a tab. */
  forTab: (tab: string) => PendingChange[];
  /** All labels for a tab, flattened — what the confirm dialog lists. */
  labelsForTab: (tab: string) => string[];
  /** Commit every uncommitted entry for a tab. Rejects on the first failure, leaving the rest
   *  registered — a half-saved tab must not look clean. */
  saveTab: (tab: string) => Promise<void>;
  /** Bumped on every register/clear, so consumers re-render when dirtiness changes. */
  version: number;
}

const Ctx = createContext<Registry | null>(null);

export function SettingsPendingProvider({ children }: { children: ReactNode }) {
  // The entries live in a ref (they hold callbacks and are read imperatively at guard time);
  // `version` is the render signal. Keeping them out of state avoids re-rendering every tab each
  // time a draft changes by one character.
  const entries = useRef(new Map<string, PendingChange>());
  const [version, setVersion] = useState(0);

  const value = useMemo<Registry>(
    () => ({
      version,
      set(id, entry) {
        const had = entries.current.has(id);
        if (entry) entries.current.set(id, entry);
        else entries.current.delete(id);
        // Only re-render when dirtiness actually flips. A draft edited from "1" to "15" stays
        // registered throughout and must not re-render the tree on each keystroke.
        if (had !== !!entry) setVersion((v) => v + 1);
      },
      forTab(tab) {
        return [...entries.current.values()].filter((e) => e.tab === tab);
      },
      labelsForTab(tab) {
        return [...entries.current.values()].filter((e) => e.tab === tab).flatMap((e) => e.labels);
      },
      async saveTab(tab) {
        for (const e of [...entries.current.values()].filter((x) => x.tab === tab)) {
          await e.save();
        }
      },
    }),
    [version],
  );

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

/** The registry. Returns null outside a provider — the onboarding wizard renders no tabs and needs
 *  no guard, so callers treat null as "nothing is pending". */
export function useSettingsPending(): Registry | null {
  return useContext(Ctx);
}

/**
 * Register an uncommitted edit for as long as `dirty` holds.
 *
 * `save` is kept in a ref so a caller can pass a fresh closure each render — the common case, since
 * it closes over the draft state — without the effect re-running and thrashing the registry.
 * Unregisters on unmount, so a tab that is switched away from cannot leave a phantom entry behind
 * (the guard runs BEFORE the switch, so by the time it unmounts the user has already decided).
 */
export function useRegisterPending(
  id: string,
  tab: string,
  dirty: boolean,
  labels: string[],
  save: () => Promise<void>,
): void {
  const reg = useSettingsPending();
  const saveRef = useRef(save);
  saveRef.current = save;
  // Same for the labels: a new array literal each render must not re-run the effect. Keyed by
  // its JSON, not a join: these are user-facing phrases with spaces in them ("Backup
  // frequency"), so splitting a joined string back apart would shred them into separate words.
  const labelsRef = useRef(labels);
  labelsRef.current = labels;
  const labelKey = JSON.stringify(labels);

  useEffect(() => {
    if (!reg) return;
    if (!dirty) {
      reg.set(id, null);
      return;
    }
    reg.set(id, { tab, labels: labelsRef.current, save: () => saveRef.current() });
    return () => reg.set(id, null);
  }, [reg, id, tab, dirty, labelKey]);
}
