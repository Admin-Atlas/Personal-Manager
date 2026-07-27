// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Teach tab's tag section (#580): re-tag the whole library from one vocabulary.
//
// Tags were coined a batch at a time, with no view of the rest of the store, so a library ends up
// with `ammun`, `chair-application`, `placement` — each defensible for the one document it sits on
// and useless as a label, because a tag on one document groups nothing. Since #276 that costs
// retrieval, not just tidiness.
//
// It sits in Teach because Teach is already where you correct how PM understands your things —
// merging a project variant so it stops recurring is the same act on a different noun.
//
// Nothing here writes until the user accepts. The pass STAGES proposals; this surface shows old
// tags against new, per document, and `commitRetag` is the only call that touches the vault. That
// separation is deliberate: a tag someone fixed by hand looks identical in the data to one the AI
// coined, and there is no undo.

import { useCallback, useEffect, useState } from "react";
import {
  commitRetag,
  discardTagProposals,
  listTagProposals,
  proposeRetag,
  retagScope,
} from "../lib/ipc";
import type { RetagScope, TagProposalRow } from "../lib/types";
import { Button, Card } from "./ui";

export function TeachTags({ onApplied }: { onApplied?: () => void }) {
  const [scope, setScope] = useState<RetagScope | null>(null);
  const [rows, setRows] = useState<TagProposalRow[]>([]);
  const [vocabulary, setVocabulary] = useState<string[] | null>(null);
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);
  const [running, setRunning] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Documents the user has excluded from the accept. Everything staged is in by default: the pass
  // is asked for wholesale, so opting out is the exception.
  const [excluded, setExcluded] = useState<Set<number>>(new Set());

  const load = useCallback(async () => {
    try {
      const [s, r] = await Promise.all([retagScope(), listTagProposals()]);
      setScope(s);
      setRows(r);
      setExcluded(new Set());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  async function run() {
    setError(null);
    setRunning(true);
    setVocabulary(null);
    setProgress(null);
    setRows([]);
    try {
      await proposeRetag((ev) => {
        if (ev.type === "vocabulary") setVocabulary(ev.tags);
        else if (ev.type === "progress") setProgress({ done: ev.done, total: ev.total });
      });
      await load();
    } catch (e) {
      setError(String(e));
    } finally {
      setRunning(false);
      setProgress(null);
    }
  }

  async function accept() {
    const ids = rows.map((r) => r.document_id).filter((id) => !excluded.has(id));
    if (ids.length === 0) return;
    setBusy(true);
    setError(null);
    try {
      await commitRetag(ids);
      await load();
      onApplied?.();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function discard() {
    setBusy(true);
    try {
      await discardTagProposals();
      setVocabulary(null);
      await load();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  function toggle(id: number) {
    setExcluded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  const accepting = rows.length - excluded.size;

  return (
    <section className="mt-8" data-help="teach-tags">
      <p className="pb-2 font-mono text-xs uppercase tracking-wide text-ink4">Tags</p>

      <Card className="p-4">
        <p className="text-sm leading-relaxed text-ink3">
          Tags are proposed a few documents at a time, so they drift into one-off labels that only
          ever land on a single file — which groups nothing. This re-reads your whole library, picks
          one set of tags that fits <em>it</em>, and re-labels everything from that set. Your
          projects and importance are not touched.
        </p>

        {/* The cost, before anything is spent. This is a paid pass over the whole library, not a
            local reshuffle, and it should never start without the user knowing that. */}
        {scope != null && scope.documents > 0 && (
          <p className="mt-2 font-mono text-xs text-ink4">
            {scope.documents} document{scope.documents === 1 ? "" : "s"} · about {scope.calls} model
            call{scope.calls === 1 ? "" : "s"}
          </p>
        )}

        {error && (
          <p className="mt-3 text-sm text-st-due" role="alert">
            {error}
          </p>
        )}

        <div className="mt-3 flex flex-wrap items-center gap-2">
          <Button variant="secondary" onClick={() => void run()} disabled={running || busy}>
            {running ? "Re-tagging…" : rows.length > 0 ? "Run again" : "Re-tag my library"}
          </Button>
          {progress && (
            <span className="font-mono text-xs text-ink4">
              {progress.done} / {progress.total}
            </span>
          )}
        </div>

        {vocabulary && vocabulary.length > 0 && (
          <div className="mt-4">
            <p className="pb-1.5 text-xs text-ink4">
              The tags PM chose for your library ({vocabulary.length}):
            </p>
            <div className="flex flex-wrap gap-1.5">
              {vocabulary.map((t) => (
                <span
                  key={t}
                  className="rounded-[var(--radius-sm)] border border-border2 bg-surface px-2 py-0.5 text-xs text-ink3"
                >
                  {t}
                </span>
              ))}
            </div>
          </div>
        )}
      </Card>

      {rows.length > 0 && (
        <div className="mt-4">
          <div className="flex flex-wrap items-center justify-between gap-2 pb-2">
            <p className="font-mono text-xs uppercase tracking-wide text-ink4">
              {rows.length} document{rows.length === 1 ? "" : "s"} would change
            </p>
            <div className="flex gap-2">
              <Button variant="tertiary" onClick={() => void discard()} disabled={busy}>
                Discard
              </Button>
              <Button onClick={() => void accept()} disabled={busy || accepting === 0}>
                {busy ? "Applying…" : `Apply ${accepting}`}
              </Button>
            </div>
          </div>

          <ul className="flex flex-col gap-2">
            {rows.map((r) => {
              const off = excluded.has(r.document_id);
              return (
                <li key={r.document_id}>
                  <Card className={`px-4 py-2.5 ${off ? "opacity-50" : ""}`}>
                    <label className="flex cursor-pointer items-start gap-3">
                      <input
                        type="checkbox"
                        checked={!off}
                        onChange={() => toggle(r.document_id)}
                        className="mt-1 shrink-0"
                        aria-label={`Apply the new tags for ${r.title}`}
                      />
                      <span className="min-w-0 flex-1">
                        <span className="block truncate text-sm text-ink2" title={r.title}>
                          {r.title}
                        </span>
                        <span className="mt-1 flex flex-wrap items-center gap-1.5">
                          <TagList tags={r.current_tags} muted />
                          <span className="text-ink4">→</span>
                          <TagList tags={r.proposed_tags} />
                        </span>
                      </span>
                    </label>
                  </Card>
                </li>
              );
            })}
          </ul>
        </div>
      )}
    </section>
  );
}

/** One side of the before/after. An empty side says so in words — a blank gap would read as a
 *  rendering failure rather than as "this document ends up with no tags", which is a real and
 *  often correct outcome for a one-off label the new vocabulary has nothing for. */
function TagList({ tags, muted }: { tags: string[]; muted?: boolean }) {
  if (tags.length === 0) {
    return <span className="text-xs italic text-ink4">no tags</span>;
  }
  return (
    <>
      {tags.map((t) => (
        <span
          key={t}
          className={`rounded-[var(--radius-sm)] px-1.5 py-0.5 text-xs ${
            muted ? "text-ink4 line-through" : "bg-accent-soft text-accent-text"
          }`}
        >
          {t}
        </span>
      ))}
    </>
  );
}
