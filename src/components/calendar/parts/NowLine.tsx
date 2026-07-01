// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The "now" marker in the time-grid: a 2px accent line with a dot at its left edge, drawn across the
// today column only. Accent (not a source colour) so "now" is never confusable with a calendar — the
// same reservation the source palette makes. The view mounts this only when today is in view.

interface Props {
  /** Pixel offset from the top of the day column (minutes-since-midnight scaled to row height). */
  topPx: number;
}

export function NowLine({ topPx }: Props) {
  return (
    <div
      aria-hidden
      className="pointer-events-none absolute inset-x-0 z-20 flex items-center"
      style={{ top: `${topPx}px` }}
    >
      <span className="-ml-1 h-2 w-2 shrink-0 rounded-full bg-accent" />
      <span className="h-[2px] flex-1 bg-accent" />
    </div>
  );
}
