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
  devRetrievalExplain,
  chatIdentityReport,
  devSidecarNetSelftest,
  devSidecarSandboxReport,
  devSystemInfo,
  devTableCounts,
  devTableList,
  devTableRows,
  sidecarStatus,
} from "../lib/ipc";
import type {
  ChatIdentityReport,
  DevRetrievalExplain,
  DevSystemInfo,
  DevTableCount,
  DevTablePage,
  NetSelftest,
  SandboxReport,
  SidecarStatus,
} from "../lib/types";
import { isDevBuild } from "../lib/capabilities";
import { Button, Select } from "./ui";
import { DevPanel } from "./dev/DevPanel";
import { DevTableGrid } from "./dev/DevTableGrid";
import { RetrievalScoreTable } from "./RetrievalScoreTable";

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

function sandboxLabel(s: SandboxReport | null): string {
  if (!s) return "—";
  switch (s.state) {
    case "confined":
      return `confined · ${s.layers.join(" + ")}`;
    case "degraded":
      return `degraded (${s.layers.join(" + ")} only) — [${s.code}] ${s.detail}`;
    case "unconfined":
      return `unconfined — [${s.code}] ${s.detail}`;
    case "not_spawned":
      return "worker not started yet";
    case "unsupported":
      return "not supported on this OS";
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
  const [sandbox, setSandbox] = useState<SandboxReport | null>(null);
  const [counts, setCounts] = useState<DevTableCount[]>([]);
  const [tables, setTables] = useState<string[]>([]);
  const [table, setTable] = useState("documents");
  const [offset, setOffset] = useState(0);
  const [page, setPage] = useState<DevTablePage | null>(null);
  const [corrections, setCorrections] = useState<DevTablePage | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Network-block self-test (issue #286): button-triggered — the ONLY thing here that makes the worker
  // attempt a socket, and the command is compiled out of release builds (so `isDevBuild` gates it).
  const [netTest, setNetTest] = useState<NetSelftest | null>(null);
  const [netTesting, setNetTesting] = useState(false);
  const [netErr, setNetErr] = useState<string | null>(null);

  // Chat-identity integrity (3.81.2). Surfaced because the defect it reports on was invisible:
  // a stripped chat vault file looked perfectly healthy right up until a Rebuild demoted the
  // conversation to an ordinary document. Running the check also runs the repair — they are the
  // same idempotent pass — so there is no way to look without fixing anything found.
  const [chatId, setChatId] = useState<ChatIdentityReport | null>(null);
  const [chatIdBusy, setChatIdBusy] = useState(false);
  const [chatIdErr, setChatIdErr] = useState<string | null>(null);

  const runChatIdentity = useCallback(() => {
    setChatIdBusy(true);
    setChatIdErr(null);
    chatIdentityReport()
      .then(setChatId)
      .catch((e) => {
        setChatId(null);
        setChatIdErr(String(e));
      })
      .finally(() => setChatIdBusy(false));
  }, []);

  // The worker spawns LAZILY, so the confinement report reads `not_spawned` until something first asks
  // it to do work — and the self-test below is exactly such a thing. Re-read it whenever that could
  // have moved, or the panel goes on reporting "worker not started yet" directly above a probe that
  // just proved the worker started and had its socket refused. Also on the error path: a self-test that
  // fails partway may still have spawned (and confined) the worker before failing.
  const readSandbox = useCallback(() => {
    devSidecarSandboxReport()
      .then(setSandbox)
      .catch(() => {});
  }, []);

  const runNetTest = useCallback(() => {
    setNetTesting(true);
    setNetErr(null);
    devSidecarNetSelftest()
      .then(setNetTest)
      .catch((e) => {
        setNetTest(null);
        setNetErr(String(e));
      })
      .finally(() => {
        setNetTesting(false);
        readSandbox();
      });
  }, [readSandbox]);

  // Retrieval explain (issue #81): button-triggered so the sidecar embed never fires on its own.
  const [query, setQuery] = useState("");
  const [topK, setTopK] = useState(6);
  const [explain, setExplain] = useState<DevRetrievalExplain | null>(null);
  const [explaining, setExplaining] = useState(false);
  const [explainErr, setExplainErr] = useState<string | null>(null);

  const runExplain = useCallback(() => {
    const q = query.trim();
    if (!q) return;
    setExplaining(true);
    setExplainErr(null);
    devRetrievalExplain(q, undefined, topK)
      .then(setExplain)
      .catch((e) => {
        setExplain(null);
        setExplainErr(String(e));
      })
      .finally(() => setExplaining(false));
  }, [query, topK]);

  const refresh = useCallback(() => {
    setError(null);
    devSystemInfo()
      .then(setInfo)
      .catch((e) => setError(String(e)));
    sidecarStatus()
      .then(setSidecar)
      .catch(() => {});
    readSandbox();
    devTableCounts()
      .then(setCounts)
      .catch(() => {});
    devTableRows("corrections", PAGE, 0)
      .then(setCorrections)
      .catch(() => {});
  }, [readSandbox]);

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
            title="Sidecar sandbox"
            helpId="dev-sandbox"
            subtitle="Whether the untrusted-file worker is confined in a no-network OS sandbox (Windows AppContainer; Linux Landlock + seccomp) — it fails open, and any fall-back or degraded state shows here with an SBX-#### code you can quote when reporting a problem."
          >
            <div className="flex flex-col">
              <Row label="Confinement" value={sandboxLabel(sandbox)} />
              {sandbox?.state === "confined" && (
                <>
                  <Row label="Enforces" value={sandbox.layers.join(" + ")} />
                  <Row label="Mechanism" value={sandbox.mechanism} />
                  <Row label="Staging dir" value={sandbox.staging_dir} />
                </>
              )}
            </div>

            {sandbox?.state === "confined" && sandbox.granted_dirs.length > 0 && (
              <div className="mt-3">
                <p className="text-xs text-ink3">Readable dirs (everything else is denied):</p>
                <ul className="mt-1 flex flex-col gap-0.5">
                  {sandbox.granted_dirs.map((d) => (
                    <li key={d} className="break-all font-mono text-[0.6875rem] text-ink4">
                      {d}
                    </li>
                  ))}
                </ul>
              </div>
            )}

            {isDevBuild && (
              <div className="mt-4 border-t border-rule pt-3">
                <div className="flex flex-wrap items-center gap-2">
                  <Button onClick={runNetTest} disabled={netTesting}>
                    {netTesting ? "Testing…" : "Run network self-test"}
                  </Button>
                  <span className="text-xs text-ink4">
                    Asks the worker to attempt one outbound socket — live proof the network block
                    holds.
                  </span>
                </div>
                {netErr && <p className="mt-2 text-xs text-[var(--st-due)]">{netErr}</p>}
                {netTest && (
                  <>
                    <p className="mt-2 text-xs">
                      <span className="text-ink4">socket: </span>
                      <span className={netTest.blocked ? "text-st-quick" : "text-st-due"}>
                        {netTest.blocked ? "✓ blocked" : "✗ not blocked"}
                      </span>
                      <span className="text-ink3"> — {netTest.detail}</span>
                      {netTest.errno != null && (
                        <span className="font-mono text-ink4"> (errno {netTest.errno})</span>
                      )}
                    </p>
                    <p className="mt-1 text-xs">
                      <span className="text-ink4">DNS: </span>
                      <span className={netTest.dns_blocked ? "text-st-quick" : "text-st-due"}>
                        {netTest.dns_blocked ? "✓ blocked" : "✗ not blocked"}
                      </span>
                      <span className="text-ink3"> — {netTest.dns_detail}</span>
                    </p>
                  </>
                )}
              </div>
            )}

            <div className="mt-4 border-t border-rule pt-3">
              <div className="flex flex-wrap items-center gap-2">
                <Button onClick={runChatIdentity} disabled={chatIdBusy}>
                  {chatIdBusy ? "Checking…" : "Check chat identity"}
                </Button>
                <span className="text-xs text-ink4">
                  Verifies every chat still carries its vault identity, and repairs any that lost
                  it. Runs automatically on unlock and before every Rebuild.
                </span>
              </div>
              {chatIdErr && <p className="mt-2 text-xs text-[var(--st-due)]">{chatIdErr}</p>}
              {chatId && (
                <>
                  <p className="mt-2 text-xs">
                    <span className="text-ink4">chats: </span>
                    <span
                      className={
                        chatId.intact === chatId.total_sessions ? "text-st-quick" : "text-st-due"
                      }
                    >
                      {chatId.intact === chatId.total_sessions
                        ? `✓ ${chatId.total_sessions} of ${chatId.total_sessions} identity-intact`
                        : `${chatId.intact} of ${chatId.total_sessions} identity-intact`}
                    </span>
                  </p>
                  <p className="mt-1 text-xs text-ink3">
                    <span className="text-ink4">this run: </span>
                    {chatId.live.restamped} file(s) restamped, {chatId.live.rows_restored} row(s)
                    restored, {chatId.live.relinked} re-linked, {chatId.live.reindex_queued} queued
                    for re-index (of {chatId.live.scanned} scanned)
                  </p>
                  {chatId.stored && (
                    <p className="mt-1 text-xs text-ink4">
                      last automatic pass: {chatId.stored.restamped} restamped,{" "}
                      {chatId.stored.rows_restored} restored, of {chatId.stored.scanned} scanned
                    </p>
                  )}
                  {chatId.live.unrepaired.length > 0 && (
                    <ul className="mt-1 text-xs text-[var(--st-due)]">
                      {chatId.live.unrepaired.map((u) => (
                        <li key={u}>{u}</li>
                      ))}
                    </ul>
                  )}
                </>
              )}
            </div>
          </DevPanel>

          <DevPanel
            title="Retrieval explain"
            helpId="dev-retrieval"
            subtitle="Run a query through the live hybrid retriever and see why each chunk ranks. Read-only; chunk text is a truncated preview."
          >
            <form
              className="flex flex-wrap items-end gap-2"
              onSubmit={(e) => {
                e.preventDefault();
                runExplain();
              }}
            >
              <input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Query to explain…"
                className="min-w-0 flex-1 rounded-[var(--radius-sm)] border border-border bg-surface px-2 py-1 text-sm text-ink"
              />
              <label className="flex items-center gap-1 text-xs text-ink3">
                k
                <input
                  type="number"
                  min={1}
                  max={50}
                  value={topK}
                  onChange={(e) =>
                    setTopK(Math.max(1, Math.min(50, Math.trunc(Number(e.target.value)) || 6)))
                  }
                  className="w-14 rounded-[var(--radius-sm)] border border-border bg-surface px-2 py-1 text-sm text-ink"
                />
              </label>
              <Button type="submit" disabled={explaining || !query.trim()}>
                {explaining ? "Running…" : "Run"}
              </Button>
            </form>

            {explainErr && <p className="mt-2 text-xs text-[var(--st-due)]">{explainErr}</p>}

            {explain && (
              <div className="mt-3">
                <p className="font-mono text-[0.6875rem] text-ink4">
                  {explain.embedder_label} · rerank {explain.reranking_enabled ? "on" : "off"}
                  {explain.reranking_enabled
                    ? ` (applied: ${explain.reranked ? "yes" : "no"})`
                    : ""}{" "}
                  · rrf k={explain.rrf_k} · half-life {explain.half_life_days}d · k={explain.k}
                </p>
                {explain.rows.length === 0 ? (
                  <p className="mt-2 text-xs text-ink4">
                    No candidates — nothing indexed matches this query.
                  </p>
                ) : (
                  <RetrievalScoreTable rows={explain.rows} />
                )}
              </div>
            )}
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
