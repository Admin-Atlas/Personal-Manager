// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Developer mode (issue #78): the dedicated, cross-cutting inspection tab. Read-only — every
// panel displays internal state pulled from the backend's redacting `dev_*` commands; nothing
// here mutates data. Shown only when the runtime `devMode` capability is on (App gates both the
// nav entry and this view). As future features ship, they add their own DevPanel here (or an
// in-context one behind `useDevMode()`), so the maintainer can confirm each one from the UI.

import { useCallback, useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import {
  devSystemInfo,
  devTableCounts,
  devTableList,
  devTableRows,
  sidecarStatus,
} from "../lib/ipc";
import type { DevSystemInfo, DevTableCount, DevTablePage, SidecarStatus } from "../lib/types";
import { Button, Select } from "./ui";
import { DevPanel } from "./dev/DevPanel";
import { DevTableGrid } from "./dev/DevTableGrid";

const PAGE = 50;

function sidecarLabel(s: SidecarStatus | null): string {
  if (!s) return "—";
  switch (s.state) {
    case "ready":
      return "ready";
    case "installing":
      return "installing…";
    case "not_installed":
      return "not installed";
    case "error":
      return `error (${s.kind})`;
  }
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline justify-between gap-4 border-b border-rule py-1 last:border-0">
      <span className="text-xs text-ink3">{label}</span>
      <span className="break-all text-right font-mono text-xs text-ink2">{value}</span>
    </div>
  );
}

export function DevView() {
  const [info, setInfo] = useState<DevSystemInfo | null>(null);
  const [version, setVersion] = useState<string | null>(null);
  const [sidecar, setSidecar] = useState<SidecarStatus | null>(null);
  const [counts, setCounts] = useState<DevTableCount[]>([]);
  const [tables, setTables] = useState<string[]>([]);
  const [table, setTable] = useState("documents");
  const [offset, setOffset] = useState(0);
  const [page, setPage] = useState<DevTablePage | null>(null);
  const [corrections, setCorrections] = useState<DevTablePage | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    setError(null);
    devSystemInfo()
      .then(setInfo)
      .catch((e) => setError(String(e)));
    sidecarStatus()
      .then(setSidecar)
      .catch(() => {});
    devTableCounts()
      .then(setCounts)
      .catch(() => {});
    devTableRows("corrections", PAGE, 0)
      .then(setCorrections)
      .catch(() => {});
  }, []);

  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch(() => {});
    devTableList()
      .then(setTables)
      .catch(() => {});
    refresh();
  }, [refresh]);

  // Reload the selected table whenever the picker or page offset changes.
  useEffect(() => {
    devTableRows(table, PAGE, offset)
      .then(setPage)
      .catch((e) => setError(String(e)));
  }, [table, offset]);

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center justify-between border-b border-border px-6 py-3">
        <div>
          <h1 className="font-head text-sm font-semibold text-ink">Developer</h1>
          <p className="text-xs text-ink3">
            Read-only inspection of PM's internals — nothing here changes data.
          </p>
        </div>
        <Button variant="tertiary" onClick={refresh}>
          Refresh
        </Button>
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto p-6">
        <div className="mx-auto flex max-w-3xl flex-col gap-4">
          {error && <p className="text-xs text-[var(--st-due)]">{error}</p>}

          <DevPanel
            title="System & build"
            helpId="dev-system"
            subtitle="The running app and this vault's index-time configuration."
          >
            <div className="flex flex-col">
              <Row label="App version" value={version ?? "—"} />
              <Row label="Schema migration" value={info ? `v${info.migration_version}` : "—"} />
              <Row
                label="Embedder"
                value={info ? `${info.embedder_label} · ${info.embedder_id}` : "—"}
              />
              <Row label="Vector dimension" value={info ? String(info.vector_dim) : "—"} />
              <Row label="Reranking" value={info ? (info.reranking_enabled ? "on" : "off") : "—"} />
              <Row
                label="Splitter version"
                value={info?.retrieval_stamp ? String(info.retrieval_stamp.splitter_version) : "—"}
              />
              <Row label="Document engine" value={sidecarLabel(sidecar)} />
            </div>
          </DevPanel>

          <DevPanel
            title="Table counts"
            helpId="dev-counts"
            subtitle="Row counts across the store — confirms at a glance that the index is populated."
          >
            <div className="grid grid-cols-2 gap-x-6 sm:grid-cols-3">
              {counts.map((c) => (
                <div
                  key={c.table}
                  className="flex items-baseline justify-between gap-2 border-b border-rule py-1"
                >
                  <span className="font-mono text-xs text-ink3">{c.table}</span>
                  <span className="font-mono text-xs text-ink2">{c.rows}</span>
                </div>
              ))}
            </div>
          </DevPanel>

          <DevPanel
            title="Raw table browser"
            helpId="dev-tables"
            subtitle="Allow-listed columns, newest first. Personal or large fields are truncated or shown as a length."
            actions={
              <Select
                value={table}
                onChange={(e) => {
                  setTable(e.target.value);
                  setOffset(0);
                }}
              >
                {tables.map((t) => (
                  <option key={t} value={t}>
                    {t}
                  </option>
                ))}
              </Select>
            }
          >
            {page ? <DevTableGrid page={page} /> : <p className="text-xs text-ink4">Loading…</p>}
            {page && page.total > PAGE && (
              <div className="mt-3 flex items-center justify-between text-xs text-ink3">
                <span>
                  {page.rows.length === 0 ? 0 : offset + 1}–{offset + page.rows.length} of{" "}
                  {page.total}
                </span>
                <span className="flex gap-2">
                  <Button
                    variant="tertiary"
                    disabled={offset === 0}
                    onClick={() => setOffset(Math.max(0, offset - PAGE))}
                  >
                    Prev
                  </Button>
                  <Button
                    variant="tertiary"
                    disabled={offset + PAGE >= page.total}
                    onClick={() => setOffset(offset + PAGE)}
                  >
                    Next
                  </Button>
                </span>
              </div>
            )}
          </DevPanel>

          <DevPanel
            title="Corrections log"
            helpId="dev-corrections"
            subtitle="Every change you've made to a proposed project / tags / importance — the raw learning signal."
          >
            {corrections && corrections.rows.length > 0 ? (
              <ul className="flex flex-col gap-1.5">
                {corrections.rows.map((r, i) => {
                  // columns: id, document_id, field, before_val, after_val, title, created_at
                  const [, , field, before, after, title, created] = r;
                  return (
                    <li key={i} className="border-b border-rule pb-1.5 text-xs last:border-0">
                      <div className="flex items-center justify-between gap-2">
                        <span className="font-mono text-ink3">{field}</span>
                        <span className="text-ink4">{created}</span>
                      </div>
                      <div className="text-ink2">
                        <span className="text-ink4">{title || "—"}: </span>
                        <span className="text-ink4 line-through">{before || "∅"}</span>
                        {" → "}
                        <span>{after || "∅"}</span>
                      </div>
                    </li>
                  );
                })}
              </ul>
            ) : (
              <p className="text-xs text-ink4">No corrections logged yet.</p>
            )}
          </DevPanel>
        </div>
      </div>
    </div>
  );
}
