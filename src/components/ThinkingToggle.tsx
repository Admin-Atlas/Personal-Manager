// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from "react";
import type { LocalLlmStatus } from "../lib/types";
import { readShowThinking, writeShowThinking } from "../lib/chatPrefs";

export const THINKING_TITLE_OFF =
  "Show your model's thinking above each reply. On Ollama this also lets the model think before it answers, so replies take longer — often tens of seconds.";
export const THINKING_TITLE_ON =
  "Thinking is on: your model's thinking appears above each reply. Click to turn it off — on Ollama, replies come back quicker without it.";

// Each state carries its WHOLE class string: `cn` is a plain joiner, not tailwind-merge, so an
// "override" class appended for the on state would sit beside the off one and the stylesheet's order
// would pick the winner. The edge is `--ink4` in BOTH states — measured across every System × Mode ×
// Accent at both Contrast levels it clears 4.89:1 against `--bg`/`--panel`, while an accent-coloured
// edge falls to 1.36:1 on a pale accent. The dot marks the on state by shape, not by fill contrast.
const OFF_CLASS =
  "inline-flex shrink-0 items-center gap-1 rounded-[var(--radius-sm)] border border-ink4 px-2 py-1 text-xs text-ink3 transition-colors hover:text-ink";
const ON_CLASS =
  "inline-flex shrink-0 items-center gap-1 rounded-[var(--radius-sm)] border border-ink4 bg-accent px-2 py-1 text-xs text-accent-ink transition-colors";

/**
 * The chat composer's Thinking button: when on, the next send asks the local model to think and
 * streams that thinking into a fold above the reply. Per-device (lib/chatPrefs), read by the chat at
 * send time — never a backend Setting, and background work never sees it.
 */
export function ThinkingToggle({ status }: { status: LocalLlmStatus | null }) {
  const [on, setOn] = useState(readShowThinking);
  // Follows writes from elsewhere — the other chat surface's button, and General's "Reset General",
  // which runs in the Settings overlay over this still-mounted composer.
  useEffect(() => {
    const onChanged = () => setOn(readShowThinking());
    window.addEventListener("pm:settings-changed", onChanged);
    return () => window.removeEventListener("pm:settings-changed", onChanged);
  }, []);
  // Zero pixels unless chat is really going to a local model: null on cloud and while On battery
  // has moved chat, so a cloud-only user sees nothing. Shown at EVERY depth when eligible: it can
  // make replies a minute slower, and a hidden-but-on control would leave that unexplained.
  if (!status?.chat_local_model) return null;

  return (
    <button
      type="button"
      aria-pressed={on}
      data-help="chat-thinking-toggle"
      title={on ? THINKING_TITLE_ON : THINKING_TITLE_OFF}
      onClick={() => {
        writeShowThinking(!on);
        // Show what was stored, not what was asked for: the next send reads the stored pref, and
        // a write that failed (storage full or blocked) leaves it off.
        setOn(readShowThinking());
      }}
      className={on ? ON_CLASS : OFF_CLASS}
    >
      {on && (
        <span
          aria-hidden
          className="h-1.5 w-1.5 shrink-0 rounded-full"
          style={{ background: "currentColor" }}
        />
      )}
      Thinking
    </button>
  );
}
