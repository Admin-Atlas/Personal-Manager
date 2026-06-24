// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A troubleshooting popup for document-engine (Python sidecar) setup. It opens
// automatically the first time setup fails (and on demand from the Documents
// banner), turning the backend's machine-readable failure `kind` into a short,
// OS-aware fix-it guide. The raw error is tucked into a "Technical details"
// disclosure for diagnosis. Built entirely from existing ui primitives.

import { Button, Card, Collapsible, Modal } from "./ui";
import type { SidecarStatus } from "../lib/types";
import { guideFor, IS_MAC, type SetupGuideMode } from "../lib/setupGuide";

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

export function DocumentEngineGuide({ open, onClose, status, busy, onRetry }: Props) {
  const isError = status?.state === "error";
  const mode: SetupGuideMode = status?.state === "error" ? status.kind : "install";
  const guide = guideFor(mode, IS_MAC);
  const actionLabel = isError ? "Retry setup" : "Set it up now";
  const rawMessage = status?.state === "error" ? status.message : null;

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

        {rawMessage && (
          <div className="mt-4">
            <Collapsible title="Technical details" defaultOpen={false}>
              <pre className="mt-2 overflow-x-auto whitespace-pre-wrap rounded-[var(--radius-sm)] border border-border bg-bg px-3 py-2 font-mono text-xs text-ink3">
                {rawMessage}
              </pre>
            </Collapsible>
          </div>
        )}
      </div>

      <div className="flex items-center justify-end border-t border-border px-6 py-4">
        <Button variant="primary" onClick={onRetry} disabled={busy}>
          {busy ? "Working…" : actionLabel}
        </Button>
      </div>
    </Modal>
  );
}
