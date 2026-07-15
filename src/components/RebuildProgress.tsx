// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The guided Re-index modal. Given `open`, it runs `rebuildIndex` once — dropping the search index
// and rebuilding it from the Markdown vault — and follows progress on the global `ingest://progress`
// event the Documents view also listens to. Self-contained so the Settings language switcher can
// launch it without re-implementing the rebuild plumbing; the Documents "Rebuild" banner remains its
// own (older, inline) entry point.
//
// The rebuild is detached from this modal: closing it (or the tab) doesn't stop the work, and the
// backend's snapshot lets whatever mounts next pick the progress back up.
//
// Safety: a non-bundled multilingual model downloads (~1 GB) at the *start* of the rebuild
// (warmup-before-destroy in Rust), so an offline failure leaves the existing index intact — we then
// call `onError` so the caller can revert the just-changed selection. The modal blocks closing
// while running, so no search is attempted during the brief width-mismatch window.

import { useEffect, useRef, useState } from "react";
import { onIngestProgress, rebuildIndex } from "../lib/ipc";
import type { IngestEvent } from "../lib/types";
import { Button, Collapsible, Modal } from "./ui";
import { IngestProgress } from "./IngestProgress";

interface Props {
  /** When true, the modal shows and a rebuild kicks off once. */
  open: boolean;
  /** Heading (default: "Re-indexing your vault"). */
  title?: string;
  /** One-line context under the heading (e.g. what the user just switched to). */
  subtitle?: string;
  /** The rebuild finished successfully. */
  onDone?: () => void;
  /** The rebuild failed (e.g. offline). The caller should revert any selection it changed. */
  onError?: () => void;
  /** The user dismissed the finished/errored modal. */
  onClose: () => void;
}

type Phase = "running" | "done" | "error";

export function RebuildProgress({ open, title, subtitle, onDone, onError, onClose }: Props) {
  const [phase, setPhase] = useState<Phase>("running");
  const [prep, setPrep] = useState<string | null>(null);
  const [files, setFiles] = useState<string[]>([]);
  // Determinate-bar inputs: `total` from the `counted` event, `processed` counted up as each
  // file lands. Null total (model download / warmup) keeps the bar an indeterminate sweep.
  const [total, setTotal] = useState<number | null>(null);
  const [processed, setProcessed] = useState(0);
  // Items that failed to re-index this run (B3-7). A wholesale connector-manifest failure surfaces
  // here as one synthetic `failed` event, so a rebuild that couldn't restore your cloud-indexed items
  // reads as a partial success, not a clean one.
  const [failedCount, setFailedCount] = useState(0);
  const [error, setError] = useState<string | null>(null);
  // Callbacks via refs so the run effect can depend only on `open` (no churn when the parent
  // passes fresh inline closures each render).
  const cb = useRef({ onDone, onError });
  cb.current = { onDone, onError };
  // Guard against React StrictMode's double-invoke (dev) firing two rebuilds.
  const started = useRef(false);

  useEffect(() => {
    if (!open) {
      // Reset for the next open.
      started.current = false;
      setPhase("running");
      setPrep(null);
      setFiles([]);
      setTotal(null);
      setProcessed(0);
      setFailedCount(0);
      setError(null);
      return;
    }
    if (started.current) return;
    started.current = true;
    void (async () => {
      try {
        await rebuildIndex();
        setPhase("done");
        cb.current.onDone?.();
      } catch (e) {
        setError(String(e));
        setPhase("error");
        cb.current.onError?.();
      }
    })();
  }, [open]);

  // Progress arrives on the global event, not a per-call channel, so it keeps reaching this modal
  // regardless of which surface started the rebuild. Subscribing separately from the run effect also
  // means a rebuild already in flight when the modal opens still renders live.
  useEffect(() => {
    if (!open) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void onIngestProgress((event: IngestEvent) => {
      switch (event.type) {
        case "preparing":
          setPrep(event.message);
          break;
        case "counted":
          setTotal(event.total);
          break;
        case "started":
          setPrep(null);
          setFiles((prev) => [...prev, event.name]);
          break;
        case "done":
          // Each file's terminal event advances the determinate bar; the names roll into
          // the running list, and the final summary line is enough for the rest.
          setProcessed((n) => n + 1);
          break;
        case "failed":
          setProcessed((n) => n + 1);
          setFailedCount((n) => n + 1);
          break;
        default:
          break;
      }
    }).then((fn) => {
      // The subscription resolves asynchronously; if we already unmounted, drop it immediately
      // rather than leaking a listener.
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [open]);

  if (!open) return null;
  const running = phase === "running";

  return (
    <Modal open={open} onClose={running ? () => {} : onClose} widthClassName="max-w-md">
      <div className="p-6">
        <h2 className="font-head text-base font-semibold text-ink">
          {title ?? "Re-indexing your vault"}
        </h2>
        {subtitle && <p className="mt-1 text-xs text-ink4">{subtitle}</p>}

        {running && (
          <IngestProgress
            className="mt-4"
            label="Re-indexing"
            processed={processed}
            total={total}
          />
        )}
        {prep && <p className="mt-3 text-sm text-ink3">{prep}</p>}

        {files.length > 0 && (
          <div className="mt-3">
            <Collapsible title="Files" meta={`${files.length}`} defaultOpen={false}>
              <ul className="max-h-40 overflow-y-auto pt-1">
                {files.map((name, i) => (
                  <li key={i} className="truncate px-1 py-0.5 text-xs text-ink3">
                    {name}
                  </li>
                ))}
              </ul>
            </Collapsible>
          </div>
        )}

        {phase === "done" && failedCount === 0 && (
          <p className="mt-4 text-sm text-[var(--st-quick)]">
            Done — your library is re-indexed with the new search language.
          </p>
        )}
        {phase === "done" && failedCount > 0 && (
          <p className="mt-4 text-sm text-st-due">
            Re-indexed, but {failedCount} item{failedCount === 1 ? "" : "s"} couldn&apos;t be
            restored this time — they&apos;ll be picked up on the next sync or rebuild. Nothing was
            removed.
          </p>
        )}
        {phase === "error" && (
          <p className="mt-4 text-sm text-st-due">
            {error} You can try again once you&apos;re back online — your documents weren&apos;t
            touched.
          </p>
        )}

        {!running && (
          <div className="mt-5 flex justify-end">
            <Button variant="primary" onClick={onClose}>
              {phase === "error" ? "Close" : "Done"}
            </Button>
          </div>
        )}
      </div>
    </Modal>
  );
}
