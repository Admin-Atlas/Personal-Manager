// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Forgetting the passphrase is the sharpest door on this panel and it was a single unguarded
// click. The dialog names the two things that actually happen rather than asking "are you
// sure": the archives may become unreadable, and the schedule is switched off. It does not
// say "gone forever" — on macOS the keychain entry is still visible in Keychain Access, so
// the true claim is that PM keeps no other copy and cannot show it to you.
//
// Rendered by `BackupSettings` as one of the last two children of its root div. `Modal` does not
// portal, so where a dialog is written is where it lands in the DOM — inside a destination section
// it would put a second button called "Disconnect" in that section while open.

import { ConfirmDialog } from "../ui";

export interface ForgetPassphraseDialogProps {
  open: boolean;
  /** The cadence half of what "Forget" costs — null when there is no schedule to lose, so the
   *  confirmation never warns about losing something the user hasn't got. */
  forgetConsequence: string | null;
  onConfirm: () => void;
  onClose: () => void;
}

export function ForgetPassphraseDialog({
  open,
  forgetConsequence,
  onConfirm,
  onClose,
}: ForgetPassphraseDialogProps) {
  return (
    <ConfirmDialog
      open={open}
      title="Forget the passphrase and turn off automatic backups?"
      danger
      confirmLabel="Forget passphrase"
      onConfirm={onConfirm}
      onClose={onClose}
    >
      <p>
        PM keeps no other copy of this passphrase and can&rsquo;t show it to you. If it isn&rsquo;t
        written down somewhere else, every backup you&rsquo;ve already made — on this computer,
        Proton Drive and Google Drive — becomes permanently unreadable.
      </p>
      {forgetConsequence && <p className="mt-2">{forgetConsequence}</p>}
      <p className="mt-2">
        Your app lock is a different secret — this doesn&rsquo;t affect getting into PM.
      </p>
    </ConfirmDialog>
  );
}
