// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The hybrid-retrieval score table (card 7H) shared by the Developer-mode retrieval panel (DevView) and
// the in-chat "Explain retrieval" panel (RetrievalExplainPanel). Both render the same DevRetrievalRow[]
// with the same columns, so the table body + vec cell live here once. Each caller keeps its own summary
// line above the table and its own empty-state below — those legitimately differ (Dev shows rrf-k /
// half-life; the wording of the "no candidates" line differs), so they are NOT part of this component.

import type { DevRetrievalRow } from "../lib/types";
import { HScroll } from "./ui";

/** The vector branch's cell: the raw `vec0` distance (lower = nearer), with its KNN rank on hover.
 *  "—" when the chunk surfaced via the keyword branch only. */
export function vecCell(r: DevRetrievalRow): { text: string; title: string } {
  if (r.vector_rank == null) return { text: "—", title: "not in the vector branch" };
  const dist = r.vector_distance != null ? r.vector_distance.toFixed(3) : "?";
  return { text: dist, title: `vector rank ${r.vector_rank}` };
}

/** The `# / chunk / vec / kw / fused / decay / rerank` table for a non-empty candidate set. */
export function RetrievalScoreTable({ rows }: { rows: DevRetrievalRow[] }) {
  return (
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
          {rows.map((r) => {
            const vec = vecCell(r);
            return (
              <tr key={r.chunk_id} className="border-t border-rule align-top">
                <td className="py-1 pr-2 font-mono text-ink3">{r.final_rank + 1}</td>
                <td className="py-1 pr-2">
                  <div className="text-ink2">
                    {r.title}
                    {r.heading ? <span className="text-ink4"> §{r.heading}</span> : null}
                  </div>
                  <div className="max-w-md whitespace-pre-wrap break-words text-ink4">
                    {r.preview}
                  </div>
                </td>
                <td className="py-1 pr-2 text-right font-mono text-ink3" title={vec.title}>
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
  );
}
