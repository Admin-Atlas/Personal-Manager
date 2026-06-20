// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useMemo, useRef, useState } from "react";
import { listModels } from "../lib/ipc";
import type { ModelInfo } from "../lib/types";

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

  const selected = useMemo(
    () => models?.find((m) => m.id === value) ?? null,
    [models, value],
  );

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
  const customAvailable =
    trimmed.length > 0 && !filtered.some((m) => m.id === trimmed);

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
        className="flex w-full items-center justify-between gap-2 rounded-lg border border-neutral-700 bg-neutral-950 px-3 py-2 text-left text-sm text-neutral-100 outline-none hover:border-neutral-600 focus:border-neutral-500"
      >
        <span className="min-w-0 flex-1 truncate">
          {selected ? selected.name : value || triggerLabel}
          {selected && (
            <span className="ml-2 text-xs text-neutral-500">{selected.id}</span>
          )}
        </span>
        <span className="shrink-0 text-neutral-500">{open ? "▴" : "▾"}</span>
      </button>

      {open && (
        <div className="absolute z-20 mt-1 w-full overflow-hidden rounded-lg border border-neutral-700 bg-neutral-900 shadow-2xl">
          <div className="border-b border-neutral-800 p-2">
            <input
              autoFocus
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search models…"
              className="w-full rounded-md border border-neutral-700 bg-neutral-950 px-3 py-1.5 text-sm text-neutral-100 outline-none focus:border-neutral-500"
            />
          </div>

          {/* Count, a price-sort toggle, and the column hint for the prices. */}
          {models && !error && (
            <div className="flex items-center gap-2 px-3 py-1 text-[10px] uppercase tracking-wide text-neutral-600">
              <span>{filtered.length} model{filtered.length === 1 ? "" : "s"}</span>
              <button
                type="button"
                onClick={() =>
                  setSort((s) => (s === "default" ? "asc" : s === "asc" ? "desc" : "default"))
                }
                title="Sort by input price"
                className="rounded px-1.5 py-0.5 normal-case tracking-normal text-neutral-400 hover:bg-neutral-800 hover:text-neutral-200"
              >
                Sort: {sort === "asc" ? "price ↑" : sort === "desc" ? "price ↓" : "default"}
              </button>
              <span className="ml-auto normal-case tracking-normal">
                price / 1M tokens · in → out
              </span>
            </div>
          )}

          <div className="max-h-72 overflow-y-auto">
            {loading && (
              <div className="px-3 py-6 text-center text-sm text-neutral-500">
                Loading models…
              </div>
            )}
            {error && (
              <div className="px-3 py-4 text-sm text-red-300">
                Couldn't load models ({error}). You can still type a model id below.
              </div>
            )}

            {!loading &&
              shown.map((m) => (
                <button
                  key={m.id}
                  type="button"
                  onClick={() => pick(m.id)}
                  className={`flex w-full items-start gap-3 px-3 py-2 text-left hover:bg-neutral-800 ${
                    m.id === value ? "bg-neutral-800/60" : ""
                  }`}
                >
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <span className="truncate text-sm text-neutral-100">{m.name}</span>
                      {m.id === value && (
                        <span className="shrink-0 text-xs text-emerald-400">✓</span>
                      )}
                    </div>
                    <div className="truncate text-xs text-neutral-500">{m.id}</div>
                    <div className="mt-1 flex flex-wrap gap-1">
                      {modelTags(m).map((t) => (
                        <span
                          key={t.label}
                          className={`rounded px-1.5 py-0.5 text-[10px] font-medium ${t.cls}`}
                        >
                          {t.label}
                        </span>
                      ))}
                    </div>
                  </div>
                  <div className="shrink-0 whitespace-nowrap pt-0.5 text-right text-xs text-neutral-300">
                    {fmtPerM(m.prompt_price)}
                    <span className="text-neutral-600"> → </span>
                    {fmtPerM(m.completion_price)}
                  </div>
                </button>
              ))}

            {!loading && !error && filtered.length > MAX_SHOWN && (
              <div className="px-3 py-2 text-center text-xs text-neutral-600">
                {filtered.length - MAX_SHOWN} more — keep typing to narrow it down.
              </div>
            )}

            {!loading && !error && filtered.length === 0 && !customAvailable && (
              <div className="px-3 py-6 text-center text-sm text-neutral-500">
                No models match “{query}”.
              </div>
            )}

            {customAvailable && (
              <button
                type="button"
                onClick={() => pick(trimmed)}
                className="flex w-full items-center gap-2 border-t border-neutral-800 px-3 py-2 text-left text-sm text-neutral-300 hover:bg-neutral-800"
              >
                Use custom model id:
                <span className="font-mono text-neutral-100">{trimmed}</span>
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

  if (free) tags.push({ label: "Free", cls: "bg-emerald-500/15 text-emerald-300" });
  if (/(?:^|[/\-])(?:o[134]|r1|qwq)|reason|think/.test(hay))
    tags.push({ label: "Reasoning", cls: "bg-violet-500/15 text-violet-300" });
  if (/cod(?:e|er|ing)/.test(hay))
    tags.push({ label: "Coding", cls: "bg-sky-500/15 text-sky-300" });
  if (m.input_modalities?.includes("image"))
    tags.push({ label: "Vision", cls: "bg-amber-500/15 text-amber-300" });
  if ((m.context_length ?? 0) >= 200_000)
    tags.push({ label: "Long context", cls: "bg-teal-500/15 text-teal-300" });
  if (!free && promptPerM > 0 && promptPerM <= 1)
    tags.push({ label: "Budget", cls: "bg-green-500/15 text-green-300" });
  else if (!free && promptPerM >= 10)
    tags.push({ label: "Premium", cls: "bg-rose-500/15 text-rose-300" });

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
