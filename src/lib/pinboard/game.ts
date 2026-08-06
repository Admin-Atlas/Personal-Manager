// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

/** The Pinboard's gamble games — the rules, pure. No React, no DOM, no randomness of its own:
 *  every draw takes the random number as an argument, so the whole thing is ordinary arithmetic
 *  that a unit test can pin exactly (the same discipline as `grid.ts`).
 *
 *  A folder can be given a GAME. Turn it on and its tile stops opening the cards and starts
 *  playing instead — you click it, something spins, and it names one of the notes inside. That
 *  note is the next thing you do.
 *
 *  A ROUND is the state that makes this more than a coin toss. Every card the game draws is
 *  recorded in the folder's `spent` list; those cards stay in the folder, greyed out, and are not
 *  offered again until every other card has had its turn — at which point the round loops all the
 *  way back and the list empties itself. The list is stored on the folder rather than held in
 *  memory precisely so a round survives closing PM: shut the laptop, come back in an hour, and the
 *  game picks up where it was rather than re-offering what you already did.
 *
 *  TIMELINES ARE NEVER DRAWN. They are legal folder children, but a dated track is not a task
 *  somebody can be told to go and do, so the pool is notes only. A timeline in a game folder is
 *  simply along for the ride. */

import type { Board, GameKind, Widget } from "./types";

/** The games on offer, in the order the menu lists them: the three that pick one card out of all of
 *  them, then the two that put a single card to you and let you play for it. */
export const GAME_KINDS: readonly GameKind[] = ["roulette", "straws", "box", "coin", "rps"];

/**
 * How a game decides.
 *
 * - `draw` — it picks one card out of the whole pool. The card it lands on is the answer.
 * - `verdict` — it offers ONE card and you play a round for it. Lose and you do it; win and it
 *   offers the next one. Either way that card has had its turn this round, so the folder is worked
 *   through rather than the same job being put to you over and over.
 *
 * This is the distinction the original list of six games glossed over. A coin has two faces and a
 * throw of rock-paper-scissors has one winner — neither can choose between seven notes, and
 * pretending otherwise would be a wheel wearing a coin's clothes.
 */
export type GameShape = "draw" | "verdict";

/** What each game is called, how it is described where it is chosen, and how it decides. The blurb
 *  is the honest one-liner: nothing here is skill, and the copy should never imply otherwise. */
export const GAME_INFO: Record<
  GameKind,
  { label: string; blurb: string; verb: string; shape: GameShape; weighted: boolean }
> = {
  roulette: {
    label: "Roulette",
    blurb: "A wheel with a wedge for every card. Spin it and see where it stops.",
    verb: "Spin",
    shape: "draw",
    // The only game with a visible proportion to hand out: a wedge is as wide as its share.
    weighted: true,
  },
  straws: {
    label: "Straws",
    blurb: "A fist of straws, one per card. Pull one and find out if it is the long one.",
    verb: "Draw",
    shape: "draw",
    // A straw's length IS the outcome, so there is nothing left for a weight to change.
    weighted: false,
  },
  box: {
    label: "Paper in a box",
    blurb: "A folded slip for every card, shaken in a box. Reach in and take one.",
    verb: "Pick",
    shape: "draw",
    weighted: false,
  },
  coin: {
    label: "Flip a coin",
    blurb: "One card at a time. Heads you do it, tails you're off the hook.",
    verb: "Flip",
    shape: "verdict",
    // Two faces and one card — no pool to take shares of.
    weighted: false,
  },
  rps: {
    label: "Rock, paper, scissors",
    blurb: "Play PM for it. Win and it offers you something else; lose and the job is yours.",
    verb: "Throw",
    shape: "verdict",
    weighted: false,
  },
};

/** The moves in a throw of rock-paper-scissors. */
export type Throw = "rock" | "paper" | "scissors";
export const THROWS: readonly Throw[] = ["rock", "paper", "scissors"];
export const THROW_LABEL: Record<Throw, string> = {
  rock: "Rock",
  paper: "Paper",
  scissors: "Scissors",
};

