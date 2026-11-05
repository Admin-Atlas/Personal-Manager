// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Settings → Storage tab: an inventory of the large, regenerable on-device components (the Python
// document engine, the optional t-SNE libraries, the photo-OCR stack, voice input's speech engine,
// the speech model, and the active search model) with their sizes and a reference-counted teardown.
// A heavy shared library can only be removed once nothing still needs it — its Remove button is
// greyed with a pill that jumps to the dependent to remove first; the cascade is also enforced in
// the backend. numpy is never offered (shared with the search model). Removing the big libraries
// asks for an extra confirm that recommends keeping them. The photo-OCR stack is also installed from
// here (its row offers Install when absent, and Install beside Remove when an update left it on disk
// but not working), so the whole feature is enabled/disabled in one place. Voice input is the same,
// with its switch on its row (Mods spec D8): Turn on downloads the speech engine, Turn off offers to
// remove it and the speech model.

import { useCallback, useEffect, useState } from "react";
import {
  installOptionalOcr,
  listStorageComponents,
  onOcrInstall,
  onVoiceInstall,
  removeStorageComponent,
} from "../lib/ipc";
import type { StorageComponent, StorageReport } from "../lib/types";
import { formatBytes } from "../lib/format";
import { switchActions, voiceRemovable } from "../lib/storageSwitch";
import { setVoiceEnabled } from "../lib/voiceSwitch";
import { prefersReducedMotion, scrollBehavior } from "../theme";
import { Button, Callout, ConfirmDialog, SectionInfo } from "./ui";
import { IngestProgress } from "./IngestProgress";

/** Human-friendly size; estimates are prefixed with "~". An *estimated* zero is unknown, not empty.
 *  The formatting itself is {@link formatBytes} — this is only the copy decision around it. The old
 *  local version floored at MB, so every sub-MB component (pyclipper) rendered a useless "0 MB". */
function formatSize(bytes: number, approximate: boolean): string {
  if (bytes <= 0) return approximate ? "—" : "0 MB";
  const s = formatBytes(bytes);
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
  if (c.id === "voice") {
    return {
      title: "Remove the speech engine?",
      danger: false,
      body: "Voice input is off, so nothing uses it. Turning voice input on again downloads it.",
    };
  }
  return { title: `Remove ${c.label}?`, danger: true, body: "This component will be removed." };
}

export function StorageSettings({ onNavigate }: { onNavigate: (tab: string) => void }) {
  const [report, setReport] = useState<StorageReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState<StorageComponent | null>(null);
  const [removingId, setRemovingId] = useState<string | null>(null);
  // The optional photo-OCR stack and voice input's engine install in place from their rows:
  // `installingId` marks the row mid-download and `installFrac` (0..1, from that component's
  // `<id>://install` progress event) drives its bar. One at a time: the backend serialises them
  // anyway, and one bar can only tell one story.
  const [installingId, setInstallingId] = useState<string | null>(null);
  const [installFrac, setInstallFrac] = useState(0);
  // Turning voice off asks first, and offers to remove the engine and the model (ticked).
  const [voiceOff, setVoiceOff] = useState(false);
  const [removeVoice, setRemoveVoice] = useState(true);
  const [switching, setSwitching] = useState(false);

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
    const progress = (e: { fraction: number }) => {
      if (!cancelled) setInstallFrac(e.fraction);
    };
    const uns = [onOcrInstall(progress), onVoiceInstall(progress)];
    return () => {
      cancelled = true;
      for (const un of uns) void un.then((fn) => fn());
    };
  }, []);

  function scrollTo(anchor: string) {
    const el = document.getElementById(`storage-${anchor}`);
    el?.scrollIntoView({ behavior: scrollBehavior(), block: "center" });
    // A Web Animations flash, so `index.css`'s `animation-duration: 0.001ms !important` cannot reach
    // it — that property governs CSS animations only. Gate it in JS or Reduced motion misses it.
    if (!prefersReducedMotion())
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

  // Install an optional component in place: photo OCR, or voice input's engine, which is installed by
  // switching voice on (also how an engine an update left stale is repaired while voice is on).
  // Re-scans on success so the row flips to its installed form (with the image-library children).
  function installComponent(c: StorageComponent) {
    const install =
      c.id === "ocr" ? installOptionalOcr : c.id === "voice" ? () => setVoiceEnabled(true) : null;
    if (!install || installingId) return;
    setInstallingId(c.id);
    setInstallFrac(0);
    setError(null);
    install()
      .then(() => load())
      .catch((e) => setError(String(e)))
      .finally(() => setInstallingId(null));
  }

  function confirmVoiceOff() {
    const remove = removeVoice && report != null && voiceRemovable(report).any;
    setSwitching(true);
    setError(null);
    setVoiceEnabled(false, remove)
      .catch((e) => setError(String(e)))
      .finally(() => {
        setSwitching(false);
        setVoiceOff(false);
        load();
      });
  }

  const copy = pending ? confirmCopy(pending) : null;
  const voiceFrees = report ? voiceRemovable(report) : { any: false, bytes: 0 };

  return (
    <div data-help="settings-storage">
      <div className="flex items-baseline justify-between">
        <h2 className="block text-sm font-medium text-ink2">On-device components</h2>
        {report && (
          <span className="shrink-0 font-mono text-xs text-ink3">
            {formatSize(report.total_bytes, false)} total
          </span>
        )}
      </div>

      {error && <Callout className="mt-3">{error}</Callout>}

      <div className="mt-3 divide-y divide-border rounded-[var(--radius)] border border-border">
        {report?.components.map((c) => (
          <ComponentRow
            key={c.id}
            c={c}
            busy={removingId === c.id || (c.switch !== null && switching)}
            installing={installingId === c.id}
            anyInstalling={installingId != null}
            frac={installFrac}
            onRemove={() => setPending(c)}
            onInstall={() => installComponent(c)}
            onTurnOff={() => {
              setRemoveVoice(true);
              setVoiceOff(true);
            }}
            onPill={scrollTo}
            onManage={onNavigate}
          />
        ))}
        {!report && <div className="px-4 py-6 text-center text-sm text-ink4">Scanning…</div>}
      </div>

      <SectionInfo>
        <p>
          What PM has downloaded to this device, and what you can safely remove. Everything here
          re-downloads on demand if you need it again.
        </p>
      </SectionInfo>

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

      <ConfirmDialog
        open={voiceOff}
        title="Turn off voice input?"
        confirmLabel="Turn off"
        danger={removeVoice && voiceFrees.any}
        busy={switching}
        onConfirm={confirmVoiceOff}
        onClose={() => setVoiceOff(false)}
      >
        <p>
          The mic leaves the chat box, and PM won&rsquo;t transcribe anything until you turn voice
          input back on here.
        </p>
        {voiceFrees.any && (
          <label className="mt-3 flex items-start gap-2 text-sm text-ink3">
            <input
              type="checkbox"
              className="mt-0.5"
              checked={removeVoice}
              onChange={(e) => setRemoveVoice(e.currentTarget.checked)}
              disabled={switching}
            />
            <span>
              Also remove the speech engine and the speech model from this computer (frees{" "}
              {formatSize(voiceFrees.bytes, false)}). Turning voice on again downloads them.
            </span>
          </label>
        )}
      </ConfirmDialog>
    </div>
  );
}

