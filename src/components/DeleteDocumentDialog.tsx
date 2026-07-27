// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Confirm deleting one document (board card #575). Shared by the Documents tab and a project's file
// list so the wording — and the promise it makes — can't drift between the two places a user reaches
// this from.
//
// Built on `ConfirmDialog`, the design's Approval pattern for irreversible actions, rather than a
// bespoke modal: it already carries the danger tint, the busy latch that blocks Esc/scrim dismissal
// mid-delete, and the `labelledBy` wiring. The project-level merge/delete dialogs are separately
// bespoke because they need a type-to-confirm input, which this pattern deliberately has no room for.
//
// The copy is derived from the document's source kind because the three deletions genuinely differ,
// and being wrong in either direction is bad: implying PM deletes your Google Drive file when it
// doesn't would stop people using this, and implying it doesn't when it does would destroy work.

import { useState } from "react";
import { deleteDocument } from "../lib/ipc";
import type { Document } from "../lib/types";
import { ConfirmDialog } from "./ui";

/** Which of the three deletions this document gets. `source_type` is null/"vault" for a document PM
 *  owns the file for; "chat" is a saved conversation; anything else is a connector pointer. */
function kindOf(doc: Document): "chat" | "pointer" | "vault" {
  if (doc.source_type === "chat") return "chat";
  if (doc.source_type && doc.source_type !== "vault") return "pointer";
  return "vault";
}

function consequence(doc: Document): string {
  switch (kindOf(doc)) {
    case "chat":
      return "This is a saved chat, so deleting it removes the conversation and its messages too — not just the transcript PM searches.";
    case "pointer":
      return "This file is indexed from a connected account, so only PM's copy of the index is removed. The original file in your cloud account is not touched.";
    default:
      return "The file is removed from your vault and from search.";
  }
}

export function DeleteDocumentDialog({
  doc,
  onClose,
  onDeleted,
}: {
  doc: Document;
  onClose: () => void;
  /** Called after a successful delete, before close — the caller refreshes its list. */
  onDeleted: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function run() {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await deleteDocument(doc.id);
      onDeleted();
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
      title={kindOf(doc) === "chat" ? "Delete this chat?" : "Delete this file?"}
      confirmLabel="Delete"
      onConfirm={() => void run()}
      onClose={onClose}
    >
      <p className="truncate font-medium text-ink2" title={doc.title}>
        {doc.title}
      </p>
      <p className="mt-2">{consequence(doc)} This can&rsquo;t be undone from inside PM.</p>
      {/* Citations are a snapshot written when an answer is written, so past answers keep listing a
          deleted file. Saying so here means the reader's later message isn't a surprise. */}
      <p className="mt-2 text-xs text-ink4">
        Answers that already cited it will keep listing it, and say it has been deleted when you
        click through.
      </p>
      {error && (
        <p
          role="alert"
          className="mt-3 rounded-[var(--radius-sm)] border px-3 py-2 text-sm text-st-due"
          style={{
            borderColor: "color-mix(in oklab, var(--st-due) 35%, transparent)",
            background: "color-mix(in oklab, var(--st-due) 12%, transparent)",
          }}
        >
          {error}
        </p>
      )}
    </ConfirmDialog>
  );
}

/** The per-row trigger. Deliberately quiet until hover/focus — a permanently-visible destructive
 *  control on every row invites the mis-click the dialog then has to catch — but always reachable by
 *  keyboard, so it never becomes a pointer-only action. */
export function DeleteDocumentButton({ onClick }: { onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={(e) => {
        e.stopPropagation();
        onClick();
      }}
      title="Delete this document"
      aria-label="Delete this document"
      className="shrink-0 rounded-[var(--radius-sm)] px-1.5 py-0.5 text-xs text-ink4 opacity-0 transition-opacity hover:text-st-due focus-visible:opacity-100 group-hover:opacity-100"
    >
      Delete
    </button>
  );
}
