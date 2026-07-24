// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useMemo, useRef, useState } from "react";
import {
  aiProviderStatus,
  commitReview,
  listProjects,
  proposeMetadata,
  reviewQueue,
} from "../lib/ipc";
import type { Document, Importance, MetadataProposal, ReviewDecision } from "../lib/types";
import { formatDate } from "../lib/format";
import { useDepth } from "../theme";
import { Button, Card, Input } from "./ui";
import { ImportancePicker } from "./ImportancePicker";
import { TagEditor } from "./TagEditor";
import { ChatBadge } from "./ChatBadge";
import { rankImportance } from "../lib/importance";
import { useReader } from "../lib/reader";
import { readReviewAiEnabled, writeReviewAiEnabled } from "../lib/reviewPrefs";

interface Props {
  /** Called after the queue changes so the parent can refresh the sidebar badge. */
  onChanged: () => void;
  /** Open the Settings dialog — the "Turn on AI"/"fix it" affordances point at AI & Models. */
  onOpenSettings: () => void;
}

interface Edit {
  project: string;
  tags: string[];
  importance: Importance;
}

const PROJECTS_LIST_ID = "review-projects";

// Module-level caches (keyed by document id) so leaving the Review tab and returning doesn't re-run
// the AI proposals — those cost tokens, and the queue can be hundreds of items deep while a Drive
// index is running. They survive unmount; `load` restores from them, only proposing for documents
// not yet cached, prunes entries for docs that have left the queue, and "Re-propose" clears them.
const proposalCache = new Map<number, MetadataProposal>();
const editCache = new Map<number, Edit>();

