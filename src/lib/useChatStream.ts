// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useRef, useState } from "react";
import { sendMessage } from "./ipc";
import type { Message } from "./types";

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

  // Keep the getter in a ref so `send` can stay stable across renders.
  const currentRef = useRef(currentConvId);
  currentRef.current = currentConvId;

  /** Drop a finished/abandoned stream's transient UI. Call when the displayed
   *  conversation changes (switch, new chat, project change) so a previous
   *  send's streaming bubble and disabled composer don't linger. */
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
    try {
      await sendMessage(convId, text, (event) => {
        // Always accumulate so a resumed view shows the full reply; only write to
        // shared UI state while this is still the conversation on screen.
        if (event.type === "token") {
          acc += event.text;
          if (isCurrent()) setStreaming(acc);
        } else if (event.type === "error" && isCurrent()) {
          setError(event.message);
        }
      });
    } catch (e) {
      if (isCurrent()) setError(String(e));
    } finally {
      if (isCurrent()) {
        setSending(false);
        setStreaming(null);
      }
    }
  }, []);

  return { messages, setMessages, streaming, sending, error, setError, clearTransient, send };
}
