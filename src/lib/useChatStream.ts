// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useRef, useState } from "react";
import { sendMessage } from "./ipc";
import { useDevMode } from "./capabilities";
import type { GroundingConfidence, Message, PromptMessage } from "./types";

/**
 * Chat send + streaming state, shared by the global chat (App) and the
 * per-project scoped chat (ProjectView). A reply streams over a Tauri `Channel`
 * that can't be cancelled from the frontend and can outlive the view the user is
 * looking at, so every write is gated on whether `convId` is still the displayed
 * conversation: a reply for a chat the user has since left is dropped instead of
 * bleeding into the one now on screen (and leaving its composer stuck disabled).
 * Both chats share this one implementation so the guard can't drift between two
 * hand-rolled copies.
 *
 * `currentConvId` is a live getter for the conversation the caller is currently
 * showing (e.g. `() => activeIdRef.current`).
 */
export function useChatStream(currentConvId: () => number | null) {
  const [messages, setMessages] = useState<Message[]>([]);
  const [streaming, setStreaming] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Developer mode only: the exact assembled request PM sent for a turn, keyed by the assistant
  // message id (from the `done` event). Ephemeral — captured live this session, never persisted — but
  // kept across conversation/tab switches: keyed by id, it only ever renders under its own turn (never a
  // reloaded-from-history one that was never captured), so it survives leaving and re-opening the chat.
  const [prompts, setPrompts] = useState<Record<number, PromptMessage[]>>({});
  // Sibling of `prompts` (card #402): the per-turn grounding-confidence readout, keyed the same way and
  // with the same lifecycle (kept across switches so a captured readout doesn't vanish on a tab switch),
  // for calibrating the gate.
  const [confidences, setConfidences] = useState<Record<number, GroundingConfidence>>({});

  // Keep the getter in a ref so `send` can stay stable across renders.
  const currentRef = useRef(currentConvId);
  currentRef.current = currentConvId;
  // Read the dev toggle through a ref for the same reason — `send` asks the backend to emit the prompt
  // only when Developer mode is on, without taking `devMode` as a dep and re-creating `send`.
  const { devMode } = useDevMode();
  const devModeRef = useRef(devMode);
  devModeRef.current = devMode;

  /** Drop a finished/abandoned stream's transient UI. Call when the displayed
   *  conversation changes (switch, new chat, project change) so a previous
   *  send's streaming bubble and disabled composer don't linger. The dev-only
   *  `prompts`/`confidences` maps are deliberately NOT cleared here — they're keyed
   *  by assistant message id, so they only attach to their own turn, and keeping
   *  them lets a captured readout survive a tab switch or conversation revisit. */
  const clearTransient = useCallback(() => {
    setStreaming(null);
    setSending(false);
    setError(null);
  }, []);

  /** Append the user's message optimistically and stream the assistant reply
   *  into `streaming`. Resolves once the exchange is persisted (whether or not
   *  the user is still viewing it) so the caller can reload persisted state. */
  const send = useCallback(async (convId: number, text: string) => {
    const isCurrent = () => currentRef.current() === convId;
    setError(null);
    const optimistic: Message = {
      id: -Date.now(),
      conversation_id: convId,
      role: "user",
      content: text,
      model: null,
      created_at: new Date().toISOString(),
    };
    setMessages((prev) => [...prev, optimistic]);
    setStreaming("");
    setSending(true);

    let acc = "";
    // Held from the `prompt` event (which fires before the first token) and committed to `prompts`
    // under the assistant message id once `done` delivers it, so the dropdown attaches to its turn.
    let captured: PromptMessage[] | null = null;
    let capturedConfidence: GroundingConfidence | null = null;
    try {
      await sendMessage(
        convId,
        text,
        (event) => {
          // Always accumulate so a resumed view shows the full reply; only write to
          // shared UI state while this is still the conversation on screen.
          if (event.type === "token") {
            acc += event.text;
            if (isCurrent()) setStreaming(acc);
          } else if (event.type === "prompt") {
            captured = event.messages;
            capturedConfidence = event.confidence;
          } else if (event.type === "done") {
            const p = captured;
            if (p && isCurrent()) setPrompts((prev) => ({ ...prev, [event.message_id]: p }));
            const c = capturedConfidence;
            if (c && isCurrent()) setConfidences((prev) => ({ ...prev, [event.message_id]: c }));
          } else if (event.type === "error" && isCurrent()) {
            setError(event.message);
          }
        },
        devModeRef.current,
      );
    } catch (e) {
      if (isCurrent()) setError(String(e));
    } finally {
      if (isCurrent()) {
        setSending(false);
        setStreaming(null);
      }
    }
  }, []);

  return {
    messages,
    setMessages,
    streaming,
    sending,
    error,
    setError,
    clearTransient,
    send,
    prompts,
    confidences,
  };
}
