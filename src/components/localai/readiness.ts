// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Pure readings of the stored endpoint, shared by every section that words what it means: which
// server it is, and whether it is on this computer.

import type { RunnerName } from "../../lib/workbenchGuide";

/** The served context below which PM says so, under Assign roles. One filing batch is ~3.5k tokens
 *  of prompt before the reply reserve, so 8192 is the point at which a batch stops being comfortable
 *  rather than the point at which it breaks — a user is better told early than told by the work
 *  quietly getting worse. */
export const COMFORTABLE_WINDOW = 8192;

/** The runner each auto-detected port belongs to — the ports `local_ai.rs` probes. */
const RUNNER_BY_PORT: Record<string, RunnerName> = {
  "11434": "Ollama",
  "1234": "LM Studio",
  "8080": "llama-server",
};

/**
 * Which server an endpoint address is, by its port, or null when the port is not one PM probes.
 *
 * Parsed, never a substring test: ":114341", or "11434" anywhere in a path, must not count. A
 * heuristic all the same — an Ollama on a custom port reads as null and degrades to the steps that
 * suit any server, and anything else on 11434 reads as Ollama and gets a download whose pull fails
 * with a clear error.
 */
export function runnerOf(url: string | null | undefined): RunnerName | null {
  if (!url) return null;
  try {
    return RUNNER_BY_PORT[new URL(url).port] ?? null;
  } catch {
    return null;
  }
}

/**
 * Whether an endpoint address is this computer: `localhost`, `127.*` or `::1`.
 *
 * What lets copy say "on this computer" — anything else is "your model server", because a LAN or
 * remote server receives what PM sends it, and saying otherwise would be the one claim about privacy
 * this tab must never get wrong.
 */
export function isLoopback(url: string | null | undefined): boolean {
  if (!url) return false;
  try {
    const host = new URL(url).hostname.toLowerCase();
    // An address, not a name that happens to start "127." — `127.example.com` is somewhere else.
    // The URL parser has already normalised shorthand like `127.1` to four parts.
    return host === "localhost" || /^127\.\d+\.\d+\.\d+$/.test(host) || host === "[::1]";
  } catch {
    return false;
  }
}

/** "on this computer" for a loopback address, else "on your model server". */
export function whereOf(url: string | null | undefined): string {
  return isLoopback(url) ? "on this computer" : "on your model server";
}
