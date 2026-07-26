// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The cadence rules for keeping the cloud connectors fresh in the background.
//
// Until now nothing refreshed a connector on its own. The only entry points were the Sync button in
// Connectors and a crash-recovery resume at unlock — and that resume only FINISHES an interrupted
// walk, it never starts a fresh one. So a file added to an indexed folder was invisible until the
// user happened to press a button they had no reason to think they needed. The calendar has had a
// 15-minute poll since it shipped; this gives the connectors the same treatment.
//
// WHY POLLING AND NOT PUSH: Google Drive's `changes.watch` and Microsoft Graph's subscriptions both
// deliver to a publicly reachable HTTPS webhook on a verified domain. A local-first desktop app has
// no such endpoint, so push is structurally unavailable — not a shortcut we're taking.
//
// Polling is cheap anyway for almost everything here: My Drive, shared drives and OneDrive all ride
// a delta cursor, so a poll with nothing to report is one request returning an empty page.
//
// The exception is Drive's "Shared with me", which has NO delta feed. Each pass re-enumerates every
// picked root and reconciles it, which is real work — so it gets its own, much longer interval, and
// the frequent passes skip it explicitly.

/** How often the delta-backed corpora are polled. Matches the calendar's cadence deliberately: two
 *  different background rhythms would be harder to reason about than one, and 15 minutes is already
 *  proven acceptable in this app. */
export const CONNECTOR_POLL_MS = 15 * 60 * 1000;

/** How often the full shared-with-me re-walk runs. */
export const SHARED_WITH_ME_POLL_MS = 60 * 60 * 1000;

/**
 * Whether this tick should also re-walk shared-with-me.
 *
 * `lastAt` is null before the first pass of a session, which returns true: the launch pass looks at
 * everything, so opening PM is always a complete refresh. That is the behaviour worth protecting —
 * "every open is up to date" is the point of the whole feature, and a partial launch pass would
 * leave a newly-shared file invisible for an hour.
 */
export function shouldIncludeSharedWithMe(
  lastAt: number | null,
  now: number,
  everyMs: number = SHARED_WITH_ME_POLL_MS,
): boolean {
  if (lastAt === null) return true;
  return now - lastAt >= everyMs;
}
