// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import {
  type CSSProperties,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
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
  reindexIndexOnly,
} from "../lib/ipc";
import { Markdown } from "../lib/markdown";
import { parentGroupStarts, segmentByLeaves, shadeLeaves } from "../lib/chunkOverlay";
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
  // Whether the stored chunk offsets still index the body we're showing. Vault docs are always
  // aligned (their body IS the indexed string); an index-only doc reports it from the live fetch (an
  // exact content-hash identity check). When false, the saved chunk map is stale and the overlay
  // would land in the wrong places — the reader offers Re-index instead of drawing it.
  const [aligned, setAligned] = useState(true);
  // The chunk-spans fetch lifecycle, so "Show chunks" never silently shows nothing: a load in
  // flight, or a failure the user can retry.
  const [spansLoading, setSpansLoading] = useState(false);
  const [spansError, setSpansError] = useState(false);
  // The on-demand Re-index (index-only only): rebuild the stored chunk map against the current body.
  const [reindexing, setReindexing] = useState(false);
  const [reindexError, setReindexError] = useState<string | null>(null);
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
    setAligned(true);
    setSpansLoading(false);
    setSpansError(false);
    setReindexing(false);
    setReindexError(null);
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
            const res = await fetchIndexOnlyBody(doc.id);
            if (!cancelled) {
              setBody(res.body);
              setBodyFull(true);
              setAligned(res.aligned);
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

  // Load the chunk spans for the current doc, tracking a loading/error state so the overlay panel can
  // say what's happening instead of silently rendering plain text (the old catch just turned the
  // toggle back off). Late resolves for a doc the reader has since left are dropped.
  async function loadSpans() {
    const forDoc = doc.id;
    setSpansLoading(true);
    setSpansError(false);
    try {
      const s = await documentChunkSpans(forDoc);
      if (docIdRef.current === forDoc) setSpans(s);
    } catch {
      if (docIdRef.current === forDoc) setSpansError(true);
    } finally {
      if (docIdRef.current === forDoc) setSpansLoading(false);
    }
  }

  function toggleChunks() {
    const next = !showChunks;
    setShowChunks(next);
    if (next && spans == null && !spansLoading) void loadSpans();
  }

  // Re-index this index-only item against its current live body, then refresh the body + spans so the
  // overlay redraws aligned. Fixes a stale chunk map (offsets left indexing the offline summary).
  async function reindex() {
    const forDoc = doc.id;
    setReindexing(true);
    setReindexError(null);
    try {
      // The command re-embeds and hands back the exact body it indexed (+ aligned), so we redraw
      // against that — no second live fetch, and no window for the source to drift between them.
      const res = await reindexIndexOnly(forDoc);
      if (docIdRef.current !== forDoc) return;
      setBody(res.body);
      setBodyFull(true);
      setAligned(res.aligned);
      setSpans(null);
      await loadSpans();
    } catch (e) {
      if (docIdRef.current === forDoc) setReindexError(String(e));
    } finally {
      if (docIdRef.current === forDoc) setReindexing(false);
    }
  }

  // Everything the body renderers need to draw the chunk overlay — or the right honest note in its
  // place (loading / failed / stale / none), with the fix affordance where one exists.
  const chunk: ChunkPanel = {
    showChunks,
    spans,
    aligned,
    isIndexOnly,
    spansLoading,
    spansError,
    reindexing,
    reindexError,
    onReindex: () => void reindex(),
    onRetrySpans: () => void loadSpans(),
    stale,
    raw: devMode,
  };

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
            chunk={chunk}
          />
        ) : body != null ? (
          <BodyView body={body} chunk={chunk} />
        ) : null}
      </div>
    </aside>
  );
}

/** Everything the body renderers need to draw the chunk overlay — or the right honest note in its
 *  place. `aligned`/`isIndexOnly` decide draw-vs-explain; the loading/error/reindex fields carry the
 *  spans-fetch lifecycle and the index-only Re-index affordance. */
interface ChunkPanel {
  showChunks: boolean;
  spans: ChunkSpan[] | null;
  /** The stored offsets index the shown body exactly (index-only reports this; vault is always true). */
  aligned: boolean;
  isIndexOnly: boolean;
  spansLoading: boolean;
  spansError: boolean;
  reindexing: boolean;
  reindexError: string | null;
  onReindex: () => void;
  onRetrySpans: () => void;
  /** Vault-level retrieval-config staleness (the separate global StaleNote), shown above a live overlay. */
  stale: boolean;
  raw: boolean;
}

/** A rendered document body, overlaid with the splitter's chunk boundaries when the user asks for
 *  them (and they can be drawn honestly). */
function BodyView({ body, chunk }: { body: string; chunk: ChunkPanel }) {
  if (!chunk.showChunks) return <Markdown>{body}</Markdown>;
  return <ChunkArea body={body} chunk={chunk} />;
}

/** The "Show chunks" surface: draws the overlay when the offsets line up, otherwise the honest note
 *  for whichever state we're in (loading / failed to load / stale map / none recorded), each with the
 *  fix where the user can make one. */
