// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The "Teach" tab (entity-resolution foundation, PR 2): a power surface onto the canonical
// `entities` / `entity_aliases` data. It introduces NO new write path — every action calls the
// same backend commands a review correction does, so an inline correction and a Teach merge end in
// identical rules-file state. Browse project entities + their aliases, rename a canonical
// everywhere at once, merge a variant into its real project so it never recurs, or add a name you
// know means the same thing. Visibility is Depth-keyed (hidden for the minimalist preset) and
// toggleable in Settings; hiding it hides only this editor — deterministic resolution keeps running.

import { useEffect, useMemo, useState } from "react";
import {
  addEntityAlias,
  listEntities,
  listProjectOverviews,
  mergeEntities,
  renameEntity,
} from "../lib/ipc";
import type { Entity } from "../lib/types";
import { useDevMode } from "../lib/capabilities";
import { useDepth } from "../theme";
import { TeachPreferences } from "./TeachPreferences";
import { DevRaw } from "./dev/DevRaw";
import { Button, Card, Input, Modal, Skeleton } from "./ui";

/** The always-present fallback bucket; we don't nudge merges for it. */
const UNSORTED = "Unsorted";

export function TeachView() {
  const { showPower } = useDepth();
  // null = first load (skeleton); [] = loaded-but-empty.
  const [entities, setEntities] = useState<Entity[] | null>(null);
  // Document counts per canonical name, joined from the focus-view overviews (existing command),
  // so a merge can say how much moves without a new backend query.
  const [docCounts, setDocCounts] = useState<Record<string, number>>({});
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // Inline editors (one open at a time per entity).
  const [renaming, setRenaming] = useState<{ id: number; value: string } | null>(null);
  const [aliasing, setAliasing] = useState<{ id: number; value: string } | null>(null);
  // The merge modal: the entity being folded away, and the chosen survivor.
  const [mergeSource, setMergeSource] = useState<Entity | null>(null);
  const [mergeTargetId, setMergeTargetId] = useState<number | null>(null);

  async function load() {
    setError(null);
    try {
      const [ents, overviews] = await Promise.all([listEntities(), listProjectOverviews()]);
      const counts: Record<string, number> = {};
      for (const o of overviews) counts[o.name] = o.doc_count;
      setDocCounts(counts);
      setEntities(ents);
    } catch (e) {
      setError(String(e));
      setEntities((prev) => prev ?? []); // don't hang on the skeleton if the first load fails
    }
  }

  useEffect(() => {
    void load();
  }, []);

  // Run a mutation, then reload from the backend so the UI reflects the true rules state.
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

  async function commitRename() {
    if (!renaming) return;
    const { id, value } = renaming;
    const name = value.trim();
    setRenaming(null);
    const current = entities?.find((e) => e.id === id)?.canonical_name;
    if (!name || name === current) return;
    await run(() => renameEntity(id, name));
  }

  async function commitAlias() {
    if (!aliasing) return;
    const { id, value } = aliasing;
    const alias = value.trim();
    setAliasing(null);
    if (!alias) return;
    await run(() => addEntityAlias(id, alias));
  }

  function openMerge(source: Entity, targetId?: number) {
    setMergeSource(source);
    setMergeTargetId(targetId ?? null);
  }

  async function confirmMerge() {
    if (!mergeSource || mergeTargetId == null) return;
    const from = mergeSource.id;
    const into = mergeTargetId;
    setMergeSource(null);
    setMergeTargetId(null);
    await run(() => mergeEntities(from, into));
  }

  // A conservative "these look identical" nudge: canonical names that normalise to the same string
  // (e.g. "Atlas - PM" vs "atlas pm"). High-precision and user-confirmed — NOT the deferred
  // embedding-based auto-detection (that's the parked "Richer entity resolution" tier).
  const suggestions = useMemo(() => findIdenticalPairs(entities ?? []), [entities]);

  const mergeTarget = entities?.find((e) => e.id === mergeTargetId) ?? null;

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center justify-between border-b border-border px-6 py-3">
        <div>
          <h1 className="font-head text-sm font-semibold text-ink">Teach</h1>
          <p className="text-xs text-ink3">
            {entities == null
              ? "Loading…"
              : entities.length === 0
                ? "No projects yet"
                : `${entities.length} project${entities.length === 1 ? "" : "s"}`}
          </p>
        </div>
      </header>

      <div className="flex-1 overflow-y-auto">
        <div className="mx-auto max-w-3xl px-6 py-6">
          <p className="mb-5 text-sm leading-relaxed text-ink3" data-help="teach-intro">
            Teach PM how your projects are named. Merge a name variant into the real project so it
            stops coming back, rename a project everywhere at once, or add a name you know means the
            same thing. Correcting a project in Review does this too — here you can do it directly.
          </p>

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

          {entities == null ? (
            <ul className="flex flex-col gap-3">
              {Array.from({ length: 4 }).map((_, i) => (
                <li key={i}>
                  <Card className="p-4">
                    <Skeleton className="h-5 w-40" />
                    <Skeleton className="mt-3 h-4 w-24" />
                  </Card>
                </li>
              ))}
            </ul>
          ) : entities.length === 0 ? (
            <p className="text-sm text-ink4">
              No projects yet. As you ingest and sort documents, the projects you file them under
              appear here — ready to rename, merge, and teach.
            </p>
          ) : (
            <>
              {suggestions.length > 0 && (
                <div className="mb-5" data-help="teach-suggestions">
                  <p className="pb-2 font-mono text-xs uppercase tracking-wide text-ink4">
                    These look like the same project
                  </p>
                  <ul className="flex flex-col gap-2">
                    {suggestions.map(([a, b]) => (
                      <li key={`${a.id}-${b.id}`}>
                        <Card className="flex items-center justify-between gap-3 px-4 py-2.5">
                          <span className="min-w-0 truncate text-sm text-ink2">
                            <span className="text-ink">{a.canonical_name}</span>
                            <span className="text-ink4"> ↔ </span>
                            <span className="text-ink">{b.canonical_name}</span>
                          </span>
                          <Button
                            variant="secondary"
                            disabled={busy}
                            onClick={() => {
                              // Default to keeping the more-used name; the modal lets you flip it.
                              const aDocs = docCounts[a.canonical_name] ?? 0;
                              const bDocs = docCounts[b.canonical_name] ?? 0;
                              const [from, into] = aDocs <= bDocs ? [a, b] : [b, a];
                              openMerge(from, into.id);
                            }}
                          >
                            Merge…
                          </Button>
                        </Card>
                      </li>
                    ))}
                  </ul>
                </div>
              )}

              <ul className="flex flex-col gap-3">
                {entities.map((entity) => (
                  <EntityCard
                    key={entity.id}
                    entity={entity}
                    docCount={docCounts[entity.canonical_name] ?? 0}
                    showPower={showPower}
                    busy={busy}
                    renaming={renaming?.id === entity.id ? renaming.value : null}
                    aliasing={aliasing?.id === entity.id ? aliasing.value : null}
                    onRenameStart={() =>
                      setRenaming({ id: entity.id, value: entity.canonical_name })
                    }
                    onRenameChange={(value) => setRenaming({ id: entity.id, value })}
                    onRenameCommit={commitRename}
                    onRenameCancel={() => setRenaming(null)}
                    onAliasStart={() => setAliasing({ id: entity.id, value: "" })}
                    onAliasChange={(value) => setAliasing({ id: entity.id, value })}
                    onAliasCommit={commitAlias}
                    onAliasCancel={() => setAliasing(null)}
                    onMerge={() => openMerge(entity)}
                  />
                ))}
              </ul>
            </>
          )}

          {/* Structured preferences (§4.5) live here too — shown even with no projects yet, since a
              global/context preference (or one migrated from the old profile) needs no project. */}
          {entities != null && <TeachPreferences projects={entities} />}
        </div>
      </div>

      <Modal
        open={mergeSource != null}
        onClose={() => (busy ? undefined : setMergeSource(null))}
        widthClassName="max-w-md"
      >
        {mergeSource && (
          <div className="p-5">
            <h2 className="font-head text-base font-semibold text-ink">Merge projects</h2>
            <p className="mt-2 text-sm leading-relaxed text-ink3">
              Fold <span className="font-medium text-ink2">{mergeSource.canonical_name}</span> into
              another project. Its {docCount(docCounts[mergeSource.canonical_name] ?? 0)} and all
              its other names move to the project you keep, and{" "}
              <span className="font-medium text-ink2">{mergeSource.canonical_name}</span>{" "}
              disappears.
            </p>

            <label className="mt-4 block text-xs text-ink3">Keep this project</label>
            <select
              value={mergeTargetId ?? ""}
              onChange={(e) => setMergeTargetId(e.target.value ? Number(e.target.value) : null)}
              className="mt-1 w-full rounded-[var(--radius-sm)] border border-border2 bg-surface px-3 py-2 text-sm text-ink2 outline-none focus:border-accent"
            >
              <option value="">Choose a project…</option>
              {entities
                ?.filter((e) => e.id !== mergeSource.id)
                .map((e) => (
                  <option key={e.id} value={e.id}>
                    {e.canonical_name}
                  </option>
                ))}
            </select>

            <div className="mt-5 flex justify-end gap-2">
              <Button variant="tertiary" onClick={() => setMergeSource(null)} disabled={busy}>
                Cancel
              </Button>
              <Button
                variant="primary"
                onClick={() => void confirmMerge()}
                disabled={busy || mergeTargetId == null}
              >
                {busy
                  ? "Merging…"
                  : mergeTarget
                    ? `Merge into ${mergeTarget.canonical_name}`
                    : "Merge"}
              </Button>
            </div>
          </div>
        )}
      </Modal>
    </div>
  );
}

