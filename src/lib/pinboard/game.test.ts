// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import {
  cardLabel,
  carryGameState,
  draw,
  GAME_INFO,
  GAME_KINDS,
  isSpent,
  keepsRound,
  livePool,
  markDrawn,
  playsGame,
  pool,
  pruneSpent,
  rpsOutcome,
  shares,
  spinTo,
  strawHeights,
  THROWS,
  wedgeAngles,
  weightOf,
} from "./game";
import { BOARD_VERSION } from "./types";
import type { Board, Widget } from "./types";

const R = { x: 0, y: 0, w: 4, h: 3 };

const note = (id: string, extra: Partial<Widget> = {}): Widget => ({
  id,
  kind: "note",
  rect: R,
  ...extra,
});
const timeline = (id: string): Widget => ({ id, kind: "timeline", rect: R, items: [] });
const folder = (id: string, children: Widget[], extra: Partial<Widget> = {}): Widget => ({
  id,
  kind: "folder",
  rect: { x: 0, y: 0, w: 3, h: 3 },
  children,
  ...extra,
});
const board = (widgets: Widget[]): Board => ({ version: BOARD_VERSION, widgets });

describe("playsGame — both halves are required", () => {
  it("is false for a plain folder", () => {
    expect(playsGame(folder("f", []))).toBe(false);
  });

  it("is false when a game is chosen but switched off — the folder just opens its cards", () => {
    expect(playsGame(folder("f", [], { game: "roulette" }))).toBe(false);
    expect(playsGame(folder("f", [], { game: "roulette", gameOn: false }))).toBe(false);
  });

  it("is false when switched on with no game chosen", () => {
    expect(playsGame(folder("f", [], { gameOn: true }))).toBe(false);
  });

  it("is true only with both", () => {
    expect(playsGame(folder("f", [], { game: "roulette", gameOn: true }))).toBe(true);
  });

  it("is false for a note, whatever it carries", () => {
    expect(playsGame(note("n", { game: "box", gameOn: true }))).toBe(false);
  });
});

describe("pool — notes only", () => {
  it("never offers a timeline, which is not a task you can be told to do", () => {
    const f = folder("f", [note("a"), timeline("t"), note("b")]);
    expect(pool(f).map((w) => w.id)).toEqual(["a", "b"]);
  });

  it("is empty for a folder with no children at all", () => {
    expect(pool(folder("f", []))).toEqual([]);
  });
});

describe("livePool — the round in progress", () => {
  it("drops what has already been drawn", () => {
    const f = folder("f", [note("a"), note("b"), note("c")], { spent: ["b"] });
    expect(livePool(f).map((w) => w.id)).toEqual(["a", "c"]);
  });

  it("ignores a spent id the folder no longer holds", () => {
    const f = folder("f", [note("a")], { spent: ["gone"] });
    expect(livePool(f).map((w) => w.id)).toEqual(["a"]);
  });

  it("is the whole pool when nothing has been drawn", () => {
    const f = folder("f", [note("a"), note("b")]);
    expect(livePool(f)).toHaveLength(2);
  });
});

describe("weightOf — the range is enforced in one place", () => {
  it("is 1 for a card nobody has tuned, so an untouched folder draws evenly", () => {
    expect(weightOf(note("a"))).toBe(1);
  });

  it("takes a real weight as given", () => {
    expect(weightOf(note("a", { weight: 0.5 }))).toBe(0.5);
    expect(weightOf(note("a", { weight: 3 }))).toBe(3);
  });

  it("clamps a hand-edited board rather than handing a wheel a silly wedge", () => {
    expect(weightOf(note("a", { weight: 0 }))).toBe(0.25);
    expect(weightOf(note("a", { weight: -5 }))).toBe(0.25);
    expect(weightOf(note("a", { weight: 9000 }))).toBe(3);
  });

  it("falls back to 1 on anything that isn't a finite number", () => {
    expect(weightOf(note("a", { weight: Number.NaN }))).toBe(1);
    expect(weightOf(note("a", { weight: Number.POSITIVE_INFINITY }))).toBe(1);
    expect(weightOf({ ...note("a"), weight: "2" } as unknown as Widget)).toBe(1);
  });
});

