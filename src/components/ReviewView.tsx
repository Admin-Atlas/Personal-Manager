// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import {
  aiProviderStatus,
  cachedProposals,
  commitReview,
  listProjects,
  proposeMetadata,
  reviewQueue,
} from "../lib/ipc";
import type { Document, Importance, MetadataProposal, ReviewDecision } from "../lib/types";
import { formatDate } from "../lib/format";
import { useDepth } from "../theme";
import { Button, Callout, Card, Input } from "./ui";
import { ImportancePicker } from "./ImportancePicker";
import { TagEditor } from "./TagEditor";
import { ChatBadge } from "./ChatBadge";
import { rankImportance } from "../lib/importance";
import { useReader } from "../lib/reader";
import { readReviewAiEnabled, writeReviewAiEnabled } from "../lib/reviewPrefs";
import {
  currentProposalRun,
  proposalCache,
  proposalsPending,
  pruneProposalCache,
  publishProposal,
  seedReviewEdit,
  subscribeToProposalRun,
  subscribeToProposals,
  withProposalRun,
} from "../lib/reviewProposals";
import { landingSeq, landingsSince, mergeLandings, onDocumentsLanded } from "../lib/documentFeed";

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

// The proposal cache and the single-run guard now live in `lib/reviewProposals`, shared with the
// background run that fires after a connector sync (#513) — otherwise the two could propose for the
// same documents at once and bill twice. Hand-edits stay local: they're UI state, and nothing
// outside this view produces them.
const editCache = new Map<number, Edit>();

/** A stable, connector-unique key for a document's parent folder — `source_type` disambiguates a leaf
 *  folder id that two connectors might share. `null` when the document has no folder (a vault / chat /
 *  photo doc), so it never groups with anything. */
function folderKeyOf(d: Document): string | null {
  return d.source_parent_folder_id ? `${d.source_type}:${d.source_parent_folder_id}` : null;
}

