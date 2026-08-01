// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useMemo, useRef, useState } from "react";
import { listModels } from "../lib/ipc";
import type { ModelInfo } from "../lib/types";
import { Button, Input, Skeleton } from "./ui";

interface Props {
  /** The currently selected model id (e.g. "anthropic/claude-sonnet-4.6"). */
  value: string;
  onChange: (modelId: string) => void;
  /** Trigger label shown when nothing is selected (e.g. "Add a model…"). */
  triggerLabel?: string;
}

type Sort = "default" | "asc" | "desc";

/**
 * A searchable dropdown over OpenRouter's whole model catalogue. Shows each
 * model's input/output price and a couple of derived "best for" tags, plus a
 * search box (the list is long). Falls back to letting the user type a custom
 * model id, so PM is never locked to the catalogue (spec §6). The catalogue is
 * fetched lazily the first time the dropdown opens.
 */
export function ModelPicker({ value, onChange, triggerLabel = "Choose a model…" }: Props) {
  const [open, setOpen] = useState(false);
  const [models, setModels] = useState<ModelInfo[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<Sort>("default");
  const rootRef = useRef<HTMLDivElement>(null);

  // Lazy-load the catalogue the first time the dropdown is opened.
  useEffect(() => {
    if (!open || models || loading) return;
    setLoading(true);
    setError(null);
    listModels()
      .then(setModels)
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [open, models, loading]);

  // Close on outside click or Escape.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const selected = useMemo(() => models?.find((m) => m.id === value) ?? null, [models, value]);

  const filtered = useMemo(() => {
    if (!models) return [];
    const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
    if (terms.length === 0) return models;
    return models.filter((m) => {
      const hay = `${m.id} ${m.name}`.toLowerCase();
      return terms.every((t) => hay.includes(t));
    });
  }, [models, query]);

  // Optional sort by input (prompt) price; unpriced models always sort last.
  const sorted = useMemo(() => {
    if (sort === "default") return filtered;
    const known = filtered.filter((m) => m.prompt_price != null);
    const unknown = filtered.filter((m) => m.prompt_price == null);
    known.sort((a, b) => (a.prompt_price! - b.prompt_price!) * (sort === "asc" ? 1 : -1));
    return [...known, ...unknown];
  }, [filtered, sort]);

  const MAX_SHOWN = 60;
  const shown = sorted.slice(0, MAX_SHOWN);

  // Let the user pick a model id that isn't in the catalogue (or hasn't loaded).
  const trimmed = query.trim();
  const customAvailable = trimmed.length > 0 && !filtered.some((m) => m.id === trimmed);

  function pick(modelId: string) {
    onChange(modelId);
    setOpen(false);
    setQuery("");
  }

  return (
    <div ref={rootRef} className="relative mt-1">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex w-full items-center justify-between gap-2 rounded-[var(--radius-sm)] border border-border2 bg-surface px-3 py-2 text-left text-sm text-ink2 outline-none transition hover:border-accent focus:border-accent"
      >
        <span className="min-w-0 flex-1 truncate">
          {selected ? selected.name : value || triggerLabel}
          {selected && <span className="ml-2 font-mono text-xs text-ink4">{selected.id}</span>}
        </span>
        <span className="shrink-0 text-ink4">{open ? "▴" : "▾"}</span>
      </button>

      {open && (
        <div className="absolute z-20 mt-1 w-full overflow-hidden rounded-[var(--radius)] border border-border bg-panel shadow-2xl">
          <div className="border-b border-rule p-2">
            <Input
              autoFocus
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search models…"
              className="py-1.5"
            />
          </div>

          {/* Count, a price-sort toggle, and the column hint for the prices. */}
          {models && !error && (
            <div className="flex items-center gap-2 px-3 py-1 font-mono text-[0.625rem] uppercase tracking-wide text-faint">
              <span>
                {filtered.length} model{filtered.length === 1 ? "" : "s"}
              </span>
              <Button
                variant="tertiary"
                size="xs"
                onClick={() =>
                  setSort((s) => (s === "default" ? "asc" : s === "asc" ? "desc" : "default"))
                }
                title="Sort by input price"
                className="normal-case tracking-normal hover:bg-surface"
              >
                Sort: {sort === "asc" ? "price ↑" : sort === "desc" ? "price ↓" : "default"}
              </Button>
              <span className="ml-auto normal-case tracking-normal">
                price / 1M tokens · in → out
              </span>
            </div>
          )}

          <div className="max-h-72 overflow-y-auto">
            {loading && (
              <div className="flex flex-col gap-1 px-3 py-3">
                {Array.from({ length: 6 }).map((_, i) => (
                  <Skeleton key={i} className="h-9 w-full" />
                ))}
              </div>
            )}
            {error && (
              <div className="px-3 py-4 text-sm text-st-due">
                Couldn't load models ({error}). You can still type a model id below.
              </div>
            )}

            {!loading &&
              shown.map((m) => (
                <button
                  key={m.id}
                  type="button"
                  onClick={() => pick(m.id)}
                  className={`flex w-full items-start gap-3 px-3 py-2 text-left transition hover:bg-surface ${
                    m.id === value ? "bg-surface" : ""
                  }`}
                >
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <span className="truncate text-sm text-ink">{m.name}</span>
                      {m.id === value && <span className="shrink-0 text-xs text-st-quick">✓</span>}
                    </div>
                    <div className="truncate font-mono text-xs text-ink4">{m.id}</div>
                    <div className="mt-1 flex flex-wrap gap-1">
                      {modelTags(m).map((t) => (
                        <span
                          key={t.label}
                          className={`rounded-[var(--radius-sm)] px-1.5 py-0.5 text-[0.625rem] font-medium ${t.cls}`}
                        >
                          {t.label}
                        </span>
                      ))}
                    </div>
                  </div>
                  <div className="shrink-0 whitespace-nowrap pt-0.5 text-right font-mono text-xs text-ink3">
                    {fmtPerM(m.prompt_price)}
                    <span className="text-ink4"> → </span>
                    {fmtPerM(m.completion_price)}
                  </div>
                </button>
              ))}

            {!loading && !error && filtered.length > MAX_SHOWN && (
              <div className="px-3 py-2 text-center text-xs text-faint">
                {filtered.length - MAX_SHOWN} more — keep typing to narrow it down.
              </div>
            )}

            {!loading && !error && filtered.length === 0 && !customAvailable && (
              <div className="px-3 py-6 text-center text-sm text-ink4">
                No models match “{query}”.
              </div>
            )}

            {customAvailable && (
              <button
                type="button"
                onClick={() => pick(trimmed)}
                className="flex w-full items-center gap-2 border-t border-rule px-3 py-2 text-left text-sm text-ink3 transition hover:bg-surface"
              >
                Use custom model id:
                <span className="font-mono text-ink">{trimmed}</span>
              </button>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

interface Tag {
  label: string;
  cls: string;
}

/** Derive a couple of "best for" tags from a model's metadata. OpenRouter doesn't
 *  hand these out, so we infer them from id/name, pricing, context, and modality.
 *  Capped at three so rows stay readable. */
function modelTags(m: ModelInfo): Tag[] {
  const hay = `${m.id} ${m.name}`.toLowerCase();
  const promptPerM = (m.prompt_price ?? 0) * 1_000_000;
  const free = (m.prompt_price ?? 0) === 0 && (m.completion_price ?? 0) === 0;
  const tags: Tag[] = [];

  if (free)
    tags.push({
      label: "Free",
      cls: "bg-[color-mix(in_oklab,var(--st-quick)_15%,transparent)] text-[var(--st-quick)]",
    });
  if (/(?:^|[/-])(?:o[134]|r1|qwq)|reason|think/.test(hay))
    tags.push({
      label: "Reasoning",
      cls: "bg-[color-mix(in_oklab,var(--st-blocked)_15%,transparent)] text-[var(--st-blocked)]",
    });
  if (/cod(?:e|er|ing)/.test(hay))
    tags.push({
      label: "Coding",
      cls: "bg-[color-mix(in_oklab,var(--st-part)_15%,transparent)] text-[var(--st-part)]",
    });
  if (m.input_modalities?.includes("image"))
    tags.push({
      label: "Vision",
      cls: "bg-[color-mix(in_oklab,var(--st-look)_15%,transparent)] text-[var(--st-look)]",
    });
  if ((m.context_length ?? 0) >= 200_000)
    tags.push({
      label: "Long context",
      cls: "bg-[color-mix(in_oklab,var(--st-track)_15%,transparent)] text-[var(--st-track)]",
    });
  if (!free && promptPerM > 0 && promptPerM <= 1)
    tags.push({
      label: "Budget",
      cls: "bg-[color-mix(in_oklab,var(--st-quick)_15%,transparent)] text-[var(--st-quick)]",
    });
  else if (!free && promptPerM >= 10)
    tags.push({
      label: "Premium",
      cls: "bg-[color-mix(in_oklab,var(--st-due)_15%,transparent)] text-[var(--st-due)]",
    });

  return tags.slice(0, 3);
}

/** Format a per-token price as a friendly per-million-tokens dollar figure. */
function fmtPerM(perToken: number | null): string {
  if (perToken == null) return "—";
  if (perToken === 0) return "Free";
  const perM = perToken * 1_000_000;
  if (perM < 1) return `$${perM.toFixed(2)}`;
  if (perM < 10) return `$${perM.toFixed(2)}`;
  return `$${perM.toFixed(0)}`;
}
