// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useMemo, useRef, useState } from "react";
import { commitReview, listProjects, proposeMetadata, reviewQueue } from "../lib/ipc";
import type { Document, Importance, MetadataProposal, ReviewDecision } from "../lib/types";
import { formatDate } from "../lib/format";
import { useDepth } from "../theme";
import { Button, Card, Input, SegmentedControl, type SegOption } from "./ui";

interface Props {
  /** Called after the queue changes so the parent can refresh the sidebar badge. */
  onChanged: () => void;
}

interface Edit {
  project: string;
  tags: string[];
  importance: Importance;
}

const PROJECTS_LIST_ID = "review-projects";
const IMPORTANCE_LEVELS: Importance[] = ["high", "medium", "low", null];

// SegmentedControl is keyed by string; encode the nullable Importance as a stable key.
const IMPORTANCE_KEY = (imp: Importance): string => imp ?? "none";
const IMPORTANCE_FROM_KEY = (key: string): Importance =>
  key === "none" ? null : (key as Importance);
const IMPORTANCE_OPTIONS: ReadonlyArray<SegOption<string>> = IMPORTANCE_LEVELS.map((level) => ({
  value: IMPORTANCE_KEY(level),
  label: level ?? "none",
}));

// Module-level caches (keyed by document id) so leaving the Review tab and returning doesn't re-run
// the AI proposals — those cost tokens, and the queue can be hundreds of items deep while a Drive
// index is running. They survive unmount; `load` restores from them, only proposing for documents
// not yet cached, prunes entries for docs that have left the queue, and "Re-propose" clears them.
const proposalCache = new Map<number, MetadataProposal>();
const editCache = new Map<number, Edit>();

export function ReviewView({ onChanged }: Props) {
  const [queue, setQueue] = useState<Document[]>([]);
  const [proposals, setProposals] = useState<Record<number, MetadataProposal>>({});
  const [edits, setEdits] = useState<Record<number, Edit>>({});
  const [projects, setProjects] = useState<string[]>([]);
  const [proposing, setProposing] = useState(false);
  const [committing, setCommitting] = useState(false);
  const [showAutofiled, setShowAutofiled] = useState(false);
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
      if (missing.length > 0) await runProposals(missing);
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
    dirtyRef.current = new Set();
    try {
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
      if (runRef.current === myRun) setError(String(e));
    } finally {
      if (runRef.current === myRun) setProposing(false);
    }
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
    if (queue.length === 0 || proposing) return;
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

  // Low-importance items auto-file into a collapsed section so nothing backs up
  // into a chore (spec §3). They're still committed by "Approve all".
  const { needsReview, autofiled } = useMemo(() => {
    const needsReview: Document[] = [];
    const autofiled: Document[] = [];
    for (const d of queue) {
      const imp = edits[d.id]?.importance ?? d.importance;
      (imp === "low" ? autofiled : needsReview).push(d);
    }
    return { needsReview, autofiled };
  }, [queue, edits]);

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
            disabled={proposing || committing || queue.length === 0}
            data-help="review-repropose"
            title="Re-run the AI proposals"
          >
            Re-propose
          </Button>
          <Button
            variant="primary"
            onClick={approveAll}
            disabled={proposing || committing || queue.length === 0}
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
                    onChange={(patch) => updateEdit(doc.id, patch)}
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
                          onChange={(patch) => updateEdit(doc.id, patch)}
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
  onChange,
}: {
  doc: Document;
  proposal?: MetadataProposal;
  edit?: Edit;
  onChange: (patch: Partial<Edit>) => void;
}) {
  const { showPower } = useDepth();
  const value = edit ?? { project: doc.project, tags: doc.tags, importance: doc.importance };

  return (
    <li>
      <Card className="p-4" data-help="review-row">
        <div className="truncate font-head text-sm font-medium text-ink" title={doc.title}>
          {doc.title}
        </div>
        {proposal?.reasoning ? (
          <p className="mt-1 text-xs text-ink3">{proposal.reasoning}</p>
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

function ImportancePicker({
  value,
  onChange,
}: {
  value: Importance;
  onChange: (value: Importance) => void;
}) {
  return (
    <SegmentedControl
      options={IMPORTANCE_OPTIONS}
      value={IMPORTANCE_KEY(value)}
      onChange={(key) => onChange(IMPORTANCE_FROM_KEY(key))}
      className="capitalize"
    />
  );
}

function TagEditor({ tags, onChange }: { tags: string[]; onChange: (tags: string[]) => void }) {
  const [draft, setDraft] = useState("");

  function add() {
    // Commas aren't allowed in tags (the vault serializes them comma-separated).
    const tag = draft.replace(/,/g, "").trim().toLowerCase();
    setDraft("");
    if (tag && !tags.includes(tag)) onChange([...tags, tag]);
  }

  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {tags.map((tag) => (
        <span
          key={tag}
          className="inline-flex items-center gap-1 rounded-[var(--radius-sm)] bg-accent-soft px-2 py-0.5 text-xs text-accent-text"
        >
          {tag}
          <button
            onClick={() => onChange(tags.filter((t) => t !== tag))}
            className="text-ink4 hover:text-ink"
            title="Remove tag"
            aria-label={`Remove tag ${tag}`}
          >
            ×
          </button>
        </span>
      ))}
      <input
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === ",") {
            e.preventDefault();
            add();
          }
        }}
        onBlur={add}
        placeholder="add tag…"
        className="w-24 bg-transparent px-1 py-0.5 text-xs text-ink2 outline-none placeholder:text-ink4"
      />
    </div>
  );
}
