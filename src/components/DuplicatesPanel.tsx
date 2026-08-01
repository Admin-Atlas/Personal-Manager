// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The duplicate check (#282): ask PM to look for documents you hold twice, read what it found, and
// decide. It only ever reports — deleting is a separate, explicit act on one named document.
//
// Both of PM's signals produce false pairs by construction (a template shares an opening; a run of
// invoices reads alike), so the design is built around the user being the judge:
//
//   * every pair says WHY it was flagged, in plain words, because "starts identically" and "reads
//     very alike" are different claims and deserve different amounts of trust;
//   * both documents are always shown side by side and openable — nothing is actionable without
//     having been readable first;
//   * "Remove" acts on ONE named document, never on "the duplicate", so there is no way to click
//     the wrong side by accident;
//   * a scan that could not run its full method says so, rather than reporting a clean result it
//     did not earn.

import { useCallback, useState } from "react";

import { deleteDocument, scanDuplicates } from "../lib/ipc";
import { formatDate } from "../lib/format";
import { useReader } from "../lib/reader";
import type { DuplicatePair, DuplicateReport, Document } from "../lib/types";
import { Button, ConfirmDialog } from "./ui";

/** How a pair was found, as a sentence rather than a score. The wording carries the confidence: an
 *  identical opening is a fact about the text, similarity is a judgement with a threshold behind it,
 *  and the two together are the only case worth calling near-certain. */
function whyFlagged(pair: DuplicatePair): string {
  if (pair.same_opening && pair.similarity !== null) {
    return "These start identically and read the same throughout.";
  }
  if (pair.same_opening) {
    return "These start identically — the same opening, ignoring formatting.";
  }
  return "These read very alike, though they don't start the same way.";
}

/** A document's origin in the words the rest of the app uses. */
function originOf(doc: Document): string {
  switch (doc.source_type) {
    case "index_only":
      return "Not stored here — indexed from a connected account";
    case "chat":
      return "A saved conversation";
    case "photo":
      return "A photo";
    case "spreadsheet":
      return "A spreadsheet";
    default:
      return "In your vault";
  }
}

function SideCard({
  doc,
  onOpen,
  onRemove,
  busy,
}: {
  doc: Document;
  onOpen: () => void;
  onRemove: () => void;
  busy: boolean;
}) {
  return (
    <div className="flex-1 rounded-md border border-border p-3">
      <button
        type="button"
        onClick={onOpen}
        className="block w-full text-left text-sm font-medium text-ink hover:underline"
      >
        {doc.title}
      </button>
      <p className="mt-1 text-xs text-ink4">{originOf(doc)}</p>
      <p className="mt-0.5 text-xs text-ink4">
        {doc.project} · added {formatDate(doc.ingested_at)}
      </p>
      <div className="mt-2 flex gap-2">
        <Button variant="tertiary" onClick={onOpen}>
          Open
        </Button>
        {/* Named, not "remove the duplicate": the button says which document it acts on, so there is
            no way to click the wrong side of a pair. */}
        <Button variant="tertiary" onClick={onRemove} disabled={busy}>
          Remove this one
        </Button>
      </div>
    </div>
  );
}