function StatusChip({ c }: { c: StorageComponent }) {
  // Removable, installable and stale rows carry their own action buttons — no chip needed. A row
  // with a switch says where the switch stands instead.
  if (c.switch !== null) {
    return (
      <span className="rounded-[var(--radius-sm)] bg-bg px-1.5 py-0.5 font-mono text-[0.6875rem] text-ink4">
        {c.switch ? "On" : "Off"}
      </span>
    );
  }
  const status = c.status;
  if (status === "removable" || status === "installable" || status === "stale") return null;
  // `blocked` means installed and held by a dependent, not missing — say "Installed" first, or a
  // shared library reads as something still to be downloaded. The blocker pill names the holder.
  const label =
    status === "required" ? "Required" : status === "in_use" ? "In use" : "Installed — in use";
  return (
    <span className="rounded-[var(--radius-sm)] bg-bg px-1.5 py-0.5 font-mono text-[0.6875rem] text-ink4">
      {label}
    </span>
  );
}

/** The progress bar's words for a component's download. */
function downloadLabel(id: string): string {
  return id === "voice" ? "Downloading the speech engine" : "Downloading photo text recognition";
}

function ComponentRow({
  c,
  busy,
  installing,
  anyInstalling,
  frac,
  onRemove,
  onInstall,
  onTurnOff,
  onPill,
  onManage,
}: {
  c: StorageComponent;
  busy: boolean;
  installing: boolean;
  anyInstalling: boolean;
  frac: number;
  onRemove: () => void;
  onInstall: () => void;
  onTurnOff: () => void;
  onPill: (anchor: string) => void;
  onManage: (tab: string) => void;
}) {
  const actions = switchActions(c);
  return (
    <div id={`storage-${c.id}`} className={c.child ? "pl-9" : ""}>
      <div className="flex items-start justify-between gap-4 px-4 py-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <span className="truncate text-sm text-ink2">{c.label}</span>
            <span className="shrink-0 font-mono text-xs text-ink4">
              {formatSize(c.size_bytes, c.approximate)}
            </span>
            <StatusChip c={c} />
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
        <div className="flex shrink-0 gap-2">
          {c.switch !== null ? (
            actions.map((a) => {
              const idle = !busy && !anyInstalling;
              if (a === "remove") {
                return (
                  <Button key={a} variant="secondary" onClick={onRemove} disabled={!idle}>
                    {busy ? "Removing…" : "Remove"}
                  </Button>
                );
              }
              if (a === "turn-off") {
                return (
                  <Button key={a} variant="secondary" onClick={onTurnOff} disabled={!idle}>
                    Turn off
                  </Button>
                );
              }
              const label =
                a === "turn-on" ? "Turn on" : a === "install" ? "Install" : "Install again";
              return (
                <Button key={a} variant="secondary" onClick={onInstall} disabled={!idle}>
                  {installing ? "Installing…" : label}
                </Button>
              );
            })
          ) : (
            <>
              {c.status === "stale" && (
                <Button
                  variant="secondary"
                  onClick={onInstall}
                  disabled={installing || anyInstalling || busy}
                >
                  {installing ? "Installing…" : "Install again"}
                </Button>
              )}
              {(c.status === "removable" || c.status === "stale") && (
                <Button variant="secondary" onClick={onRemove} disabled={busy || installing}>
                  {busy ? "Removing…" : "Remove"}
                </Button>
              )}
              {c.status === "blocked" && (
                <Button variant="secondary" disabled>
                  Remove
                </Button>
              )}
              {c.status === "installable" && (
                <Button variant="secondary" onClick={onInstall} disabled={anyInstalling}>
                  {installing ? "Installing…" : "Install"}
                </Button>
              )}
            </>
          )}
        </div>
      </div>
      {installing && (
        <IngestProgress
          mode="percent"
          processed={Math.round(frac * 100)}
          total={100}
          label={downloadLabel(c.id)}
          className="px-4 pb-3"
        />
      )}
    </div>
  );
}
