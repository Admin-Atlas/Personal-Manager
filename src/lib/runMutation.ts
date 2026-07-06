// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

/** Run an IPC mutation, routing any rejection to `setError` instead of leaving it as a
 *  silent unhandled promise rejection — the click looks like it worked until the next
 *  refetch quietly reverts it (audit F3-8). Clears the error on entry so a fresh attempt
 *  starts clean, and returns whether the mutation succeeded, so callers can gate follow-up
 *  work on it. The busy latch and the post-success refetch stay at the call site; this is
 *  just the try/catch seam the milestone handlers were missing (the Teach view's local
 *  `run` is the fuller busy+error+reload version of the same shape). */
export async function runMutation(
  fn: () => Promise<unknown>,
  setError: (message: string | null) => void,
): Promise<boolean> {
  setError(null);
  try {
    await fn();
    return true;
  } catch (e) {
    setError(String(e));
    return false;
  }
}
