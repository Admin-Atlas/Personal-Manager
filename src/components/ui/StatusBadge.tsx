// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The project-status badge (spec §4.1 / DESIGN_TOKENS.md §7). Replaces FocusView's hardcoded
// STATUS_META.cls — colour now comes from the semantic --st-* tokens, and the treatment diverges
// per System: slate = tinted pill, editorial = coloured dot + italic label, terminal = "● label".
// STATUS_LABEL is the single source for the human strings (CommandPalette imports it too).

import { useTheme } from "../../theme";
import type { StatusKey } from "../../theme";
import type { ProjectStatus } from "../../lib/types";
import { cn } from "./cn";

const STATUS_KEY: Record<ProjectStatus, StatusKey> = {
  due_soon: "due",
  blocked: "blocked",
  quick_win: "quick",
  take_a_look: "look",
  part_of: "part",
  on_track: "track",
};

export const STATUS_LABEL: Record<ProjectStatus, string> = {
  due_soon: "Due soon",
  blocked: "Blocked",
  quick_win: "Quick win",
  take_a_look: "Take a look",
  part_of: "Part of",
  on_track: "On track",
};

export interface StatusBadgeProps {
  status: ProjectStatus;
  /** Override the label (e.g. "Part of <parent>"). */
  label?: string;
  className?: string;
}

export function StatusBadge({ status, label, className }: StatusBadgeProps) {
  const { system } = useTheme();
  const color = `var(--st-${STATUS_KEY[status]})`;
  const text = label ?? STATUS_LABEL[status];

  if (system === "slate") {
    return (
      <span
        className={cn(
          "inline-flex items-center rounded-full border px-2 py-0.5 text-xs font-medium",
          className,
        )}
        style={{
          color,
          background: `color-mix(in oklab, ${color} 14%, transparent)`,
          borderColor: `color-mix(in oklab, ${color} 30%, transparent)`,
        }}
      >
        {text}
      </span>
    );
  }

  if (system === "terminal") {
    return (
      <span className={cn("inline-flex items-center gap-1 font-mono text-xs", className)} style={{ color }}>
        <span aria-hidden>●</span>
        {text}
      </span>
    );
  }

  // editorial: coloured dot + italic label
  return (
    <span className={cn("inline-flex items-center gap-1.5 text-sm", className)} style={{ color }}>
      <span aria-hidden className="inline-block h-1.5 w-1.5 rounded-full" style={{ background: color }} />
      <span className="font-head italic">{text}</span>
    </span>
  );
}
