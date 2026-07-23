// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useState, type ReactNode } from "react";

import { Button, ConfirmDialog } from "../ui";

/** A small inline "Reset" affordance shown next to a control that differs from its default (#445).
 *  The caller decides when to render it (i.e. when `value !== default`); this is just the button,
 *  styled to sit quietly beside a label or control. */
export function ResetLink({
  onReset,
  label = "Reset",
  title = "Reset to default",
}: {
  onReset: () => void;
  label?: string;
  title?: string;
}) {
  return (
    <button
      type="button"
      onClick={onReset}
      title={title}
      className="shrink-0 text-xs text-ink4 underline-offset-2 transition hover:text-ink hover:underline"
    >
      {label}
    </button>
  );
}

/** The guarded per-tab "Reset this tab to defaults" footer (#445): a secondary button that opens a
 *  ConfirmDialog restating exactly what will (and won't) reset. Disabled — with a plain "already at
 *  default" note — when the tab has nothing to restore, so the control still tells the user it exists.
 *  `onReset` may be async (it awaits the backend writes); an error surfaces inline. */
export function TabResetSection({
  tabName,
  isDefault,
  onReset,
  confirmBody,
}: {
  tabName: string;
  isDefault: boolean;
  onReset: () => void | Promise<void>;
  confirmBody: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function run() {
    setOpen(false);
    setError(null);
    setBusy(true);
    try {
      await onReset();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="mt-6 border-t border-border pt-4">
      <div className="flex items-center justify-between gap-3">
        <div className="min-w-0">
          <p className="text-sm font-medium text-ink2">Reset {tabName}</p>
          <p className="text-xs text-ink4">
            {isDefault
              ? "Everything on this tab is at its default."
              : "Restore this tab's settings to their defaults."}
          </p>
        </div>
        <Button
          variant="secondary"
          onClick={() => setOpen(true)}
          disabled={isDefault || busy}
          className="shrink-0"
        >
          Reset to defaults
        </Button>
      </div>
      {error && <p className="mt-2 text-xs text-st-due">{error}</p>}
      <ConfirmDialog
        open={open}
        title={`Reset ${tabName} to defaults?`}
        confirmLabel="Reset"
        onConfirm={() => void run()}
        onClose={() => setOpen(false)}
      >
        {confirmBody}
      </ConfirmDialog>
    </div>
  );
}