/** Who won a throw, from the player's side. A tie is played again on the SAME card — nobody has
 *  won anything, so the card must not be counted as having had its turn. */
export function rpsOutcome(mine: Throw, theirs: Throw): "win" | "lose" | "tie" {
  if (mine === theirs) return "tie";
  const beats: Record<Throw, Throw> = { rock: "scissors", paper: "rock", scissors: "paper" };
  return beats[mine] === theirs ? "win" : "lose";
}

/** The lowest and highest share a card may be given, and the steps offered between them. A weight
 *  is a MULTIPLE of an even share, so 1 is "the same as everyone else" — which is what every card
 *  is worth until somebody says otherwise. */
export const WEIGHT_CHOICES: readonly number[] = [0.25, 0.5, 1, 2, 3];
const MIN_WEIGHT = 0.25;
const MAX_WEIGHT = 3;

/** A card's share of the draw. The one place the range is enforced, so a hand-edited or
 *  out-of-range board can't hand a wheel a negative wedge or an infinite one. */
export function weightOf(card: Widget): number {
  const w = card.weight;
  if (typeof w !== "number" || !Number.isFinite(w)) return 1;
  return Math.min(MAX_WEIGHT, Math.max(MIN_WEIGHT, w));
}

/** Is this folder currently a game folder — i.e. does clicking its tile play rather than open?
 *  Both halves matter: a folder can remember a game while having it switched off, which is how
 *  turning the game off gets you a perfectly ordinary folder back without forgetting your choice. */
export function playsGame(folder: Widget): boolean {
  return folder.kind === "folder" && !!folder.game && folder.gameOn === true;
}

/** Every card a game could ever draw from this folder, greyed or not. */
export function pool(folder: Widget): Widget[] {
  return (folder.children ?? []).filter((c) => c.kind === "note");
}

/**
 * Does this folder keep a round at all?
 *
 * Off (`repeat`) there is no memory between plays: every card is in every draw and the same one can
 * come up twice running. That is a legitimate thing to want — a round is a fairness promise, and
 * sometimes you would rather have the honest coin. It is one flag with three consequences, all
 * routed through here and the two functions below, so nothing can honour it by halves: nothing is
 * recorded as drawn, nothing greys out, and nothing is held back from the next play.
 */
export function keepsRound(folder: Widget): boolean {
  return folder.repeat !== true;
}

/** The cards still in play this round: the pool minus everything already drawn. This is what a
 *  game actually offers, and what the surface should show as available. A folder that repeats has
 *  no round, so its whole pool is always in play — including anything a previously-kept round left
 *  behind in `spent`, which must never quietly shrink a wheel that has stopped keeping one. */
export function livePool(folder: Widget): Widget[] {
  if (!keepsRound(folder)) return pool(folder);
  const spent = new Set(folder.spent ?? []);
  return pool(folder).filter((c) => !spent.has(c.id));
}

/** Has this card already been drawn in the round now in progress? (What "greyed out" means.) */
export function isSpent(folder: Widget, childId: string): boolean {
  return keepsRound(folder) && (folder.spent ?? []).includes(childId);
}

/**
 * Draw one card from `candidates` using `rnd` — a number in [0, 1), supplied by the caller so the
 * draw is deterministic under test and the randomness has exactly one home in the app.
 *
 * `weighted` gives each card a share of the line proportional to {@link weightOf}; without it every
 * card gets the same share. The two agree exactly when no weight has been set, since an untouched
 * card weighs 1 — so turning weighting on for a folder nobody has tuned changes nothing.
 *
 * `rnd` is scaled to the total rather than used to walk the list, and the last card is the
 * fallback, so a value of exactly 1 (which the contract excludes, but a caller could still pass)
 * lands on the last card instead of running off the end.
 */
