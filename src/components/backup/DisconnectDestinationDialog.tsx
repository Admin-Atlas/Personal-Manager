// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Disconnect, on the pattern every read connector already uses (CloudDriveConnection,
// CalendarConnection, LocalFolderConnection, IcsFeedSubscription): what is KEPT first, what
// stops second, and the per-destination caveat last. This panel was the only one in the app
// holding a Disconnect with no confirmation at all.
//
// One dialog covers both destinations because only one of the two can ever be open; if that ever
// stops being true, split it rather than widening the union. Rendered by `BackupSettings` as the
// last child of its root div — `Modal` does not portal, and inside a destination section this
// would put a second button called "Disconnect" in that section while open.

import { ConfirmDialog } from "../ui";

export interface DisconnectDestinationDialogProps {
  /** Which destination's confirmation is open, or null for none. */
  which: "proton" | "gdrive" | null;
  /** Whether the BACKUP account is also connected as a read-only Drive source — it decides what
   *  disconnecting Google costs. */
  gdriveAlsoConnector: boolean;
  onConfirm: () => void;
  onClose: () => void;
}

export function DisconnectDestinationDialog({
  which,
  gdriveAlsoConnector,
  onConfirm,
  onClose,
}: DisconnectDestinationDialogProps) {
  return (
    <ConfirmDialog
      open={which !== null}
      title={which === "gdrive" ? "Disconnect Google Drive backups?" : "Disconnect Proton Drive?"}
      danger
      confirmLabel="Disconnect"
      onConfirm={onConfirm}
      onClose={onClose}
    >
      {which === "gdrive" ? (
        <>
          <p>
            The backups already on your Google Drive are kept — nothing is deleted. PM stops backing
            up there: scheduled runs and the trimming that keeps only your most recent backups stop,
            and you can&rsquo;t restore from Drive until you grant access again.
          </p>
          {gdriveAlsoConnector ? (
            <p className="mt-2">
              This account is also connected as a read-only source, so its sign-in is kept and that
              connector keeps working.
            </p>
          ) : (
            // Hedged deliberately. Disconnect forgets PM's token WITHOUT revoking the grant at
            // Google's end, so the old per-file authority may or may not survive a re-approval —
            // PM's own Drive code assumes it does not (a 403 appNotAuthorizedToFile on archives
            // an earlier grant uploaded). Neither over-promise nor stay silent about it.
            <p className="mt-2">
              PM&rsquo;s Drive sign-in for this account is deleted. Granting access again runs a
              fresh approval, and Google&rsquo;s permission covers only the files the current
              approval created — so PM may no longer be able to trim or replace the archives it
              uploaded before. They stay in your Drive either way.
            </p>
          )}
        </>
      ) : (
        <>
          <p>
            The backups already on your Proton Drive are kept — nothing is deleted. PM stops backing
            up there: scheduled runs and the trimming that keeps only your most recent backups stop,
            and you can&rsquo;t restore from Proton until you sign in again.
          </p>
          {/* True at HEAD and worth saying: `proton_disconnect` does not clear
              `backup_proton_enabled` (which defaults to true), so the schedule keeps advertising
              a destination the scheduler will skip. Clearing the flag is a backend change and a
              separate decision; telling the truth about it is not. */}
          <p className="mt-2">
            Automatic backups keep listing Proton Drive until you untick it under &ldquo;Automatic
            backups&rdquo; — a scheduled run skips a destination it can&rsquo;t reach.
          </p>
          <p className="mt-2">
            This signs the Proton Drive command-line tool out on this computer, so anything else
            using it is signed out too.
          </p>
        </>
      )}
    </ConfirmDialog>
  );
}
