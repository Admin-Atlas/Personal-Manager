// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The whole UI of the pop-out Calendar window (the `calendar` webview, popout.rs, #1039). Bobby: "i
// want to have a window which can just display the calendar tab, and the full function / features
// etc. just pop out so i can it side by side with whatever im doing on the main scrren."
//
// The full Calendar tab, and only that. It is NOT `<App/>`: a second App would repeat the whole boot
// (the vault gates, the connector polls, the briefing scheduler's checks, the resume effects) in a
// window showing one tab. `main.tsx` branches on `?window=calendar` and mounts this instead.
//
// What it keeps from App, because the Calendar relies on it:
//   - Its own gate. It shows nothing while PM is locked, the vault is waiting for its passphrase or
//     another profile holds a shared vault (the curtain), and it follows the curtain while open. The
//     backend also refuses to open it while locked (`popout::open`).
//   - The stored preferences (`storedPrefs`): hidden calendars and Runway windows live in the
//     encrypted store, and writes before hydration are dropped. `followStoredPrefs` keeps this window
//     and the main one from writing each other's changes away.
//   - The link interceptor and the sideways-scroll normaliser, which App installs for the whole app.
//
// Links that lead elsewhere in PM (a project, the pinboard) go to the main window and bring it
// forward (`show_in_main_window`).

import { useCallback, useEffect, useState } from "react";
import { CalendarView } from "./components/calendar/CalendarView";
import { Button } from "./components/ui";
import {
  appLockStatus,
  onVaultAcquired,
  onVaultCurtain,
  showInMainWindow,
  vaultLockStatus,
  vaultStatus,
} from "./lib/ipc";
import { followStoredPrefs, hydrateStoredPrefs } from "./lib/storedPrefs";
import { subscribeUntilCleanup } from "./lib/subscribe";
import { useExternalLinks } from "./lib/useExternalLinks";
import { installAxisScrollNormalizer } from "./lib/scrollAxis";

type Gate = "checking" | "open" | "locked" | "curtain";

export default function CalendarWindow() {
  useExternalLinks();
  useEffect(() => installAxisScrollNormalizer(), []);
  useEffect(() => followStoredPrefs(), []);

  const [gate, setGate] = useState<Gate>("checking");
  const check = useCallback(async () => {
    try {
      const [lock, vault, writer] = await Promise.all([
        appLockStatus(),
        vaultStatus(),
        vaultLockStatus().catch(() => null),
      ]);
      if (writer && !writer.active) return setGate("curtain");
      if (lock.locked || vault.needs_unlock || vault.fault) return setGate("locked");
      await hydrateStoredPrefs();
      setGate("open");
    } catch {
      // The store isn't open, which is a lock as far as this window can tell.
      setGate("locked");
    }
  }, []);

  useEffect(() => {
    void check();
    const offs = [
      // Another profile took the shared vault: the store closed under this window too.
      subscribeUntilCleanup(() => onVaultCurtain(() => setGate("curtain"))),
      subscribeUntilCleanup(() => onVaultAcquired(() => void check())),
    ];
    return () => offs.forEach((off) => off());
  }, [check]);

  if (gate === "open")
    return (
      <main className="flex h-full min-h-0 flex-1 flex-col">
        <CalendarView
          onOpenProject={(project) => void showInMainWindow({ view: "project", project })}
          onOpenPinboard={() => void showInMainWindow({ view: "pinboard" })}
        />
      </main>
    );
  if (gate === "checking") return null;
  return (
    <div className="flex h-full items-center justify-center px-6">
      <div className="max-w-sm text-center">
        <p className="text-sm text-ink2">
          {gate === "curtain"
            ? "Another account on this computer is using your vault, so the calendar is hidden."
            : "PM is locked, so the calendar is hidden."}
        </p>
        <p className="mt-1 text-xs text-ink4">
          {gate === "curtain"
            ? "It comes back here when PM has the vault again."
            : "Unlock PM in its main window, then try again."}
        </p>
        {gate === "locked" && (
          <Button variant="secondary" size="sm" onClick={() => void check()} className="mt-3">
            Try again
          </Button>
        )}
      </div>
    </div>
  );
}
