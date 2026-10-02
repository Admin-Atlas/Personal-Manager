// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useRef, useState } from "react";
import { sendMessage } from "./ipc";
import { useDevMode } from "./capabilities";
import { readShowThinking } from "./chatPrefs";
import type {
  ChatFallback,
  ChatThought,
  GroundingConfidence,
  LiveThought,
  Message,
  PromptMessage,
  ServedBy,
  ThoughtFold,
} from "./types";

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
  // Which provider actually answered each turn ("local"/"cloud"), keyed by assistant message id (from
  // the `done` event) — the source for ChatView's per-message "via <model> - local/cloud" footer. Same
  // lifecycle as `prompts`/`confidences`: captured live, never persisted, kept across switches (it only
  // renders under its own turn, so a reloaded-from-history turn simply has no entry and shows model-only).
  // A turn the On battery policy moved is stored as "cloud-on-battery" (#432): it is a choice the
  // footer words, not a fallback, so it never sets the strip below.
  const [providers, setProviders] = useState<Record<number, ServedBy>>({});
  // Transient like `error`: set when a turn fell back from the preferred local endpoint to cloud (the
  // `fallback` event, which arrives after the tokens and before `done`). Rendered as the dismissible
  // FallbackStrip and cleared on dismiss / next send / conversation switch — NOT on `done`.
  const [fallback, setFallback] = useState<ChatFallback | null>(null);
  // The chat Thinking toggle's output. `streamingThought` is the live thinking while a reply streams,
  // transient like `streaming`. `thoughts` has the same lifecycle as `prompts`: keyed by assistant
  // message id, committed on `done`, never persisted, kept across switches. Thinking is model output
  // and never saved, so a reloaded turn has none.
  const [streamingThought, setStreamingThought] = useState<LiveThought | null>(null);
  const [thoughts, setThoughts] = useState<Record<number, ChatThought>>({});
  // Where the user left the live thinking fold, per conversation: one reply streams per conversation
  // at a time, so the conversation names the turn until `done` gives it a message id. The live fold
  // and the settled one are separate mounts, with a gap between them while the messages reload, so
  // without this a fold opened to read mid-answer snapped shut and lost its place as the turn
  // settled. A ref, not state: nothing renders from it until a fold mounts and seeds from it (through
  // `streamingThought` and the committed thought). An entry is dropped when its conversation's next
  // send starts, and nothing reads it in between.
  const liveFolds = useRef(new Map<number, ThoughtFold>());

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
   *  them lets a captured readout survive a tab switch or conversation revisit.
   *  `thoughts` is kept for the same reason; only the live `streamingThought` goes. */
  const clearTransient = useCallback(() => {
    setStreaming(null);
    setStreamingThought(null);
    setSending(false);
    setError(null);
    setFallback(null);
  }, []);

  /** Dismiss the fallback honesty strip (the user has seen it). Transient-only; a new send or a
   *  conversation switch also clears it. */
  const dismissFallback = useCallback(() => setFallback(null), []);

  /** The live thinking fold reporting where the user left it (ChatView's `onLiveFold`). Only the
   *  conversation on screen has a live fold, so that is the one it belongs to. */
  const noteLiveFold = useCallback((fold: ThoughtFold) => {
    const id = currentRef.current();
    if (id !== null) liveFolds.current.set(id, fold);
  }, []);

  /** Append the user's message optimistically and stream the assistant reply
   *  into `streaming`. Resolves once the exchange is persisted (whether or not
   *  the user is still viewing it) so the caller can reload persisted state.
   *
   *  Resolves **`true` only when the exchange actually completed** — the type-ahead queue (#152)
   *  must stop rather than send a second user turn after a failed one, which the backend's
   *  alternation guard would reject anyway. Tracked in a local, not from `error` state: an error
   *  arriving for a conversation the user has since left never reaches `setError`, but it is still
   *  a failure the queue has to respect. Never rejects. */
  const send = useCallback(async (convId: number, text: string): Promise<boolean> => {
    const isCurrent = () => currentRef.current() === convId;
    let ok = true;
    setError(null);
    setFallback(null);
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
    setStreamingThought(null);
    setSending(true);

    let acc = "";
    // Held from the `prompt` event (which fires before the first token) and committed to `prompts`
    // under the assistant message id once `done` delivers it, so the dropdown attaches to its turn.
    let captured: PromptMessage[] | null = null;
    let capturedConfidence: GroundingConfidence | null = null;
    // This turn's thinking, accumulated the same way as `acc` and committed to `thoughts` on `done`.
    // `skipped` is a `no_room` note: the turn was answered without thinking, and the UI says so.
    let live: LiveThought | null = null;
    let skipped = false;
    liveFolds.current.delete(convId);
    // Publish the live thought with where the user left its fold, so a fold that mounts again (the
    // user came back mid-reply) opens where they left it.
    const showThought = () => {
      if (live && isCurrent())
        setStreamingThought({ ...live, fold: liveFolds.current.get(convId) });
    };
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
            // The first answer token stops the thinking clock — once per turn. The thought is
            // published on every token, not only that one: a view the user left and came back to
            // mid-answer had it cleared, and would otherwise lose the fold until `done`.
            if (live && live.answeredAt === null) live = { ...live, answeredAt: Date.now() };
            showThought();
          } else if (event.type === "thinking") {
            live = live
              ? { ...live, text: live.text + event.text }
              : { text: event.text, startedAt: Date.now(), answeredAt: null };
            // `streaming` too: ChatView draws the live block inside the streaming column, and a view
            // the user left and came back to mid-thought had both cleared — without this it would
            // stay blank until the first answer token, which can be minutes away.
            if (isCurrent()) setStreaming(acc);
            showThought();
          } else if (event.type === "thinking_note") {
            if (event.note === "fell_back") {
              // The local model's thinking, under a reply the cloud is about to give: drop it.
              live = null;
              if (isCurrent()) setStreamingThought(null);
            } else if (event.note === "no_room") {
              skipped = true;
            }
          } else if (event.type === "prompt") {
            captured = event.messages;
            capturedConfidence = event.confidence;
          } else if (event.type === "done") {
            const p = captured;
            if (p && isCurrent()) setPrompts((prev) => ({ ...prev, [event.message_id]: p }));
            const c = capturedConfidence;
            if (c && isCurrent()) setConfidences((prev) => ({ ...prev, [event.message_id]: c }));
            const t: LiveThought | null = live;
            // Only a reply the local model gave carries a thought. A cloud reply never sits under
            // local thinking (`fell_back` already dropped any that was shown), nor under "Answered
            // without thinking": a `no_room` turn whose local leg then failed before answering was
            // answered by the cloud, which never thinks here, so neither that line's reason nor its
            // fix applies to it.
            if ((t || skipped) && event.served_by === "local" && isCurrent()) {
              const thought: ChatThought = {
                text: t?.text ?? "",
                seconds: t
                  ? Math.max(0, Math.floor(((t.answeredAt ?? Date.now()) - t.startedAt) / 1000))
                  : null,
                skipped,
                fold: liveFolds.current.get(convId),
              };
              setThoughts((prev) => ({ ...prev, [event.message_id]: thought }));
            }
            if (isCurrent()) {
              const servedBy: ServedBy = event.on_battery ? "cloud-on-battery" : event.served_by;
              setProviders((prev) => ({ ...prev, [event.message_id]: servedBy }));
            }
          } else if (event.type === "fallback") {
            if (isCurrent())
              setFallback({
                from_model: event.from_model,
                to_model: event.to_model,
                reason: event.reason,
              });
          } else if (event.type === "error") {
            ok = false;
            if (isCurrent()) setError(event.message);
          }
        },
        devModeRef.current,
        // Read at send time, synchronously, so `send` needs no dep on it: a queued message goes with
        // the toggle as it stands the moment it is sent, not as it stood when it was typed.
        readShowThinking(),
      );
    } catch (e) {
      ok = false;
      if (isCurrent()) setError(String(e));
    } finally {
      if (isCurrent()) {
        setSending(false);
        setStreaming(null);
        setStreamingThought(null);
      }
    }
    return ok;
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
    providers,
    fallback,
    dismissFallback,
    streamingThought,
    thoughts,
    noteLiveFold,
  };
}
