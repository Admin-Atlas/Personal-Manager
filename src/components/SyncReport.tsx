// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useState } from "react";
import type { SyncReport as SyncReportData } from "../lib/types";

/**
 * The **post-sync summary** shared by every index-only connector (Drive / OneDrive / local folders):
 * how many files were indexed/updated/removed, and an expandable list of any that couldn't be
 * (unsupported types, fetch/read errors) so the user knows exactly what was left out. `helpId` scopes
 * the connector-specific help anchor (e.g. `settings-drive-report`).
 */
export function SyncReport({
  report,
  helpId,
  onDismiss,
}: {
  report: SyncReportData;
  helpId: string;
  onDismiss: () => void;
}) {
  const [showIssues, setShowIssues] = useState(false);
  const touched = report.indexed + report.updated + report.removed;
  const issueCount = report.issues.length;
  return (
    <div className="mt-3 rounded-[var(--radius)] border border-border p-3" data-help={helpId}>
      <div className="flex items-start justify-between gap-2">
        <div className="text-xs text-ink2">
          {report.cancelled ? (
            <span className="font-medium text-ink">Indexing stopped.</span>
          ) : (
            <span className="font-medium text-ink">Sync complete.</span>
          )}{" "}
          <span className="text-ink3">
            Indexed {report.indexed} · updated {report.updated} · removed {report.removed}
            {touched === 0 && " · nothing new"}.
          </span>
          {report.cancelled && (
            <span className="text-ink4">
              {" "}
              Everything indexed so far is kept — sync again to finish.
            </span>
          )}
        </div>
        <button
          type="button"
          onClick={onDismiss}
          aria-label="Dismiss summary"
          className="shrink-0 text-ink4 hover:text-ink2"
        >
          ×
        </button>
      </div>

      {report.issues.length > 0 && (
        <div className="mt-2">
          <button
            type="button"
            onClick={() => setShowIssues((v) => !v)}
            className="font-mono text-[10px] uppercase tracking-wide text-ink3 hover:text-ink"
          >
            {showIssues ? "▾" : "▸"} {issueCount}
            {report.issues_truncated ? "+" : ""} file
            {issueCount === 1 && !report.issues_truncated ? "" : "s"} not indexed
          </button>
          {showIssues && (
            <ul className="mt-1.5 max-h-40 space-y-1 overflow-auto">
              {report.issues.map((iss, i) => (
                <li key={i} className="text-[11px] leading-tight">
                  <span className="text-ink2">{iss.name}</span>
                  <span className="text-ink4"> — {iss.reason}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}

      <p className="mt-2 text-[11px] text-ink4">
        Indexed files are searchable and appear in Documents.
      </p>
    </div>
  );
}
