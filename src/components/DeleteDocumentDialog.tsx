// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Confirm deleting one document (board card #575). Shared by the Documents tab and a project's file
// list so the wording — and the promise it makes — can't drift between the two places a user reaches
// this from. The bulk "delete every file PM can't reach" confirmation (#1017) lives here too, so its
// promise about the source files is written beside the one-file version it must agree with.
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
import { deleteDocument, deleteUnreachableDocuments } from "../lib/ipc";
import type { Document } from "../lib/types";
import { Callout, ConfirmDialog } from "./ui";

/** Which deletion this document gets.
 *
 *  `index_only` is the ONLY pointer kind — its body lives at the source (a cloud account or a
 *  watched folder) and PM holds no file for it. Every other kind, `photo` and `spreadsheet`
 *  included, keeps its body in a Markdown file in the vault, which PM removes. This used to read
 *  `source_type !== "vault"`, which quietly promised a photo's original was safe in a cloud account
 *  the user had never connected — while the backend, making the same mistake, left the vault file
 *  behind for the next Rebuild to resurrect. */
function kindOf(doc: Document): "chat" | "pointer" | "photo" | "vault" {
  if (doc.source_type === "chat") return "chat";
  if (doc.source_type === "index_only") return "pointer";
  if (doc.source_type === "photo") return "photo";
  return "vault";
}

function consequence(doc: Document): string {
  switch (kindOf(doc)) {
    case "chat":
      return "This is a saved chat, so deleting it removes the conversation and its messages too — not just the transcript PM searches.";
    case "pointer":
      return "This file is indexed from a connected account or a watched folder, so only PM’s copy of the index is removed. The file itself is not touched.";
    case "photo":
      // Two things live in the vault for a photo — the text PM read out of the image, and the image
      // itself when "keep a copy" was ticked. `Document` doesn't say which, so the wording covers
      // both without claiming either.
      return "Everything PM keeps for this image goes: the text it read out of it, and the copy of the picture itself if you chose to keep one. The picture you imported from is left alone.";
    default:
      return "PM’s copy in your vault is removed, and the file is gone from search. The file you imported from is left alone.";
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
        <Callout as="p" size="md" className="mt-3">
          {error}
        </Callout>
      )}
    </ConfirmDialog>
  );
}

/** The documents the bulk delete offers (#1017): every one whose source PM can no longer reach, or
 *  whose file is gone at the source, split by which so the dialog can say how many of each. */
export function deadSources(docs: Document[]): {
  ids: number[];
  unreachable: number;
  missing: number;
  /** How many of them PM keeps a vault copy of — anything that isn't a pointer. */
  vaultCopies: number;
} {
  const dead = docs.filter(
    (d) =>
      d.source_type !== "chat" &&
      (d.source_state === "unreachable" || d.source_state === "source_missing"),
  );
  return {
    ids: dead.map((d) => d.id),
    unreachable: dead.filter((d) => d.source_state === "unreachable").length,
    missing: dead.filter((d) => d.source_state === "source_missing").length,
    vaultCopies: dead.filter((d) => kindOf(d) !== "pointer").length,
  };
}

const files = (n: number) => `${n} file${n === 1 ? "" : "s"}`;

/** Confirm deleting every unreachable / source-deleted document at once. The backend re-checks each
 *  id, so one a sync brought back while this was open is kept — which is why the done count can come
 *  in under the number shown here. */
export function DeleteUnreachableDialog({
  docs,
  onClose,
  onDeleted,
}: {
  docs: Document[];
  onClose: () => void;
  /** Called with how many went, after a successful delete and before close. */
  onDeleted: (count: number) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Frozen at open: the list refreshing underneath (a sync landing) must not change what the user is
  // confirming while they read it.
  const [dead] = useState(() => deadSources(docs));

  async function run() {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      onDeleted(await deleteUnreachableDocuments(dead.ids));
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
      title={`Delete ${files(dead.ids.length)} PM can’t reach?`}
      confirmLabel="Delete all"
      onConfirm={() => void run()}
      onClose={onClose}
    >
      <ul className="list-disc pl-5 text-ink2">
        {dead.unreachable > 0 && (
          <li>
            {files(dead.unreachable)} whose folder or account can&rsquo;t be reached right now
          </li>
        )}
        {dead.missing > 0 && <li>{files(dead.missing)} deleted at the source</li>}
      </ul>
      <p className="mt-2">
        Only PM&rsquo;s index of them is removed — the files at the source aren&rsquo;t touched.
        {dead.vaultCopies > 0 &&
          ` PM’s own vault copy goes too for ${dead.vaultCopies === 1 ? "the one" : `the ${dead.vaultCopies}`} it keeps.`}{" "}
        Their project, importance and tags are lost, and this can&rsquo;t be undone from inside PM.
      </p>
      <p className="mt-2 text-xs text-ink4">
        If a folder or account comes back later, its files are indexed again as new, unsorted files.
      </p>
      {error && (
        <Callout as="p" size="md" className="mt-3">
          {error}
        </Callout>
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
