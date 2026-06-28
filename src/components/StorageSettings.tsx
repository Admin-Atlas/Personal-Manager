// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Settings → Storage tab: an inventory of the large, regenerable on-device components (the Python
// document engine, the optional t-SNE libraries, the photo-OCR stack, the speech model, and the
// active search model) with their sizes and a reference-counted teardown. A heavy shared library can
// only be removed once nothing still needs it — its Remove button is greyed with a pill that jumps to
// the dependent to remove first; the cascade is also enforced in the backend. numpy is never offered
// (shared with the search model). Removing the big libraries asks for an extra confirm that recommends
// keeping them. The photo-OCR stack is also installed from here (its row offers Install when absent),
// so the whole feature is enabled/disabled in one place.

import { useCallback, useEffect, useState } from "react";
import {
  installOptionalOcr,
  listStorageComponents,
  onOcrInstall,
  removeStorageComponent,
} from "../lib/ipc";
import type { StorageComponent, StorageReport } from "../lib/types";
import { Button, ConfirmDialog } from "./ui";
import { IngestProgress } from "./IngestProgress";

/** Human-friendly size; estimates are prefixed with "~". */
function formatSize(bytes: number, approximate: boolean): string {
  if (bytes <= 0) return approximate ? "—" : "0 MB";
  const mb = bytes / (1024 * 1024);
  const s = mb >= 1024 ? `${(mb / 1024).toFixed(1)} GB` : `${Math.round(mb)} MB`;
  return approximate ? `~${s}` : s;
}

/** The big t-SNE libraries — removing these gets the extra "recommend keeping" confirm. */
const HEAVY_LIBS = new Set(["scikit-learn", "scipy"]);
/** The OCR image libraries — removed in the cascade after photo text recognition; their own confirm. */
const OCR_LIBS = new Set(["opencv-python", "shapely", "pyclipper"]);

function confirmCopy(c: StorageComponent): { title: string; body: string; danger: boolean } {
  if (HEAVY_LIBS.has(c.id)) {
    return {
      title: `Remove ${c.label}?`,
      danger: true,
      body: "These libraries are only used by the enhanced map layout and total roughly 150 MB. We recommend keeping them unless you're short on space — removing them means another download if you re-enable the enhanced layout.",
    };
  }
  if (OCR_LIBS.has(c.id)) {
    return {
      title: `Remove ${c.label}?`,
      danger: true,
      body: "This library is only used by photo text recognition. We recommend keeping it unless you're short on space — removing it means another download if you reinstall photo text recognition.",
    };
  }
  if (c.id === "openTSNE") {
    return {
      title: "Remove the enhanced layout?",
      danger: true,
      body: "The Map returns to the basic (PCA) layout. You can download the enhanced layout again any time from Settings → Memory map.",
    };
  }
  if (c.id === "ocr") {
    return {
      title: "Remove photo text recognition?",
      danger: true,
      body: "New photos and screenshots will be indexed by their date and location only, with no text. You can reinstall it any time — from here, or the next time you drop a photo.",
    };
  }
  if (c.id === "whisper") {
    return {
      title: "Remove the speech model?",
      danger: false,
      body: "It re-downloads (~145 MB) automatically the next time you record a voice note.",
    };
  }
  return { title: `Remove ${c.label}?`, danger: true, body: "This component will be removed." };
}

