// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The daily briefing — a short "here's your picture today" synthesis (Step 7), rendered wherever the
// user has asked to see it. Lifted out of FocusView so the Focus card, the sidebar panel and the
// floating panel are literally the same component reading the same provider, rather than three
// copies that drift.
//
// The variants differ only in chrome (a Card vs. a bare block, heading size, how much vertical room
// the text may take). The body, the empty/generating states and the Refresh control are shared —
// which is the point: a change to how a briefing reads lands everywhere at once.

import { Button, Card } from "./ui";
import { Markdown } from "../lib/markdown";
import { formatDate } from "../lib/format";
import { useBriefing } from "../lib/briefing";

/** How much chrome the briefing wears. `card` is the Focus tab's full-width card; `panel` is the
 *  compact form used by the sidebar footer and the floating panel, where vertical room is scarce. */
export type BriefingVariant = "card" | "panel";

/** A circular-arrow refresh glyph. Hand-rolled in the house style (24x24, stroke=currentColor, round
 *  caps) because no icon library is a dependency — see settings/tabIcons.tsx. */
function RefreshIcon({ spinning }: { spinning: boolean }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className={`h-3.5 w-3.5 ${spinning ? "animate-spin motion-reduce:animate-none" : ""}`}
    >
      <path d="M21 12a9 9 0 1 1-2.64-6.36" />
      <path d="M21 3v6h-6" />
    </svg>
  );
}

function RefreshButton({ busy, onRefresh }: { busy: boolean; onRefresh: () => void }) {
  return (
    <Button
      variant="tertiary"
      onClick={onRefresh}
      disabled={busy}
      title="Regenerate today's briefing from your current projects and calendar"
      aria-label={busy ? "Regenerating today's briefing" : "Regenerate today's briefing"}
      className="flex items-center gap-1 px-2 py-0.5 text-xs"
    >
      <RefreshIcon spinning={busy} />
      <span>{busy ? "Refreshing…" : "Refresh"}</span>
    </Button>
  );
}

/**
 * Today's briefing. Reads the shared provider, so every mounted instance shows the same text and one
 * Refresh updates all of them.
 *
 * Renders nothing at all when there is no briefing and none is being generated (e.g. an empty store)
 * — a zero-height empty state, so an enabled-but-empty surface never leaves a stray box behind.
 */
export function Briefing({
  variant = "card",
  className = "",
  fill = false,
}: {
  variant?: BriefingVariant;
  className?: string;
  /** Panel variant only: grow the text to the host's height instead of stopping at `max-h-48`.
   *  For hosts the USER can resize — the OS briefing window and the in-app floating panel — where a
   *  fixed cap left the window growing while the text stayed pinned, i.e. dead space below
   *  "Updated …". The sidebar must NOT pass it: there the cap is load-bearing, keeping a long
   *  briefing from pushing the nav out of reach. */
  fill?: boolean;
}) {
  const { briefing, busy, refresh } = useBriefing();
  const text = briefing?.briefing.trim() ?? "";
  if (!text && !busy) return null;

  const onRefresh = () => void refresh();
  const body = text ? (
    <div className="pm-inline-md text-sm leading-relaxed text-ink2">
      <Markdown>{text}</Markdown>
    </div>
  ) : (
    <p className="text-sm text-ink4">Putting together your briefing…</p>
  );
  const updated = text && briefing?.updated_at && (
    <p className="mt-2 text-xs text-ink4">Updated {formatDate(briefing.updated_at)}</p>
  );

  if (variant === "card") {
    return (
      <Card className={`mb-5 px-4 py-3 ${className}`} data-help="focus-briefing">
        <div className="mb-2 flex items-center justify-between">
          <h2 className="font-mono text-xs font-semibold uppercase tracking-wide text-ink3">
            Today
          </h2>
          <RefreshButton busy={busy} onRefresh={onRefresh} />
        </div>
        {body}
        {updated}
      </Card>
    );
  }

  // Panel: no Card chrome (the host supplies its own) and a smaller heading. The text scrolls inside
  // a bounded box in the sidebar, where growing would push the nav out of reach — but in a
  // user-resizable host (`fill`) it takes the height it is given, so the window and its content grow
  // together instead of leaving a black gap under "Updated …".
  return (
    <div
      className={`${fill ? "flex h-full min-h-0 flex-col" : ""} ${className}`}
      data-help="focus-briefing"
    >
      <div className="mb-1 flex shrink-0 items-center justify-between gap-2">
        <h2 className="font-mono text-[0.6875rem] font-semibold uppercase tracking-wide text-faint">
          Today
        </h2>
        <RefreshButton busy={busy} onRefresh={onRefresh} />
      </div>
      <div className={fill ? "min-h-0 flex-1 overflow-y-auto" : "max-h-48 overflow-y-auto"}>
        {body}
      </div>
      {updated}
    </div>
  );
}
