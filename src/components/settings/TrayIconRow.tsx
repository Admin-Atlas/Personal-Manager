// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from "react";

import { getTrayEnabled, setTrayEnabled } from "../../lib/ipc";
import { SettingRow, Toggle } from "../ui";

/**
 * The tray / menu-bar icon toggle, rendered in TWO places: General, where it has always lived, and
 * Local AI, where it decides something quite different.
 *
 * A mirrored control is normally exactly what this codebase deletes — the #538 audit removed a set of
 * them and the doctrine since has been "one control, one home". Three things make this the exception
 * rather than a relapse, and they are written down here so the next delivery audit does not have to
 * re-derive them:
 *
 *   1. **The truth is a backend row**, not component state. Both homes read and write the same
 *      setting through the same two commands, so they cannot disagree — which was the actual failure
 *      mode the doctrine exists to prevent.
 *   2. **The two homes are never mounted together.** The settings tabs are conditional renders, so
 *      only one of these is ever on screen; there is no "which one is right" moment for a user.
 *   3. **It genuinely governs the Local AI decision.** With the tray on, closing the window leaves PM
 *      running — which is the difference between "release the card when I close this" and "release it
 *      when I quit", and the release policy sitting three rows below is meaningless without it. Making
 *      someone leave the tab to find out whether closing the window even ends the session would be a
 *      worse answer than a shared row.
 *
 * There is a live precedent for exactly this shape: Text size is owned by Accessibility and mirrored
 * into General's Appearance section.
 */
export function TrayIconRow({ helpId }: { helpId: string }) {
  // Backend-owned (Rust reads it at boot), so it loads asynchronously rather than seeding from
  // localStorage the way its neighbours in General do.
  const [on, setOn] = useState(false);
  useEffect(() => {
    let alive = true;
    getTrayEnabled()
      .then((v) => alive && setOn(v))
      .catch(() => {
        /* leave it off — the tray is optional */
      });
    return () => {
      alive = false;
    };
  }, []);

  function change(next: boolean) {
    // Optimistic, with a rollback: the toggle should move under the finger, and a failed write must
    // not leave the UI claiming a state the backend never took.
    setOn(next);
    void setTrayEnabled(next).catch(() => setOn(!next));
  }

  return (
    <SettingRow label="Tray / menu bar icon" helpId={helpId}>
      {(a11y) => <Toggle {...a11y} checked={on} onChange={change} />}
    </SettingRow>
  );
}
