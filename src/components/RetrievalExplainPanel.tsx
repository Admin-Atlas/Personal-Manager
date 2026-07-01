// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// In-chat "Retrieval explain" (card 7H): the transparency panel from Developer mode, surfaced to
// graduated users right under the chat composer. It shows which chunks a query retrieves and how
// they scored, and exposes the one lever that matters — the retrieval depth `k`, the size of the
// candidate POOL the reranker sees (not a display count). Dragging the slider PREVIEWS a pool live;
// a distinct "Use this depth" button is the only thing that commits it (no silent, one-drag change).
// A plain-language diagnostic reads the user's own explain state and RECOMMENDS what to change — it
// never actuates; the user makes the change themselves.
//
// Gated by `teachVisible` — the same graduation toggle that governs the Review/Teach tabs — so it's
// hidden on the calm/minimal surface and recoverable from Appearance settings. The component owns
// that gate, so both chat surfaces (global + project) mount it unconditionally.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { DevRetrievalExplain, DevRetrievalRow, Message } from "../lib/types";
import { getSettings, retrievalDiagnose, retrievalExplain, setRetrievalK } from "../lib/ipc";
import { useTheme } from "../theme";
import { Button, HScroll } from "./ui";

/** The depth bounds mirror the backend clamp (`db::RETRIEVAL_K_{MIN,MAX}`). */
const K_MIN = 1;
const K_MAX = 50;

interface Props {
  messages: Message[];
  /** Project scope for a project-scoped chat; omitted for the global chat. Passed straight to the
   *  retriever so the panel explains exactly what a real turn in this surface would retrieve. */
  project?: string;
}

/** The vector branch's cell: the raw `vec0` distance (lower = nearer), rank on hover; "—" when the
 *  chunk surfaced via the keyword branch only. Mirrors the Developer-mode panel. */
function vecCell(r: DevRetrievalRow): { text: string; title: string } {
  if (r.vector_rank == null) return { text: "—", title: "not in the vector branch" };
  const dist = r.vector_distance != null ? r.vector_distance.toFixed(3) : "?";
  return { text: dist, title: `vector rank ${r.vector_rank}` };
}