export function draw(candidates: readonly Widget[], rnd: number, weighted = false): Widget | null {
  if (candidates.length === 0) return null;
  const shares = candidates.map((c) => (weighted ? weightOf(c) : 1));
  const total = shares.reduce((n, s) => n + s, 0);
  const target = Math.min(1, Math.max(0, rnd)) * total;
  let acc = 0;
  for (let i = 0; i < candidates.length; i++) {
    acc += shares[i];
    if (acc > target) return candidates[i];
  }
  return candidates[candidates.length - 1];
}

/** Each candidate's share of the wheel as a fraction of the whole, in the order given — what the
 *  wedges are drawn from, so what you see is exactly what the draw uses. */
export function shares(candidates: readonly Widget[], weighted: boolean): number[] {
  const raw = candidates.map((c) => (weighted ? weightOf(c) : 1));
  const total = raw.reduce((n, s) => n + s, 0);
  return total > 0 ? raw.map((s) => s / total) : raw.map(() => 0);
}

/** Where each wedge sits on the wheel: degrees clockwise from the top, which is where the pointer
 *  is. `mid` is what a spin aims at, and what a wedge's label is laid along. */
export function wedgeAngles(
  fractions: readonly number[],
): { start: number; end: number; mid: number }[] {
  let acc = 0;
  return fractions.map((f) => {
    const start = acc * 360;
    acc += f;
    const end = acc * 360;
    return { start, end, mid: (start + end) / 2 };
  });
}

/**
 * The rotation that brings the wedge at `mid` to rest under the pointer, having turned at least
 * `turns` whole times on the way.
 *
 * The wheel's angle ACCUMULATES rather than being recomputed from zero each spin. Resetting it
 * would make the second spin unwind backwards to reach a wedge earlier in the list, which reads as
 * the wheel changing its mind; and holding one growing number means the CSS transition on the
 * transform is armed at a constant duration and simply runs whenever the number moves — the one
 * shape that reliably animates, since a transition that is switched on in the same style change as
 * the property it animates is at the mercy of how the engine batches that recalculation.
 */
export function spinTo(from: number, mid: number, turns = 4): number {
  const base = from + 360 * Math.max(1, turns);
  // The extra 0–360° that lands the wedge's middle exactly on the pointer.
  const delta = (((-mid - base) % 360) + 360) % 360;
  return base + delta;
}

/**
 * How long each straw is once the fist opens, in pixels.
 *
 * The winner's is always the longest, by a margin nobody has to squint at; every other straw gets a
 * stable length from its position, so a re-render mid-pull can't shuffle them. Deterministic on
 * purpose — the randomness already happened when the card was drawn, and a straw's length is a
 * picture of that answer rather than a second, disagreeing draw.
 */
export function strawHeights(count: number, winner: number, tall = 176, short = 62): number[] {
  const spread = Math.max(1, tall - short - 24);
  return Array.from({ length: count }, (_, i) =>
    i === winner ? tall : short + ((i * 37) % spread),
  );
}

/**
 * The `spent` list after `childId` is drawn.
 *
 * When that leaves nothing in play the round is over and this returns an EMPTY list — the loop all
 * the way back. Doing it here, at the moment of the draw, rather than lazily at the next one, is
 * what makes the stored round honest: a folder is never left persisted in a state where every card
 * is greyed out, which would read as "this game is finished" to anyone re-opening it later.
 */
export function markDrawn(folder: Widget, childId: string): string[] {
  const spent = [...new Set([...(folder.spent ?? []), childId])];
  const inPlay = pool(folder).filter((c) => !spent.includes(c.id));
  return inPlay.length === 0 ? [] : spent;
}

/**
 * Drop ids the folder no longer holds.
 *
 * A card that was popped out, deleted, or dragged somewhere else is not "already drawn" — it is
 * gone, and remembering it would shrink a later round for no reason a user could see. Also the
 * cleanup for the whole feature: nothing else prunes the list, so a folder that is played, emptied
 * and refilled over months never accumulates dead ids in the stored board.
 *
 * Returns the SAME array when nothing needs dropping, so a caller can skip a write.
 */
