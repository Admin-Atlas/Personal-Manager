// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { LocalLlmStatus } from "../lib/types";
import { localEndpointState, LOCAL_STATE_TOKEN } from "../lib/localStatus";

/**
 * A compact "Local · <state>" pill for the chat composer, next to the context meter. Renders
 * NOTHING (zero pixels) unless a local endpoint is configured — a cloud-only user sees no change at
 * all. States mirror the Local AI tab's status chip: connected / resting (using cloud) / unreachable.
 */
export function ProviderChip({ status }: { status: LocalLlmStatus | null }) {
  const state = localEndpointState(status);
  if (state === null) return null;

  const label =
    state === "connected"
      ? "connected"
      : state === "resting"
        ? "resting (using cloud)"
        : "unreachable";
  const token = LOCAL_STATE_TOKEN[state];

  return (
    <span
      className="inline-flex items-center gap-1 rounded-[var(--radius-sm)] px-1.5 py-0.5 text-[10px] font-medium"
      style={{
        color: `var(${token})`,
        background: `color-mix(in oklab, var(${token}) 15%, transparent)`,
      }}
      title="Local model endpoint status — manage it in Settings → Local AI"
    >
      <span className="font-mono opacity-70">Local</span>
      <span>·</span>
      {label}
    </span>
  );
}
