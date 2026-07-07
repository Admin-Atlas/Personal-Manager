// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import {
  type CSSProperties,
  type PointerEvent as ReactPointerEvent,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { ChunkSpan, Document, ImageData } from "../lib/types";
import {
  documentChunkSpans,
  fetchIndexOnlyBody,
  openSource,
  readDocumentBody,
  readDocumentImage,
} from "../lib/ipc";
import { Markdown } from "../lib/markdown";
import {
  offsetsAlignToBody,
  parentGroupStarts,
  segmentByLeaves,
  shadeLeaves,
} from "../lib/chunkOverlay";
import { formatDate } from "../lib/format";
import { useDepth } from "../theme";
import { useDevMode } from "../lib/capabilities";

// The document reader: a read-only, docked view onto already-indexed state. It dispatches on
// `source_type` (a new type later = a new case), renders content through the app's single sanitizing
// Markdown boundary, and — for power users — paints the splitter's chunk boundaries over the body.
// It never mutates the store: no editing, no re-chunking, no boundary editing. Mounted once at app
// scope (see lib/reader.tsx) and opened from the Documents tab, a project's file list, or a chat
// citation.

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

// The reader is a resizable right dock. It never grows past half the window (the reading pane should
// never crowd out the surface it was opened from) and never shrinks below a comfortable column. The
// last width is remembered across opens.
const READER_MIN_WIDTH = 340;
const READER_DEFAULT_WIDTH = 480;
const READER_WIDTH_KEY = "pm.reader.width";

/** Clamp a candidate panel width to [min, half the window], reading the live viewport width. */
function clampReaderWidth(w: number): number {
  const max = Math.round(window.innerWidth * 0.5);
  const min = Math.min(READER_MIN_WIDTH, max);
  return Math.max(min, Math.min(max, w));
}

