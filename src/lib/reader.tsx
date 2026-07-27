// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The app-level document reader mount. The reader (a docked, read-only view onto an already-indexed
// document) is opened from several surfaces — the Documents tab, a project's file list, a clickable
// source citation in chat, and a memory-map node — so it can't live inside any one of them. This
// provider holds
// the one open document, mounts the single `<DocumentReader>` at app scope (it is `position: fixed`,
// so it floats over whatever view is active), and hands every surface an `openReader`/`openReaderById`
// via context — mirroring the app's other root contexts (Help/Theme/Capability) rather than threading
// a callback through every intermediate component.

import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from "react";
import type { Document } from "./types";
import { getDocument, vaultStatus } from "./ipc";
import { DocumentReader } from "../components/DocumentReader";

interface ReaderState {
  /** The document the reader is currently showing, or null when closed. */
  current: Document | null;
  /** Open the reader onto a document already in hand (Documents tab, project file list). */
  openReader: (doc: Document) => void;
  /** Open the reader from just an id (a chat citation carries only `document_id`). Resolves the full
   *  document; if it has since been deleted, surfaces {@link missing} instead of doing nothing. */
  openReaderById: (id: number) => void;
  closeReader: () => void;
  /** Set when a citation pointed at a document that no longer exists — the answer's citations are a
   *  JSON snapshot taken at answer time, so they outlive the file. Null when nothing is amiss. */
  missing: string | null;
  dismissMissing: () => void;
}

const ReaderContext = createContext<ReaderState | null>(null);

export function ReaderProvider({
  view,
  onOpenProject,
  children,
}: {
  view: string;
  /** Navigate to a document's project (the reader's clickable project name). Provided by App; the
   *  reader auto-closes on the resulting view change. */
  onOpenProject?: (project: string) => void;
  children: ReactNode;
}) {
  const [current, setCurrent] = useState<Document | null>(null);
  // Set when a citation resolved to nothing — see `openReaderById`.
  const [missing, setMissing] = useState<string | null>(null);
  // Vault-level retrieval staleness (one global signal — never per-document) for the reader's chunk
  // overlay note. Read once; it only changes on a config change + rebuild, both of which are rare.
  const [stale, setStale] = useState(false);

  useEffect(() => {
    vaultStatus()
      .then((vs) => setStale(vs?.retrieval_rebuild_needed ?? false))
      .catch(() => {});
  }, []);

  // Close the reader when the user navigates to another top-level view, so the fixed panel never
  // strands over an unrelated surface. Opening a doc within a view (a citation, a file row) doesn't
  // change `view`, so it stays open there.
  useEffect(() => {
    setCurrent(null);
  }, [view]);

  const openReader = useCallback((doc: Document) => setCurrent(doc), []);
  const closeReader = useCallback(() => setCurrent(null), []);
  const dismissMissing = useCallback(() => setMissing(null), []);
  const openReaderById = useCallback((id: number) => {
    // The reader needs the full Document (source type, external ref, project…); a citation carries only
    // the id, so fetch just that one document (F-48) rather than materialising the whole list — which
    // grows with connector estates — to find one row.
    setMissing(null);
    getDocument(id)
      .then(setCurrent)
      .catch(() =>
        // The citation outlived its document. `messages.citations` is a JSON snapshot written at
        // answer time, so deleting a file (or the project holding it) leaves every past answer still
        // listing it. This used to swallow the error, which read as a dead click on a real link.
        setMissing(
          "That file has been deleted. Re-ingest it to read it here — this answer still lists it because citations are recorded when the answer is written.",
        ),
      );
  }, []);

  return (
    <ReaderContext.Provider
      value={{ current, openReader, openReaderById, closeReader, missing, dismissMissing }}
    >
      {children}
      {missing && (
        <div
          role="status"
          className="fixed bottom-4 left-1/2 z-50 max-w-md -translate-x-1/2 rounded-[var(--radius)] border px-4 py-3 text-sm text-ink2 shadow-lg"
          style={{
            borderColor: "color-mix(in oklab, var(--st-look) 35%, transparent)",
            background: "color-mix(in oklab, var(--st-look) 14%, var(--bg))",
          }}
        >
          <div className="flex items-start justify-between gap-3">
            <span>{missing}</span>
            <button
              onClick={dismissMissing}
              aria-label="Dismiss"
              className="shrink-0 text-ink4 hover:text-ink"
            >
              ×
            </button>
          </div>
        </div>
      )}
      {current && (
        <DocumentReader
          doc={current}
          stale={stale}
          onClose={closeReader}
          onOpenProject={onOpenProject}
        />
      )}
    </ReaderContext.Provider>
  );
}

export function useReader(): ReaderState {
  const ctx = useContext(ReaderContext);
  if (!ctx) throw new Error("useReader must be used within <ReaderProvider>");
  return ctx;
}
