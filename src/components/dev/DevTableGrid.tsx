// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A redacted table page rendered as a horizontally-scrollable grid (issue #78). Shared by the
// Dev tab's raw-table browser and the in-context Documents chunk inspector. The cells are already
// redacted by the backend, so this only lays them out.

import type { DevTablePage } from "../../lib/types";

export function DevTableGrid({ page }: { page: DevTablePage }) {
  if (page.rows.length === 0) return <p className="text-xs text-ink4">No rows.</p>;
  return (
    <div className="overflow-x-auto">
      <table className="w-full border-collapse text-left font-mono text-xs">
        <thead>
          <tr className="border-b border-border2 text-ink3">
            {page.columns.map((c) => (
              <th key={c} className="whitespace-nowrap px-2 py-1 font-medium">
                {c}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {page.rows.map((row, i) => (
            <tr key={i} className="border-b border-rule text-ink2 last:border-0">
              {row.map((cell, j) => (
                <td key={j} className="whitespace-nowrap px-2 py-1 align-top">
                  {cell || "—"}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