describe("shares — what the wheel draws is what the wheel uses", () => {
  it("splits evenly when weighting is off, whatever the cards say", () => {
    const cards = [note("a", { weight: 3 }), note("b"), note("c", { weight: 0.5 })];
    expect(shares(cards, false)).toEqual([1 / 3, 1 / 3, 1 / 3]);
  });

  it("gives each card its share of the total when weighting is on", () => {
    const cards = [note("a", { weight: 3 }), note("b", { weight: 1 })];
    expect(shares(cards, true)).toEqual([0.75, 0.25]);
  });

  it("always sums to one", () => {
    const cards = [note("a", { weight: 0.25 }), note("b", { weight: 2 }), note("c")];
    const total = shares(cards, true).reduce((n, s) => n + s, 0);
    expect(total).toBeCloseTo(1, 10);
  });

  it("returns no shares for no cards rather than dividing by zero", () => {
    expect(shares([], true)).toEqual([]);
  });
});

describe("draw — uniform, and the caller owns the randomness", () => {
  const cards = [note("a"), note("b"), note("c"), note("d")];

  it("splits [0,1) evenly across the candidates", () => {
    expect(draw(cards, 0)?.id).toBe("a");
    expect(draw(cards, 0.24)?.id).toBe("a");
    expect(draw(cards, 0.25)?.id).toBe("b");
    expect(draw(cards, 0.5)?.id).toBe("c");
    expect(draw(cards, 0.99)?.id).toBe("d");
  });

  it("lands on the last card rather than off the end when handed exactly 1", () => {
    expect(draw(cards, 1)?.id).toBe("d");
  });

  it("clamps a negative to the first card", () => {
    expect(draw(cards, -0.5)?.id).toBe("a");
  });

  it("returns null for an empty pool", () => {
    expect(draw([], 0.5)).toBeNull();
  });

  it("always returns the only candidate", () => {
    expect(draw([note("solo")], 0.99)?.id).toBe("solo");
  });

  it("gives a weighted card a proportionally bigger slice of [0,1)", () => {
    // a is worth 3, b is worth 1 — so a takes the first three quarters of the line.
    const weighted = [note("a", { weight: 3 }), note("b", { weight: 1 })];
    expect(draw(weighted, 0, true)?.id).toBe("a");
    expect(draw(weighted, 0.74, true)?.id).toBe("a");
    expect(draw(weighted, 0.75, true)?.id).toBe("b");
    expect(draw(weighted, 0.99, true)?.id).toBe("b");
  });

  it("ignores weights entirely when the game doesn't weigh", () => {
    const weighted = [note("a", { weight: 3 }), note("b", { weight: 1 })];
    expect(draw(weighted, 0.4, false)?.id).toBe("a");
    expect(draw(weighted, 0.6, false)?.id).toBe("b");
  });

  it("is unchanged by turning weighting on for a folder nobody has tuned", () => {
    const plain = [note("a"), note("b"), note("c"), note("d")];
    for (const r of [0, 0.1, 0.24, 0.25, 0.5, 0.74, 0.75, 0.99]) {
      expect(draw(plain, r, true)?.id).toBe(draw(plain, r, false)?.id);
    }
  });
});

describe("rpsOutcome — from the player's side", () => {
  it("knows what beats what", () => {
    expect(rpsOutcome("rock", "scissors")).toBe("win");
    expect(rpsOutcome("paper", "rock")).toBe("win");
    expect(rpsOutcome("scissors", "paper")).toBe("win");
  });

  it("knows what it loses to", () => {
    expect(rpsOutcome("scissors", "rock")).toBe("lose");
    expect(rpsOutcome("rock", "paper")).toBe("lose");
    expect(rpsOutcome("paper", "scissors")).toBe("lose");
  });

  it("calls a match a tie — nobody has won, so the card has not had its turn", () => {
    for (const t of THROWS) expect(rpsOutcome(t, t)).toBe("tie");
  });

  it("is decided for every pairing there is", () => {
    for (const a of THROWS) {
      for (const b of THROWS) {
        expect(["win", "lose", "tie"]).toContain(rpsOutcome(a, b));
      }
    }
  });
});

