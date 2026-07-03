// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import {
  devApplyChangeEvent,
  devDocumentChunks,
  ensureSidecar,
  fetchIndexOnlyBody,
  ingestPaths,
  installOptionalOcr,
  listDocuments,
  onOcrInstall,
  openExternalRef,
  optionalOcrStatus,
  promoteIndexOnly,
  rebuildIndex,
  sidecarStatus,
  vaultStatus,
} from "../lib/ipc";
import type { Document, DevTablePage, IngestEvent, SidecarStatus } from "../lib/types";
import { formatDate } from "../lib/format";
import { rankImportance } from "../lib/importance";
import { isDevBuild, useDevMode } from "../lib/capabilities";
import { useDepth } from "../theme";
import { Button, Card, Collapsible, ConfirmDialog } from "./ui";
import { DevTableGrid } from "./dev/DevTableGrid";
import { IngestProgress } from "./IngestProgress";
import { DocumentEngineGuide } from "./DocumentEngineGuide";

type ItemStatus = "working" | "done" | "skipped" | "failed";
interface ProgressItem {
  name: string;
  status: ItemStatus;
  detail?: string;
}

interface Summary {
  ingested: number;
  skipped: number;
  failed: number;
}

// Sorting for the document table. The available columns depend on the Depth preset (Ingested only
// shows on Power), so a header only sorts when it's rendered. `null` = the backend's default order
// (newest first). Importance is ranked high > medium > low > none > archive rather than alphabetically.
type SortKey = "title" | "project" | "importance" | "chunks" | "ingested";
interface DocSort {
  key: SortKey;
  dir: "asc" | "desc";
}
// Columns where "biggest first" is the more useful default on first click.
const SORT_DESC_FIRST = new Set<SortKey>(["importance", "chunks", "ingested"]);

// Image extensions that go through the photo pipeline (mirrors `PHOTO_EXTS` in the Rust ingest). Used
// only to decide whether to offer the one-time OCR install before a drop — the backend re-checks.
const PHOTO_EXTS = new Set(["jpg", "jpeg", "png", "webp", "heic"]);
function hasPhotos(paths: string[]): boolean {
  return paths.some((p) => {
    const ext = p.split(".").pop()?.toLowerCase();
    return ext != null && PHOTO_EXTS.has(ext);
  });
}

interface Props {
  /** Jump to the Review view (the sorting-review queue). */
  onReviewClick?: () => void;
}

