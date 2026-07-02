// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A single-day event chip in the Month grid: a source-tinted pill with the title (and, in Power
// depth, its start time). The fill and left rule are the per-source colour mixed into transparency
// via color-mix, so the categorical hue arrives as a prop and no source hex is written in a component.
// Multi-day events render as bands (AllDayBand-style), not chips; Min depth collapses chips to dots.

interface Props {
  summary: string;
  color: string;
  /** Local clock label for a timed event, e.g. "09:30"; empty for all-day. */
  timeLabel: string;
  /** Depth gate: show the time prefix (Power). */
  showTime: boolean;
}

export function EventChip({ summary, color, timeLabel, showTime }: Props) {
  return (
    <div
      className="flex items-center gap-1 overflow-hidden rounded-[var(--radius-sm)] border-l-[2px] px-1 py-px text-[11px] leading-tight"
      style={{
        background: `color-mix(in oklab, ${color} 16%, transparent)`,
        borderLeftColor: color,
      }}
      title={summary}
      aria-label={[timeLabel, summary].filter(Boolean).join(", ")}
    >
      {showTime && timeLabel && (
        <span className="shrink-0 font-mono text-[9px] text-ink4">{timeLabel}</span>
      )}
      <span className="truncate font-head text-ink">{summary}</span>
    </div>
  );
}