describe("GAME_INFO — the shape of each game is stated, not assumed", () => {
  it("marks the three pool games as draws and the two one-card games as verdicts", () => {
    expect(GAME_INFO.roulette.shape).toBe("draw");
    expect(GAME_INFO.straws.shape).toBe("draw");
    expect(GAME_INFO.box.shape).toBe("draw");
    expect(GAME_INFO.coin.shape).toBe("verdict");
    expect(GAME_INFO.rps.shape).toBe("verdict");
  });

  it("weighs the wheel and nothing else — it is the only one with a visible proportion", () => {
    const weighted = GAME_KINDS.filter((k) => GAME_INFO[k].weighted);
    expect(weighted).toEqual(["roulette"]);
  });

  it("describes every game it offers", () => {
    for (const k of GAME_KINDS) {
      expect(GAME_INFO[k].label.length).toBeGreaterThan(0);
      expect(GAME_INFO[k].blurb.length).toBeGreaterThan(0);
      expect(GAME_INFO[k].verb.length).toBeGreaterThan(0);
    }
  });
});

describe("markDrawn — and the loop all the way back", () => {
  it("adds the drawn card", () => {
    const f = folder("f", [note("a"), note("b"), note("c")]);
    expect(markDrawn(f, "a")).toEqual(["a"]);
  });

  it("keeps the order cards were drawn in", () => {
    const f = folder("f", [note("a"), note("b"), note("c")], { spent: ["c"] });
    expect(markDrawn(f, "a")).toEqual(["c", "a"]);
  });

  it("never records the same card twice", () => {
    const f = folder("f", [note("a"), note("b")], { spent: ["a"] });
    expect(markDrawn(f, "a")).toEqual(["a"]);
  });

  it("EMPTIES the round when the last card is drawn, rather than leaving every card greyed", () => {
    const f = folder("f", [note("a"), note("b")], { spent: ["a"] });
    expect(markDrawn(f, "b")).toEqual([]);
  });

  it("loops back on a one-card folder every single time", () => {
    const f = folder("f", [note("only")]);
    expect(markDrawn(f, "only")).toEqual([]);
  });

  it("counts only notes when deciding the round is over — a timeline never blocks the loop", () => {
    const f = folder("f", [note("a"), timeline("t")]);
    expect(markDrawn(f, "a")).toEqual([]);
  });
});

describe("pruneSpent — a card that left is not 'already drawn'", () => {
  it("drops ids the folder no longer holds", () => {
    const f = folder("f", [note("a")], { spent: ["a", "popped", "deleted"] });
    expect(pruneSpent(f)).toEqual(["a"]);
  });

  it("returns the SAME array when nothing needs dropping, so a caller can skip the write", () => {
    const spent = ["a"];
    const f = folder("f", [note("a"), note("b")], { spent });
    expect(pruneSpent(f)).toBe(spent);
  });

  it("returns the same empty array for a folder that has never been played", () => {
    const f = folder("f", [note("a")]);
    expect(pruneSpent(f)).toEqual([]);
  });

  it("drops a card that became a timeline's problem — pool is notes only", () => {
    const f = folder("f", [timeline("t")], { spent: ["t"] });
    expect(pruneSpent(f)).toEqual([]);
  });
});

describe("isSpent", () => {
  it("reads the round", () => {
    const f = folder("f", [note("a"), note("b")], { spent: ["a"] });
    expect(isSpent(f, "a")).toBe(true);
    expect(isSpent(f, "b")).toBe(false);
  });

  it("is false on a folder that has never been played", () => {
    expect(isSpent(folder("f", [note("a")]), "a")).toBe(false);
  });
});

describe("cardLabel", () => {
  it("prefers the title", () => {
    expect(cardLabel(note("n", { title: "Ring the dentist", text: "body" }))).toBe(
      "Ring the dentist",
    );
  });

  it("falls back to the first non-empty line of the text", () => {
    expect(cardLabel(note("n", { text: "\n\n  Book the van\nand pack it" }))).toBe("Book the van");
  });

  it("strips a checkbox marker", () => {
    expect(cardLabel(note("n", { text: "[] ring the dentist" }))).toBe("ring the dentist");
    expect(cardLabel(note("n", { text: "[x] ring the dentist" }))).toBe("ring the dentist");
  });

  it("strips bullet, dash, numbered, roman and heading markers", () => {
    expect(cardLabel(note("n", { text: ". a bullet" }))).toBe("a bullet");
    expect(cardLabel(note("n", { text: "- a dash point" }))).toBe("a dash point");
    expect(cardLabel(note("n", { text: "1. a numbered one" }))).toBe("a numbered one");
    expect(cardLabel(note("n", { text: "iv. a roman one" }))).toBe("a roman one");
    expect(cardLabel(note("n", { text: "## a heading" }))).toBe("a heading");
  });

  it("does not eat a bare word that merely starts with a marker character", () => {
    expect(cardLabel(note("n", { text: "-nodash" }))).toBe("-nodash");
  });

  it("names an empty note rather than showing a blank wedge", () => {
    expect(cardLabel(note("n"))).toBe("Untitled note");
    expect(cardLabel(note("n", { title: "   ", text: "  \n " }))).toBe("Untitled note");
  });
});

