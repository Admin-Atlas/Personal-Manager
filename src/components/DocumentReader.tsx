// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { type CSSProperties, useEffect, useMemo, useState } from "react";
import type { ChunkSpan, Document, ImageData } from "../lib/types";
import { documentChunkSpans, openSource, readDocumentBody, readDocumentImage } from "../lib/ipc";
import { Markdown } from "../lib/markdown";
import { segmentByLeaves, shadeBuckets } from "../lib/chunkOverlay";
import { formatDate } from "../lib/format";
import { useDepth } from "../theme";
import { useDevMode } from "../lib/capabilities";

// The document reader: a read-only, docked view onto already-indexed state. It dispatches on
// `source_type` (a new type later = a new case), renders local Markdown through the app's single
// sanitizing boundary, and — for power users — paints the splitter's chunk boundaries over the body.
// It never mutates the store: no editing, no re-chunking, no boundary editing.

// The live source types the reader can render. Anything else falls to a graceful placeholder rather than
// raw bytes or a broken view.
const KNOWN_TYPES = new Set(["vault", "index_only", "chat", "photo", "spreadsheet"]);
const TYPE_LABEL: Record<string, string> = {
  vault: "Document",
  index_only: "Indexed",
  chat: "Conversation",
  photo: "Image",
  spreadsheet: "Spreadsheet",
};

interface Props {
  doc: Document;
  /** Vault-level retrieval-config staleness (one global signal — never per-document). */
  stale: boolean;
  onClose: () => void;
}

