// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from "react";

import { deleteOrphanFiles, dismissOrphanSweep, scanOrphanFiles } from "../lib/ipc";
import type { SweepPlan } from "../lib/types";
import { Button, Dialog } from "./ui";

/**
 * **ONE-TIME cleanup — DELETE THIS FILE in the release after the one that ships it** (the follow-up
 * to card #651), along with the `sweep` Rust module, its three `ipc.ts` wrappers and the `SweepPlan`
 * types.
 *
 * Before #620, deleting a photo or spreadsheet dropped the database rows and left the encrypted
 * Markdown in the vault; a photo saved with "keep a copy" also left its original in `photos/`. The
 * leftovers appear in no view and no search — but the vault file is what a Rebuild reads, so a photo
 * the user deliberately deleted comes back as a document on the next Rebuild. That is the harm, and
 * it is what the banner leads with; "some files are using disk space" would not be worth a dialog.
 *
 * Nothing appears for someone still onboarding, once the user has answered either way, or when
 * there is nothing to clean up — the backend returns an empty, unrefused plan in all three cases, so
 * there is no version bookkeeping here at all. A refusal is also silence: this is an uninvited
 * cleanup, and "PM tried to tidy up and could not" is not worth interrupting anyone for.
 *
 * The list shows file names only (Bobby's call). PM has no record of these files, so there is
 * genuinely nothing else honest to show — no title, no project, not even when they were added.
 */
export function OrphanSweepBanner() {
  const [plan, setPlan] = useState<SweepPlan | null>(null);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [removed, setRemoved] = useState<number | null>(null);

  useEffect(() => {
    let cancelled = false;
    scanOrphanFiles()
      .then((p) => {
        if (!cancelled) setPlan(p);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  if (removed !== null) {
    return (
      <Notice onDismiss={() => setRemoved(null)}>
        Removed {removed} leftover file{removed === 1 ? "" : "s"} from your vault.
      </Notice>
    );
  }

  if (!plan || plan.refusal || plan.orphans.length === 0) return null;

  const count = plan.orphans.length;
  const files = `${count} file${count === 1 ? "" : "s"}`;

  async function run() {
    if (!plan || busy) return;
    setBusy(true);
    setError(null);
    try {
      const gone = await deleteOrphanFiles(plan.orphans);
      setPlan(null);
      setOpen(false);
      setRemoved(gone);
    } catch (e: unknown) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function dismiss() {
    setPlan(null);
    setOpen(false);
    await dismissOrphanSweep().catch(() => {});
  }

  return (
    <>
      <Notice onDismiss={() => void dismiss()} dismissLabel="Not now">
        <strong className="font-semibold text-ink">Some deleted files are still on disk.</strong> PM
        found {files} in your vault that no longer belong to anything, left behind by a bug in an
        older version. They do not show up anywhere, but a rebuild would bring them back as
        documents.{" "}
        <Button variant="tertiary" onClick={() => setOpen(true)}>
          Review them
        </Button>
      </Notice>

      <Dialog
        open={open}
        onClose={() => (busy ? undefined : setOpen(false))}
        widthClassName="max-w-lg"
        title={`Remove ${files}?`}
        footer={
          <>
            <Button variant="tertiary" onClick={() => setOpen(false)} disabled={busy}>
              Cancel
            </Button>
            <Button variant="primary" onClick={() => void run()} disabled={busy}>
              {busy ? "Deleting…" : `Delete ${files}`}
            </Button>
          </>
        }
      >
        <p className="mt-2 text-sm leading-relaxed text-ink3">
          These were left in your vault when you deleted the documents they belonged to. PM no
          longer has any record of them, so it can only show you the file names.
        </p>

        <ul className="mt-3 max-h-56 overflow-y-auto rounded border border-border bg-surface p-2 font-mono text-xs text-ink3">
          {plan.orphans.map((path) => (
            <li key={path} className="truncate py-0.5">
              {path}
            </li>
          ))}
        </ul>

        <p className="mt-3 text-sm leading-relaxed text-ink2">
          <strong className="font-semibold text-ink">Back up your vault first.</strong> This
          permanently deletes these files from disk. It cannot be undone from inside PM, and a
          backup is the only way back.
        </p>

        <p className="mt-2 text-sm leading-relaxed text-ink4">
          PM will not touch anything still in use: your documents, chats and photos, your settings,
          or the encrypted files that hold your classifications.
        </p>

        {error && (
          <p role="alert" className="mt-3 text-sm text-st-due">
            {error}
          </p>
        )}
      </Dialog>
    </>
  );
}

/** The banner shell, shared by the offer and the confirmation so the two read as one thing. */
function Notice({
  children,
  onDismiss,
  dismissLabel = "Dismiss",
}: {
  children: React.ReactNode;
  onDismiss: () => void;
  dismissLabel?: string;
}) {
  return (
    <div className="flex items-center justify-between gap-3 border-b border-border bg-accent-soft px-4 py-2 text-sm text-ink2">
      <span>{children}</span>
      <span className="flex shrink-0 items-center gap-2">
        <Button variant="tertiary" onClick={onDismiss}>
          {dismissLabel}
        </Button>
      </span>
    </div>
  );
}