describe("carryGameState — a round is not part of the undoable document", () => {
  it("keeps the live round when an unrelated edit is undone", () => {
    const restored = board([folder("f", [note("a"), note("b")], { spent: [], color: "st-due" })]);
    const live = board([
      folder("f", [note("a"), note("b")], { spent: ["a"], game: "roulette", gameOn: true }),
    ]);
    const out = carryGameState(restored, live);
    const f = out.widgets[0];
    expect(f.spent).toEqual(["a"]);
    expect(f.game).toBe("roulette");
    expect(f.gameOn).toBe(true);
    // …while everything that IS the user's document still comes back from the snapshot.
    expect(f.color).toBe("st-due");
  });

  it("carries autoPopOut too", () => {
    const restored = board([folder("f", [], { autoPopOut: true })]);
    const live = board([folder("f", [], { autoPopOut: false })]);
    expect(carryGameState(restored, live).widgets[0].autoPopOut).toBe(false);
  });

  it("CLEARS a field the live folder no longer has, rather than resurrecting it", () => {
    const restored = board([folder("f", [], { game: "box", gameOn: true, spent: ["x"] })]);
    const live = board([folder("f", [])]);
    const f = carryGameState(restored, live).widgets[0];
    expect(f.game).toBeUndefined();
    expect(f.gameOn).toBeUndefined();
    expect(f.spent).toBeUndefined();
  });

  it("leaves a folder the live board no longer has alone — there is nothing to carry", () => {
    const restored = board([folder("gone", [], { game: "straws", gameOn: true })]);
    const live = board([]);
    expect(carryGameState(restored, live).widgets[0].game).toBe("straws");
  });

  it("does not touch notes or timelines", () => {
    const restored = board([note("n", { text: "before" }), timeline("t")]);
    const live = board([note("n", { text: "after" }), timeline("t")]);
    expect(carryGameState(restored, live).widgets[0].text).toBe("before");
  });

  it("returns the SAME board object when nothing needs carrying", () => {
    const b = board([folder("f", [note("a")], { game: "box", gameOn: true, spent: ["a"] })]);
    const live = board([
      folder("f", [note("a")], { game: "box", gameOn: true, spent: b.widgets[0].spent }),
    ]);
    expect(carryGameState(b, live)).toBe(b);
  });
});

describe("wedgeAngles — where each wedge sits, and what a spin aims at", () => {
  it("lays the wedges end to end around the whole circle", () => {
    const a = wedgeAngles(shares([note("a"), note("b"), note("c")], false));
    expect(a[0].start).toBeCloseTo(0);
    expect(a[0].end).toBeCloseTo(120);
    expect(a[1].start).toBeCloseTo(a[0].end);
    expect(a[2].end).toBeCloseTo(360);
  });

  it("puts mid in the middle of its own wedge, which is what the wheel stops on", () => {
    for (const w of wedgeAngles(shares([note("a"), note("b"), note("c")], false))) {
      expect(w.mid).toBeCloseTo((w.start + w.end) / 2);
    }
  });

  it("follows the shares, so an uneven wheel has uneven wedges", () => {
    const cards = [note("a", { weight: 3 }), note("b")];
    const a = wedgeAngles(shares(cards, true));
    expect(a[0].end).toBeCloseTo(270);
    expect(a[1].end - a[1].start).toBeCloseTo(90);
  });

  it("has nothing to say about an empty wheel", () => {
    expect(wedgeAngles([])).toEqual([]);
  });
});

/** How far the wheel, left at `angle`, is from resting with `mid` under the pointer at the top. */
const offBy = (angle: number, mid: number) => {
  const off = (((angle + mid) % 360) + 360) % 360;
  return Math.min(off, 360 - off);
};

