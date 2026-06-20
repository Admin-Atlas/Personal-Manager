// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useMemo, useRef, useState } from "react";
import { commitReview, listProjects, proposeMetadata, reviewQueue } from "../lib/ipc";
import type { Document, Importance, MetadataProposal, ReviewDecision } from "../lib/types";

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
      // Seed edits from each document's current values until a proposal lands.
      setEdits(
        Object.fromEntries(
          q.map((d) => [d.id, { project: d.project, tags: d.tags, importance: d.importance }]),
        ),
      );
      setProposals({});
      if (q.length > 0) await runProposals();
    } catch (e) {
      setError(String(e));
    }
  }

  async function runProposals() {
    const myRun = ++runRef.current;
    setProposing(true);
    setError(null);
    dirtyRef.current = new Set();
    try {
      await proposeMetadata((event) => {
        if (runRef.current !== myRun) return; // superseded run or unmounted
        if (event.type !== "proposed") return;
        const { document_id, proposal } = event;
        setProposals((prev) => ({ ...prev, [document_id]: proposal }));
        // Don't clobber a row the user has already hand-edited while proposals stream.
        if (dirtyRef.current.has(document_id)) return;
        setEdits((prev) => ({
          ...prev,
          [document_id]: {
            project: proposal.project,
            tags: proposal.tags,
            importance: proposal.importance,
          },
        }));
      });
    } catch (e) {
      if (runRef.current === myRun) setError(String(e));
    } finally {
      if (runRef.current === myRun) setProposing(false);
    }
  }

  function updateEdit(id: number, patch: Partial<Edit>) {
    dirtyRef.current.add(id);
    setEdits((prev) => ({ ...prev, [id]: { ...prev[id], ...patch } }));
  }

  function decisionFor(doc: Document): ReviewDecision {
    const edit = edits[doc.id] ?? { project: doc.project, tags: doc.tags, importance: doc.importance };
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
      <header className="flex items-center justify-between border-b border-neutral-800 px-6 py-3">
        <div>
          <h1 className="text-sm font-semibold text-neutral-100">Review</h1>
          <p className="text-xs text-neutral-500">
            {queue.length === 0
              ? "Nothing to review"
              : `${queue.length} to review${proposing ? " · proposing…" : ""}`}
          </p>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={runProposals}
            disabled={proposing || committing || queue.length === 0}
            data-help="review-repropose"
            title="Re-run the AI proposals"
            className="rounded-lg px-3 py-1.5 text-sm text-neutral-400 hover:bg-neutral-800 hover:text-neutral-200 disabled:opacity-40"
          >
            Re-propose
          </button>
          <button
            onClick={approveAll}
            disabled={proposing || committing || queue.length === 0}
            data-help="review-approve-all"
            className="rounded-lg bg-neutral-100 px-3 py-1.5 text-sm font-medium text-neutral-900 hover:bg-white disabled:cursor-not-allowed disabled:opacity-40"
          >
            {committing ? "Saving…" : "Approve all"}
          </button>
        </div>
      </header>

      <div className="flex-1 overflow-y-auto">
        <div className="mx-auto max-w-3xl px-6 py-6">
          {error && (
            <div className="mb-4 rounded-lg border border-red-900 bg-red-950/50 px-3 py-2 text-sm text-red-300">
              {error}
            </div>
          )}

          {queue.length === 0 ? (
            <p className="text-sm text-neutral-600">
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
                    className="text-xs uppercase tracking-wide text-neutral-500 hover:text-neutral-300"
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
  const value = edit ?? { project: doc.project, tags: doc.tags, importance: doc.importance };

  return (
    <li className="rounded-xl border border-neutral-800 bg-neutral-900/50 p-4" data-help="review-row">
      <div className="truncate text-sm font-medium text-neutral-100" title={doc.title}>
        {doc.title}
      </div>
      {proposal?.reasoning ? (
        <p className="mt-1 text-xs text-neutral-500">{proposal.reasoning}</p>
      ) : (
        <p className="mt-1 text-xs text-neutral-600">Awaiting proposal…</p>
      )}

      <div className="mt-3 flex flex-wrap items-center gap-x-6 gap-y-3">
        <label className="flex items-center gap-2 text-xs text-neutral-400">
          Project
          <input
            list={PROJECTS_LIST_ID}
            value={value.project}
            onChange={(e) => onChange({ project: e.target.value })}
            className="w-44 rounded-md border border-neutral-700 bg-neutral-950 px-2 py-1 text-sm text-neutral-100 outline-none focus:border-neutral-500"
          />
        </label>

        <div className="flex items-center gap-2 text-xs text-neutral-400">
          Importance
          <ImportancePicker value={value.importance} onChange={(importance) => onChange({ importance })} />
        </div>
      </div>

      <div className="mt-3">
        <TagEditor tags={value.tags} onChange={(tags) => onChange({ tags })} />
      </div>
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
    <div className="inline-flex overflow-hidden rounded-md border border-neutral-700">
      {IMPORTANCE_LEVELS.map((level) => {
        const active = value === level;
        return (
          <button
            key={level ?? "none"}
            onClick={() => onChange(level)}
            className={`px-2 py-1 text-xs capitalize ${
              active ? "bg-neutral-200 text-neutral-900" : "text-neutral-400 hover:bg-neutral-800"
            }`}
          >
            {level ?? "none"}
          </button>
        );
      })}
    </div>
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
          className="inline-flex items-center gap-1 rounded-full bg-neutral-800 px-2 py-0.5 text-xs text-neutral-300"
        >
          {tag}
          <button
            onClick={() => onChange(tags.filter((t) => t !== tag))}
            className="text-neutral-500 hover:text-neutral-200"
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
        className="w-24 bg-transparent px-1 py-0.5 text-xs text-neutral-200 outline-none placeholder:text-neutral-600"
      />
    </div>
  );
}
