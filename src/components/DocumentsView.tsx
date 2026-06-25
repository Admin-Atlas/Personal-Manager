// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import {
  ensureSidecar,
  ingestPaths,
  listDocuments,
  rebuildIndex,
  sidecarStatus,
  vaultStatus,
} from "../lib/ipc";
import type { Document, IngestEvent, SidecarStatus } from "../lib/types";
import { formatDate } from "../lib/format";
import { useDepth } from "../theme";
import { Button, Card, Collapsible, ConfirmDialog } from "./ui";
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
  const { showPower } = useDepth();

  // `busy` inside the drag-drop listener would be stale; read it via a ref.
  const busyRef = useRef(false);
  busyRef.current = busy;

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
        if (!busyRef.current && payload.paths.length > 0) {
          runIngest(payload.paths);
        }
      }
    });
    return () => {
      unlisten.then((fn) => fn());
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

  async function runIngest(paths: string[]) {
    if (busy || paths.length === 0) return;
    setBusy(true);
    setItems([]);
    setTotal(null);
    setProcessed(0);
    setSummary(null);
    setError(null);
    setPrep(null);
    try {
      await ingestPaths(paths, handleEvent);
      setStatus(await sidecarStatus());
    } catch (e) {
      setError(String(e));
      setStatus(await sidecarStatus().catch(() => null));
    } finally {
      setBusy(false);
      await refresh();
    }
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
          <Button onClick={pickFiles} disabled={busy}>
            Add files
          </Button>
          <Button onClick={pickFolder} disabled={busy}>
            Add folder
          </Button>
          <Button
            variant="tertiary"
            onClick={() => setConfirmRebuild(true)}
            disabled={busy}
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
            <p className="text-sm text-ink2">{busy ? "Working…" : "Drop files or a folder here"}</p>
            <p className="mt-1 text-xs text-ink3">
              PDFs, Office docs, images, HTML, CSV/JSON, text — converted, chunked, embedded, and
              indexed locally.
            </p>
          </div>

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
                    <th className="py-2 font-medium">Title</th>
                    <th className="py-2 font-medium">Project</th>
                    <th className="py-2 font-medium">Importance</th>
                    <th className="py-2 text-right font-medium">Chunks</th>
                    {showPower && <th className="py-2 text-right font-medium">Ingested</th>}
                  </tr>
                </thead>
                <tbody>
                  {documents.map((doc) => (
                    <tr key={doc.id} className="border-b border-rule">
                      <td className="py-2 pr-3">
                        <div className="truncate text-ink" title={doc.title}>
                          {doc.title}
                        </div>
                        {doc.source_path && (
                          <div className="truncate text-xs text-ink4" title={doc.source_path}>
                            {doc.source_path}
                          </div>
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
                      <td className="py-2 pr-3 text-right text-ink3">{doc.chunk_count}</td>
                      {showPower && (
                        <td className="py-2 text-right text-ink4">{formatDate(doc.ingested_at)}</td>
                      )}
                    </tr>
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
