// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { LocalBlockedRoot } from "../../lib/types";

/**
 * Which sentence the "Already downloaded" panel should say.
 *
 * Pulled out of the component and made pure for one reason: the branch a stock Linux Ollama install
 * actually lands in is the branch that was wrong, and it is unreachable from a render test without
 * standing up a server and a service account. PM has to tell seven genuinely different situations
 * apart; until this card it collapsed them into three, so a machine with a running Ollama holding
 * two models was told "No model folder found for Ollama, LM Studio and Hugging Face".
 *
 * That was not a cosmetic slip. The packaged Linux install runs the service as its own `ollama` user
 * with home `/usr/share/ollama` at mode 0700, so the crawl gets EACCES — and `Path::is_dir()`, which
 * every root probe used to be, reports a permission denial and a missing directory identically. The
 * backend now separates them (`local_disk::probe_root`), and this resolves what to say about it.
 */

/** What the panel knows. Every field is a fact from the backend; nothing is inferred upstream. */
export interface DownloadedInputs {
  /** Models on disk that the endpoint is NOT serving — the only ones worth listing, since a served
   *  model is already assignable above. */
  unservedCount: number;
  /** How many models the endpoint answered with. `null` means nothing answered: not configured,
   *  unreachable, or refused. That is a different fact from `0`, and flattening the two is what
   *  makes a first-time installer indistinguishable from a wrong address. */
  endpointInventory: number | null;
  /** Runners whose model folder the crawl found AND could read. */
  foundRunners: string[];
  /** How many models the crawl found, BEFORE the served ones were filtered out. */
  diskFound: number;
  /** Roots that are there and unreadable. */
  blocked: LocalBlockedRoot[];
}

export type DownloadedState =
  /** There are models to list. */
  | { kind: "list" }
  /** The endpoint holds models and PM can see all of them, so nothing is left unserved to list. */
  | { kind: "endpointHasAll"; count: number }
  /** A runner's folder is here, and everything in it is already being served. */
  | { kind: "allServed"; runners: string[] }
  /** A runner's folder is here and empty — what you see the moment you delete your last model. */
  | { kind: "folderEmpty"; runners: string[] }
  /** A running server with nothing pulled into it — a first-time installer's exact state. */
  | { kind: "endpointEmpty" }
  /** A store PM can prove is there and is not allowed to read. */
  | { kind: "blocked"; root: LocalBlockedRoot }
  /** Nothing found by either rung. The only state that may say "no model folder found". */
  | { kind: "noFolder" };

/**
 * Resolve the panel's state. The order IS the design, so it is written as one flat ladder.
 *
 * `endpointHasAll` outranks every folder branch because the endpoint is the stronger rung: the
 * server is the one process guaranteed to be allowed to read its own store, so when it answers, a
 * folder probe that disagrees is describing PM's permissions rather than the machine.
 *
 * `endpointEmpty` sits BELOW the folder branches, and the asymmetry is deliberate. "The server has
 * nothing" is only the interesting sentence when nothing else has anything either; someone with
 * three LM Studio models and a freshly installed Ollama should be told about the three.
 *
 * `blocked` sits second-to-last rather than first for the same reason: it explains an ABSENCE, so it
 * is only worth saying when there is an absence left to explain. It is what a Linux user sees before
 * they have connected anything — which is exactly when the old copy told them Ollama wasn't there.
 */
export function downloadedState(i: DownloadedInputs): DownloadedState {
  if (i.unservedCount > 0) return { kind: "list" };
  if (i.endpointInventory !== null && i.endpointInventory > 0)
    return { kind: "endpointHasAll", count: i.endpointInventory };
  if (i.foundRunners.length > 0)
    return i.diskFound > 0
      ? { kind: "allServed", runners: i.foundRunners }
      : { kind: "folderEmpty", runners: i.foundRunners };
  if (i.endpointInventory === 0) return { kind: "endpointEmpty" };
  if (i.blocked.length > 0) return { kind: "blocked", root: i.blocked[0] };
  return { kind: "noFolder" };
}
