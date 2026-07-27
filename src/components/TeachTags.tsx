// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Teach tab's tag section (#580, #579): manage the tags you have, and re-tag the library.
//
// It sits in Teach because Teach is already where you correct how PM understands your things —
// merging a project variant so it stops recurring is the same act on a different noun.
//
// Three things live here, in the order you'd want them:
//
//   1. YOUR TAGS — every free-form label with its document count, × to remove it everywhere, and
//      click-to-rename. Plus the "these look like the same tag" nudge that folds `tax` into `taxes`.
//   2. THE RE-TAG PASS — propose a vocabulary for the whole library, EDIT it, then label from it.
//      The vocabulary is the one decision the pass turns on and it is forty words, so reviewing it
//      costs seconds; reviewing the consequences of a bad one means reading every proposal.
//   3. THE PROPOSALS — old tags against new, per document, nothing written until accepted.
//
// Every write here is bulk and irreversible (a tag rewrite touches the vault), which is why the
// destructive actions confirm and the pass stages rather than applies.

import { useCallback, useEffect, useState } from "react";
import {
  applyRetagVocabulary,
  commitRetag,
  deleteTag,
  discardTagProposals,
  listTagProposals,
  listTags,
  proposeRetagVocabulary,
  renameTag,
  retagScope,
} from "../lib/ipc";
import { findSimilarTags } from "../lib/tagSimilarity";
import type { RetagScope, TagProposalRow, TagSummary } from "../lib/types";
import { Button, Card, Input, Modal } from "./ui";

