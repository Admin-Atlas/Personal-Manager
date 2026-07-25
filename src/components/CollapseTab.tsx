// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The slim reopen tab shown in place of a snap-collapsed side panel (see useResizable's collapse
// mode). It hugs the window edge the panel was docked to and points inward; clicking it reopens the
// panel at its minimum width. Shared so the left sidebar and the project focus aside look and behave
// identically.

interface Props {
  /** Which window edge the collapsed panel was docked to. */
  side: "left" | "right";
  /** Reopen the panel (useResizable's `expand`). */
  onExpand: () => void;
}

export function CollapseTab({ side, onExpand }: Props) {
  return (
    <button
      type="button"
      onClick={onExpand}
      title="Show panel"
      aria-label="Show panel"
      className={`flex h-full w-5 min-w-[var(--tap-min,24px)] shrink-0 items-center justify-center bg-panel text-ink4 transition-colors hover:bg-surface hover:text-ink ${
        side === "left" ? "border-r border-border" : "border-l border-border"
      }`}
    >
      <span aria-hidden className="text-sm">
        {side === "left" ? "›" : "‹"}
      </span>
    </button>
  );
}