function EntityCard({
  entity,
  docCount,
  showPower,
  busy,
  renaming,
  aliasing,
  onRenameStart,
  onRenameChange,
  onRenameCommit,
  onRenameCancel,
  onAliasStart,
  onAliasChange,
  onAliasCommit,
  onAliasCancel,
  onMerge,
}: {
  entity: Entity;
  docCount: number;
  showPower: boolean;
  busy: boolean;
  renaming: string | null;
  aliasing: string | null;
  onRenameStart: () => void;
  onRenameChange: (v: string) => void;
  onRenameCommit: () => void;
  onRenameCancel: () => void;
  onAliasStart: () => void;
  onAliasChange: (v: string) => void;
  onAliasCommit: () => void;
  onAliasCancel: () => void;
  onMerge: () => void;
}) {
  // The aliases are every known name; drop the canonical self-alias to show only the *variants*
  // that resolve to it.
  const variants = entity.aliases.filter((a) => a !== entity.canonical_name);
  const { devMode } = useDevMode();

  return (
    <li>
      <Card className="p-4" data-help="teach-entity">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0 flex-1">
            {renaming != null ? (
              <div className="flex items-center gap-2">
                <Input
                  autoFocus
                  value={renaming}
                  onChange={(e) => onRenameChange(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") onRenameCommit();
                    if (e.key === "Escape") onRenameCancel();
                  }}
                  className="w-56"
                  aria-label="Project name"
                />
                <Button variant="primary" onClick={onRenameCommit} disabled={busy}>
                  Save
                </Button>
                <Button variant="tertiary" onClick={onRenameCancel} disabled={busy}>
                  Cancel
                </Button>
              </div>
            ) : (
              <div className="flex items-center gap-1.5">
                <span
                  className="min-w-0 truncate font-head text-sm font-medium text-ink"
                  title={entity.canonical_name}
                >
                  {entity.canonical_name}
                </span>
                {entity.user_confirmed && (
                  <span
                    className="inline-flex shrink-0 items-center gap-0.5 rounded-[var(--radius-sm)] bg-accent-soft px-1.5 py-0.5 text-xs text-accent-text"
                    title="You've confirmed this project"
                  >
                    ✓ Confirmed
                  </span>
                )}
              </div>
            )}
            <p className="mt-1 font-mono text-xs text-ink4">
              {docCount} document{docCount === 1 ? "" : "s"}
              {showPower ? ` · id ${entity.id}` : ""}
            </p>
          </div>

          {renaming == null && (
            <div className="flex shrink-0 items-center gap-1">
              <Button variant="tertiary" onClick={onRenameStart} disabled={busy}>
                Rename
              </Button>
              <Button variant="tertiary" onClick={onMerge} disabled={busy}>
                Merge…
              </Button>
              <Button variant="tertiary" onClick={onAliasStart} disabled={busy}>
                + Name
              </Button>
            </div>
          )}
        </div>

        <div className="mt-3">
          <p className="pb-1.5 text-xs text-ink4">Also known as</p>
          <div className="flex flex-wrap items-center gap-1.5">
            {variants.length === 0 && aliasing == null && (
              <span className="text-xs text-faint">No other names yet.</span>
            )}
            {variants.map((alias) => (
              <span
                key={alias}
                className="inline-flex items-center rounded-[var(--radius-sm)] bg-accent-soft px-2 py-0.5 text-xs text-accent-text"
                title={`Resolves to ${entity.canonical_name}`}
              >
                {alias}
              </span>
            ))}
            {aliasing != null && (
              <input
                autoFocus
                value={aliasing}
                onChange={(e) => onAliasChange(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") onAliasCommit();
                  if (e.key === "Escape") onAliasCancel();
                }}
                onBlur={onAliasCommit}
                placeholder="another name…"
                aria-label={`Add a name for ${entity.canonical_name}`}
                className="w-32 rounded-[var(--radius-sm)] bg-transparent px-1 py-0.5 text-xs text-ink2 outline-none placeholder:text-ink4"
              />
            )}
          </div>
        </div>

        {devMode && (
          <DevRaw
            label="entity"
            fields={[
              ["entity_id", entity.id],
              ["confidence", `${Math.round(entity.confidence * 100)}%`],
              ["user_confirmed", entity.user_confirmed ? "yes" : "no"],
              ["aliases", entity.aliases.length],
            ]}
          />
        )}
      </Card>
    </li>
  );
}

/** "N documents" with singular/plural — small helper for the merge copy. */
function docCount(n: number): string {
  return `${n} document${n === 1 ? "" : "s"}`;
}

/** Normalise a name for the identical-pair nudge: lowercase, strip everything but a-z0-9. */
function normalizeName(s: string): string {
  return s.toLowerCase().replace(/[^a-z0-9]+/g, "");
}

/** Pairs of project entities whose canonical names normalise to the same string — a conservative,
 *  high-precision "these look identical" nudge (not the deferred embedding-based detection). Capped
 *  so the prompt never floods the page. */
function findIdenticalPairs(entities: Entity[]): Array<[Entity, Entity]> {
  const pairs: Array<[Entity, Entity]> = [];
  for (let i = 0; i < entities.length; i++) {
    for (let j = i + 1; j < entities.length; j++) {
      const a = entities[i];
      const b = entities[j];
      if (a.canonical_name === UNSORTED || b.canonical_name === UNSORTED) continue;
      const na = normalizeName(a.canonical_name);
      const nb = normalizeName(b.canonical_name);
      if (na && nb && na === nb) pairs.push([a, b]);
      if (pairs.length >= 5) return pairs;
    }
  }
  return pairs;
}
