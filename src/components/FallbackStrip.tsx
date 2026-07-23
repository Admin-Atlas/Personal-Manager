// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { ChatFallback } from "../lib/types";
import { shortModel } from "../lib/format";

/**
 * Map the backend fallback slug (`ChatEvent::Fallback.reason`, e.g. `hard_failure:timeout` /
 * `cooldown`) to a short, plain-voiced "why" clause. Exported for unit testing and so the copy lives
 * in one place. An unknown slug degrades to a generic clause rather than leaking the raw token.
 */
export function fallbackCopy(reason: string): string {
  if (reason === "cooldown") return "your local model was resting after repeated errors";
  if (reason === "power_policy") return "PM switched to the cloud to save power";
  if (reason.startsWith("hard_failure:")) {
    switch (reason.slice("hard_failure:".length)) {
      case "timeout":
        return "your local model timed out";
      case "refused":
        return "your local model wasn't reachable";
      case "model_loading":
        return "your local model was still loading";
      case "malformed_stream":
      case "degenerate_stream":
        return "your local model's reply was unusable";
      case "reply_too_large":
        return "your local model's reply was too large";
      default:
        return "your local model couldn't answer";
    }
  }
  return "your local model couldn't answer";
}

/**
 * The dismissible chat honesty strip (#297): shown when a turn fell back from the user's preferred
 * local endpoint to the cloud. A fell-back reply is REAL (not an error), so this is a caution
 * (`--st-look`), not the red error banner. Sits beside the error banner in both chat hosts.
 */
export function FallbackStrip({
  fallback,
  onDismiss,
}: {
  fallback: ChatFallback;
  onDismiss: () => void;
}) {
  return (
    <div
      role="status"
      className="flex items-center gap-2 border-b px-4 py-2 text-sm"
      style={{
        color: "var(--st-look)",
        borderColor: "color-mix(in oklab, var(--st-look) 40%, transparent)",
        background: "color-mix(in oklab, var(--st-look) 12%, transparent)",
      }}
    >
      <span className="min-w-0 flex-1">
        This reply came from the cloud — {fallbackCopy(fallback.reason)}
        {fallback.to_model ? ` (via ${shortModel(fallback.to_model)})` : ""}.
      </span>
      <button
        type="button"
        onClick={onDismiss}
        title="Dismiss"
        aria-label="Dismiss"
        className="shrink-0 text-ink4 hover:text-ink2"
      >
        ✕
      </button>
    </div>
  );
}
