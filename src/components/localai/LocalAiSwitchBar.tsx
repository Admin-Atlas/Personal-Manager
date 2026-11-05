// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useState } from "react";

import { setLocalAiEnabled } from "../../lib/localAiSwitch";
import { Callout, ConfirmDialog, SettingRow, Toggle } from "../ui";

/** The Local AI switch (D7), at the top of the tab it switches. The tab exists only while it is
 *  on, so here it only ever reads on, and switching off takes the tab away: the reader lands on AI
 *  & Models, where the way back on and the "pick a model" prompt live ("one control, one home":
 *  never both at once). Outside the reset's remount key and not a section: it is the tab's master,
 *  not one of its settings, and the tab's reset leaves it. */
export function LocalAiSwitchBar({ onSwitchedOff }: { onSwitchedOff?: () => void } = {}) {
  const [confirmOff, setConfirmOff] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function turnOff() {
    setBusy(true);
    setError(null);
    try {
      await setLocalAiEnabled(false);
      setConfirmOff(false);
      onSwitchedOff?.();
    } catch (e) {
      setError(String(e));
      setConfirmOff(false);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="mt-5">
      <SettingRow
        label="Local AI"
        description="Run AI on your own computer. Turn this off and PM stops using local models, and this tab goes away until you turn it back on in AI & Models."
        helpId="settings-localai-switch"
        emphasis="strong"
        spacing="none"
      >
        {(a11y) => (
          <Toggle
            {...a11y}
            checked
            disabled={busy}
            onChange={(on) => {
              if (!on) setConfirmOff(true);
            }}
          />
        )}
      </SettingRow>
      {error && <Callout className="mt-2">{error}</Callout>}
      {/* What off does, said before it happens, in the order it happens. The Ollama line is the
          one people would otherwise assume the other way: PM can pull a model but has no delete. */}
      <ConfirmDialog
        open={confirmOff}
        title="Turn off Local AI?"
        confirmLabel="Turn off"
        busy={busy}
        onConfirm={() => void turnOff()}
        onClose={() => setConfirmOff(false)}
      >
        <p>
          PM stops using your model server. The models you chose for chat and background work are
          cleared, a download or test that&apos;s running stops, and PM hands back the memory it was
          using on your graphics card.
        </p>
        <p className="mt-2">
          Anything set to use a local model won&apos;t run until you pick a model in AI &amp;
          Models. Nothing moves to the cloud on its own.
        </p>
        <p className="mt-2">
          Models you downloaded into Ollama stay there. PM doesn&apos;t delete them, so they keep
          using disk space until you remove them in Ollama.
        </p>
        <p className="mt-2">
          PM keeps your server&apos;s address and token, so turning Local AI back on reconnects in
          one click. You then pick your models again; until you do, those jobs still don&apos;t move
          to the cloud.
        </p>
      </ConfirmDialog>
    </div>
  );
}
