// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { Message } from "./types";
import { formatDate } from "./format";

/** A chat is "idle" past this gap — reopening it offers a clean start (board card 7E). 24h ≈ "a
 *  different day's train of thought", so continuing it silently would blur two conversations. */
export const IDLE_PROMPT_MS = 24 * 60 * 60 * 1000;

/** The power-user word triggers that start a fresh chat instead of sending a message. Typing one of
 *  these (and nothing else) is the keyboard parity for the "+ New chat" button — it must never reach
 *  the model or the vault, so it's matched before the send path. Case-/space-insensitive. */
export function isNewChatTrigger(text: string): boolean {
  const t = text.trim().toLowerCase();
  return t === "/new" || t === "/done";
}

/** The date a loaded conversation went idle (its last activity, as DD-MM-YYYY), or null when it's
 *  fresh enough to just continue. Pure: the newest message older than the idle gap ⇒ reopening this
 *  is resuming a stale thread, so the UI can offer a clean start. Empty/mid-stream ⇒ null. */
export function idleSince(messages: Message[], now: number): string | null {
  const last = messages[messages.length - 1];
  if (!last) return null;
  const t = new Date(last.created_at).getTime();
  if (Number.isNaN(t) || now - t < IDLE_PROMPT_MS) return null;
  return formatDate(last.created_at);
}
