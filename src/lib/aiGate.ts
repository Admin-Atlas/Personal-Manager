// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { AiProviderStatus } from "./types";

/**
 * Whether PM has an AI provider ready, so the first-run onboarding wizard can be dismissed (#295).
 * ANY one suffices: a cloud key, a configured local endpoint, or an explicit "set up AI later".
 * Pure. Existing keyed users satisfy `has_cloud_key`, so this is a strict SUPERSET of the old
 * key-only gate — no migration, no backfill, and a keyed user who later deletes the key falls back
 * to the wizard exactly as before.
 */
export function aiReady(s: AiProviderStatus): boolean {
  return s.onboarding_done || s.has_cloud_key || s.local_configured;
}