export function DuplicatesPanel() {
  const { openReader } = useReader();
  const [report, setReport] = useState<DuplicateReport | null>(null);
  const [scanning, setScanning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pendingRemove, setPendingRemove] = useState<Document | null>(null);
  const [removing, setRemoving] = useState(false);
  // Documents removed since the scan. The report is a snapshot, and re-scanning after every removal
  // would make clearing three duplicates take three full sweeps — so pairs are hidden locally and the
  // count stays honest by counting what is still on screen.
  const [removed, setRemoved] = useState<Set<number>>(new Set());

  const scan = useCallback(async () => {
    setScanning(true);
    setError(null);
    setRemoved(new Set());
    try {
      setReport(await scanDuplicates());
    } catch (e) {
      setError(String(e));
      setReport(null);
    } finally {
      setScanning(false);
    }
  }, []);

  async function confirmRemove() {
    if (!pendingRemove) return;
    setRemoving(true);
    try {
      await deleteDocument(pendingRemove.id);
      setRemoved((prev) => new Set(prev).add(pendingRemove.id));
      setPendingRemove(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setRemoving(false);
    }
  }

  const visible = (report?.pairs ?? []).filter((p) => !removed.has(p.a.id) && !removed.has(p.b.id));

  return (
    <div className="mt-4 rounded-lg border border-border p-4" data-help="documents-duplicates">
      <div className="flex items-start justify-between gap-3">
        <div>
          <h3 className="text-sm font-medium text-ink2">Duplicate check</h3>
          <p className="mt-1 text-xs text-ink4">
            Looks for documents you have twice. It shows you both and never removes anything on its
            own.
          </p>
        </div>
        <Button variant="secondary" onClick={() => void scan()} disabled={scanning}>
          {scanning ? "Checking…" : report ? "Check again" : "Check for duplicates"}
        </Button>
      </div>

      {error && (
        <p role="alert" className="mt-3 text-xs text-[var(--st-due)]">
          {error}
        </p>
      )}

      {report && !scanning && (
        <div className="mt-3">
          <p className="text-xs text-ink4">
            {visible.length === 0
              ? `Nothing looks duplicated across your ${report.scanned} documents.`
              : `${visible.length} possible ${visible.length === 1 ? "pair" : "pairs"} across your ${report.scanned} documents.`}
          </p>
          {/* A partial method must never be reported as a clean result. */}
          {report.similarity_skipped && (
            <p className="mt-1 text-xs text-[var(--st-due)]">
              Your library is past {report.similarity_limit.toLocaleString()} documents, so PM
              compared openings only this time — pairs that differ in wording won&rsquo;t be here.
            </p>
          )}
        </div>
      )}

      {visible.length > 0 && (
        <ul className="mt-3 space-y-3">
          {visible.map((pair) => (
            <li key={`${pair.a.id}-${pair.b.id}`} className="rounded-lg border border-border p-3">
              <p className="text-xs text-ink3">{whyFlagged(pair)}</p>
              <div className="mt-2 flex flex-col gap-2 sm:flex-row">
                <SideCard
                  doc={pair.a}
                  busy={removing}
                  onOpen={() => openReader(pair.a)}
                  onRemove={() => setPendingRemove(pair.a)}
                />
                <SideCard
                  doc={pair.b}
                  busy={removing}
                  onOpen={() => openReader(pair.b)}
                  onRemove={() => setPendingRemove(pair.b)}
                />
              </div>
            </li>
          ))}
        </ul>
      )}

      <ConfirmDialog
        open={pendingRemove !== null}
        title="Remove this document?"
        confirmLabel="Remove"
        danger
        busy={removing}
        onConfirm={() => void confirmRemove()}
        onClose={() => setPendingRemove(null)}
      >
        <p>
          <span className="font-medium">{pendingRemove?.title}</span> will be removed from PM, along
          with everything it contributes to search.
        </p>
        {pendingRemove?.source_type === "index_only" ? (
          <p>
            The file itself stays where it is in your connected account — PM only drops its own
            pointer to it.
          </p>
        ) : (
          <p>Its file in your vault goes too. This can&rsquo;t be undone.</p>
        )}
        <p>The other document in the pair is left alone.</p>
      </ConfirmDialog>
    </div>
  );
}

/** The one-time suggestion that the duplicate check exists (#282).
 *
 *  An off-by-default tool inside a Settings tab is a tool nobody finds, and this one is off by
 *  default for a good reason (its signals produce false pairs, so PM should not volunteer them). One
 *  dismissible card in the view it would act on is the smallest thing that resolves that tension:
 *  it names what the feature does, it is refusable, and refusing it is remembered. */
export function DuplicateNudge({
  onEnable,
  onDismiss,
}: {
  onEnable: () => void;
  onDismiss: () => void;
}) {
  return (
    <div className="mt-4 rounded-lg border border-border p-4">
      <p className="text-sm text-ink2">Have you got the same document twice?</p>
      <p className="mt-1 text-xs text-ink4">
        PM can look through your library for documents that appear to be duplicates — the same file
        imported twice, or one that also lives in a connected account. It shows you both and never
        removes anything by itself.
      </p>
      <div className="mt-3 flex gap-2">
        <Button variant="secondary" onClick={onEnable}>
          Turn it on
        </Button>
        <Button variant="tertiary" onClick={onDismiss}>
          No thanks
        </Button>
      </div>
    </div>
  );
}