export function pruneSpent(folder: Widget): string[] {
  const spent = folder.spent ?? [];
  if (spent.length === 0) return spent;
  const ids = new Set(pool(folder).map((c) => c.id));
  const kept = spent.filter((id) => ids.has(id));
  return kept.length === spent.length ? spent : kept;
}

/**
 * The same folder with its round pruned to the cards it still holds — the form to reach for at any
 * site that takes a card OUT of a folder.
 *
 * {@link pruneSpent} ran in only two places: on the way in from the store, and on the draw that pops
 * its own winner. Every OTHER way a card leaves — popped out by hand, deleted, dragged onto the
 * board — left its id behind in the round, so the legend counted a card that was no longer there
 * and a folder with one card left in a finished round read "-1 of 1 still in". The count is derived
 * from the two lists, so they have to be maintained together; doing it here is what makes that
 * structural rather than a rule every future call site has to remember.
 *
 * Returns the SAME folder object when nothing needs dropping, so a caller can skip a write — and
 * never ADDS a `spent` field to a folder that has none, which would put a game's bookkeeping on a
 * plain folder and change the stored board for nothing.
 */
export function withPrunedRound(folder: Widget): Widget {
  const spent = folder.spent;
  if (!spent?.length) return folder;
  const kept = pruneSpent(folder);
  return kept === spent ? folder : { ...folder, spent: kept };
}

/** A card's name in a game: its title, else the first line of its text with any list or heading
 *  marker taken off, else a placeholder. A wedge or a straw has room for a handful of words, and
 *  "- [ ] ring the dentist" should read as "ring the dentist". */
export function cardLabel(w: Widget): string {
  const title = (w.title ?? "").trim();
  if (title) return title;
  const first = (w.text ?? "")
    .split("\n")
    .map((l) => l.trim())
    .find(Boolean);
  const stripped = (first ?? "")
    .replace(/^(#{1,6}|[-*+.>]|\d+\.|[ivxIVX]+\.|\[[ xX]?\])\s+/, "")
    .trim();
  return stripped || "Untitled note";
}

/** The fields that hold a folder's game and the round in progress. Exported because
 *  `commitForPatch` has to recognise a patch that only touches them — see {@link carryGameState}
 *  for why a change to one of these cannot be undone, and therefore must not offer to be. */
export const GAME_FIELDS = ["game", "gameOn", "spent", "autoPopOut", "repeat"] as const;

/**
 * Re-graft the LIVE game state onto a board restored by undo or redo.
 *
 * The pinboard's history is a stack of whole boards (`history.ts`), so every snapshot carries
 * whatever the round happened to look like when it was taken. Without this, undoing something
 * entirely unrelated — a tint, a title keystroke, a card nudged an inch — would roll the round back
 * with it and un-grey cards you had already drawn, mid-game. Marking the draw itself silent cannot
 * fix that: the stale value rides in *other* changes' snapshots, not the draw's.
 *
 * So a round is deliberately NOT part of the undoable document. It is a record of things that
 * already happened, like the note-ingest stamps `commitForPatch` excludes for the same reason —
 * undo cannot un-draw a card any more than it can un-file a document. Folders that the restored
 * board doesn't contain are left alone; there is nothing live to carry onto them.
 */
export function carryGameState(restored: Board, live: Board): Board {
  const liveFolders = new Map(
    live.widgets.filter((w) => w.kind === "folder").map((w) => [w.id, w] as const),
  );
  let changed = false;
  const widgets = restored.widgets.map((w) => {
    if (w.kind !== "folder") return w;
    const now = liveFolders.get(w.id);
    if (!now) return w;
    const next = { ...w };
    for (const f of GAME_FIELDS) {
      if (now[f] === undefined) delete next[f];
      else Object.assign(next, { [f]: now[f] });
    }
    if (GAME_FIELDS.every((f) => next[f] === w[f])) return w;
    changed = true;
    return next;
  });
  return changed ? { ...restored, widgets } : restored;
}