describe("spinTo — the wheel only ever goes forward, and lands where it said it would", () => {
  it("brings the wedge's middle to rest exactly under the pointer", () => {
    for (const mid of [0, 37.5, 90, 180, 271.3, 359.9]) {
      expect(offBy(spinTo(0, mid), mid)).toBeLessThan(1e-9);
    }
  });

  it("turns at least the whole number of times it was asked for", () => {
    expect(spinTo(0, 90, 4)).toBeGreaterThanOrEqual(4 * 360);
    expect(spinTo(0, 90, 1)).toBeGreaterThanOrEqual(360);
    expect(spinTo(1000, 90, 4)).toBeGreaterThanOrEqual(1000 + 4 * 360);
  });

  it("never unwinds — a wedge earlier in the list is reached by going round again", () => {
    // The bug this rules out: recomputing the angle from zero each spin, so landing on wedge 0
    // after wedge 3 rotates the wheel BACKWARDS, which reads as it changing its mind.
    let angle = 0;
    for (const mid of [300, 30, 200, 10, 359]) {
      const next = spinTo(angle, mid);
      expect(next).toBeGreaterThan(angle);
      expect(offBy(next, mid)).toBeLessThan(1e-9);
      angle = next;
    }
  });

  it("still turns once when asked for none, so a spin is always a spin", () => {
    expect(spinTo(0, 90, 0)).toBeGreaterThanOrEqual(360);
    expect(spinTo(0, 90, -3)).toBeGreaterThanOrEqual(360);
  });
});

describe("strawHeights — the winner's is the long one, and nothing shuffles mid-pull", () => {
  it("gives one length per straw", () => {
    expect(strawHeights(5, 2)).toHaveLength(5);
    expect(strawHeights(1, 0)).toHaveLength(1);
    expect(strawHeights(0, -1)).toEqual([]);
  });

  it("makes the winner strictly the tallest, at every size of folder", () => {
    for (const count of [1, 2, 3, 5, 8, 13, 30]) {
      for (let winner = 0; winner < count; winner++) {
        const h = strawHeights(count, winner);
        const others = h.filter((_, i) => i !== winner);
        expect(others.every((x) => x < h[winner])).toBe(true);
      }
    }
  });

  it("keeps every straw within the range it was given", () => {
    const h = strawHeights(12, 4, 176, 62);
    expect(Math.min(...h)).toBeGreaterThanOrEqual(62);
    expect(Math.max(...h)).toBe(176);
  });

  it("is the same every time — a re-render mid-pull must not reshuffle the fist", () => {
    expect(strawHeights(7, 3)).toEqual(strawHeights(7, 3));
  });

  it("does not fall over before anything has been drawn", () => {
    const h = strawHeights(4, -1);
    expect(h).toHaveLength(4);
    expect(h.every((x) => Number.isFinite(x))).toBe(true);
  });
});

describe("keepsRound — the folder that would rather have an honest coin toss", () => {
  it("keeps a round by default, and by explicit choice", () => {
    expect(keepsRound(folder("f", []))).toBe(true);
    expect(keepsRound(folder("f", [], { repeat: false }))).toBe(true);
  });

  it("keeps none when the folder is set to repeat", () => {
    expect(keepsRound(folder("f", [], { repeat: true }))).toBe(false);
  });

  it("puts every card in every draw, so the same one can come up twice running", () => {
    const f = folder("f", [note("a"), note("b"), note("c")], { repeat: true, spent: ["a", "b"] });
    expect(livePool(f).map((w) => w.id)).toEqual(["a", "b", "c"]);
  });

  it("greys nothing, even where a round left ids behind", () => {
    // The half-honoured version of this flag is the dangerous one: a folder that stopped keeping a
    // round but kept greying what the old one had recorded would show cards as unavailable while
    // happily drawing them.
    const f = folder("f", [note("a"), note("b")], { repeat: true, spent: ["a"] });
    expect(isSpent(f, "a")).toBe(false);
    expect(livePool(f)).toHaveLength(2);
  });

  it("goes straight back to the round it kept when repeating is switched off again", () => {
    const f = folder("f", [note("a"), note("b")], { repeat: false, spent: ["a"] });
    expect(isSpent(f, "a")).toBe(true);
    expect(livePool(f).map((w) => w.id)).toEqual(["b"]);
  });
});
