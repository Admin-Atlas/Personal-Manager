// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// How the thinking fold's header words its timing. Pure and kept apart from ThinkingBlock.tsx, so
// that file exports components only (fast refresh) and these are testable without a DOM.

/** Whole seconds as the header words them: "48 s", "1 min", "1 min 12 s". */
export function formatThoughtSeconds(s: number): string {
  if (s < 60) return `${s} s`;
  const rest = s % 60;
  return `${Math.floor(s / 60)} min${rest > 0 ? ` ${rest} s` : ""}`;
}

/** The fold's header. Live and not yet answering, it counts up; after that it says how long it took.
 *  `seconds` is null when nothing was timed. */
export function thinkingTitle(t: {
  live: boolean;
  answered: boolean;
  seconds: number | null;
}): string {
  if (t.live && !t.answered) {
    return t.seconds !== null && t.seconds >= 1
      ? `Thinking… ${formatThoughtSeconds(t.seconds)}`
      : "Thinking…";
  }
  if (t.seconds === null) return "Thought";
  return t.seconds < 1
    ? "Thought for under a second"
    : `Thought for ${formatThoughtSeconds(t.seconds)}`;
}