export function DocumentReader({ doc, stale, onClose }: Props) {
  // Two orthogonal axes: `showPower` (density preset) turns the overlay ON; `devMode` (capability)
  // swaps the overlay substrate from rendered Markdown to raw source.
  const { showPower } = useDepth();
  const { devMode } = useDevMode();

  const [body, setBody] = useState<string | null>(null);
  const [image, setImage] = useState<ImageData | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showChunks, setShowChunks] = useState(false);
  const [spans, setSpans] = useState<ChunkSpan[] | null>(null);

  const known = KNOWN_TYPES.has(doc.source_type);
  const isIndexOnly = doc.source_type === "index_only";

  // Load the body (or image) whenever the selected document changes.
  useEffect(() => {
    let cancelled = false;
    setBody(null);
    setImage(null);
    setError(null);
    setShowChunks(false);
    setSpans(null);
    setLoading(true);
    (async () => {
      try {
        if (!known) return;
        if (doc.source_type === "photo") {
          const img = await readDocumentImage(doc.id);
          if (cancelled) return;
          if (img) {
            setImage(img);
            return;
          }
          // No saved original — fall through to the OCR/synthetic Markdown body.
        }
        const text = await readDocumentBody(doc.id);
        if (!cancelled) setBody(text);
      } catch (e) {
        if (!cancelled) setError(String(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [doc.id, doc.source_type, known]);

  // Esc closes the reader (matches the app's other dismissible surfaces).
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  // The overlay only applies to Markdown-rendered docs — never over an image, never for index-only
  // (whose "body" is the offline summary, not chunk-aligned).
  const renderedAsMarkdown = body != null && image == null && !isIndexOnly;
  const canOverlay = renderedAsMarkdown && showPower;

  async function toggleChunks() {
    const next = !showChunks;
    setShowChunks(next);
    if (next && spans == null) {
      try {
        setSpans(await documentChunkSpans(doc.id));
      } catch {
        // Read-only diagnostic — drop back to the plain reader on failure.
        setShowChunks(false);
      }
    }
  }

  return (
    <aside className="fixed right-0 top-0 z-40 flex h-full w-[min(480px,100vw)] flex-col border-l border-border bg-panel shadow-2xl">
      <div className="flex items-start gap-2 border-b border-rule px-4 py-3">
        <div className="min-w-0 flex-1">
          <div className="truncate font-head text-base text-ink" title={doc.title}>
            {doc.title}
          </div>
          <div className="mt-0.5 flex flex-wrap items-center gap-x-2 text-xs text-ink4">
            <span>{TYPE_LABEL[doc.source_type] ?? doc.source_type}</span>
            {doc.project && <span>· {doc.project}</span>}
            <span>· {formatDate(doc.ingested_at)}</span>
          </div>
        </div>
        {canOverlay && (
          <button
            type="button"
            onClick={() => void toggleChunks()}
            className="shrink-0 text-xs text-accent-text hover:brightness-110"
            title="Show the chunk boundaries the splitter placed"
          >
            {showChunks ? "Hide chunks" : "Show chunks"}
          </button>
        )}
        <button
          type="button"
          onClick={onClose}
          aria-label="Close reader"
          className="shrink-0 text-ink4 hover:text-ink"
        >
          ✕
        </button>
      </div>

      <div className="flex-1 overflow-auto px-4 py-3">
        {loading ? (
          <p className="text-sm text-ink4">Loading…</p>
        ) : error ? (
          <p className="text-sm text-st-due">{error}</p>
        ) : !known ? (
          <Placeholder type={doc.source_type} />
        ) : image ? (
          <img
            src={`data:${image.mime};base64,${image.base64}`}
            alt={doc.title}
            className="max-w-full rounded-[var(--radius-sm)]"
          />
        ) : isIndexOnly ? (
          <IndexOnlyBody doc={doc} summary={body ?? ""} />
        ) : body != null ? (
          showChunks && spans ? (
            <>
              {stale && <StaleNote />}
              <ChunkOverlayView body={body} spans={spans} raw={devMode} />
            </>
          ) : (
            <Markdown>{body}</Markdown>
          )
        ) : null}
      </div>
    </aside>
  );
}

/** Index-only reader: the offline summary plus the one affordance to reach the real source. */
function IndexOnlyBody({ doc, summary }: { doc: Document; summary: string }) {
  const isLocal = doc.source_id?.startsWith("local:") ?? false;
  return (
    <div>
      {doc.external_ref && (
        <button
          type="button"
          onClick={() => void openSource(doc.id).catch(() => {})}
          className="rounded-[var(--radius-sm)] bg-accent px-3 py-1.5 text-sm text-accent-ink hover:brightness-110"
        >
          {isLocal ? "Reveal in file manager" : "Open source"}
        </button>
      )}
      {summary ? (
        <div className="mt-3">
          <Markdown>{summary}</Markdown>
        </div>
      ) : (
        <p className="mt-3 text-sm text-ink4">No offline summary is stored for this item.</p>
      )}
      <p className="mt-3 text-xs text-ink4">
        This item is indexed but its full text isn't stored offline — open the source to read it, or
        use "Show full text" in the list to fetch it live.
      </p>
    </div>
  );
}

/** The chunk-boundary overlay: each leaf chunk's source slice as its own block, shaded by parent group,
 *  divided per leaf. Rendered Markdown by default; raw monospace source under dev mode. */
function ChunkOverlayView({
  body,
  spans,
  raw,
}: {
  body: string;
  spans: ChunkSpan[];
  raw: boolean;
}) {
  const segments = useMemo(() => segmentByLeaves(body, spans), [body, spans]);
  const shades = useMemo(() => shadeBuckets(segments), [segments]);

  if (segments.length === 0) {
    return <p className="text-sm text-ink4">No chunk spans to display for this document.</p>;
  }

  return (
    <div className="overflow-hidden rounded-[var(--radius-sm)] border border-rule">
      {segments.map((seg) => (
        <div
          key={seg.chunkId}
          className={`border-t border-rule px-2 py-1 first:border-t-0 ${
            shades.get(seg.chunkId) === 1 ? "bg-accent-soft" : ""
          }`}
          // Zero-dep virtualization: off-screen chunk blocks skip layout/paint until scrolled near.
          style={
            {
              contentVisibility: "auto",
              containIntrinsicSize: "auto 3rem",
            } as CSSProperties
          }
          title={`chunk #${seg.ordinal}${
            seg.parentId != null ? ` · parent ${seg.parentId}` : " · no parent"
          }`}
        >
          {raw ? (
            <pre className="m-0 whitespace-pre-wrap break-words font-mono text-xs leading-relaxed text-ink3">
              {seg.text}
            </pre>
          ) : (
            <Markdown>{seg.text}</Markdown>
          )}
        </div>
      ))}
    </div>
  );
}

function StaleNote() {
  return (
    <div className="mb-3 rounded-[var(--radius-sm)] border border-border2 bg-surface px-3 py-2 text-xs text-ink3">
      These boundaries reflect the last index — the retrieval config has changed since. Rebuild to
      refresh the vault's chunks.
    </div>
  );
}

function Placeholder({ type }: { type: string }) {
  return (
    <div className="rounded-[var(--radius-sm)] border border-dashed border-border2 p-4 text-sm text-ink4">
      No reader for “{type}” documents yet.
    </div>
  );
}