export function ReviewView({ onChanged, onOpenSettings }: Props) {
  const [queue, setQueue] = useState<Document[]>([]);
  const [proposals, setProposals] = useState<Record<number, MetadataProposal>>({});
  const [edits, setEdits] = useState<Record<number, Edit>>({});
  const [projects, setProjects] = useState<string[]>([]);
  const [proposing, setProposing] = useState(false);
  // Whether a proposal run this view didn't start is outstanding — an arrival batch, or the sweep
  // after a connector sync. Seeded from the shared module rather than `false`, so opening Review
  // mid-sync starts out knowing.
  const [bgProposing, setBgProposing] = useState(proposalsPending);
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
  // The row whose "apply to the rest of this folder" panel is open, plus which sibling ids are ticked
  // (all, by default). One panel open at a time. This is B — a deterministic bulk file, no AI involved.
  const [folderApply, setFolderApply] = useState<{ docId: number; checked: Set<number> } | null>(
    null,
  );
  // Rows the user has hand-edited; a late streaming proposal must not overwrite
  // them. Reset at the start of each proposal run (including Re-propose).
  const dirtyRef = useRef<Set<number>>(new Set());
  // Bumped on each proposal run and on unmount, so a late streaming callback from
  // a superseded run (or after the view is gone) can't write stale proposals.
  const runRef = useRef(0);
  useEffect(() => () => void runRef.current++, []);

  // A background run never touches `proposing` — it isn't this view's run — so Approve has to be
  // gated on suggestions being outstanding whoever asked for them. Filing a row before its
  // suggestion lands commits the document's blank pre-review values and drops it out of the queue
  // for good, so the shared module's in-flight state is mirrored here.
  useEffect(() => {
    // Re-read on (re)subscribe: StrictMode tears an effect's subscription down and straight back
    // up, and a transition landing in that gap would otherwise be missed — which reads as a
    // randomly stuck button rather than as a bug.
    setBgProposing(proposalsPending());
    return subscribeToProposalRun(setBgProposing);
  }, []);

  /** Suggestion work is outstanding somewhere — this view's run, an arrival batch, or the sweep. */
  const busy = proposing || bgProposing;
  /** This row's own suggestion is still coming. It becomes approvable the moment ITS proposal
   *  lands, not when the whole batch finishes. */
  const awaiting = (id: number) => busy && !proposals[id];
  // What "Approve all" would file right now: rows still awaiting a suggestion are held back, and the
  // rest file normally — so a 200-file sync never leaves the button dead for minutes. Inlines
  // `awaiting` rather than calling it: that function is rebuilt every render, so naming it as a
  // dependency would defeat the memo.
  const approvable = useMemo(
    () => queue.filter((d) => !(busy && !proposals[d.id])),
    [queue, proposals, busy],
  );
  const heldBack = queue.length - approvable.length;

  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Paint proposals produced by a background run (after a connector sync, #513) as they land, so an
  // open Review tab fills in live rather than only on the next reload. A row the user has already
  // hand-edited keeps their edit — the same rule the view's own streaming callback follows.
  useEffect(() => {
    return subscribeToProposals((documentId, proposal) => {
      setProposals((prev) => ({ ...prev, [documentId]: proposal }));
      if (dirtyRef.current.has(documentId)) return;
      // Copy the tag array rather than aliasing the proposal's, matching `seedReviewEdit`: this path
      // used to alias it, and `decisionFor` compares the edit against the proposal to decide what to
      // log as a correction — with both sides the same array, an in-place edit would move both and
      // the correction would silently never be recorded.
      const edit = {
        project: proposal.project,
        tags: [...proposal.tags],
        importance: proposal.importance,
      };
      editCache.set(documentId, edit);
      setEdits((prev) => ({ ...prev, [documentId]: edit }));
    });
  }, []);

  // New arrivals join the queue live, so a sync fills Review in front of the user instead of
  // presenting a finished list only once the whole run ends. Every landed document is by definition
  // unreviewed (the backend filters), so it belongs in the queue.
  //
  // Seeding `edits` here is what makes "every row in `queue` has an entry in `edits`" a real
  // invariant rather than a coincidence of load ordering — and it is a crash fix, not tidiness.
  // `updateEdit` spreads `prev[id]`, so one keystroke in an unseeded row's Project field produces a
  // partial `{project}` with no tags; `ReviewRow`'s `value = edit ?? {…}` then stops falling back
  // and `<TagEditor tags={undefined}>` throws on `tags.map`. With suggestions off (the default) an
  // arrival is never seeded by the proposal subscription either, so this needs no race at all.
  useEffect(() => {
    return onDocumentsLanded((landed) => {
      setQueue((prev) => mergeLandings(prev, landed));
      setEdits((prev) => {
        const next = { ...prev };
        for (const d of landed) {
          // A re-emit of the same document must not undo a hand-edit or a streamed seed.
          if (next[d.id]) continue;
          next[d.id] = seedReviewEdit(editCache.get(d.id), proposalCache.get(d.id), d);
        }
        return next;
      });
    });
  }, []);

  async function load() {
    setError(null);
    try {
      // See DocumentsView: capture the sequence before the await so a document landing during it
      // isn't lost to the wholesale `setQueue` below.
      const since = landingSeq();
      const [queried, p, cached] = await Promise.all([
        reviewQueue(),
        listProjects(),
        cachedProposals(),
      ]);
      const merged = mergeLandings(queried, landingsSince(since));
      setQueue(merged);
      setProjects(p);
      // EVERYTHING below is keyed on the merged queue, never on the query result: a document that
      // landed during the await is on screen, so pruning, seeding and the propose set must all count
      // it. Half of this function used to read the pre-merge list, which meant such a document was
      // rendered with no seeded edit (one keystroke away from throwing in TagEditor), had the
      // proposal this very function had just cached painted away, and was left out of the `missing`
      // backstop — so it sat on "Awaiting proposal…" until something reloaded the tab.
      const ids = new Set(merged.map((d) => d.id));
      pruneProposalCache(ids);
      for (const id of [...editCache.keys()]) if (!ids.has(id)) editCache.delete(id);
      // Hydrate the in-memory cache from the persisted proposals so a restart repaints what the model
      // already produced. Only genuinely un-proposed docs then fall into the `missing` pass below, so
      // re-opening the app never re-bills for a proposal it already has. A proposal already in memory
      // (freshly streamed) wins over the DB copy.
      for (const { document_id, proposal } of cached) {
        if (ids.has(document_id) && !proposalCache.has(document_id)) {
          proposalCache.set(document_id, proposal);
        }
      }
      // Restore any cached proposals/edits; seed the rest from each document's current values.
      const restored: Record<number, MetadataProposal> = {};
      const seededEdits: Record<number, Edit> = {};
      for (const d of merged) {
        const hit = proposalCache.get(d.id);
        if (hit) restored[d.id] = hit;
        seededEdits[d.id] = seedReviewEdit(editCache.get(d.id), hit, d);
      }
      setProposals(restored);
      setEdits(seededEdits);
      // Only ask the model for documents we don't already have a proposal for — so peeking at the
      // tab (or a few new items arriving) never re-runs proposals the model already produced.
      const missing = merged.filter((d) => !proposalCache.has(d.id)).map((d) => d.id);
      // Only ask the model when suggestions are turned on — otherwise the user files these by hand.
      if (missing.length > 0 && readReviewAiEnabled()) await runProposals(missing);
    } catch (e) {
      setError(String(e));
    }
  }

  // Regenerate from scratch (the explicit "Re-propose" action): clear the cache for the queue so
  // every row is proposed afresh, discarding prior proposals and hand-edits. Any run already in
  // flight (the tab's own, or a background one after a sync) is allowed to settle first — otherwise
  // it would keep publishing into the cache we are about to clear.
  async function repropose() {
    await currentProposalRun();
    for (const d of queue) {
      proposalCache.delete(d.id);
      editCache.delete(d.id);
    }
    setProposals({});
    // Reseed the visible values from each document's own metadata, exactly as a fresh `load` would.
    // Clearing the caches but leaving `edits` alone left every row still showing the DISCARDED run's
    // values — and `decisionFor` compares the edit against the (now absent) proposal, so approving
    // such a row logged those stale values as a correction the user never made.
    setEdits(Object.fromEntries(queue.map((d) => [d.id, seedReviewEdit(undefined, undefined, d)])));
    await runProposals(queue.map((d) => d.id));
  }

  async function runProposals(ids: number[]) {
    if (ids.length === 0) return;
    const myRun = ++runRef.current;
    setProposing(true);
    setError(null);
    setAiError(null);
    dirtyRef.current = new Set();
    try {
      // Joins a background run already covering these documents rather than starting a second one
      // (#513). Either way results arrive through the shared subscription above, so the tab fills
      // in live and nothing is billed twice.
      await withProposalRun(async () => {
        // Suggestions are on, but they need a working model. No provider linked, or a live failure
        // (no credits, an unreachable local endpoint, a rejected key) becomes a calm "here's why —
        // file by hand" note rather than a red error, and never blocks manual filing.
        const status = await aiProviderStatus();
        if (runRef.current !== myRun) return;
        if (!status.has_cloud_key && !status.local_configured) {
          setAiError("no AI model is linked yet");
          return;
        }
        await proposeMetadata((event) => {
          if (event.type !== "proposed") return;
          // Publishing updates the shared cache and notifies the subscription, which owns the
          // state updates — so background and foreground runs paint through one path.
          //
          // Deliberately NOT gated on `runRef`: once the stream is running the model is being paid
          // for, so every proposal it produces must reach the shared cache. Gating this dropped
          // them on the floor whenever a second caller bumped the counter mid-stream — including
          // the common one, where leaving and re-entering the tab makes `load` call `runProposals`
          // for documents this very run is still streaming, and `withProposalRun` JOINS this run
          // rather than starting another. The publish path is safe unguarded: the subscription is
          // torn down on unmount (so no state update lands on a dead view), it already protects
          // hand-edits via `dirtyRef`, and `repropose` awaits the in-flight run before clearing the
          // cache — so there is no stale-write window left for the guard to close. It is exactly
          // what the background run does (`runProposalsAfterSync`).
          publishProposal(event.document_id, event.proposal);
        }, ids);
      });
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
      // The `proposed_*` mirrors above stay as they are — the backend's alias capture reads that
      // baseline. This says whether they mean anything: with no proposal they are just the
      // document's own values, so there is no difference for the backend to log as a correction.
      had_proposal: !!proposal,
    };
  }

  // File everything that is ready. Rows still awaiting a suggestion are left behind rather than
  // filed blind, so the teardown has to be selective: a wholesale `setQueue([])` would take those
  // rows off the screen while they stayed `reviewed = 0` in the store — invisible and unfilable.
  async function approveAll() {
    if (approvable.length === 0 || committing || committingIds.size > 0) return;
    setCommitting(true);
    setError(null);
    try {
      await commitReview(approvable.map(decisionFor));
      const done = new Set(approvable.map((d) => d.id));
      for (const id of done) {
        proposalCache.delete(id);
        editCache.delete(id);
      }
      setQueue((q) => q.filter((d) => !done.has(d.id)));
      setProposals((prev) => {
        const next = { ...prev };
        for (const id of done) delete next[id];
        return next;
      });
      setEdits((prev) => {
        const next = { ...prev };
        for (const id of done) delete next[id];
        return next;
      });
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
    // Must mirror this button's own `disabled` exactly (see the Approve button below). It used to
    // bail on `proposing` alone while the button stayed enabled for any row that already had its
    // proposal — so mid-run the button looked live and silently did nothing. A row whose proposal
    // has arrived is complete and can be filed; only one still waiting on the model is blocked.
    if (committing || committingIds.has(doc.id) || awaiting(doc.id)) return;
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

  // --- B: apply a filing to the rest of a folder ------------------------------------------------
  // Group the live queue by parent folder so a row can offer to file the OTHER unsorted files from the
  // same folder the same way. Recomputed from the live queue, so filing items keeps the counts honest.
  const folderGroups = useMemo(() => {
    const map = new Map<string, number[]>();
    for (const d of queue) {
      const key = folderKeyOf(d);
      if (!key) continue;
      const list = map.get(key);
      if (list) list.push(d.id);
      else map.set(key, [d.id]);
    }
    return map;
  }, [queue]);

  /** The other in-queue documents sharing `doc`'s folder (excluding itself); empty when it has none.
   *  Siblings still awaiting a suggestion are excluded, so the panel's count is what it will
   *  actually file rather than an offer to file rows it must not touch. */
  function folderSiblings(doc: Document): Document[] {
    const key = folderKeyOf(doc);
    if (!key) return [];
    const ids = new Set(folderGroups.get(key) ?? []);
    return queue.filter((d) => d.id !== doc.id && ids.has(d.id) && !awaiting(d.id));
  }

  /** The project a row would apply to its folder — trimmed, and only when it's a real one (the button
   *  stays hidden until a project is chosen, so "Unsorted" is never bulk-applied). */
  function folderApplyProject(doc: Document): string | null {
    const p = (edits[doc.id]?.project ?? doc.project).trim();
    return p && p.toLowerCase() !== "unsorted" ? p : null;
  }

  function openFolderApply(doc: Document) {
    setFolderApply({ docId: doc.id, checked: new Set(folderSiblings(doc).map((d) => d.id)) });
  }
  function toggleFolderSibling(id: number) {
    setFolderApply((cur) => {
      if (!cur) return cur;
      const checked = new Set(cur.checked);
      if (checked.has(id)) checked.delete(id);
      else checked.add(id);
      return { ...cur, checked };
    });
  }

  // File `doc` plus the ticked folder-siblings, all into `doc`'s project — each keeps its own tags and
  // importance. Confirm-gated by the panel, reversible (it's just filing), and no model is called.
  async function applyFolder(doc: Document, siblingIds: number[]) {
    const project = folderApplyProject(doc);
    if (!project) return;
    const ids = [doc.id, ...siblingIds];
    if (ids.some(awaiting) || committing || ids.some((id) => committingIds.has(id))) return;
    setCommittingIds((s) => {
      const next = new Set(s);
      ids.forEach((id) => next.add(id));
      return next;
    });
    setError(null);
    try {
      const byId = new Map(queue.map((d) => [d.id, d]));
      const decisions = ids
        .map((id) => byId.get(id))
        .filter((d): d is Document => !!d)
        .map((d) => ({ ...decisionFor(d), project }));
      await commitReview(decisions);
      const idSet = new Set(ids);
      for (const id of ids) {
        proposalCache.delete(id);
        editCache.delete(id);
      }
      setQueue((q) => q.filter((d) => !idSet.has(d.id)));
      setProposals((prev) => {
        const next = { ...prev };
        ids.forEach((id) => delete next[id]);
        return next;
      });
      setEdits((prev) => {
        const next = { ...prev };
        ids.forEach((id) => delete next[id]);
        return next;
      });
      setFolderApply(null);
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setCommittingIds((s) => {
        const next = new Set(s);
        ids.forEach((id) => next.delete(id));
        return next;
      });
    }
  }

  // One row (+ its folder-apply panel when open), shared by the main list and the auto-filed one.
  function renderRow(doc: Document) {
    const siblings = folderSiblings(doc);
    const canApply = folderApplyProject(doc) !== null;
    const panel = folderApply && folderApply.docId === doc.id ? folderApply : null;
    return (
      <Fragment key={doc.id}>
        <ReviewRow
          doc={doc}
          proposal={proposals[doc.id]}
          edit={edits[doc.id]}
          committing={committingIds.has(doc.id)}
          awaiting={awaiting(doc.id)}
          disabled={committing || committingIds.has(doc.id) || awaiting(doc.id)}
          noSuggestions={!aiEnabled || !!aiError}
          folderApplyCount={canApply && !panel ? siblings.length : 0}
          folderName={doc.source_parent_folder_name}
          onChange={(patch) => updateEdit(doc.id, patch)}
          onApprove={() => void commitOne(doc)}
          onApplyFolder={() => openFolderApply(doc)}
        />
        {panel && (
          <FolderApplyPanel
            folderName={doc.source_parent_folder_name}
            project={folderApplyProject(doc) ?? "Unsorted"}
            siblings={siblings}
            checked={panel.checked}
            busy={committingIds.has(doc.id)}
            onToggle={toggleFolderSibling}
            onApply={() => void applyFolder(doc, [...panel.checked])}
            onCancel={() => setFolderApply(null)}
          />
        )}
      </Fragment>
    );
  }

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center justify-between border-b border-border px-6 py-3">
        <div>
          <h1 className="font-head text-sm font-semibold text-ink">Review</h1>
          <p className="text-xs text-ink3">
            {queue.length === 0
              ? "Nothing to review"
              : `${queue.length} to review${busy ? " · proposing…" : ""}`}
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button
            variant="tertiary"
            onClick={() => void repropose()}
            disabled={
              busy || committing || committingIds.size > 0 || queue.length === 0 || !aiEnabled
            }
            data-help="review-repropose"
            title="Re-run the AI proposals"
          >
            Re-propose
          </Button>
          {/* Stays clickable during a background run and names its scope instead. A blanket-disabled
              Approve-all would sit dead for the length of a 200-file sync with nothing on screen
              saying why; held-back rows are the ones whose suggestion is still coming. */}
          <Button
            variant="primary"
            onClick={approveAll}
            disabled={approvable.length === 0 || committing || committingIds.size > 0}
            title={
              heldBack === 0
                ? undefined
                : approvable.length === 0
                  ? "Waiting for AI suggestions…"
                  : `${heldBack} still waiting on a suggestion`
            }
            data-help="review-approve-all"
          >
            {committing
              ? "Saving…"
              : heldBack > 0
                ? `Approve ${approvable.length} ready`
                : "Approve all"}
          </Button>
        </div>
      </header>

      <div className="flex-1 overflow-y-auto">
        <div className="mx-auto max-w-3xl px-6 py-6">
          {error && (
            <Callout size="md" className="mb-4">
              {error}
            </Callout>
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

              <ul className="flex flex-col gap-3">{needsReview.map(renderRow)}</ul>

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
                    <ul className="mt-3 flex flex-col gap-3">{autofiled.map(renderRow)}</ul>
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
  awaiting,
  disabled,
  noSuggestions,
  folderApplyCount,
  folderName,
  onChange,
  onApprove,
  onApplyFolder,
}: {
  doc: Document;
  proposal?: MetadataProposal;
  edit?: Edit;
  /** This row is being filed by its own Approve button (drives its "Saving…" label). */
  committing: boolean;
  /** This row's own suggestion is still coming — the reason Approve is unavailable, as opposed to a
   *  commit being in flight. A disabled control has no other affordance, so it becomes the button's
   *  tooltip (the row already prints "Awaiting proposal…" below the title). */
  awaiting: boolean;
  /** Approve is unavailable — this row's own proposal is still streaming, or a commit is in flight.
   *  A row becomes approvable the moment ITS proposal lands, not when the whole batch finishes. */
  disabled: boolean;
  /** No suggestion is coming (AI off, or it failed) — prompt the user to fill the fields in. */
  noSuggestions: boolean;
  /** How many OTHER unsorted files share this document's folder — 0 hides the folder-apply action
   *  (also 0 until a real project is chosen for this row). */
  folderApplyCount: number;
  /** The folder's display name (leaf) for the folder-apply label. */
  folderName: string | null;
  onChange: (patch: Partial<Edit>) => void;
  /** File just this document with the values shown, leaving the rest of the queue. */
  onApprove: () => void;
  /** Open the "file the rest of this folder the same way" panel. */
  onApplyFolder: () => void;
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
            size="sm"
            onClick={onApprove}
            disabled={disabled}
            className="shrink-0"
            data-help="review-approve-one"
            title={
              awaiting
                ? "Waiting for this file's suggestion"
                : "File just this document with the values shown"
            }
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

        {folderApplyCount > 0 && (
          <button
            type="button"
            onClick={onApplyFolder}
            disabled={disabled}
            data-help="review-apply-folder"
            className="mt-3 text-left text-xs text-accent-text transition hover:brightness-110 disabled:opacity-50"
          >
            Apply this filing to {folderApplyCount} other file{folderApplyCount === 1 ? "" : "s"}{" "}
            from
            {folderName ? ` ${folderName}` : " this folder"} →
          </button>
        )}
      </Card>
    </li>
  );
}

/** The deterministic "file the rest of this folder the same way" panel (B). Lists the folder's other
 *  unsorted files with a tick each (all on by default) and, on Apply, files this document plus the
 *  ticked ones into the row's chosen project — each keeping its own tags and importance. No AI. */
function FolderApplyPanel({
  folderName,
  project,
  siblings,
  checked,
  busy,
  onToggle,
  onApply,
  onCancel,
}: {
  folderName: string | null;
  project: string;
  siblings: Document[];
  checked: Set<number>;
  busy: boolean;
  onToggle: (id: number) => void;
  onApply: () => void;
  onCancel: () => void;
}) {
  return (
    <li>
      <Card className="border-accent-soft p-4" data-help="review-folder-panel">
        <p className="text-sm text-ink2">
          Apply <span className="font-medium text-accent-text">{project}</span> to this file and the
          ticked files from <span className="font-medium">{folderName ?? "this folder"}</span>:
        </p>
        <ul className="mt-2 flex max-h-48 flex-col gap-1 overflow-y-auto">
          {siblings.map((s) => (
            <li key={s.id}>
              <label className="flex items-center gap-2 text-sm text-ink3">
                <input
                  type="checkbox"
                  checked={checked.has(s.id)}
                  onChange={() => onToggle(s.id)}
                  className="accent-[var(--accent)]"
                />
                <span className="truncate">{s.title}</span>
              </label>
            </li>
          ))}
        </ul>
        <div className="mt-3 flex justify-end gap-2">
          <Button variant="tertiary" onClick={onCancel} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" onClick={onApply} disabled={busy || checked.size === 0}>
            {busy ? "Filing…" : `File ${checked.size + 1} files`}
          </Button>
        </div>
      </Card>
    </li>
  );
}
