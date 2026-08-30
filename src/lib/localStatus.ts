// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { LocalLlmStatus } from "./types";

/** The tri-state health of a configured local endpoint. `resting` = inside the dead-host cooldown
 *  (repeated failures), during which background work goes to cloud until it lifts. */
export type LocalEndpointState = "connected" | "resting" | "unreachable";

/**
 * Classify a local-endpoint status snapshot into its health, or `null` when no endpoint is
 * configured — the caller renders NOTHING in that case (the zero-pixel contract: a cloud-only user
 * never sees the provider surfaces). Pure, so the chat sidebar line, the composer ProviderChip and
 * the tests all agree on the mapping.
 */
export function localEndpointState(status: LocalLlmStatus | null): LocalEndpointState | null {
  if (!status || !status.configured) return null;
  if (status.in_cooldown) return "resting";
  return status.reachable ? "connected" : "unreachable";
}

/**
 * The words each endpoint state gets.
 *
 * One table because the two surfaces reading it had already drifted: the composer chip said
 * "resting (using cloud)" while the sidebar line said "resting - using cloud", and the test that
 * exists to stop exactly that asserted only the substring both happened to share. A shared
 * classifier does not make shared copy; this does.
 */
export const LOCAL_STATE_LABEL: Record<LocalEndpointState, string> = {
  connected: "connected",
  resting: "resting (using cloud)",
  unreachable: "unreachable",
};

/** The design-system status token (colour var) for each state: quick=green, look=amber, due=red. */
export const LOCAL_STATE_TOKEN: Record<LocalEndpointState, string> = {
  connected: "--st-quick",
  resting: "--st-look",
  unreachable: "--st-due",
};