export function ReviewView({ onChanged, onOpenSettings }: Props) {
  const [queue, setQueue] = useState<Document[]>([]);
  const [proposals, setProposals] = useState<Record<number, MetadataProposal>>({});
  const [edits, setEdits] = useState<Record<number, Edit>>({});
  const [projects, setProjects] = useState<string[]>([]);
  const [proposing, setProposing] = useState(false);
  const [committing, setCommitting] = useState(false);
  // Documents being filed one at a time via a row's own Approve button (distinct from the bulk
  // "Approve all") — tracked per id so each such row shows its own progress without disabling
  // the whole queue.
  const [committingIds, setCommittingIds] = useState<Set<number>>(new Set());
  const [showAutofiled, setShowAutofiled] = useState(false);
  // Whether Review asks the model for suggestions (default off — the AI is an enhancement, not a
  // requirement). The banner nudges the user to turn it on; the Settings → AI & Models toggle mirrors it.
  const [aiEnabled, setAiEnabled] = useState(readReviewAiEnabled);
  // When suggestions are ON but couldn't run, the plain-language reason (no model linked, no credits,
  // an unreachable local endpoint …) so the user can fix it — shown as a calm note, never a red error.
  const [aiError, setAiError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Rows the user has hand-edited; a late streaming proposal must not overwrite
  // them. Reset at the start of each proposal run (including Re-propose).
  const dirtyRef = useRef<Set<number>>(new Set());
  // Bumped on each proposal run and on unmount, so a late streaming callback from
  // a superseded run (or after the view is gone) can't write stale proposals.
  const runRef = useRef(0);
  useEffect(() => () => void runRef.current++, []);

  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function load() {
    setError(null);
    try {
      const [q, p] = await Promise.all([reviewQueue(), listProjects()]);
      setQueue(q);
      setProjects(p);
      // Prune cache entries for documents that have left the queue (committed/removed elsewhere).
      const ids = new Set(q.map((d) => d.id));
      for (const id of [...proposalCache.keys()]) if (!ids.has(id)) proposalCache.delete(id);
      for (const id of [...editCache.keys()]) if (!ids.has(id)) editCache.delete(id);
      // Restore any cached proposals/edits; seed the rest from each document's current values.
      const restored: Record<number, MetadataProposal> = {};
      const seededEdits: Record<number, Edit> = {};
      for (const d of q) {
        const cached = proposalCache.get(d.id);
        if (cached) restored[d.id] = cached;
        seededEdits[d.id] = editCache.get(d.id) ?? {
          project: d.project,
          tags: d.tags,
          importance: d.importance,
        };
      }
      setProposals(restored);
      setEdits(seededEdits);
      // Only ask the model for documents we don't already have a proposal for — so peeking at the
      // tab (or a few new items arriving) never re-runs proposals the model already produced.
      const missing = q.filter((d) => !proposalCache.has(d.id)).map((d) => d.id);
      // Only ask the model when suggestions are turned on — otherwise the user files these by hand.
      if (missing.length > 0 && readReviewAiEnabled()) await runProposals(missing);
    } catch (e) {
      setError(String(e));
    }
  }

  // Regenerate from scratch (the explicit "Re-propose" action): clear the cache for the queue so
  // every row is proposed afresh, discarding prior proposals and hand-edits.
  function repropose() {
    for (const d of queue) {
      proposalCache.delete(d.id);
      editCache.delete(d.id);
    }
    setProposals({});
    void runProposals(queue.map((d) => d.id));
  }

  async function runProposals(ids: number[]) {
    if (ids.length === 0) return;
    const myRun = ++runRef.current;
    setProposing(true);
    setError(null);
    setAiError(null);
    dirtyRef.current = new Set();
    try {
      // Suggestions are on, but they need a working model. No provider linked, or a live failure
      // (no credits, an unreachable local endpoint, a rejected key) becomes a calm "here's why — file
      // by hand" note rather than a red error, and never blocks manual filing.
      const status = await aiProviderStatus();
      if (runRef.current !== myRun) return;
      if (!status.has_cloud_key && !status.local_configured) {
        setAiError("no AI model is linked yet");
        return;
      }
      await proposeMetadata((event) => {
        if (runRef.current !== myRun) return; // superseded run or unmounted
        if (event.type !== "proposed") return;
        const { document_id, proposal } = event;
        proposalCache.set(document_id, proposal);
        setProposals((prev) => ({ ...prev, [document_id]: proposal }));
        // Don't clobber a row the user has already hand-edited while proposals stream.
        if (dirtyRef.current.has(document_id)) return;
        const edit = {
          project: proposal.project,
          tags: proposal.tags,
          importance: proposal.importance,
        };
        editCache.set(document_id, edit);
        setEdits((prev) => ({ ...prev, [document_id]: edit }));
      }, ids);
    } catch (e) {
      if (runRef.current === myRun) setAiError(String(e));
    } finally {
      if (runRef.current === myRun) setProposing(false);
    }
  }

  // Turn suggestions on from the banner: remember the choice, then propose for everything still
  // un-suggested. If a model isn't set up yet, runProposals surfaces the reason as a calm note.
  function enableAi() {
    writeReviewAiEnabled(true);
    setAiEnabled(true);
    const missing = queue.filter((d) => !proposalCache.has(d.id)).map((d) => d.id);
    void runProposals(missing);
  }

  function updateEdit(id: number, patch: Partial<Edit>) {
    dirtyRef.current.add(id);
    setEdits((prev) => {
      const next = { ...prev[id], ...patch } as Edit;
      // Persist hand-edits across tab switches too, so returning doesn't lose them.
      editCache.set(id, next);
      return { ...prev, [id]: next };
    });
  }

  function decisionFor(doc: Document): ReviewDecision {
    const edit = edits[doc.id] ?? {
      project: doc.project,
      tags: doc.tags,
      importance: doc.importance,
    };
    const proposal = proposals[doc.id];
    return {
      document_id: doc.id,
      project: edit.project.trim() || "Unsorted",
      tags: edit.tags,
      importance: edit.importance,
      proposed_project: proposal ? proposal.project : doc.project,
      proposed_tags: proposal ? proposal.tags : doc.tags,
      proposed_importance: proposal ? proposal.importance : doc.importance,
    };
  }

  async function approveAll() {
    if (queue.length === 0 || proposing || committingIds.size > 0) return;
    setCommitting(true);
    setError(null);
    try {
      await commitReview(queue.map(decisionFor));
      for (const d of queue) {
        proposalCache.delete(d.id);
        editCache.delete(d.id);
      }
      setQueue([]);
      setProposals({});
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setCommitting(false);
    }
  }

  // File a single document (a row's own Approve button) with the values currently shown, leaving
  // the rest of the queue in place — so a confident item can be cleared without committing all.
  async function commitOne(doc: Document) {
    if (proposing || committing || committingIds.has(doc.id)) return;
    setCommittingIds((s) => new Set(s).add(doc.id));
    setError(null);
    try {
      await commitReview([decisionFor(doc)]);
      proposalCache.delete(doc.id);
      editCache.delete(doc.id);
      setQueue((q) => q.filter((d) => d.id !== doc.id));
      setProposals((prev) => {
        const next = { ...prev };
        delete next[doc.id];
        return next;
      });
      setEdits((prev) => {
        const next = { ...prev };
        delete next[doc.id];
        return next;
      });
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setCommittingIds((s) => {
        const next = new Set(s);
        next.delete(doc.id);
        return next;
      });
    }
  }

  // Low-importance items auto-file into a collapsed section so nothing backs up
  // into a chore (spec §3). They're still committed by "Approve all".
  const { needsReview, autofiled } = useMemo(() => {
    const needsReview: Document[] = [];
    const autofiled: Document[] = [];
    // Bucket + order by the AI's PROPOSED importance, not the user's live edit. The proposal is
    // stable once it has streamed in, so hand-picking an importance on a row never re-sorts the
    // list or yanks the row into the collapsed section under the user — it stays where they left
    // it (their pick is still what gets committed, via `decisionFor`). Ordering still animates as
    // proposals arrive, since this memo keys on `proposals`.
    for (const d of queue) {
      const imp = proposals[d.id]?.importance ?? d.importance;
      (imp === "low" ? autofiled : needsReview).push(d);
    }
    // High → low, so the AI's most-important picks rise to the top. Untriaged sits above archive;
    // title breaks ties for a stable order.
    const eff = (d: Document) => rankImportance(proposals[d.id]?.importance ?? d.importance);
    needsReview.sort((a, b) => eff(b) - eff(a) || a.title.localeCompare(b.title));
    return { needsReview, autofiled };
  }, [queue, proposals]);

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center justify-between border-b border-border px-6 py-3">
        <div>
          <h1 className="font-head text-sm font-semibold text-ink">Review</h1>
          <p className="text-xs text-ink3">
            {queue.length === 0
              ? "Nothing to review"
              : `${queue.length} to review${proposing ? " · proposing…" : ""}`}
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button
            variant="tertiary"
            onClick={repropose}
            disabled={
              proposing || committing || committingIds.size > 0 || queue.length === 0 || !aiEnabled
            }
            data-help="review-repropose"
            title="Re-run the AI proposals"
          >
            Re-propose
          </Button>
          <Button
            variant="primary"
            onClick={approveAll}
            disabled={proposing || committing || committingIds.size > 0 || queue.length === 0}
            data-help="review-approve-all"
          >
            {committing ? "Saving…" : "Approve all"}
          </Button>
        </div>
      </header>

      <div className="flex-1 overflow-y-auto">
        <div className="mx-auto max-w-3xl px-6 py-6">
          {error && (
            <div
              className="mb-4 rounded-[var(--radius)] border px-3 py-2 text-sm text-st-due"
              style={{
                borderColor: "color-mix(in oklab, var(--st-due) 40%, transparent)",
                background: "color-mix(in oklab, var(--st-due) 15%, transparent)",
              }}
            >
              {error}
            </div>
          )}

          {/* Suggestions off (the fresh-install default): nudge to turn them on — a big help when
              importing a lot — while manual filing stays fully available. */}
          {queue.length > 0 && !aiEnabled && (
            <div className="mb-4 flex flex-wrap items-center justify-between gap-3 rounded-[var(--radius)] border border-border bg-panel px-3 py-2.5 text-sm text-ink3">
              <p className="min-w-0 flex-1">
                Turn on AI suggestions to have PM propose a project, tags and importance for each
                item — a real help when you're importing a lot. You can always set them yourself and
                Approve.
              </p>
              <Button variant="secondary" onClick={enableAi} className="shrink-0">
                Turn on AI
              </Button>
            </div>
          )}

          {/* Suggestions on but no model could run: name the reason so the user can debug (no credits,
              no model linked, endpoint down, …) and point them at Settings — filing by hand still works. */}
          {queue.length > 0 && aiEnabled && aiError && (
            <div className="mb-4 flex flex-wrap items-center justify-between gap-3 rounded-[var(--radius)] border border-border bg-panel px-3 py-2.5 text-sm text-ink3">
              <p className="min-w-0 flex-1">
                No AI available right now — continue by hand.{" "}
                <span className="text-ink4">(The reason: {aiError}.)</span>
              </p>
              <Button variant="tertiary" onClick={onOpenSettings} className="shrink-0">
                Open Settings
              </Button>
            </div>
          )}

          {queue.length === 0 ? (
            <p className="text-sm text-ink4">
              Every document is sorted. New items appear here after you ingest them.
            </p>
          ) : (
            <>
              <datalist id={PROJECTS_LIST_ID}>
                {projects.map((p) => (
                  <option key={p} value={p} />
                ))}
              </datalist>

              <ul className="flex flex-col gap-3">
                {needsReview.map((doc) => (
                  <ReviewRow
                    key={doc.id}
                    doc={doc}
                    proposal={proposals[doc.id]}
                    edit={edits[doc.id]}
                    committing={committingIds.has(doc.id)}
                    disabled={
                      committing || committingIds.has(doc.id) || (proposing && !proposals[doc.id])
                    }
                    noSuggestions={!aiEnabled || !!aiError}
                    onChange={(patch) => updateEdit(doc.id, patch)}
                    onApprove={() => void commitOne(doc)}
                  />
                ))}
              </ul>

              {autofiled.length > 0 && (
                <div className="mt-5">
                  <button
                    onClick={() => setShowAutofiled((v) => !v)}
                    data-help="review-autofiled"
                    className="font-mono text-xs uppercase tracking-wide text-ink3 hover:text-ink"
                  >
                    {showAutofiled ? "▾" : "▸"} Auto-filed · low importance ({autofiled.length})
                  </button>
                  {showAutofiled && (
                    <ul className="mt-3 flex flex-col gap-3">
                      {autofiled.map((doc) => (
                        <ReviewRow
                          key={doc.id}
                          doc={doc}
                          proposal={proposals[doc.id]}
                          edit={edits[doc.id]}
                          committing={committingIds.has(doc.id)}
                          disabled={
                            committing ||
                            committingIds.has(doc.id) ||
                            (proposing && !proposals[doc.id])
                          }
                          noSuggestions={!aiEnabled || !!aiError}
                          onChange={(patch) => updateEdit(doc.id, patch)}
                          onApprove={() => void commitOne(doc)}
                        />
                      ))}
                    </ul>
                  )}
                </div>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
}

function ReviewRow({
  doc,
  proposal,
  edit,
  committing,
  disabled,
  noSuggestions,
  onChange,
  onApprove,
}: {
  doc: Document;
  proposal?: MetadataProposal;
  edit?: Edit;
  /** This row is being filed by its own Approve button (drives its "Saving…" label). */
  committing: boolean;
  /** Approve is unavailable — this row's own proposal is still streaming, or a commit is in flight.
   *  A row becomes approvable the moment ITS proposal lands, not when the whole batch finishes. */
  disabled: boolean;
  /** No suggestion is coming (AI off, or it failed) — prompt the user to fill the fields in. */
  noSuggestions: boolean;
  onChange: (patch: Partial<Edit>) => void;
  /** File just this document with the values shown, leaving the rest of the queue. */
  onApprove: () => void;
}) {
  const { showPower } = useDepth();
  // Open the same shared, app-level document reader the Documents tab and project file list use
  // (mounted once via ReaderProvider) — click the title to read the document while triaging it.
  const { openReader, current: readerDoc } = useReader();
  const value = edit ?? { project: doc.project, tags: doc.tags, importance: doc.importance };

  return (
    <li>
      <Card className="p-4" data-help="review-row">
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={() => openReader(doc)}
            className={`-mx-1.5 min-w-0 flex-1 cursor-pointer truncate rounded-[var(--radius-sm)] px-1.5 py-0.5 text-left font-head text-sm font-medium transition-colors hover:bg-surface hover:text-accent-text ${
              readerDoc?.id === doc.id ? "bg-accent-soft text-accent-text" : "text-ink"
            }`}
            title={`Open “${doc.title}”`}
          >
            {doc.title}
          </button>
          {doc.source_type === "chat" && <ChatBadge />}
          <Button
            variant="secondary"
            onClick={onApprove}
            disabled={disabled}
            className="shrink-0 px-2 py-1 text-xs"
            data-help="review-approve-one"
            title="File just this document with the values shown"
          >
            {committing ? "Saving…" : "Approve"}
          </Button>
        </div>
        {proposal?.reasoning ? (
          <p className="mt-1 text-xs text-ink3">{proposal.reasoning}</p>
        ) : noSuggestions ? (
          <p className="mt-1 text-xs text-ink4">
            No AI suggestion — set the project, importance and tags below.
          </p>
        ) : (
          <p className="mt-1 text-xs text-ink4">Awaiting proposal…</p>
        )}
        {showPower && (
          <p className="mt-1 font-mono text-xs text-ink4">ingested {formatDate(doc.ingested_at)}</p>
        )}

        <div className="mt-3 flex flex-wrap items-center gap-x-6 gap-y-3">
          <label className="flex items-center gap-2 text-xs text-ink3" data-help="review-project">
            Project
            <Input
              list={PROJECTS_LIST_ID}
              value={value.project}
              onChange={(e) => onChange({ project: e.target.value })}
              className="w-44"
            />
          </label>

          <div className="flex items-center gap-2 text-xs text-ink3" data-help="review-importance">
            Importance
            <ImportancePicker
              value={value.importance}
              onChange={(importance) => onChange({ importance })}
            />
          </div>
        </div>

        <div className="mt-3" data-help="review-tags">
          <TagEditor tags={value.tags} onChange={(tags) => onChange({ tags })} />
        </div>
      </Card>
    </li>
  );
}
