// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A troubleshooting popup for document-engine (Python sidecar) setup. It opens
// automatically the first time setup fails (and on demand from the Documents
// banner), turning the backend's machine-readable failure `kind` into a short,
// OS-aware fix-it guide. The raw error is tucked into a "Technical details"
// disclosure for diagnosis. Built entirely from existing ui primitives.
//
// One failure class is special: `packaging_bug` means the Python that ships
// inside PM is incomplete (a defect on our side, not the user's environment), so
// the primary action becomes "report it" rather than "retry" or "fix your setup".

import { useEffect, useState } from "react";
import { Button, Card, Collapsible, Modal } from "./ui";
import { IngestProgress } from "./IngestProgress";
import { useDepth } from "../theme";
import type { SidecarStatus } from "../lib/types";
import { onPythonInstall } from "../lib/ipc";
import { CHANGELOG } from "../lib/changelog";
import { guideFor, IS_MAC, type SetupGuideMode } from "../lib/setupGuide";

const REPO_URL = "https://github.com/Admin-Atlas/Personal-Manager";

interface Props {
  open: boolean;
  onClose: () => void;
  /** Current engine status — drives which guide and action label to show. */
  status: SidecarStatus | null;
  /** Setup in progress: disables the action button. */
  busy: boolean;
  /** Run (or retry) setup. */
  onRetry: () => void;
}

/** Render a step string, turning `backtick` spans into inline code chips. */
function withCode(text: string) {
  return text.split("`").map((part, i) =>
    i % 2 === 1 ? (
      <code
        key={i}
        className="rounded-[var(--radius-sm)] bg-bg px-1 py-0.5 font-mono text-[0.85em] text-ink"
      >
        {part}
      </code>
    ) : (
      <span key={i}>{part}</span>
    ),
  );
}

/** Pre-fill a GitHub issue for a packaging bug: the app version and the captured
 *  engine output, and nothing from the user's documents. */
function buildReportUrl(detail: string | null): string {
  const version = CHANGELOG[0]?.version ?? "unknown";
  const ua = typeof navigator !== "undefined" ? navigator.userAgent : "unknown";
  const title = `Windows: document engine failed to start (bundled Python incomplete) — v${version}`;
  const body = [
    "**What happened**",
    "PM's document engine couldn't start, and PM classified it as a packaging bug.",
    "",
    `- **PM version:** ${version}`,
    `- **System:** ${ua}`,
    "",
    "**Engine output**",
    "```",
    (detail ?? "(none captured)").slice(0, 4000),
    "```",
  ].join("\n");
  return `${REPO_URL}/issues/new?title=${encodeURIComponent(title)}&body=${encodeURIComponent(body)}`;
}

export function DocumentEngineGuide({ open, onClose, status, busy, onRetry }: Props) {
  const { showPower } = useDepth();

  // macOS only: when no Python is found, setup downloads one and streams byte
  // progress over `python://install`. Show a percentage bar while that runs; it
  // never fires on Windows/Linux (or on a Mac that already has Python), so the bar
  // simply never appears there. Reset when a fresh setup starts.
  const [downloadFrac, setDownloadFrac] = useState(0);
  useEffect(() => {
    if (busy) setDownloadFrac(0);
  }, [busy]);
  useEffect(() => {
    let cancelled = false;
    const un = onPythonInstall((e) => {
      if (!cancelled) setDownloadFrac(e.fraction);
    });
    return () => {
      cancelled = true;
      void un.then((fn) => fn());
    };
  }, []);
  const showDownload = busy && downloadFrac > 0 && downloadFrac < 1;

  const isError = status?.state === "error";
  const kind = status?.state === "error" ? status.kind : null;
  const isPackagingBug = kind === "packaging_bug";
  const mode: SetupGuideMode = isError ? status.kind : "install";
  const guide = guideFor(mode, IS_MAC);
  const actionLabel = isError ? "Retry setup" : "Set it up now";
  const rawMessage = status?.state === "error" ? status.message : null;
  // A packaging bug isn't locally fixable — route the user to a pre-filled report
  // instead of sending them to chase a fix that can't work.
  const reportUrl = isPackagingBug ? buildReportUrl(rawMessage) : null;

  return (
    <Modal
      open={open}
      onClose={onClose}
      labelledBy="doc-engine-guide-title"
      widthClassName="max-w-xl"
      className="flex max-h-[80vh] flex-col"
    >
      <div className="flex items-center justify-between border-b border-border px-6 py-4">
        <h1 id="doc-engine-guide-title" className="font-head text-lg font-semibold text-ink">
          {guide.title}
        </h1>
        <Button variant="tertiary" onClick={onClose}>
          Close
        </Button>
      </div>

      <div className="flex-1 overflow-y-auto px-6 py-4">
        <p className="text-sm text-ink2">{guide.summary}</p>

        <Card className="mt-4 p-4">
          <ol className="space-y-3">
            {guide.steps.map((step, i) => (
              <li key={i} className="flex gap-3 text-sm text-ink2">
                <span className="mt-0.5 select-none font-mono text-xs text-ink4">{i + 1}.</span>
                <span>{withCode(step)}</span>
              </li>
            ))}
          </ol>
        </Card>

        {showDownload && (
          <IngestProgress
            mode="percent"
            processed={Math.round(downloadFrac * 100)}
            total={100}
            label="Downloading Python"
            className="mt-4"
          />
        )}

        {rawMessage && (
          <div className="mt-4">
            <Collapsible title="Technical details" defaultOpen={showPower}>
              <pre className="mt-2 overflow-x-auto whitespace-pre-wrap rounded-[var(--radius-sm)] border border-border bg-bg px-3 py-2 font-mono text-xs text-ink3">
                {rawMessage}
              </pre>
            </Collapsible>
          </div>
        )}

        {isPackagingBug && showPower && (
          <div className="mt-4">
            <p className="mb-1 text-xs font-semibold uppercase tracking-wide text-ink4">
              Diagnostic commands
            </p>
            <pre className="overflow-x-auto whitespace-pre-wrap rounded-[var(--radius-sm)] border border-border bg-bg px-3 py-2 font-mono text-xs text-ink3">
              {[
                "# list the bundled interpreter's files",
                'Get-ChildItem -Recurse "$env:LOCALAPPDATA\\PM\\python" | Select FullName',
                "",
                "# confirm it can import its own standard library",
                '& "$env:LOCALAPPDATA\\PM\\python\\python.exe" -c "import encodings, venv, ssl"',
              ].join("\n")}
            </pre>
          </div>
        )}
      </div>

      <div className="flex items-center justify-end gap-2 border-t border-border px-6 py-4">
        {reportUrl ? (
          <>
            <Button variant="tertiary" onClick={onRetry} disabled={busy}>
              {busy ? "Working…" : "Retry anyway"}
            </Button>
            <a
              href={reportUrl}
              target="_blank"
              rel="noreferrer"
              className="inline-flex items-center justify-center gap-1.5 rounded-[var(--radius-sm)] bg-accent px-3 py-1.5 text-sm font-semibold text-accent-ink transition hover:brightness-105"
            >
              Report on GitHub
            </a>
          </>
        ) : (
          <Button variant="primary" onClick={onRetry} disabled={busy}>
            {busy ? "Working…" : actionLabel}
          </Button>
        )}
      </div>
    </Modal>
  );
}