export function DocumentsView({ onReviewClick }: Props) {
  const [documents, setDocuments] = useState<Document[]>([]);
  const [status, setStatus] = useState<SidecarStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [prep, setPrep] = useState<string | null>(null);
  const [items, setItems] = useState<ProgressItem[]>([]);
  // Determinate-bar inputs: `total` from the `counted` event, `processed` counted up as
  // each file lands. Null total (setup / model download) keeps the bar an indeterminate sweep.
  const [total, setTotal] = useState<number | null>(null);
  const [processed, setProcessed] = useState(0);
  const [summary, setSummary] = useState<Summary | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmRebuild, setConfirmRebuild] = useState(false);
  const [rebuildNeeded, setRebuildNeeded] = useState(false);
  const [guideOpen, setGuideOpen] = useState(false);
  // Photo ingest (board card #135). The opt-in to copy dropped originals into the vault's photos/
  // folder (off by default — references like documents). When OCR isn't installed and a photo is
  // dropped, `photoPrompt` holds the paths awaiting the install decision (install, or skip → EXIF
  // only); `installingOcr`/`ocrFrac` drive the one-time download progress.
  const [copyPhotosToVault, setCopyPhotosToVault] = useState(false);
  const [photoPrompt, setPhotoPrompt] = useState<string[] | null>(null);
  const [installingOcr, setInstallingOcr] = useState(false);
  const [ocrFrac, setOcrFrac] = useState(0);
  // Dev-only (debug builds): drive the index-only substrate without a real connector.
  const [devTitle, setDevTitle] = useState("");
  const [devBody, setDevBody] = useState("");
  const { showPower } = useDepth();
  const [sort, setSort] = useState<DocSort | null>(null);
  // Developer mode (issue #78): an in-place chunk inspector. Clicking a document's chunk count
  // (when devMode is on) expands its leaf/parent chunk breakdown, fetched read-only on demand.
  const { devMode } = useDevMode();
  // The index-only test harnesses below need BOTH gates: `isDevBuild` is the hard floor (they
  // write synthetic state, so they are tree-shaken out of release builds), and `devMode` makes the
  // runtime toggle the single master switch for every developer surface — so turning dev mode off
  // hides them even in a `tauri dev` build, like every other developer surface.
  const showHarness = isDevBuild && devMode;
  const [chunksFor, setChunksFor] = useState<number | null>(null);
  const [chunkPage, setChunkPage] = useState<DevTablePage | null>(null);
  // Index-only "show full text": an index-only document's body is never stored — fetch it live from
  // the source (Drive) on demand and show it inline.
  const [bodyFor, setBodyFor] = useState<number | null>(null);
  const [bodyText, setBodyText] = useState<string | null>(null);
  const [bodyError, setBodyError] = useState<string | null>(null);
  // "Import fully" (promote): pull a Drive Sheet's full grid and index it locally, flipping it off
  // index-only. Tracks the in-flight doc id so its row shows progress + disables the button.
  const [promoting, setPromoting] = useState<number | null>(null);

  async function toggleBody(docId: number) {
    if (bodyFor === docId) {
      setBodyFor(null);
      setBodyText(null);
      setBodyError(null);
      return;
    }
    setBodyFor(docId);
    setBodyText(null);
    setBodyError(null);
    try {
      setBodyText(await fetchIndexOnlyBody(docId));
    } catch (e) {
      setBodyError(String(e));
    }
  }

  // Promote a Drive Sheet (index-only) to a full local spreadsheet import. Fetches the whole grid,
  // indexes it locally, and reloads so the row reflects its new source type.
  async function promote(docId: number) {
    if (promoting != null) return;
    setPromoting(docId);
    setError(null);
    try {
      await promoteIndexOnly(docId);
      if (bodyFor === docId) {
        setBodyFor(null);
        setBodyText(null);
        setBodyError(null);
      }
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setPromoting(null);
    }
  }

  async function toggleChunks(docId: number) {
    if (chunksFor === docId) {
      setChunksFor(null);
      setChunkPage(null);
      return;
    }
    setChunksFor(docId);
    setChunkPage(null);
    try {
      setChunkPage(await devDocumentChunks(docId));
    } catch {
      /* read-only diagnostic — leave the panel empty on failure */
    }
  }

  // `busy` inside the drag-drop listener would be stale; read it via a ref. The OCR install runs
  // outside the `busy` ingest lifecycle, so it gets its own ref to block a second drop mid-download.
  const busyRef = useRef(false);
  busyRef.current = busy;
  const installingOcrRef = useRef(false);
  installingOcrRef.current = installingOcr;

  // Pop the troubleshooting guide once each time setup enters an error state,
  // resetting when it leaves so a later failure reopens it (and closing it once
  // setup succeeds).
  const guideAutoOpened = useRef(false);

  useEffect(() => {
    refresh();
    sidecarStatus()
      .then(setStatus)
      .catch(() => {});
  }, []);

  useEffect(() => {
    if (status?.state === "error") {
      if (!guideAutoOpened.current) {
        guideAutoOpened.current = true;
        setGuideOpen(true);
      }
    } else {
      guideAutoOpened.current = false;
      if (status?.state === "ready") setGuideOpen(false);
    }
  }, [status?.state]);

  // Window-level file drag-and-drop (Tauri gives us absolute paths).
  useEffect(() => {
    const unlisten = getCurrentWebview().onDragDropEvent((event) => {
      const payload = event.payload;
      if (payload.type === "over" || payload.type === "enter") {
        setDragging(true);
      } else if (payload.type === "leave") {
        setDragging(false);
      } else if (payload.type === "drop") {
        setDragging(false);
        if (!busyRef.current && !installingOcrRef.current && payload.paths.length > 0) {
          runIngest(payload.paths);
        }
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Live progress for the one-time OCR download (mirrors the t-SNE install bar in Settings).
  useEffect(() => {
    let cancelled = false;
    const un = onOcrInstall((e) => {
      if (!cancelled) setOcrFrac(e.fraction);
    });
    return () => {
      cancelled = true;
      void un.then((fn) => fn());
    };
  }, []);

  async function refresh() {
    try {
      setDocuments(await listDocuments());
      // A retrieval-config change (new chunking/splitter/model) flags a one-time Rebuild;
      // re-reading here also clears the banner once a rebuild has brought the index in line.
      const vs = await vaultStatus().catch(() => null);
      setRebuildNeeded(vs?.retrieval_rebuild_needed ?? false);
    } catch (e) {
      setError(String(e));
    }
  }

  function handleEvent(event: IngestEvent) {
    switch (event.type) {
      case "preparing":
        setPrep(event.message);
        break;
      case "counted":
        setTotal(event.total);
        break;
      case "started":
        setPrep(null);
        setItems((prev) => [...prev, { name: event.name, status: "working" }]);
        break;
      case "done":
        setProcessed((n) => n + 1);
        setItems((prev) =>
          replaceLastWorking(prev, {
            name: event.document.title,
            status: "done",
            detail: `${event.document.chunk_count} chunk${
              event.document.chunk_count === 1 ? "" : "s"
            }`,
          }),
        );
        break;
      case "skipped":
        setProcessed((n) => n + 1);
        setItems((prev) =>
          replaceLastWorking(prev, {
            name: lastName(prev),
            status: "skipped",
            detail: event.reason,
          }),
        );
        break;
      case "failed":
        setProcessed((n) => n + 1);
        setItems((prev) =>
          replaceLastWorking(prev, {
            name: lastName(prev),
            status: "failed",
            detail: event.error,
          }),
        );
        break;
      case "finished":
        setSummary({
          ingested: event.ingested,
          skipped: event.skipped,
          failed: event.failed,
        });
        break;
    }
  }

  // Entry point for every ingest (drop + the Add buttons). If photos are in the batch and OCR isn't
  // installed yet, pause to offer the one-time install (the user can decline → photos still ingest
  // with their date/location metadata); otherwise go straight to the work.
  async function runIngest(paths: string[]) {
    if (busy || installingOcr || paths.length === 0) return;
    if (hasPhotos(paths)) {
      const ready = await optionalOcrStatus()
        .then((s) => s.installed)
        .catch(() => false);
      if (!ready) {
        setPhotoPrompt(paths);
        return;
      }
    }
    await startIngest(paths);
  }

  async function startIngest(paths: string[]) {
    if (busy || paths.length === 0) return;
    setBusy(true);
    setItems([]);
    setTotal(null);
    setProcessed(0);
    setSummary(null);
    setError(null);
    setPrep(null);
    try {
      await ingestPaths(paths, handleEvent, copyPhotosToVault);
      setStatus(await sidecarStatus());
    } catch (e) {
      setError(String(e));
      setStatus(await sidecarStatus().catch(() => null));
    } finally {
      setBusy(false);
      await refresh();
    }
  }

  // OCR prompt → "Install": download the component (showing progress), then ingest. A failed install
  // still falls through to ingest (EXIF-only) so the drop is never lost — the error surfaces in the
  // banner and OCR can be retried from Settings → Storage.
  async function installOcrThenIngest() {
    const paths = photoPrompt;
    setPhotoPrompt(null);
    if (!paths) return;
    setInstallingOcr(true);
    setOcrFrac(0);
    setError(null);
    try {
      await installOptionalOcr();
    } catch (e) {
      setError(String(e));
    } finally {
      setInstallingOcr(false);
    }
    await startIngest(paths);
  }

  // OCR prompt → "Not now" (or dismissed): ingest anyway, EXIF-only. OCR can be enabled later.
  function skipOcrAndIngest() {
    const paths = photoPrompt;
    setPhotoPrompt(null);
    if (paths) void startIngest(paths);
  }

  async function pickFiles() {
    const selected = await open({ multiple: true, directory: false });
    if (selected) runIngest(Array.isArray(selected) ? selected : [selected]);
  }

  async function pickFolder() {
    const selected = await open({ directory: true });
    if (selected) runIngest([selected as string]);
  }

  async function doRebuild() {
    if (busy) return;
    setBusy(true);
    setItems([]);
    setTotal(null);
    setProcessed(0);
    setSummary(null);
    setError(null);
    setPrep(null);
    try {
      await rebuildIndex(handleEvent);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
      await refresh();
    }
  }

  // Dev-only: register a pasted body as an index-only document (board card 3). The source id is
  // derived from the title so registering the same title twice exercises the stable-id dedupe.
  async function devAddPointer() {
    const title = devTitle.trim();
    const body = devBody.trim();
    if (busy || !title || !body) return;
    setBusy(true);
    setError(null);
    try {
      await devApplyChangeEvent(
        "add",
        `dev:${title}`,
        title,
        body,
        `dev://${encodeURIComponent(title)}`,
      );
      setDevTitle("");
      setDevBody("");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
      await refresh();
    }
  }

  // Dev-only: fire a simulated change event at an existing index-only item (board card 3, PR B) and
  // watch its badge/state react. "update" re-embeds from the body box (or a default tweak).
  async function devFire(
    kind: "update" | "delete" | "rename" | "source_failure",
    sourceId: string,
  ) {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const body = kind === "update" ? devBody.trim() || `${sourceId} — edited` : null;
      const externalRef = kind === "rename" ? `dev://renamed/${sourceId}` : null;
      await devApplyChangeEvent(kind, sourceId, null, body, externalRef);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
      await refresh();
    }
  }

  async function doSetup() {
    setBusy(true);
    setError(null);
    setTotal(null);
    setProcessed(0);
    setPrep("Setting up the document engine (one-time)…");
    try {
      await ensureSidecar();
      setStatus(await sidecarStatus());
    } catch (e) {
      setError(String(e));
      setStatus(await sidecarStatus().catch(() => null));
    } finally {
      setBusy(false);
      setPrep(null);
    }
  }

  const unreviewed = documents.filter((d) => !d.reviewed).length;

  // Click a column header to sort; same header again flips the direction. A new column starts in its
  // natural direction (descending for importance/chunks/ingested, ascending for title/project).
  function toggleSort(key: SortKey) {
    setSort((cur) =>
      cur?.key === key
        ? { key, dir: cur.dir === "asc" ? "desc" : "asc" }
        : { key, dir: SORT_DESC_FIRST.has(key) ? "desc" : "asc" },
    );
  }

  const sortedDocuments = useMemo(() => {
    if (!sort) return documents;
    const factor = sort.dir === "asc" ? 1 : -1;
    return [...documents].sort((a, b) => {
      let c = 0;
      switch (sort.key) {
        case "title":
          c = a.title.localeCompare(b.title);
          break;
        case "project":
          c = a.project.localeCompare(b.project);
          break;
        case "importance":
          c = rankImportance(a.importance) - rankImportance(b.importance);
          break;
        case "chunks":
          c = a.chunk_count - b.chunk_count;
          break;
        case "ingested":
          c = a.ingested_at.localeCompare(b.ingested_at);
          break;
      }
      if (c === 0) c = a.title.localeCompare(b.title); // stable tiebreak
      return c * factor;
    });
  }, [documents, sort]);

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center justify-between border-b border-border px-6 py-3">
        <div>
          <h1 className="font-head text-sm font-semibold text-ink">Documents</h1>
          <p className="text-xs text-ink3">
            {documents.length} ingested · drag files in or use the buttons
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button onClick={pickFiles} disabled={busy || installingOcr}>
            Add files
          </Button>
          <Button onClick={pickFolder} disabled={busy || installingOcr}>
            Add folder
          </Button>
          <Button
            variant="tertiary"
            onClick={() => setConfirmRebuild(true)}
            disabled={busy || installingOcr}
            data-help="documents-rebuild"
            title="Drop the index and rebuild it from the Markdown vault"
          >
            Rebuild
          </Button>
        </div>
      </header>

      <div className="flex-1 overflow-y-auto">
        <div className="mx-auto max-w-3xl px-6 py-6">
          {unreviewed > 0 && onReviewClick && (
            <button
              onClick={onReviewClick}
              data-help="documents-review-banner"
              className="mb-4 flex w-full items-center justify-between rounded-[var(--radius-sm)] border px-3 py-2 text-sm transition-colors hover:brightness-110"
              style={{
                borderColor: "color-mix(in oklab, var(--st-due) 50%, transparent)",
                background: "color-mix(in oklab, var(--st-due) 12%, transparent)",
                color: "var(--st-due)",
              }}
            >
              <span>
                {unreviewed} document{unreviewed === 1 ? "" : "s"} to review
              </span>
              <span aria-hidden>→</span>
            </button>
          )}
          {rebuildNeeded && !busy && status?.state !== "error" && (
            <Banner tone="info">
              <div className="flex items-center justify-between gap-3">
                <span>
                  PM's chunking improved in this update. Rebuild the search index once to get the
                  benefit — your documents aren't changed, they're just re-indexed.
                </span>
                <button
                  onClick={() => setConfirmRebuild(true)}
                  className="shrink-0 underline"
                  disabled={busy}
                >
                  Rebuild now
                </button>
              </div>
            </Banner>
          )}
          {status?.state === "error" && (
            <Banner tone="warn">
              <div className="flex items-center justify-between gap-3">
                <span>
                  {status.kind === "packaging_bug"
                    ? "The document engine couldn't start — this looks like a problem with PM itself."
                    : "The document engine needs setup to finish."}
                </span>
                <button
                  onClick={() => setGuideOpen(true)}
                  className="shrink-0 underline"
                  disabled={busy}
                >
                  {status.kind === "packaging_bug" ? "Details & report" : "Troubleshoot"}
                </button>
              </div>
            </Banner>
          )}
          {status?.state === "not_installed" && (
            <Banner tone="info">
              The document engine isn't installed yet. It's a one-time setup (needs Python).{" "}
              <button onClick={doSetup} className="underline" disabled={busy}>
                Set it up now
              </button>{" "}
              <button onClick={() => setGuideOpen(true)} className="underline" disabled={busy}>
                What's needed?
              </button>
            </Banner>
          )}
          {error && status?.state !== "error" && <Banner tone="warn">{error}</Banner>}

          <div
            onClick={pickFiles}
            data-help="documents-dropzone"
            className={`cursor-pointer rounded-[var(--radius)] border-2 border-dashed p-10 text-center transition-colors ${
              dragging ? "border-accent bg-surface" : "border-border2 hover:border-border"
            }`}
          >
            <p className="text-sm text-ink2">
              {busy || installingOcr ? "Working…" : "Drop files or a folder here"}
            </p>
            <p className="mt-1 text-xs text-ink3">
              PDFs, Office docs, photos &amp; screenshots, HTML, CSV/JSON, text — converted,
              chunked, embedded, and indexed locally.
            </p>
          </div>

          {installingOcr && (
            <IngestProgress
              mode="percent"
              processed={Math.round(ocrFrac * 100)}
              total={100}
              label="Downloading photo text recognition"
              className="mt-2"
            />
          )}

          <label className="mt-3 flex cursor-pointer items-start gap-2 text-xs text-ink3">
            <input
              type="checkbox"
              checked={copyPhotosToVault}
              onChange={(e) => setCopyPhotosToVault(e.target.checked)}
              className="mt-0.5 accent-[var(--accent)]"
            />
            <span>
              Save a copy of dropped photos in the vault. Off by default, PM references photos where
              they are; turn this on to keep a copy (useful for screenshots you delete after) — it
              follows your vault's encryption.
            </span>
          </label>

          {/* TEST HARNESS — writes synthetic state, so it is build-time gated (tree-shaken from
              release) AND respects the runtime devMode toggle (the master developer switch). See
              `showHarness` above (issue #78). */}
          {showHarness && (
            <Card className="mt-4 p-3">
              <Collapsible
                defaultOpen={false}
                title={
                  <span className="font-mono text-xs uppercase tracking-wide text-ink3">
                    Dev — add an indexed-only item
                  </span>
                }
              >
                <div className="flex flex-col gap-2 pt-2">
                  <input
                    value={devTitle}
                    onChange={(e) => setDevTitle(e.target.value)}
                    placeholder="Title"
                    className="rounded-[var(--radius-sm)] border border-border bg-surface px-2 py-1 text-sm text-ink"
                  />
                  <textarea
                    value={devBody}
                    onChange={(e) => setDevBody(e.target.value)}
                    placeholder="Body — embedded + summarised, never stored (the index-only pointer)"
                    rows={3}
                    className="rounded-[var(--radius-sm)] border border-border bg-surface px-2 py-1 text-sm text-ink"
                  />
                  <div className="flex justify-end">
                    <Button
                      onClick={devAddPointer}
                      disabled={busy || !devTitle.trim() || !devBody.trim()}
                    >
                      Register indexed-only
                    </Button>
                  </div>
                </div>
              </Collapsible>
            </Card>
          )}

          {showHarness && documents.some((d) => d.source_type === "index_only" && d.source_id) && (
            <Card className="mt-4 p-3">
              <Collapsible
                defaultOpen={false}
                title={
                  <span className="font-mono text-xs uppercase tracking-wide text-ink3">
                    Dev — simulate a source change (observe-and-react)
                  </span>
                }
              >
                <ul className="flex flex-col gap-1.5 pt-2">
                  {documents
                    .filter((d) => d.source_type === "index_only" && d.source_id)
                    .map((d) => (
                      <li key={d.id} className="flex items-center justify-between gap-2 text-sm">
                        <span className="truncate text-ink2" title={d.title}>
                          {d.title}
                        </span>
                        <span className="flex shrink-0 gap-1 font-mono text-xs">
                          {(["update", "delete", "rename", "source_failure"] as const).map((k) => (
                            <button
                              key={k}
                              onClick={() => devFire(k, d.source_id!)}
                              disabled={busy}
                              className="rounded border border-border px-1.5 py-0.5 text-ink3 hover:text-ink disabled:opacity-50"
                            >
                              {k === "source_failure" ? "fail" : k}
                            </button>
                          ))}
                        </span>
                      </li>
                    ))}
                </ul>
              </Collapsible>
            </Card>
          )}

          {(prep || items.length > 0 || summary) && (
            <Card className="mt-4 p-3">
              {busy && (
                <IngestProgress
                  className="mb-2"
                  label="Ingesting documents"
                  processed={processed}
                  total={total}
                />
              )}
              {prep && <p className="px-1 py-1 text-sm text-ink3">{prep}</p>}
              {items.length > 0 && (
                <Collapsible title="Activity" meta={`${items.length}`}>
                  <ul className="flex flex-col gap-1 pt-1">
                    {items.map((item, i) => (
                      <li
                        key={i}
                        className="flex items-center justify-between gap-3 px-1 py-0.5 text-sm"
                      >
                        <span className="truncate text-ink2">{item.name}</span>
                        <span className={`shrink-0 text-xs ${statusColor(item.status)}`}>
                          {statusLabel(item)}
                        </span>
                      </li>
                    ))}
                  </ul>
                </Collapsible>
              )}
              {summary && (
                <p className="mt-2 border-t border-rule px-1 pt-2 text-xs text-ink3">
                  Done — {summary.ingested} ingested, {summary.skipped} skipped, {summary.failed}{" "}
                  failed.
                </p>
              )}
            </Card>
          )}

          <div className="mt-6">
            {documents.length === 0 ? (
              <p className="text-sm text-ink4">No documents yet.</p>
            ) : (
              <table className="w-full text-left text-sm">
                <thead className="font-mono text-xs uppercase tracking-wide text-ink3">
                  <tr className="border-b border-border">
                    <SortHeader label="Title" sortKey="title" sort={sort} onSort={toggleSort} />
                    <SortHeader label="Project" sortKey="project" sort={sort} onSort={toggleSort} />
                    <SortHeader
                      label="Importance"
                      sortKey="importance"
                      sort={sort}
                      onSort={toggleSort}
                    />
                    <SortHeader
                      label="Chunks"
                      sortKey="chunks"
                      sort={sort}
                      onSort={toggleSort}
                      align="right"
                    />
                    {showPower && (
                      <SortHeader
                        label="Ingested"
                        sortKey="ingested"
                        sort={sort}
                        onSort={toggleSort}
                        align="right"
                      />
                    )}
                  </tr>
                </thead>
                <tbody>
                  {sortedDocuments.map((doc) => (
                    <Fragment key={doc.id}>
                      <tr className="border-b border-rule">
                        <td className="py-2 pr-3">
                          <div className="flex items-center gap-2">
                            <div className="truncate text-ink" title={doc.title}>
                              {doc.title}
                            </div>
                            {doc.source_type === "index_only" && <SourceBadge doc={doc} />}
                          </div>
                          {doc.source_type === "index_only" ? (
                            <div className="mt-0.5 flex items-center gap-3 text-xs">
                              {doc.external_ref && (
                                <button
                                  type="button"
                                  onClick={() => void openExternalRef(doc.id)}
                                  className="text-accent-text hover:brightness-110"
                                >
                                  Open in Drive
                                </button>
                              )}
                              {doc.source_state !== "source_missing" && (
                                <button
                                  type="button"
                                  onClick={() => void toggleBody(doc.id)}
                                  className="text-accent-text hover:brightness-110"
                                >
                                  {bodyFor === doc.id ? "Hide text" : "Show full text"}
                                </button>
                              )}
                              {/* A Google Sheet (its webViewLink points at /spreadsheets/) can be
                                  imported fully — pulled grid-and-all into a local spreadsheet. */}
                              {doc.source_state !== "source_missing" &&
                                doc.external_ref?.includes("/spreadsheets/") && (
                                  <button
                                    type="button"
                                    onClick={() => void promote(doc.id)}
                                    disabled={promoting != null}
                                    className="text-accent-text hover:brightness-110 disabled:opacity-50"
                                    title="Download the whole spreadsheet and index it locally"
                                  >
                                    {promoting === doc.id ? "Importing…" : "Import fully"}
                                  </button>
                                )}
                            </div>
                          ) : (
                            doc.source_path && (
                              <div className="truncate text-xs text-ink4" title={doc.source_path}>
                                {doc.source_path}
                              </div>
                            )
                          )}
                        </td>
                        <td className="py-2 pr-3 text-ink3">
                          <span className="inline-flex items-center gap-1.5">
                            {!doc.reviewed && (
                              <span
                                className="inline-block h-1.5 w-1.5 rounded-full"
                                style={{ background: "var(--st-due)" }}
                                title="Awaiting review"
                              />
                            )}
                            {doc.project}
                          </span>
                        </td>
                        <td className="py-2 pr-3 capitalize text-ink3">{doc.importance ?? "—"}</td>
                        <td className="py-2 pr-3 text-right text-ink3">
                          {devMode ? (
                            <button
                              type="button"
                              onClick={() => toggleChunks(doc.id)}
                              className="font-mono text-ink3 underline decoration-dotted underline-offset-2 hover:text-ink"
                              title="Inspect this document's chunk breakdown"
                            >
                              {doc.chunk_count}
                            </button>
                          ) : (
                            doc.chunk_count
                          )}
                        </td>
                        {showPower && (
                          <td className="py-2 text-right text-ink4">
                            {formatDate(doc.ingested_at)}
                          </td>
                        )}
                      </tr>
                      {/* Index-only "show full text": the live-fetched body, expanded under its row. */}
                      {bodyFor === doc.id && (
                        <tr>
                          <td colSpan={showPower ? 5 : 4} className="pb-3">
                            <div className="rounded-[var(--radius-sm)] border border-border bg-surface p-3">
                              {bodyError ? (
                                <p className="text-xs text-st-due">{bodyError}</p>
                              ) : bodyText == null ? (
                                <p className="text-xs text-ink4">Fetching from the source…</p>
                              ) : (
                                <pre className="max-h-72 overflow-auto whitespace-pre-wrap break-words font-sans text-xs text-ink3">
                                  {bodyText}
                                </pre>
                              )}
                            </div>
                          </td>
                        </tr>
                      )}
                      {/* Dev-mode chunk breakdown, expanded directly under its document (read-only). */}
                      {devMode && chunksFor === doc.id && (
                        <tr>
                          <td colSpan={showPower ? 5 : 4} className="pb-3">
                            <div className="rounded-[var(--radius-sm)] border border-border bg-surface p-3">
                              <p className="mb-2 font-mono text-xs uppercase tracking-wide text-ink3">
                                chunks · doc_id {doc.id}
                                {chunkPage ? ` · ${chunkPage.total} total` : ""}
                              </p>
                              {chunkPage ? (
                                <DevTableGrid page={chunkPage} />
                              ) : (
                                <p className="text-xs text-ink4">Loading…</p>
                              )}
                            </div>
                          </td>
                        </tr>
                      )}
                    </Fragment>
                  ))}
                </tbody>
              </table>
            )}
          </div>
        </div>
      </div>

      <ConfirmDialog
        open={confirmRebuild}
        title="Rebuild the index?"
        danger
        confirmLabel="Rebuild"
        onConfirm={() => {
          setConfirmRebuild(false);
          void doRebuild();
        }}
        onClose={() => setConfirmRebuild(false)}
      >
        This drops the search index and rebuilds it from the Markdown vault. Your documents
        aren&apos;t deleted, but rebuilding can take a while on a large library.
      </ConfirmDialog>

      <ConfirmDialog
        open={photoPrompt != null}
        title="Read text from your photos?"
        confirmLabel="Install & continue"
        cancelLabel="Not now"
        onConfirm={() => void installOcrThenIngest()}
        onClose={skipOcrAndIngest}
      >
        To make the text inside photos and screenshots searchable, PM can install on-device text
        recognition (a one-time ~70–100 MB download). It runs fully on your device. If you skip it,
        the photos are still added — indexed by their date and location — and you can turn text
        recognition on any time under Settings → Storage.
      </ConfirmDialog>

      <DocumentEngineGuide
        open={guideOpen}
        onClose={() => setGuideOpen(false)}
        status={status}
        busy={busy}
        onRetry={doSetup}
      />
    </div>
  );
}

function replaceLastWorking(items: ProgressItem[], replacement: ProgressItem) {
  const next = [...items];
  for (let i = next.length - 1; i >= 0; i--) {
    if (next[i].status === "working") {
      next[i] = replacement;
      return next;
    }
  }
  next.push(replacement);
  return next;
}

function lastName(items: ProgressItem[]): string {
  for (let i = items.length - 1; i >= 0; i--) {
    if (items[i].status === "working") return items[i].name;
  }
  return "";
}

function statusLabel(item: ProgressItem): string {
  switch (item.status) {
    case "working":
      return "…";
    case "done":
      return item.detail ?? "done";
    case "skipped":
      return `skipped — ${item.detail ?? ""}`;
    case "failed":
      return `failed — ${item.detail ?? ""}`;
  }
}

function statusColor(status: ItemStatus): string {
  switch (status) {
    case "working":
      return "text-ink4";
    case "done":
      return "text-[var(--st-quick)]";
    case "skipped":
      return "text-ink4";
    case "failed":
      return "text-[var(--st-due)]";
  }
}

/** Badge marking a document as indexed-only (and, once observe-and-react lands, whether its source
 *  has gone missing or unreachable). A vault document shows nothing. */
function SourceBadge({ doc }: { doc: Document }) {
  const warn = doc.source_state !== "ok";
  const label =
    doc.source_state === "source_missing"
      ? "Source missing"
      : doc.source_state === "unreachable"
        ? "Source unreachable"
        : "Indexed-only";
  const title =
    doc.source_state === "source_missing"
      ? "The source was deleted — kept findable, but its body can't be fetched"
      : doc.source_state === "unreachable"
        ? "The source can't be reached right now (offline, or access expired)"
        : "Indexed by pointer — body fetched live on demand, summary readable offline";
  return (
    <span
      className="shrink-0 rounded-full border border-border bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide text-ink3"
      style={
        warn
          ? {
              borderColor: "color-mix(in oklab, var(--st-due) 50%, transparent)",
              background: "color-mix(in oklab, var(--st-due) 12%, transparent)",
              color: "var(--st-due)",
            }
          : undefined
      }
      title={title}
    >
      {label}
    </span>
  );
}

/** A clickable column header that sorts the document table and shows the active direction. */
function SortHeader({
  label,
  sortKey,
  sort,
  onSort,
  align,
}: {
  label: string;
  sortKey: SortKey;
  sort: DocSort | null;
  onSort: (key: SortKey) => void;
  align?: "right";
}) {
  const active = sort?.key === sortKey;
  return (
    <th className={`py-2 font-medium ${align === "right" ? "text-right" : ""}`}>
      <button
        type="button"
        onClick={() => onSort(sortKey)}
        className={`inline-flex items-center gap-1 hover:text-ink ${active ? "text-ink2" : ""}`}
        title={`Sort by ${label.toLowerCase()}`}
      >
        {label}
        <span aria-hidden className="text-[9px] leading-none">
          {active && sort ? (sort.dir === "asc" ? "▲" : "▼") : "↕"}
        </span>
      </button>
    </th>
  );
}

function Banner({ tone, children }: { tone: "info" | "warn"; children: React.ReactNode }) {
  if (tone === "warn") {
    return (
      <div
        className="mb-4 rounded-[var(--radius-sm)] border px-3 py-2 text-sm"
        style={{
          borderColor: "color-mix(in oklab, var(--st-due) 50%, transparent)",
          background: "color-mix(in oklab, var(--st-due) 12%, transparent)",
          color: "var(--st-due)",
        }}
      >
        {children}
      </div>
    );
  }
  return (
    <div className="mb-4 rounded-[var(--radius-sm)] border border-border bg-surface px-3 py-2 text-sm text-ink2">
      {children}
    </div>
  );
}