export function RetrievalExplainPanel({ messages, project }: Props) {
  const { teachVisible } = useTheme();

  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  // The persisted retrieval depth (what a real chat turn uses); null until loaded. `k` is the slider
  // value — a preview until committed back to `savedK` via "Use this depth".
  const [savedK, setSavedK] = useState<number | null>(null);
  const [k, setK] = useState(6);
  const [explain, setExplain] = useState<DevRetrievalExplain | null>(null);
  // The query the current `explain` was actually run with — passed to the diagnostic so it never reasons
  // about a query the displayed rows didn't come from (the user may edit the box after Explain without re-running).
  const [explainedQuery, setExplainedQuery] = useState("");
  const [running, setRunning] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [committing, setCommitting] = useState(false);

  const [symptom, setSymptom] = useState("");
  const [advice, setAdvice] = useState<string | null>(null);
  const [diagnosing, setDiagnosing] = useState(false);
  const [diagErr, setDiagErr] = useState<string | null>(null);

  // Default the query to the user's most recent turn, so opening the panel explains the retrieval
  // behind the answer they're looking at.
  const lastUserQuery = useMemo(() => {
    for (let i = messages.length - 1; i >= 0; i--) {
      if (messages[i].role === "user") return messages[i].content;
    }
    return "";
  }, [messages]);

  const runExplain = useCallback(
    (q: string, depth: number) => {
      const text = q.trim();
      if (!text) {
        setExplain(null);
        setExplainedQuery("");
        return;
      }
      setRunning(true);
      setErr(null);
      retrievalExplain(text, project, depth)
        .then((res) => {
          setExplain(res);
          setExplainedQuery(text);
        })
        .catch((e) => {
          setExplain(null);
          setExplainedQuery("");
          setErr(String(e));
        })
        .finally(() => setRunning(false));
    },
    [project],
  );

  // Load the persisted depth the first time the panel opens; seed the slider from it.
  useEffect(() => {
    if (!open || savedK != null) return;
    getSettings()
      .then((s) => {
        setSavedK(s.retrieval_k);
        setK(s.retrieval_k);
      })
      .catch(() => {
        setSavedK(6);
        setK(6);
      });
  }, [open, savedK]);

  // Seed the query box with the last user turn once, when the panel opens.
  useEffect(() => {
    if (open && !query && lastUserQuery) setQuery(lastUserQuery);
  }, [open, query, lastUserQuery]);

  // Live preview: re-run the explain (debounced) whenever the depth slider moves — and once on open,
  // when `savedK` first lands. The query is read from a ref so moving the slider doesn't couple to
  // keystrokes; editing the query re-runs explicitly via the form submit instead.
  const queryRef = useRef(query);
  queryRef.current = query;
  useEffect(() => {
    if (!open || savedK == null) return;
    const q = queryRef.current.trim();
    if (!q) return;
    const t = window.setTimeout(() => runExplain(q, k), 300);
    return () => window.clearTimeout(t);
  }, [k, open, savedK, runExplain]);

  const commitDepth = useCallback(() => {
    setCommitting(true);
    setErr(null);
    setRetrievalK(k)
      .then(() => setSavedK(k))
      .catch((e) => setErr(String(e)))
      .finally(() => setCommitting(false));
  }, [k]);

  const diagnose = useCallback(() => {
    const s = symptom.trim();
    if (!s || !explain) return;
    setDiagnosing(true);
    setDiagErr(null);
    setAdvice(null);
    // Use the query the explain actually ran with, NOT the live box (which may have been edited since),
    // so the model reasons about the same query/rows pair it's shown.
    retrievalDiagnose(s, explainedQuery, explain)
      .then(setAdvice)
      .catch((e) => setDiagErr(String(e)))
      .finally(() => setDiagnosing(false));
  }, [symptom, explain, explainedQuery]);

  if (!teachVisible) return null;

  const dirty = savedK != null && k !== savedK;

  return (
    <div className="border-t border-border bg-panel" data-help="chat-retrieval-explain">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center justify-between px-4 py-1.5 text-left text-xs text-ink4 hover:text-ink2"
        title="See which notes this chat retrieved and tune how deep it searches"
      >
        <span>
          <span aria-hidden="true">🔍</span> Explain retrieval
        </span>
        <span className="font-mono text-faint">{open ? "▾" : "▸"}</span>
      </button>

      {open && (
        <div className="max-h-[46vh] overflow-y-auto px-4 pb-3">
          <form
            className="flex flex-wrap items-end gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              runExplain(query, k);
            }}
          >
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Query to explain…"
              className="min-w-0 flex-1 rounded-[var(--radius-sm)] border border-border bg-surface px-2 py-1 text-sm text-ink"
            />
            <Button type="submit" variant="secondary" disabled={running || !query.trim()}>
              {running ? "Running…" : "Explain"}
            </Button>
          </form>

          {/* The depth lever. The explainer makes legible that k widens the reranker's pool — it is
              NOT "show me more results". */}
          <div className="mt-3 rounded-[var(--radius-sm)] border border-border bg-surface p-3">
            <div className="flex items-center gap-3">
              <label className="flex flex-1 items-center gap-2 text-xs text-ink3">
                Depth (k)
                <input
                  type="range"
                  min={K_MIN}
                  max={K_MAX}
                  value={k}
                  onChange={(e) => setK(Number(e.target.value))}
                  className="flex-1 accent-[var(--accent)]"
                  aria-label="Retrieval depth (candidate pool size)"
                />
                <span className="w-6 text-right font-mono text-ink2">{k}</span>
              </label>
              <Button
                variant="primary"
                onClick={commitDepth}
                disabled={!dirty || committing}
                title={
                  dirty
                    ? "Make this the depth every chat searches at"
                    : "This is already your retrieval depth"
                }
              >
                {committing ? "Saving…" : "Use this depth"}
              </Button>
            </div>
            <p className="mt-2 text-[11px] leading-snug text-ink4">
              Depth is the size of the candidate <em>pool</em> the reranker gets to weigh — not how
              many results are shown. Widen it and a note that ranked just below the cut can finally
              reach the reranker; a note beyond it never does, however relevant it is.
              {savedK != null && !dirty ? ` Your saved depth is ${savedK}.` : ""}
              {dirty ? " Previewing — not saved yet." : ""}
            </p>
          </div>

          {err && <p className="mt-2 text-xs text-[var(--st-due)]">{err}</p>}

          {explain && (
            <div className="mt-3">
              <p className="font-mono text-[11px] text-ink4">
                {explain.embedder_label} · rerank {explain.reranking_enabled ? "on" : "off"}
                {explain.reranking_enabled
                  ? ` (applied: ${explain.reranked ? "yes" : "no"})`
                  : ""}{" "}
                · k={explain.k}
              </p>
              {explain.rows.length === 0 ? (
                <p className="mt-2 text-xs text-ink4">
                  No candidates — nothing indexed matched this query.
                </p>
              ) : (
                <HScroll className="mt-2">
                  <table className="w-full border-collapse text-left text-xs">
                    <thead>
                      <tr className="text-ink4">
                        <th className="py-1 pr-2 font-medium">#</th>
                        <th className="py-1 pr-2 font-medium">chunk</th>
                        <th className="py-1 pr-2 text-right font-medium">vec</th>
                        <th className="py-1 pr-2 text-right font-medium">kw</th>
                        <th className="py-1 pr-2 text-right font-medium">fused</th>
                        <th className="py-1 pr-2 text-right font-medium">decay</th>
                        <th className="py-1 text-right font-medium">rerank</th>
                      </tr>
                    </thead>
                    <tbody>
                      {explain.rows.map((r) => {
                        const vec = vecCell(r);
                        return (
                          <tr key={r.chunk_id} className="border-t border-rule align-top">
                            <td className="py-1 pr-2 font-mono text-ink3">{r.final_rank + 1}</td>
                            <td className="py-1 pr-2">
                              <div className="text-ink2">
                                {r.title}
                                {r.heading ? (
                                  <span className="text-ink4"> §{r.heading}</span>
                                ) : null}
                              </div>
                              <div className="max-w-md whitespace-pre-wrap break-words text-ink4">
                                {r.preview}
                              </div>
                            </td>
                            <td
                              className="py-1 pr-2 text-right font-mono text-ink3"
                              title={vec.title}
                            >
                              {vec.text}
                            </td>
                            <td className="py-1 pr-2 text-right font-mono text-ink3">
                              {r.keyword_rank ?? "—"}
                            </td>
                            <td className="py-1 pr-2 text-right font-mono text-ink3">
                              {r.fused_score.toFixed(4)}
                            </td>
                            <td className="py-1 pr-2 text-right font-mono text-ink3">
                              {r.decay_factor.toFixed(2)}
                            </td>
                            <td className="py-1 text-right font-mono text-ink3">
                              {r.reranker_score != null ? r.reranker_score.toFixed(3) : "—"}
                            </td>
                          </tr>
                        );
                      })}
                    </tbody>
                  </table>
                </HScroll>
              )}
            </div>
          )}

          {/* The recommend-don't-actuate diagnostic. It reads the state above and advises; any change
              is the user's to make on the slider — there is deliberately no apply button here. */}
          <div className="mt-3 border-t border-rule pt-3">
            <label className="text-xs text-ink3" htmlFor="retrieval-symptom">
              Not finding what you expect? Describe it and PM will suggest what to try.
            </label>
            <div className="mt-1 flex flex-wrap items-end gap-2">
              <input
                id="retrieval-symptom"
                value={symptom}
                onChange={(e) => setSymptom(e.target.value)}
                placeholder="e.g. it keeps missing notes I know I wrote"
                className="min-w-0 flex-1 rounded-[var(--radius-sm)] border border-border bg-surface px-2 py-1 text-sm text-ink"
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    diagnose();
                  }
                }}
              />
              <Button
                variant="secondary"
                onClick={diagnose}
                disabled={diagnosing || !symptom.trim() || !explain}
                title={
                  explain ? "" : "Run an explain first so the diagnostic has something to read"
                }
              >
                {diagnosing ? "Thinking…" : "Diagnose"}
              </Button>
            </div>
            {diagErr && <p className="mt-2 text-xs text-[var(--st-due)]">{diagErr}</p>}
            {advice && (
              <div className="mt-2 whitespace-pre-wrap rounded-[var(--radius-sm)] border border-border bg-surface px-3 py-2 text-xs leading-relaxed text-ink2">
                {advice}
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
