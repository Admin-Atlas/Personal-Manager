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

/** The games on offer, in the order the menu lists them. */
export const GAME_KINDS: readonly GameKind[] = ["roulette", "straws", "box"];

/** What each game is called, and how it is described where it is chosen. The blurb is the honest
 *  one-liner — every one of these picks uniformly at random; the difference is entirely theatre,
 *  and the copy should never imply otherwise. */
export const GAME_INFO: Record<GameKind, { label: string; blurb: string; verb: string }> = {
  roulette: {
    label: "Roulette",
    blurb: "A wheel with a wedge for every card. Spin it and see where it stops.",
    verb: "Spin",
  },
  straws: {
    label: "Straws",
    blurb: "A fist of straws, one per card. Pull one and find out if it is the long one.",
    verb: "Draw",
  },
  box: {
    label: "Paper in a box",
    blurb: "A folded slip for every card, shaken in a box. Reach in and take one.",
    verb: "Pick",
  },
};

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

/** The cards still in play this round: the pool minus everything already drawn. This is what a
 *  game actually offers, and what the surface should show as available. */
export function livePool(folder: Widget): Widget[] {
  const spent = new Set(folder.spent ?? []);
  return pool(folder).filter((c) => !spent.has(c.id));
}

/** Has this card already been drawn in the round now in progress? (What "greyed out" means.) */
export function isSpent(folder: Widget, childId: string): boolean {
  return (folder.spent ?? []).includes(childId);
}

/**
 * Draw one card from `candidates` using `rnd` — a number in [0, 1), supplied by the caller so the
 * draw is deterministic under test and the randomness has exactly one home in the app.
 *
 * Uniform: every candidate is equally likely. `rnd` is scaled to the list rather than used to walk
 * it, so a value of exactly 1 (which the contract excludes, but a caller could still pass) lands on
 * the last card instead of running off the end.
 */
export function draw(candidates: readonly Widget[], rnd: number): Widget | null {
  if (candidates.length === 0) return null;
  const i = Math.min(candidates.length - 1, Math.max(0, Math.floor(rnd * candidates.length)));
  return candidates[i];
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

/** The fields that hold a folder's game and the round in progress. */
const GAME_FIELDS = ["game", "gameOn", "spent", "autoPopOut"] as const;

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