export function TeachTags() {
  const [tags, setTags] = useState<TagSummary[]>([]);
  const [scope, setScope] = useState<RetagScope | null>(null);
  const [rows, setRows] = useState<TagProposalRow[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // The proposed vocabulary, once a pass has been asked for. `null` = no pass in flight. It is
  // EDITABLE — this is the state the user is being asked to approve, not a readout.
  const [vocabulary, setVocabulary] = useState<string[] | null>(null);
  const [draftTag, setDraftTag] = useState("");
  const [proposing, setProposing] = useState(false);
  const [labelling, setLabelling] = useState(false);
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);

  // Documents excluded from the accept. Everything staged is in by default: the pass was asked for
  // wholesale, so opting out is the exception.
  const [excluded, setExcluded] = useState<Set<number>>(new Set());
  const [renaming, setRenaming] = useState<{ from: string; value: string } | null>(null);
  const [deleting, setDeleting] = useState<TagSummary | null>(null);

  const load = useCallback(async () => {
    try {
      const [t, s, r] = await Promise.all([listTags(), retagScope(), listTagProposals()]);
      setTags(t.filter((x) => x.kind !== "project"));
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

  async function run(fn: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await fn();
      await load();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function propose() {
    setError(null);
    setProposing(true);
    setRows([]);
    try {
      setVocabulary(await proposeRetagVocabulary());
    } catch (e) {
      setError(String(e));
    } finally {
      setProposing(false);
    }
  }

  async function label() {
    if (!vocabulary || vocabulary.length === 0) return;
    setError(null);
    setLabelling(true);
    setProgress(null);
    try {
      await applyRetagVocabulary(vocabulary, (ev) => {
        if (ev.type === "progress") setProgress({ done: ev.done, total: ev.total });
      });
      await load();
    } catch (e) {
      setError(String(e));
    } finally {
      setLabelling(false);
      setProgress(null);
    }
  }

  async function accept() {
    const ids = rows.map((r) => r.document_id).filter((id) => !excluded.has(id));
    if (ids.length === 0) return;
    await run(async () => {
      await commitRetag(ids);
      setVocabulary(null);
    });
  }

  function addDraftTag() {
    const name = draftTag.trim().toLowerCase();
    setDraftTag("");
    if (!name || !vocabulary) return;
    if (vocabulary.some((t) => t.toLowerCase() === name)) return;
    setVocabulary([...vocabulary, name]);
  }

  const similar = findSimilarTags(tags);
  const accepting = rows.length - excluded.size;
  const working = busy || proposing || labelling;

  return (
    <section className="mt-8" data-help="teach-tags">
      <p className="pb-2 font-mono text-xs uppercase tracking-wide text-ink4">Tags</p>

      {error && (
        <p className="mb-3 text-sm text-st-due" role="alert">
          {error}
        </p>
      )}

      {/* ---------------------------------------------------------------- your tags */}
      <Card className="p-4">
        <p className="text-sm leading-relaxed text-ink3">
          Your free-form labels. A tag earns its place by grouping things — one that only ever lands
          on a single file isn&apos;t doing anything, so remove it or fold it into one that fits.
        </p>

        {tags.length === 0 ? (
          <p className="mt-3 text-sm text-ink4">
            No tags yet. They arrive as PM files your documents.
          </p>
        ) : (
          <div className="mt-3 flex flex-wrap gap-1.5">
            {tags.map((t) => (
              <span
                key={t.name}
                className="group inline-flex items-center gap-1 rounded-[var(--radius-sm)] border border-border2 bg-surface px-2 py-0.5 text-xs text-ink3"
              >
                <button
                  type="button"
                  onClick={() => setRenaming({ from: t.name, value: t.name })}
                  disabled={working}
                  className="hover:text-ink disabled:opacity-50"
                  title={`Rename "${t.name}" everywhere`}
                >
                  {t.name}
                </button>
                <span className="text-ink4">{t.documents}</span>
                <button
                  type="button"
                  onClick={() => setDeleting(t)}
                  disabled={working}
                  // Persistently visible with hit padding, not a hover-revealed glyph: TeachView's
                  // alias chips learned that the hard way.
                  className="-mr-0.5 px-1 text-ink4 transition hover:text-ink disabled:opacity-50"
                  aria-label={`Remove the tag ${t.name} from every document`}
                  title={`Remove "${t.name}" from every document`}
                >
                  ×
                </button>
              </span>
            ))}
          </div>
        )}

        {similar.length > 0 && (
          <div className="mt-4">
            <p className="pb-1.5 text-xs text-ink4">These look like the same tag</p>
            <ul className="flex flex-col gap-1.5">
              {similar.map(([keep, fold]) => (
                <li key={`${keep.name}-${fold.name}`} className="flex items-center gap-2 text-xs">
                  <span className="min-w-0 flex-1 truncate text-ink3">
                    <span className="text-ink">{fold.name}</span>
                    <span className="text-ink4"> → </span>
                    <span className="text-ink">{keep.name}</span>
                  </span>
                  <Button
                    variant="tertiary"
                    disabled={working}
                    onClick={() => void run(() => renameTag(fold.name, keep.name))}
                  >
                    Fold
                  </Button>
                </li>
              ))}
            </ul>
          </div>
        )}
      </Card>

      {/* ---------------------------------------------------------------- the pass */}
      <Card className="mt-3 p-4">
        <p className="text-sm leading-relaxed text-ink3">
          Re-tag everything from one set of labels. PM reads your whole library and suggests a
          vocabulary that suits it; you edit that list, then it labels every document from your
          version. Projects and importance are never touched.
        </p>

        {scope != null && scope.documents > 0 && (
          <p className="mt-2 font-mono text-xs text-ink4">
            {scope.documents} document{scope.documents === 1 ? "" : "s"} · about {scope.calls} model
            call{scope.calls === 1 ? "" : "s"}
          </p>
        )}

        <div className="mt-3 flex flex-wrap items-center gap-2">
          <Button variant="secondary" onClick={() => void propose()} disabled={working}>
            {proposing ? "Reading your library…" : vocabulary ? "Suggest again" : "Suggest tags"}
          </Button>
          {progress && (
            <span className="font-mono text-xs text-ink4">
              {progress.done} / {progress.total}
            </span>
          )}
        </div>

        {vocabulary && (
          <div className="mt-4">
            <p className="pb-1.5 text-xs text-ink4">
              {vocabulary.length} tag{vocabulary.length === 1 ? "" : "s"} — remove any you
              don&apos;t want, add any you know you need, then label.
            </p>
            <div className="flex flex-wrap items-center gap-1.5">
              {vocabulary.map((t) => (
                <span
                  key={t}
                  className="inline-flex items-center gap-1 rounded-[var(--radius-sm)] bg-accent-soft px-2 py-0.5 text-xs text-accent-text"
                >
                  {t}
                  <button
                    type="button"
                    onClick={() => setVocabulary(vocabulary.filter((x) => x !== t))}
                    disabled={working}
                    className="-mr-0.5 px-1 opacity-70 transition hover:opacity-100 disabled:opacity-40"
                    aria-label={`Drop ${t} from the vocabulary`}
                  >
                    ×
                  </button>
                </span>
              ))}
              <input
                value={draftTag}
                onChange={(e) => setDraftTag(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    addDraftTag();
                  }
                }}
                onBlur={addDraftTag}
                disabled={working}
                aria-label="Add a tag to the vocabulary"
                placeholder="add a tag…"
                className="w-32 bg-transparent px-1 py-0.5 text-xs text-ink2 outline-none placeholder:text-ink4"
              />
            </div>
            <div className="mt-3">
              <Button onClick={() => void label()} disabled={working || vocabulary.length === 0}>
                {labelling ? "Labelling…" : `Label my library from these ${vocabulary.length}`}
              </Button>
            </div>
          </div>
        )}
      </Card>

      {/* ---------------------------------------------------------------- the proposals */}
      {rows.length > 0 && (
        <div className="mt-4">
          <div className="flex flex-wrap items-center justify-between gap-2 pb-2">
            <p className="font-mono text-xs uppercase tracking-wide text-ink4">
              {rows.length} document{rows.length === 1 ? "" : "s"} would change
            </p>
            <div className="flex gap-2">
              <Button
                variant="tertiary"
                onClick={() => void run(() => discardTagProposals())}
                disabled={working}
              >
                Discard
              </Button>
              <Button onClick={() => void accept()} disabled={working || accepting === 0}>
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
                        onChange={() =>
                          setExcluded((prev) => {
                            const next = new Set(prev);
                            if (next.has(r.document_id)) next.delete(r.document_id);
                            else next.add(r.document_id);
                            return next;
                          })
                        }
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

      {/* Both confirmations name the scale, because both rewrite vault files with no undo. */}
      <Modal open={deleting != null} onClose={() => (busy ? undefined : setDeleting(null))}>
        {deleting && (
          <div className="p-5">
            <h2 className="font-head text-base font-semibold text-ink">Remove this tag?</h2>
            <p className="mt-2 text-sm leading-relaxed text-ink3">
              <span className="text-ink">{deleting.name}</span> comes off {deleting.documents}{" "}
              document{deleting.documents === 1 ? "" : "s"}, in your vault as well as here. Nothing
              else about them changes, and no files are deleted — but this can&apos;t be undone.
            </p>
            <div className="mt-4 flex justify-end gap-2">
              <Button variant="tertiary" onClick={() => setDeleting(null)} disabled={busy}>
                Cancel
              </Button>
              <Button
                onClick={() => {
                  const name = deleting.name;
                  setDeleting(null);
                  void run(() => deleteTag(name));
                }}
                disabled={busy}
              >
                Remove it
              </Button>
            </div>
          </div>
        )}
      </Modal>

      <Modal open={renaming != null} onClose={() => (busy ? undefined : setRenaming(null))}>
        {renaming && (
          <div className="p-5">
            <h2 className="font-head text-base font-semibold text-ink">Rename this tag</h2>
            <p className="mt-2 text-sm leading-relaxed text-ink3">
              Changes <span className="text-ink">{renaming.from}</span> on every document that
              carries it. If the new name is already a tag, the two are folded into one.
            </p>
            <Input
              value={renaming.value}
              onChange={(e) => setRenaming({ ...renaming, value: e.target.value })}
              aria-label="New tag name"
              className="mt-3"
              autoFocus
            />
            <div className="mt-4 flex justify-end gap-2">
              <Button variant="tertiary" onClick={() => setRenaming(null)} disabled={busy}>
                Cancel
              </Button>
              <Button
                onClick={() => {
                  const { from, value } = renaming;
                  const next = value.trim().toLowerCase();
                  setRenaming(null);
                  if (!next || next === from.toLowerCase()) return;
                  void run(() => renameTag(from, next));
                }}
                disabled={busy}
              >
                Rename
              </Button>
            </div>
          </div>
        )}
      </Modal>
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
