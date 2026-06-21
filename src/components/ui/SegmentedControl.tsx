// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A small segmented toggle. Generalises ReviewView's importance picker and backs the Settings
// System / Mode / Depth pickers. Active segment = accent fill; the group is one token-bordered
// strip.

import type { ReactNode } from "react";
import { cn } from "./cn";

export interface SegOption<T extends string> {
  value: T;
  label: ReactNode;
  title?: string;
}

export interface SegmentedControlProps<T extends string> {
  options: ReadonlyArray<SegOption<T>>;
  value: T;
  onChange: (value: T) => void;
  className?: string;
}

export function SegmentedControl<T extends string>({
  options,
  value,
  onChange,
  className,
}: SegmentedControlProps<T>) {
  return (
    <div
      className={cn(
        "inline-flex items-center gap-0.5 rounded-[var(--radius-sm)] border border-border2 p-0.5",
        className,
      )}
      role="group"
    >
      {options.map((opt) => {
        const active = opt.value === value;
        return (
          <button
            key={opt.value}
            type="button"
            title={opt.title}
            aria-pressed={active}
            onClick={() => onChange(opt.value)}
            className={cn(
              "rounded-[var(--radius-sm)] px-2.5 py-1 text-xs transition",
              active ? "bg-accent text-accent-ink font-medium" : "text-ink3 hover:text-ink",
            )}
          >
            {opt.label}
          </button>
        );
      })}
    </div>
  );
}
