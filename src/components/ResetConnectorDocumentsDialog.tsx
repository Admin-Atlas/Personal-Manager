// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Confirm resetting the connector documents (#1019): every file PM indexed from a connected account
// or watched folder is removed with its filing, then fetched again from scratch. The vault half of
// Bobby's request is parked, so this dialog speaks only of connectors.
//
// A tick-to-confirm rather than ConfirmDialog alone: this throws away the filing of what can be the
// whole library in one click, and a plain Confirm is one Enter away from that. The tick names the
// loss in the user's own terms (project, importance, tags), which is the thing worth reading.

import { useState } from "react";
import { resetConnectorDocuments } from "../lib/ipc";
import type { Document } from "../lib/types";
import { Callout, ConfirmDialog } from "./ui";

/** How many documents the reset would remove: the index-only pointers, nothing PM holds a file for. */
export function connectorDocumentCount(docs: Document[]): number {
  return docs.filter((d) => d.source_type === "index_only").length;
}

export function ResetConnectorDocumentsDialog({
  docs,
  onClose,
  onReset,
}: {
  docs: Document[];
  onClose: () => void;
  /** Called with how many went, after a successful reset and before close. */
  onReset: (count: number) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [understood, setUnderstood] = useState(false);
  const count = connectorDocumentCount(docs);
  const files = `${count} file${count === 1 ? "" : "s"}`;

  async function run() {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      onReset(await resetConnectorDocuments());
      onClose();
    } catch (e: unknown) {
      setError(String(e));
      setBusy(false);
    }
  }

  return (
    <ConfirmDialog
      open
      danger
      busy={busy}
      title="Reset your connector files?"
      confirmLabel="Reset and fetch again"
      confirmDisabled={!understood}
      initialFocus="cancel"
      onConfirm={() => void run()}
      onClose={onClose}
    >
      <p>
        PM removes the {files} it indexed from your connected accounts and folders, then syncs each
        of them from scratch. They come back as new files, unsorted, for Review to file again.
      </p>
      <ul className="mt-2 list-disc pl-5">
        <li>The files themselves, in Drive, OneDrive or your folders, aren&rsquo;t touched.</li>
        <li>Files you added to the vault yourself, and your chats, aren&rsquo;t affected.</li>
        <li>
          Fetching everything again can take a while on a big library, and uses data while it
          downloads.
        </li>
      </ul>
      <label className="mt-3 flex cursor-pointer items-start gap-2 text-ink2">
        <input
          type="checkbox"
          className="mt-1"
          checked={understood}
          onChange={(e) => setUnderstood(e.target.checked)}
          disabled={busy}
        />
        <span>
          I understand their project, linked projects, importance and tags are lost, and this
          can&rsquo;t be undone from inside PM.
        </span>
      </label>
      {error && (
        <Callout as="p" size="md" className="mt-3">
          {error}
        </Callout>
      )}
    </ConfirmDialog>
  );
}
