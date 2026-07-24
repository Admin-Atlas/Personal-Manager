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
   *  document from the list; a no-op if it's since been deleted. */
  openReaderById: (id: number) => void;
  closeReader: () => void;
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
  const openReaderById = useCallback((id: number) => {
    // The reader needs the full Document (source type, external ref, project…); a citation carries only
    // the id, so fetch just that one document (F-48) rather than materialising the whole list — which
    // grows with connector estates — to find one row.
    getDocument(id)
      .then(setCurrent)
      .catch(() => {});
  }, []);

  return (
    <ReaderContext.Provider value={{ current, openReader, openReaderById, closeReader }}>
      {children}
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