function ChunkArea({ body, chunk }: { body: string; chunk: ChunkPanel }) {
  const { spans, spansLoading, spansError, aligned, isIndexOnly, reindexing, reindexError } = chunk;
  const reindexAction = <ReindexAction reindexing={reindexing} onReindex={chunk.onReindex} />;

  // The spans fetch failed — say so and let them retry, rather than silently reverting to plain text.
  if (spansError) {
    return (
      <>
        <ChunkStatusNote
          title="Couldn't load the chunk boundaries"
          action={<InlineAction label="Try again" onClick={chunk.onRetrySpans} />}
        >
          Something went wrong reading this document&apos;s saved chunk map. This is only a view —
          your document itself is unaffected.
        </ChunkStatusNote>
        <Markdown>{body}</Markdown>
      </>
    );
  }
  // Still loading (or not requested yet).
  if (spans == null || spansLoading) {
    return (
      <>
        <ChunkStatusNote title="Loading chunk boundaries…" />
        <Markdown>{body}</Markdown>
      </>
    );
  }
  // Index-only item whose saved offsets index a different version than the body shown — Re-index fixes it.
  if (isIndexOnly && !aligned) {
    return (
      <>
        <ChunkStatusNote title="Chunk boundaries are out of date" action={reindexAction}>
          PM&apos;s saved chunk map for this item was built from a different version of the source
          than the copy shown here — most often because the index was rebuilt from the short summary
          PM keeps offline. Re-index it to rebuild the map from its current text; the boundaries
          will then line up.
        </ChunkStatusNote>
        {reindexError && <p className="mb-3 text-xs text-st-due">{reindexError}</p>}
        <Markdown>{body}</Markdown>
      </>
    );
  }
  // Aligned, but nothing to draw — no leaf offsets were recorded for this document.
  const hasLeaves = spans.some(
    (s) => s.kind === "leaf" && s.start_offset != null && s.end_offset != null,
  );
  if (!hasLeaves) {
    return (
      <>
        <ChunkStatusNote
          title="No chunk boundaries recorded"
          action={isIndexOnly ? reindexAction : undefined}
        >
          {isIndexOnly
            ? "This item has no saved chunk boundaries. Re-index it to compute them from its current text."
            : "This document was indexed by an older version of PM, before it recorded chunk boundaries. Rebuild the index from the Documents tab (the “Rebuild” button) to add them."}
        </ChunkStatusNote>
        {isIndexOnly && reindexError && <p className="mb-3 text-xs text-st-due">{reindexError}</p>}
        <Markdown>{body}</Markdown>
      </>
    );
  }
  // Aligned + has leaves → the real overlay (the global retrieval-staleness note rides above it).
  return (
    <>
      {chunk.stale && <StaleNote />}
      <ChunkOverlayView body={body} spans={spans} raw={chunk.raw} />
    </>
  );
}

/** A small framed note in place of the chunk overlay: a title, an optional explanation, and an
 *  optional action (Re-index / Try again). */
function ChunkStatusNote({
  title,
  children,
  action,
}: {
  title: string;
  children?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="mb-3 rounded-[var(--radius-sm)] border border-border2 bg-surface px-3 py-2 text-xs text-ink3">
      <p className="font-medium text-ink2">{title}</p>
      {children && <p className="mt-1">{children}</p>}
      {action && <div className="mt-2">{action}</div>}
    </div>
  );
}

function ReindexAction({ reindexing, onReindex }: { reindexing: boolean; onReindex: () => void }) {
  return (
    <button
      type="button"
      onClick={onReindex}
      disabled={reindexing}
      className="rounded-[var(--radius-sm)] bg-accent px-2.5 py-1 text-xs text-accent-ink hover:brightness-110 disabled:opacity-50"
    >
      {reindexing ? "Re-indexing…" : "Re-index this item"}
    </button>
  );
}

function InlineAction({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="rounded-[var(--radius-sm)] border border-border px-2.5 py-1 text-xs text-ink2 hover:bg-surface"
    >
      {label}
    </button>
  );
}

/** Index-only reader: the live-fetched full body when the source is reachable (so it reads like any
 *  local document), otherwise the offline summary — always with one button to open the real source. */
function IndexOnlyBody({
  doc,
  body,
  full,
  fallbackReason,
  chunk,
}: {
  doc: Document;
  body: string;
  full: boolean;
  fallbackReason: string | null;
  chunk: ChunkPanel;
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
          <BodyView body={body} chunk={chunk} />
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
  // Alignment + no-leaf handling now live upstream in `ChunkArea`, which only renders this once the
  // stored offsets are known to index `body` exactly and there is at least one leaf to draw.
  const segments = useMemo(() => segmentByLeaves(body, spans), [body, spans]);
  const shades = useMemo(() => shadeLeaves(segments), [segments]);
  const groupStarts = useMemo(() => parentGroupStarts(segments), [segments]);

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
