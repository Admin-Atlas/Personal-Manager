// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { RunnerGuide } from "../../lib/workbenchGuide";
import { withCode } from "../withCode";

/**
 * One local server, described the way someone choosing between them needs it: what it is, who it
 * suits, how models get into it, what might rule it out, whether it stays running, then the steps.
 *
 * Its own component because the comparison is not the only place a runner's guide belongs — and two
 * hand-kept copies of one card would drift the way the guide's own facts did before `lifecycle` got
 * a field. The guide's strings mark commands with backticks, so every line that can carry one goes
 * through `withCode`: printed raw, the backticks read as part of the command.
 */
export function RunnerGuideCard({ guide: g }: { guide: RunnerGuide }) {
  return (
    <div className="rounded-[var(--radius-sm)] border border-border p-2.5">
      <div className="flex flex-wrap items-baseline gap-x-2">
        <span className="text-sm text-ink2">{g.name}</span>
        <span className="font-mono text-[0.625rem] text-ink4">port {g.port}</span>
      </div>
      <p className="mt-0.5">{g.summary}</p>
      <p className="mt-1.5 text-ink3">{g.bestFor}</p>
      <p className="mt-1">
        <span className="text-ink3">Models:</span> {withCode(g.models)}
      </p>
      {/* Unfolded, never a caret: a hardware exclusion and "does this stay running?" are
          gating facts, and the settings doctrine folds prose but not those. Lifecycle sits
          immediately before the steps because it is what decides whether the steps are a
          one-time setup or something you redo every session. */}
      {g.caveat && <p className="mt-1 text-ink3">Worth knowing: {withCode(g.caveat)}</p>}
      <p className="mt-1 text-ink3">
        <span className="text-ink3">Staying running:</span> {withCode(g.lifecycle)}
      </p>
      <ol className="ml-4 mt-1.5 list-decimal space-y-1">
        {g.steps.map((s, i) => (
          <li key={i}>{withCode(s)}</li>
        ))}
      </ol>
    </div>
  );
}