export function StorageSettings({ onNavigate }: { onNavigate: (tab: string) => void }) {
  const [report, setReport] = useState<StorageReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState<StorageComponent | null>(null);
  const [removingId, setRemovingId] = useState<string | null>(null);
  // The optional photo-OCR stack installs in place from its inventory row: `installingId` marks the
  // row mid-download and `installFrac` (0..1, from the `ocr://install` progress event) drives its bar.
  const [installingId, setInstallingId] = useState<string | null>(null);
  const [installFrac, setInstallFrac] = useState(0);

  const load = useCallback(() => {
    listStorageComponents()
      .then(setReport)
      .catch((e) => setError(String(e)));
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  useEffect(() => {
    let cancelled = false;
    const un = onOcrInstall((e) => {
      if (!cancelled) setInstallFrac(e.fraction);
    });
    return () => {
      cancelled = true;
      void un.then((fn) => fn());
    };
  }, []);

  function scrollTo(anchor: string) {
    const el = document.getElementById(`storage-${anchor}`);
    el?.scrollIntoView({ behavior: "smooth", block: "center" });
    el?.animate?.([{ opacity: 0.4 }, { opacity: 1 }], { duration: 600 });
  }

  function confirmRemove() {
    const c = pending;
    if (!c) return;
    setRemovingId(c.id);
    setError(null);
    removeStorageComponent(c.id)
      .then(() => load())
      .catch((e) => setError(String(e)))
      .finally(() => {
        setRemovingId(null);
        setPending(null);
      });
  }

  // Install an optional component in place (photo-OCR is the only installable one today). Re-scans on
  // success so the row flips to its installed/removable form (with the image-library children).
  function installComponent(c: StorageComponent) {
    if (c.id !== "ocr") return;
    setInstallingId(c.id);
    setInstallFrac(0);
    setError(null);
    installOptionalOcr()
      .then(() => load())
      .catch((e) => setError(String(e)))
      .finally(() => setInstallingId(null));
  }

  const copy = pending ? confirmCopy(pending) : null;

  return (
    <div data-help="settings-storage">
      <div className="flex items-baseline justify-between">
        <div>
          <label className="block text-sm font-medium text-ink2">On-device components</label>
          <p className="mt-1 text-xs text-ink4">
            What PM has downloaded to this device, and what you can safely remove. Everything here
            re-downloads on demand if you need it again.
          </p>
        </div>
        {report && (
          <span className="shrink-0 font-mono text-xs text-ink3">
            {formatSize(report.total_bytes, false)} total
          </span>
        )}
      </div>

      {error && (
        <div
          className="mt-3 rounded-[var(--radius-sm)] border px-3 py-2 text-xs"
          style={{
            borderColor: "color-mix(in oklab, var(--st-due) 45%, transparent)",
            background: "color-mix(in oklab, var(--st-due) 15%, transparent)",
            color: "var(--st-due)",
          }}
        >
          {error}
        </div>
      )}

      <div className="mt-3 divide-y divide-border rounded-[var(--radius)] border border-border">
        {report?.components.map((c) => (
          <ComponentRow
            key={c.id}
            c={c}
            busy={removingId === c.id}
            installing={installingId === c.id}
            frac={installFrac}
            onRemove={() => setPending(c)}
            onInstall={() => installComponent(c)}
            onPill={scrollTo}
            onManage={onNavigate}
          />
        ))}
        {!report && <div className="px-4 py-6 text-center text-sm text-ink4">Scanning…</div>}
      </div>

      <ConfirmDialog
        open={pending != null}
        title={copy?.title ?? ""}
        confirmLabel="Remove"
        danger={copy?.danger}
        busy={removingId != null}
        onConfirm={confirmRemove}
        onClose={() => setPending(null)}
      >
        {copy?.body}
      </ConfirmDialog>
    </div>
  );
}

function StatusChip({ status }: { status: StorageComponent["status"] }) {
  // Removable and installable rows carry their own action button — no chip needed.
  if (status === "removable" || status === "installable") return null;
  const label =
    status === "required" ? "Required" : status === "in_use" ? "In use" : "Needs a step first";
  return (
    <span className="rounded-[var(--radius-sm)] bg-bg px-1.5 py-0.5 font-mono text-[11px] text-ink4">
      {label}
    </span>
  );
}

function ComponentRow({
  c,
  busy,
  installing,
  frac,
  onRemove,
  onInstall,
  onPill,
  onManage,
}: {
  c: StorageComponent;
  busy: boolean;
  installing: boolean;
  frac: number;
  onRemove: () => void;
  onInstall: () => void;
  onPill: (anchor: string) => void;
  onManage: (tab: string) => void;
}) {
  return (
    <div id={`storage-${c.id}`} className={c.child ? "pl-9" : ""}>
      <div className="flex items-start justify-between gap-4 px-4 py-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <span className="truncate text-sm text-ink2">{c.label}</span>
            <span className="shrink-0 font-mono text-xs text-ink4">
              {formatSize(c.size_bytes, c.approximate)}
            </span>
            <StatusChip status={c.status} />
          </div>
          <p className="mt-0.5 text-xs text-ink4">{c.detail}</p>
          {c.note && <p className="mt-0.5 text-xs text-ink4">{c.note}</p>}
          {(c.blockers.length > 0 || c.manage) && (
            <div className="mt-1.5 flex flex-wrap gap-1.5">
              {c.blockers.map((b) => (
                <button
                  key={b.anchor}
                  type="button"
                  onClick={() => onPill(b.anchor)}
                  className="rounded-full border border-border2 px-2 py-0.5 text-xs text-ink3 transition hover:text-ink hover:border-accent"
                >
                  {b.label} →
                </button>
              ))}
              {c.manage && (
                <button
                  type="button"
                  onClick={() => onManage(c.manage!.tab)}
                  className="rounded-full border border-border2 px-2 py-0.5 text-xs text-ink3 transition hover:text-ink hover:border-accent"
                >
                  {c.manage.label} →
                </button>
              )}
            </div>
          )}
        </div>
        <div className="shrink-0">
          {c.status === "removable" && (
            <Button variant="secondary" onClick={onRemove} disabled={busy}>
              {busy ? "Removing…" : "Remove"}
            </Button>
          )}
          {c.status === "blocked" && (
            <Button variant="secondary" disabled>
              Remove
            </Button>
          )}
          {c.status === "installable" && (
            <Button variant="secondary" onClick={onInstall} disabled={installing}>
              {installing ? "Installing…" : "Install"}
            </Button>
          )}
        </div>
      </div>
      {installing && (
        <IngestProgress
          mode="percent"
          processed={Math.round(frac * 100)}
          total={100}
          label="Downloading photo text recognition"
          className="px-4 pb-3"
        />
      )}
    </div>
  );
}