export function DocumentReader({ doc, stale, onClose }: Props) {
  // Two orthogonal axes: `showPower` (density preset) turns the overlay ON; `devMode` (capability)
  // swaps the overlay substrate from rendered Markdown to raw source.
  const { showPower } = useDepth();
  const { devMode } = useDevMode();

  const [body, setBody] = useState<string | null>(null);
  // Whether `body` is the full text the chunk offsets index into. True for local docs and for an
  // index-only doc whose live body we fetched; false when we fell back to the offline summary.
  const [bodyFull, setBodyFull] = useState(false);
  // Set when an index-only full-text fetch failed and we're showing the offline summary instead.
  const [indexOnlyFallback, setIndexOnlyFallback] = useState<string | null>(null);
  const [image, setImage] = useState<ImageData | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showChunks, setShowChunks] = useState(false);
  const [spans, setSpans] = useState<ChunkSpan[] | null>(null);
  // Latest doc id, so a slow chunk-spans fetch that resolves after the reader moved to another
  // document is dropped rather than painted over the new one.
  const docIdRef = useRef(doc.id);
  docIdRef.current = doc.id;
  const [width, setWidth] = useState<number>(() => {
    const saved = Number(localStorage.getItem(READER_WIDTH_KEY));
    return Number.isFinite(saved) && saved > 0 ? saved : READER_DEFAULT_WIDTH;
  });

  const known = KNOWN_TYPES.has(doc.source_type);
  const isIndexOnly = doc.source_type === "index_only";

  // Load the body (or image) whenever the selected document changes.
  useEffect(() => {
    let cancelled = false;
    setBody(null);
    setBodyFull(false);
    setIndexOnlyFallback(null);
    setImage(null);
    setError(null);
    setShowChunks(false);
    setSpans(null);
    setLoading(true);
    void (async () => {
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
        if (isIndexOnly) {
          // An index-only body is never stored offline — fetch the full live copy so it reads like any
          // other document (and so its chunk offsets line up). If the source can't be reached (offline,
          // removed, or access expired), fall back to the ~500-char summary PM keeps offline.
          try {
            const full = await fetchIndexOnlyBody(doc.id);
            if (!cancelled) {
              setBody(full);
              setBodyFull(true);
            }
          } catch (e) {
            const summary = await readDocumentBody(doc.id).catch(() => "");
            if (!cancelled) {
              setBody(summary);
              setBodyFull(false);
              setIndexOnlyFallback(String(e));
            }
          }
          return;
        }
        const text = await readDocumentBody(doc.id);
        if (!cancelled) {
          setBody(text);
          setBodyFull(true);
        }
      } catch (e) {
        if (!cancelled) setError(String(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [doc.id, doc.source_type, known, isIndexOnly]);

  // Esc closes the reader (matches the app's other dismissible surfaces).
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  // Remember the chosen width across opens (and re-clamp a stored width that no longer fits, e.g. after
  // the window shrank). CSS `max-width: 50vw` caps the live render too, so a stale value is never wrong.
  useEffect(() => {
    localStorage.setItem(READER_WIDTH_KEY, String(width));
  }, [width]);

  // Drag the left edge to resize. The panel is docked to the right, so width = distance from the pointer
  // to the right edge of the window; clamped to [min, half-window]. Pointer capture keeps the drag alive
  // even when the cursor moves faster than the handle.
  function startResize(e: ReactPointerEvent<HTMLDivElement>) {
    e.preventDefault();
    const handle = e.currentTarget;
    handle.setPointerCapture(e.pointerId);
    const onMove = (ev: globalThis.PointerEvent) => {
      setWidth(clampReaderWidth(window.innerWidth - ev.clientX));
    };
    const onUp = () => {
      handle.releasePointerCapture(e.pointerId);
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  }

  // The overlay only applies where the body is the full text the offsets index — never over an image,
  // and never over an index-only summary fallback (whose text the offsets don't align to).
  const renderedAsMarkdown = body != null && image == null && (!isIndexOnly || bodyFull);
  const canOverlay = renderedAsMarkdown && showPower;

  async function toggleChunks() {
    const next = !showChunks;
    setShowChunks(next);
    if (next && spans == null) {
      const forDoc = doc.id;
      try {
        const s = await documentChunkSpans(forDoc);
        if (docIdRef.current === forDoc) setSpans(s);
      } catch {
        // Read-only diagnostic — drop back to the plain reader on failure.
        if (docIdRef.current === forDoc) setShowChunks(false);
      }
    }
  }

  return (
    // Docked to the right, starting *below* the custom title bar (`top-9` = its `h-9`) so its drag
    // region and window controls stay reachable — the reader never covers the window chrome.
    <aside
      className="fixed bottom-0 right-0 top-9 z-40 flex flex-col border-l border-border bg-panel shadow-2xl"
      style={{ width, maxWidth: "50vw", minWidth: `${READER_MIN_WIDTH}px` }}
    >
      {/* Left-edge grip: drag to resize up to half the window. */}
      <div
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize reader"
        onPointerDown={startResize}
        className="absolute inset-y-0 left-0 z-10 w-1.5 -translate-x-1/2 cursor-col-resize bg-transparent transition-colors hover:bg-accent-soft"
      />
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
          <IndexOnlyBody
            doc={doc}
            body={body ?? ""}
            full={bodyFull}
            fallbackReason={indexOnlyFallback}
            showChunks={showChunks}
            spans={spans}
            stale={stale}
            raw={devMode}
          />
        ) : body != null ? (
          <BodyView body={body} showChunks={showChunks} spans={spans} stale={stale} raw={devMode} />
        ) : null}
      </div>
    </aside>
  );
}

/** A rendered document body, optionally overlaid with the splitter's chunk boundaries. */
function BodyView({
  body,
  showChunks,
  spans,
  stale,
  raw,
}: {
  body: string;
  showChunks: boolean;
  spans: ChunkSpan[] | null;
  stale: boolean;
  raw: boolean;
}) {
  if (showChunks && spans) {
    return (
      <>
        {stale && <StaleNote />}
        <ChunkOverlayView body={body} spans={spans} raw={raw} />
      </>
    );
  }
  return <Markdown>{body}</Markdown>;
}

/** Index-only reader: the live-fetched full body when the source is reachable (so it reads like any
 *  local document), otherwise the offline summary — always with one button to open the real source. */
function IndexOnlyBody({
  doc,
  body,
  full,
  fallbackReason,
  showChunks,
  spans,
  stale,
  raw,
}: {
  doc: Document;
  body: string;
  full: boolean;
  fallbackReason: string | null;
  showChunks: boolean;
  spans: ChunkSpan[] | null;
  stale: boolean;
  raw: boolean;
}) {
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
      {full ? (
        <div className="mt-3">
          <BodyView body={body} showChunks={showChunks} spans={spans} stale={stale} raw={raw} />
        </div>
      ) : (
        <>
          <p className="mt-3 text-xs text-ink4">
            {fallbackReason
              ? "PM couldn't fetch the full text right now — the source may be offline, removed, or need re-authorising. Showing the summary it keeps offline."
              : "No offline summary is stored for this item."}
          </p>
          {body && (
            <div className="mt-2">
              <Markdown>{body}</Markdown>
            </div>
          )}
        </>
      )}
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
  const aligned = useMemo(() => offsetsAlignToBody(body, spans), [body, spans]);
  const segments = useMemo(
    () => (aligned ? segmentByLeaves(body, spans) : []),
    [body, spans, aligned],
  );
  const shades = useMemo(() => shadeLeaves(segments), [segments]);
  const groupStarts = useMemo(() => parentGroupStarts(segments), [segments]);

  // The stored offsets don't index this exact body (an index-only item re-embedded from its summary):
  // render it plainly rather than paint boundaries in the wrong places.
  if (!aligned) {
    return (
      <>
        <div className="mb-3 rounded-[var(--radius-sm)] border border-border2 bg-surface px-3 py-2 text-xs text-ink3">
          Chunk boundaries aren&apos;t available for this fetched copy — re-index this source to see
          them.
        </div>
        <Markdown>{body}</Markdown>
      </>
    );
  }

  if (segments.length === 0) {
    return <p className="text-sm text-ink4">No chunk spans to display for this document.</p>;
  }

  return (
    <div className="overflow-hidden rounded-[var(--radius-sm)] border border-rule">
      {segments.map((seg, i) => (
        <div
          key={seg.chunkId}
          className={`px-2 py-1 ${
            i === 0
              ? ""
              : groupStarts.has(seg.chunkId)
                ? "border-t-2 border-border" // heavier divider between sibling parent groups
                : "border-t border-rule" // thin divider between leaves within a group
          } ${shades.get(seg.chunkId) === 1 ? "bg-accent-soft" : ""}`}
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
