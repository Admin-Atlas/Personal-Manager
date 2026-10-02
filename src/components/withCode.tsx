// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

/** Render a step string, turning `backtick` spans into inline code chips.
 *
 *  Shared by every guide that writes its steps as plain strings — the document-engine setup popup
 *  and the local-server guides in Settings › Local AI. Those strings mark their commands with
 *  backticks, and a guide that prints the backticks themselves is asking someone to copy them. */
export function withCode(text: string) {
  return text.split("`").map((part, i) =>
    i % 2 === 1 ? (
      <code
        key={i}
        className="rounded-[var(--radius-sm)] bg-bg px-1 py-0.5 font-mono text-[0.85em] text-ink"
      >
        {part}
      </code>
    ) : (
      <span key={i}>{part}</span>
    ),
  );
}
