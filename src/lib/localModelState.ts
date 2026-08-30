// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { LocalLlmStatus } from "./types";

/** Which of PM's two AI roles a footer row is speaking for. */
export type LocalRole = "chat" | "background";

/**
 * What is happening to a role's local model right now, or `null` when there is nothing to say.
 *
 * `null` covers three different silences on purpose, because all three should render identically —
 * as the row already rendered before any of this existed: no endpoint configured, the role routed to
 * cloud, or an endpoint PM cannot ask. The last is the one worth naming: `/api/ps` is Ollama's route,
 * and llama-server, LM Studio and a `/v1`-only proxy do not have it. "PM cannot tell" must never
 * collapse into "not loaded" — that inversion would tell those users their model is unloaded, every
 * second of the time it is in fact resident.
 */
export type LocalModelActivity = "answering" | "loaded" | "released" | "unloaded";

/** The state of one role's local model, as a pure reading of the status snapshot. */
export function localModelActivity(
  status: LocalLlmStatus | null,
  role: LocalRole,
): LocalModelActivity | null {
  if (!status || !status.configured) return null;
  const chat = role === "chat";
  // No local model for this role means the row is naming a CLOUD model, and none of this applies.
  if (!(chat ? status.chat_local_model : status.background_local_model)) return null;
  // In flight outranks everything below it: PM knows this from its own slot, with no server to ask
  // and nothing to be stale about, and it is the one state the user is watching for.
  if (chat ? status.chat_answering : status.background_answering) return "answering";
  const loaded = chat ? status.chat_loaded : status.background_loaded;
  if (loaded === null) return null;
  if (loaded) return "loaded";
  // Not loaded, and PM's own housekeeping is why. Worth separating from the server's own eviction:
  // one is a setting the user chose and can change, the other is their server's business.
  return (chat ? status.chat_released : status.background_released) ? "released" : "unloaded";
}

/** The word each state gets. One table, so two surfaces cannot word the same fact differently. */
export const ACTIVITY_LABEL: Record<LocalModelActivity, string> = {
  answering: "answering",
  loaded: "loaded",
  released: "released",
  unloaded: "not loaded",
};

/** The sentence behind each state — what it means, and what it costs you. */
export const ACTIVITY_DETAIL: Record<LocalModelActivity, string> = {
  answering: "PM is asking this model right now.",
  loaded: "Loaded on your graphics card, so the next message starts answering straight away.",
  released:
    "PM handed the memory back, as your release setting asks. The next message loads it again, which takes a few seconds.",
  unloaded:
    "Your server isn't holding it at the moment. The next message loads it, which takes a few seconds.",
};
