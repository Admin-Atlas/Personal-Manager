// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The folder games — "gamble your next task".
//
// A folder can be handed a game (`lib/pinboard/game.ts` holds the rules). Its tile then plays
// instead of opening, and this is what it plays: a wheel, a fist of straws, a box of folded slips,
// a coin, or a throw of rock-paper-scissors. It lands on one of the notes inside. That note is the
// next thing you do.
//
// THREE RULES THIS FILE EXISTS TO HOLD:
//
// 1. THE OUTCOME IS DECIDED BEFORE THE ANIMATION STARTS, never by it. PM has two reduced-motion
//    signals and they fail in OPPOSITE directions — under the OS query the keyframes are never
//    emitted at all (so an `animationend` handler would never fire, and the game would hang
//    forever), while under the app's own "Reduced" setting they complete in 0.001ms (so the same
//    handler fires instantly and the result flashes past). Deciding first and treating the motion
//    as pure theatre is the only shape that behaves for everyone. `prefersReducedMotion()` is read
//    at click time — never cached, never from React context — and simply collapses the wait to zero.
//
// 2. THE STAGE IS FROZEN AT THE MOMENT OF THE DRAW. Recording the draw immediately (rule 1) takes
//    the winner out of `livePool` on the very next render, so a stage rendered straight from the
//    live pool loses the card it is supposed to be pointing at: the wheel could not find the wedge
//    to stop on, the winning straw was not among the straws, and the slip that rises out of the box
//    was never drawn. Every draw game therefore plays against `staged` — the candidates exactly as
//    they were when the button was pressed — until the next round is asked for. That is what makes
//    the animations exist at all, and it is also why a winner that auto-pops-out to the board can
//    still be shown as the answer instead of a shrug.
//
// 3. A TRANSITION IS ARMED BEFORE THE VALUE IT ANIMATES MOVES. Switching a transition on in the
//    same style change as the property it animates leaves the result at the mercy of how the engine
//    batches that recalculation. So durations are held in state, set at click time, and stay put —
//    the transform/height/offset alone is what changes when a play begins.
//
// And the result is ANNOUNCED, not merely drawn. A spinning wheel is a graphic; the answer to "what
// should I do next" has to reach a screen reader too. The live region is mounted with the whole
// surface rather than with the result, because an `aria-live` region only announces changes to a
// region that ALREADY existed — one that appears alongside its own first message says nothing.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Button, Popover, Select, Toggle, VisuallyHidden } from "./ui";
import { prefersReducedMotion } from "../theme/motion";
import {
  cardLabel,
  draw,
  GAME_INFO,
  GAME_KINDS,
  isSpent,
  keepsRound,
  livePool,
  pool,
  rpsOutcome,
  shares,
  spinTo,
  strawHeights,
  THROW_LABEL,
  THROWS,
  wedgeAngles,
  WEIGHT_CHOICES,
  weightOf,
  type Throw,
} from "../lib/pinboard/game";
import { TINT_PALETTE } from "../lib/pinboard/palette";
import type { GameKind, Widget } from "../lib/pinboard/types";

/** How long each game's theatre runs, in ms. Collapsed to 0 when motion is reduced, and when there
 *  is only one card left — a wheel turning four times to land on the only wedge there is would be
 *  theatre at the user's expense. */
const SPIN_MS: Record<GameKind, number> = {
  roulette: 2800,
  straws: 1800,
  box: 2000,
  coin: 1400,
  rps: 1350,
};

/** Every stage is this tall, so the controls under it don't jump as the game changes. */
const STAGE = 190;

/** A uniform number in [0, 1) from the platform CSPRNG, falling back to `Math.random` where it
 *  isn't there. Randomness lives HERE and nowhere else — `game.ts` takes the number as an argument
 *  so its rules stay pure and testable. */
function randomUnit(): number {
  try {
    const buf = new Uint32Array(1);
    crypto.getRandomValues(buf);
    return buf[0] / 2 ** 32;
  } catch {
    return Math.random();
  }
}

/** A card's wedge/straw/slip colour: its own note tint where it has one, else the palette in
 *  order, so a folder of untinted notes still reads as separate pieces. Token names only. */
function cardToken(card: Widget, index: number): string {
  return card.color ?? TINT_PALETTE[index % TINT_PALETTE.length].token;
}

/** A tint token as a solid-enough fill to tell wedges apart, mixed toward the panel so it sits in
 *  whatever theme is on. Never hex — the `--st-*` tokens are theme-adaptive. */
function fillOf(token: string, spent: boolean): string {
  const strength = spent ? 10 : 55;
  return `color-mix(in oklab, var(--${token}) ${strength}%, var(--panel))`;
}

/** A label cut to fit a wedge or a slip, with a real ellipsis rather than a cliff. */
function clip(text: string, max: number): string {
  return text.length <= max ? text : `${text.slice(0, max - 1).trimEnd()}…`;
}

type Phase = "idle" | "playing" | "done";

/**
 * The game surface: a stage, the control that plays it, and a legend of what is still in play.
 *
 * `onDraw` is called ONCE per completed play, at the moment the winner is decided rather than when
 * the animation ends — so a card is never drawn twice by an impatient second click, and a game
 * closed mid-spin still recorded what it drew.
 */
export function FolderGame({
  folder,
  game,
  onDraw,
  onPopOut,
  onWeight,
  onResetRound,
  onAutoPopOut,
  onRepeat,
}: {
  folder: Widget;
  game: GameKind;
  onDraw: (childId: string, assigned: boolean) => void;
  onPopOut: (childId: string) => void;
  onWeight: (childId: string, weight: number) => void;
  onResetRound: () => void;
  onAutoPopOut: (next: boolean) => void;
  onRepeat: (next: boolean) => void;
}) {
  const cards = useMemo(() => pool(folder), [folder]);
  if (cards.length === 0) {
    return (
      <div className="flex min-h-0 flex-1 items-center justify-center p-3">
        <EmptyGame />
      </div>
    );
  }
  const props = {
    folder,
    game,
    cards,
    onDraw,
    onPopOut,
    onWeight,
    onResetRound,
    onAutoPopOut,
    onRepeat,
  };
  // The two shapes are genuinely different games, not one game with two skins: a wheel picks out of
  // all of them, a coin puts ONE to you and lets you play for it. Splitting here keeps each one's
  // state where it belongs instead of a single component holding both sets.
  return GAME_INFO[game].shape === "draw" ? <DrawGame {...props} /> : <VerdictGame {...props} />;
}

interface GameProps {
  folder: Widget;
  game: GameKind;
  cards: Widget[];
  onDraw: (childId: string, assigned: boolean) => void;
  onPopOut: (childId: string) => void;
  onWeight: (childId: string, weight: number) => void;
  onResetRound: () => void;
  onAutoPopOut: (next: boolean) => void;
  onRepeat: (next: boolean) => void;
}

/** The three that pick one card out of the whole folder: the wheel, the straws, the box. */
function DrawGame({
  folder,
  game,
  cards,
  onDraw,
  onPopOut,
  onWeight,
  onResetRound,
  onAutoPopOut,
  onRepeat,
}: GameProps) {
  const [phase, setPhase] = useState<Phase>("idle");
  const [winnerId, setWinnerId] = useState<string | null>(null);
  // The candidates AS THEY WERE when the play began — see rule 2 at the top of this file.
  const [staged, setStaged] = useState<Widget[] | null>(null);
  // The wheel's accumulated rotation and the timing every stage animates at. Both live here rather
  // than in the stage so they survive the stage re-rendering, and so the duration is already in
  // place before the value it governs moves (rule 3).
  const [spin, setSpin] = useState({ angle: 0, ms: SPIN_MS[game] });
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const inPlay = useMemo(() => livePool(folder), [folder]);
  const info = GAME_INFO[game];

  // A play in flight must not outlive the surface: the callback would land on an unmounted tree,
  // and the draw it was going to report has already been reported (see `play`).
  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );

  // Switching game mid-round would otherwise leave the previous game's winner on the stage.
  useEffect(() => {
    setPhase("idle");
    setWinnerId(null);
    setStaged(null);
    setSpin({ angle: 0, ms: SPIN_MS[game] });
  }, [game]);

  const play = useCallback(() => {
    if (phase === "playing" || inPlay.length === 0) return;
    const table = inPlay;
    const winner = draw(table, randomUnit(), info.weighted);
    if (!winner) return;
    const index = table.indexOf(winner);
    const wait = prefersReducedMotion() || table.length < 2 ? 0 : SPIN_MS[game];
    setStaged(table);
    setWinnerId(winner.id);
    setSpin((s) => ({
      angle: spinTo(s.angle, wedgeAngles(shares(table, info.weighted))[index].mid),
      ms: wait,
    }));
    // Recorded NOW, not when the theatre finishes: the round is the honest part, and it must
    // survive the surface being closed (or PM being quit) halfway through a spin.
    onDraw(winner.id, true);
    if (wait === 0) {
      setPhase("done");
      return;
    }
    setPhase("playing");
    timer.current = setTimeout(() => setPhase("done"), wait);
  }, [phase, inPlay, game, info.weighted, onDraw]);

  const replay = useCallback(() => {
    setPhase("idle");
    setWinnerId(null);
    setStaged(null);
    // "Go again" with nothing left in play is a request to start over, not a dead button. (The
    // round normally empties itself on the last draw; this covers the folder whose last card was
    // moved out to the board instead.)
    if (livePool(folder).length === 0) onResetRound();
  }, [folder, onResetRound]);

  const spinning = phase === "playing";
  const settled = phase === "done";
  // The stage plays against the frozen table; the live pool is what the CONTROLS reason about.
  const table = staged ?? inPlay;
  const winner = winnerId ? table.find((c) => c.id === winnerId) : undefined;
  // Still in the folder, so there is still something to move out of it.
  const stillHere = !!winner && cards.some((c) => c.id === winner.id);

  return (
    <div className="flex h-full min-h-0 flex-col">
      {/* Mounted with the surface, not with the result — see the note at the top of this file. */}
      <VisuallyHidden role="status" aria-live="polite">
        {settled && winner
          ? `${info.label} chose ${cardLabel(winner)}.${stillHere ? "" : " It moved out to your board."}`
          : ""}
      </VisuallyHidden>

      <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 overflow-auto p-3">
        <Stage
          game={game}
          cards={table}
          winnerId={winnerId}
          spinning={spinning}
          settled={settled}
          weighted={info.weighted}
          angle={spin.angle}
          ms={spin.ms}
        />
        <Verdict
          info={info}
          settled={settled}
          spinning={spinning}
          winner={winner}
          stillHere={stillHere}
          remaining={inPlay.length}
          onPlay={play}
          onAgain={replay}
          onPopOut={onPopOut}
        />
      </div>

      <Legend
        folder={folder}
        cards={cards}
        winnerId={settled ? winnerId : null}
        weighted={info.weighted}
        onWeight={onWeight}
        onReset={onResetRound}
        onAutoPopOut={onAutoPopOut}
        onRepeat={onRepeat}
      />
    </div>
  );
}

/** The two that put ONE card to you and let you play for it: the coin and the throw.
 *
 *  The loop is the same for both — offer a card, play a two-outcome round, lose and it's yours,
 *  win and it offers the next one. Only the theatre differs, and who counts as having won. Either
 *  way that card has had its turn this round, which is what stops a folder putting the same job to
 *  you over and over; but a card you DODGED is not work, so it is never popped out to the board. */
function VerdictGame({
  folder,
  game,
  cards,
  onDraw,
  onPopOut,
  onWeight,
  onResetRound,
  onAutoPopOut,
  onRepeat,
}: GameProps) {
  const [offeredId, setOfferedId] = useState<string | null>(null);
  const [phase, setPhase] = useState<Phase>("idle");
  const [heads, setHeads] = useState(false);
  const [mine, setMine] = useState<Throw | null>(null);
  const [theirs, setTheirs] = useState<Throw | null>(null);
  const [result, setResult] = useState<"do" | "dodge" | "tie" | null>(null);
  const [ms, setMs] = useState(SPIN_MS[game]);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const inPlay = useMemo(() => livePool(folder), [folder]);
  const info = GAME_INFO[game];
  // The card is looked up in the whole pool, not the live one: it stops being "in play" the instant
  // it is settled, and it is still the card on the table until you ask for another.
  const offered = offeredId ? cards.find((c) => c.id === offeredId) : undefined;

  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );

  const clear = useCallback(() => {
    setOfferedId(null);
    setPhase("idle");
    setMine(null);
    setTheirs(null);
    setResult(null);
  }, []);

  // Changing game mid-round must not leave the previous one's card sitting on the table.
  useEffect(() => clear(), [game, clear]);

  /** Put the next card on the table. Nothing is spent yet — it hasn't been decided. */
  const offer = useCallback(() => {
    const pick = draw(inPlay, randomUnit());
    if (!pick) {
      // Nothing left to offer, so offering IS starting over — the same move "Go again" makes.
      onResetRound();
      clear();
      return;
    }
    setOfferedId(pick.id);
    setPhase("idle");
    setMine(null);
    setTheirs(null);
    setResult(null);
  }, [inPlay, onResetRound, clear]);

  /** Settle the offered card. Decided FIRST, exactly as in the draw games — the animation is
   *  theatre, and under reduced motion there isn't one. */
  const settleWith = useCallback(
    (outcome: "do" | "dodge") => {
      if (!offeredId) return;
      onDraw(offeredId, outcome === "do");
      const wait = prefersReducedMotion() ? 0 : SPIN_MS[game];
      setMs(wait);
      setResult(outcome);
      if (wait === 0) {
        setPhase("done");
        return;
      }
      setPhase("playing");
      timer.current = setTimeout(() => setPhase("done"), wait);
    },
    [offeredId, game, onDraw],
  );

  const flip = useCallback(() => {
    if (phase === "playing" || !offeredId) return;
    const isHeads = randomUnit() < 0.5;
    setHeads(isHeads);
    settleWith(isHeads ? "do" : "dodge");
  }, [phase, offeredId, settleWith]);

  const throwHand = useCallback(
    (hand: Throw) => {
      if (phase === "playing" || !offeredId) return;
      const pm = THROWS[Math.min(2, Math.floor(randomUnit() * 3))];
      setMine(hand);
      setTheirs(pm);
      const outcome = rpsOutcome(hand, pm);
      if (outcome === "tie") {
        // Nobody won, so the card has NOT had its turn — throw again on the same one. The shake
        // still runs, because you did throw; only the card's fate is unchanged.
        const wait = prefersReducedMotion() ? 0 : SPIN_MS[game];
        setMs(wait);
        setResult("tie");
        if (wait === 0) {
          setPhase("done");
          return;
        }
        setPhase("playing");
        timer.current = setTimeout(() => setPhase("done"), wait);
        return;
      }
      settleWith(outcome === "lose" ? "do" : "dodge");
    },
    [phase, offeredId, game, settleWith],
  );

  const busy = phase === "playing";
  const settled = phase === "done";
  const announcement =
    settled && result === "do"
      ? `That one is yours: ${offered ? cardLabel(offered) : "your card"}.`
      : settled && result === "dodge"
        ? "You won — off the hook for that one."
        : settled && result === "tie"
          ? "A tie. Throw again."
          : "";

  return (
    <div className="flex h-full min-h-0 flex-col">
      <VisuallyHidden role="status" aria-live="polite">
        {announcement}
      </VisuallyHidden>

      <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 overflow-auto p-3">
        {!offered ? (
          <div className="flex flex-col items-center gap-2 text-center">
            <p className="max-w-xs text-xs text-ink4">{info.blurb}</p>
            <Button size="sm" onClick={offer}>
              {inPlay.length === 0 ? "Start the round over" : "Offer me one"}
            </Button>
          </div>
        ) : (
          <>
            <div className="max-w-xs text-center">
              <p className="text-[0.625rem] uppercase tracking-wide text-ink4">On the table</p>
              <p className="mt-1 text-sm font-medium text-ink">{cardLabel(offered)}</p>
            </div>

            {game === "coin" ? (
              <Coin heads={heads} tossing={busy} shown={busy || settled} ms={ms} />
            ) : (
              <Hands mine={mine} theirs={theirs} shaking={busy} ms={ms} />
            )}

            {busy ? (
              <p className="text-xs text-ink4">{info.verb}ing&hellip;</p>
            ) : settled ? (
              <div className="flex flex-col items-center gap-2 text-center">
                <p className="max-w-xs text-sm font-medium text-ink">
                  {result === "do"
                    ? "That one's yours."
                    : result === "dodge"
                      ? "You're off the hook."
                      : "A tie — go again."}
                </p>
                <div className="flex items-center gap-2">
                  {result === "do" && cards.some((c) => c.id === offered.id) && (
                    <Button variant="secondary" size="sm" onClick={() => onPopOut(offered.id)}>
                      Move out to the board
                    </Button>
                  )}
                  {result === "tie" ? (
                    <Button size="sm" onClick={() => setPhase("idle")}>
                      Throw again
                    </Button>
                  ) : (
                    <Button variant="tertiary" size="sm" onClick={offer}>
                      {inPlay.length === 0
                        ? "That was the last one — go again"
                        : "Offer me another"}
                    </Button>
                  )}
                </div>
              </div>
            ) : game === "coin" ? (
              <Button size="sm" onClick={flip}>
                Flip for it
              </Button>
            ) : (
              <div className="flex items-center gap-2">
                {THROWS.map((t) => (
                  <Button key={t} variant="secondary" size="sm" onClick={() => throwHand(t)}>
                    <span className="flex items-center gap-1.5">
                      <ThrowGlyph hand={t} className="h-4 w-4" />
                      {THROW_LABEL[t]}
                    </span>
                  </Button>
                ))}
              </div>
            )}
          </>
        )}
      </div>

      <Legend
        folder={folder}
        cards={cards}
        winnerId={settled && result === "do" ? offeredId : null}
        weighted={info.weighted}
        onWeight={onWeight}
        onReset={onResetRound}
        onAutoPopOut={onAutoPopOut}
        onRepeat={onRepeat}
      />
    </div>
  );
}

/** The coin: thrown up in an arc while it turns over, landing on the face it already decided on.
 *  The arc and the turn are separate elements on purpose — one animation each, rather than two
 *  things fighting over one transform. */
function Coin({
  heads,
  tossing,
  shown,
  ms,
}: {
  heads: boolean;
  tossing: boolean;
  shown: boolean;
  ms: number;
}) {
  const turns = shown ? 360 * 3 + (heads ? 0 : 180) : 0;
  return (
    <div
      role="img"
      aria-label={shown ? (heads ? "Heads" : "Tails") : "A coin, not yet flipped"}
      className="flex items-center justify-center"
      style={{ height: STAGE, width: STAGE, perspective: "700px" }}
    >
      <div className={tossing ? "pm-game-toss" : ""} style={{ animationDuration: `${ms}ms` }}>
        <div
          className="relative h-24 w-24"
          style={{
            transformStyle: "preserve-3d",
            transform: `rotateX(${turns}deg)`,
            transitionProperty: "transform",
            transitionDuration: `${ms}ms`,
            transitionTimingFunction: "cubic-bezier(0.25, 0.6, 0.2, 1)",
          }}
        >
          <CoinFace label="Heads" />
          <CoinFace label="Tails" back />
        </div>
      </div>
    </div>
  );
}

function CoinFace({ label, back }: { label: string; back?: boolean }) {
  return (
    <div
      aria-hidden="true"
      className="absolute inset-0 flex items-center justify-center rounded-full border-2 text-xs font-medium uppercase tracking-wide"
      style={{
        backfaceVisibility: "hidden",
        transform: back ? "rotateX(180deg)" : undefined,
        borderColor: "var(--border2)",
        background: `radial-gradient(circle at 34% 30%, color-mix(in oklab, var(--st-look) 62%, var(--panel)), color-mix(in oklab, var(--st-look) 26%, var(--panel)))`,
        color: "var(--ink2)",
        boxShadow: "inset 0 0 0 4px color-mix(in oklab, var(--panel) 55%, transparent)",
      }}
    >
      {label}
    </div>
  );
}

/** The two throws, side by side — both fists bobbing through the three-count, then both open. */
function Hands({
  mine,
  theirs,
  shaking,
  ms,
}: {
  mine: Throw | null;
  theirs: Throw | null;
  shaking: boolean;
  ms: number;
}) {
  const beat = Math.max(1, Math.round(ms / 3));
  return (
    <div className="flex items-center justify-center gap-5" style={{ height: STAGE }}>
      <HandFace label="You" hand={shaking ? "rock" : mine} shaking={shaking} beat={beat} />
      <span aria-hidden="true" className="text-xs uppercase tracking-wide text-ink4">
        vs
      </span>
      <HandFace label="PM" hand={shaking ? "rock" : theirs} shaking={shaking} beat={beat} />
    </div>
  );
}

function HandFace({
  label,
  hand,
  shaking,
  beat,
}: {
  label: string;
  hand: Throw | null;
  shaking: boolean;
  beat: number;
}) {
  return (
    <div className="flex flex-col items-center gap-1.5">
      <div
        className={`flex h-20 w-20 items-center justify-center rounded-[var(--radius)] border ${
          shaking ? "pm-game-bob" : ""
        }`}
        style={{
          borderColor: "var(--border2)",
          background: "color-mix(in oklab, var(--st-track) 34%, var(--panel))",
          color: "var(--ink2)",
          animationDuration: `${beat}ms`,
          animationIterationCount: 3,
        }}
      >
        {hand ? (
          <ThrowGlyph hand={hand} className="h-10 w-10" />
        ) : (
          <span className="text-lg text-ink4">?</span>
        )}
      </div>
      <span className="text-[0.625rem] uppercase tracking-wide text-ink4">{label}</span>
    </div>
  );
}

/** Rock, paper and scissors as the THINGS rather than as hands: a stone, a sheet, a pair of
 *  scissors. Hand-rolled like every other glyph in the app, and far more legible at 16px than an
 *  attempt at three fists would be. */
function ThrowGlyph({ hand, className }: { hand: Throw; className: string }) {
  const common = {
    viewBox: "0 0 24 24",
    className,
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 1.5,
    "aria-hidden": true as const,
  };
  if (hand === "rock") {
    return (
      <svg {...common}>
        <path d="M5 13.2 8.2 6h7.6l3.2 7.2-4.4 4.8h-5L5 13.2Z" strokeLinejoin="round" />
        <path d="M8.2 6 10.6 11.6 5 13.2M10.6 11.6 14.6 18M10.6 11.6 15.8 6M10.6 11.6 19 13.2" />
      </svg>
    );
  }
  if (hand === "paper") {
    return (
      <svg {...common}>
        <path d="M6 4h7l5 5v11H6V4Z" strokeLinejoin="round" />
        <path d="M13 4v5h5" strokeLinejoin="round" />
      </svg>
    );
  }
  return (
    <svg {...common}>
      <circle cx="7" cy="17.6" r="2.4" />
      <circle cx="17" cy="17.6" r="2.4" />
      <path d="M8.7 15.8 18.4 4.6M15.3 15.8 5.6 4.6" strokeLinecap="round" />
    </svg>
  );
}

/** A game folder with nothing to draw from. The two ordinary folder views both say how to fill a
 *  folder, and this replaces them, so it has to say it too rather than showing an empty wheel. */
function EmptyGame() {
  return (
    <div className="max-w-xs text-center">
      <p className="text-sm text-ink3">Nothing to gamble with yet.</p>
      <p className="mt-1 text-xs text-ink4">
        Drag a note onto this folder&rsquo;s tile to file it here, then come back and spin.
        Timelines can live in here too, but they&rsquo;re never drawn &mdash; a dated track
        isn&rsquo;t a task.
      </p>
    </div>
  );
}

/** The play button and what the game has to say for itself. */
function Verdict({
  info,
  settled,
  spinning,
  winner,
  stillHere,
  remaining,
  onPlay,
  onAgain,
  onPopOut,
}: {
  info: { label: string; blurb: string; verb: string };
  settled: boolean;
  spinning: boolean;
  winner?: Widget;
  /** The winner is still in the folder, so there is still something to move out of it. */
  stillHere: boolean;
  remaining: number;
  onPlay: () => void;
  onAgain: () => void;
  onPopOut: (childId: string) => void;
}) {
  if (spinning) {
    return <p className="text-xs text-ink4">{info.verb}ning&hellip;</p>;
  }
  if (settled) {
    return (
      <div className="flex flex-col items-center gap-2 text-center">
        <p className="text-[0.625rem] uppercase tracking-wide text-ink4">Do this next</p>
        <p className="max-w-xs text-sm font-medium text-ink">
          {winner ? cardLabel(winner) : "It moved out to your board."}
        </p>
        <div className="flex items-center gap-2">
          {winner && stillHere && (
            <Button variant="secondary" size="sm" onClick={() => onPopOut(winner.id)}>
              Move out to the board
            </Button>
          )}
          {/* Never disabled: with nothing left in play this is the button that starts the round
              over, which is exactly what somebody who just took the last card wants next. */}
          <Button variant="tertiary" size="sm" onClick={onAgain}>
            {remaining === 0 ? "Start the round over" : "Go again"}
          </Button>
        </div>
      </div>
    );
  }
  // One card left is not a gamble, and pretending otherwise wastes three seconds to tell somebody
  // what they can already see. The play still takes a press — it is a decision, not an accident.
  const sole = remaining === 1;
  return (
    <div className="flex flex-col items-center gap-2 text-center">
      <p className="max-w-xs text-xs text-ink4">
        {sole ? "One card in here — nothing to gamble on." : info.blurb}
      </p>
      <Button size="sm" onClick={onPlay} disabled={remaining === 0}>
        {remaining === 0 ? "Nothing left to draw" : sole ? "Take it" : info.verb}
      </Button>
    </div>
  );
}

interface StageProps {
  /** The frozen table — see rule 2 at the top of this file. */
  cards: Widget[];
  winnerId: string | null;
  spinning: boolean;
  settled: boolean;
  weighted: boolean;
  /** The wheel's accumulated rotation, in degrees. */
  angle: number;
  /** How long this play's theatre runs. Held constant across the play so the transitions it drives
   *  are already armed when the value they animate moves. */
  ms: number;
}

/** Which game is drawn on the stage. */
function Stage(props: StageProps & { game: GameKind }) {
  if (props.game === "roulette") return <Wheel {...props} />;
  if (props.game === "straws") return <Straws {...props} />;
  return <Box {...props} />;
}

const WHEEL = 200;
const R = WHEEL / 2;
/** Where a wedge's label sits, as a fraction of the radius: far enough out to have room, near
 *  enough in that the longest label still clears the rim. */
const LABEL_IN = 0.3;
const LABEL_OUT = 0.94;

/** One wedge path, drawn from the centre. */
function wedgePath(from: number, to: number): string {
  const p = (deg: number) => {
    const rad = ((deg - 90) * Math.PI) / 180;
    return `${R + R * Math.cos(rad)} ${R + R * Math.sin(rad)}`;
  };
  const large = to - from > 180 ? 1 : 0;
  return `M ${R} ${R} L ${p(from)} A ${R} ${R} 0 ${large} 1 ${p(to)} Z`;
}

/** The roulette wheel: a wedge per card still in play, sized by that card's share of the draw,
 *  named along its own spoke, and spun to land the winner under the pointer. The wedges are cut
 *  from the SAME shares the draw uses, so a wheel can never show one thing and pick by another. */
function Wheel({ cards, winnerId, settled, weighted, angle, ms }: StageProps) {
  const fractions = useMemo(() => shares(cards, weighted), [cards, weighted]);
  const angles = useMemo(() => wedgeAngles(fractions), [fractions]);
  return (
    <div className="relative" style={{ width: WHEEL, height: WHEEL + 12 }}>
      {/* The pointer the wedge stops under. */}
      <svg
        viewBox="0 0 14 12"
        aria-hidden="true"
        className="absolute left-1/2 top-0 z-10 w-3.5 -translate-x-1/2 text-accent"
        fill="currentColor"
      >
        <path d="M7 12 L0 0 L14 0 Z" />
      </svg>
      <svg
        viewBox={`0 0 ${WHEEL} ${WHEEL}`}
        role="img"
        aria-label={`A wheel with ${cards.length} wedge${cards.length === 1 ? "" : "s"}`}
        className="absolute bottom-0"
        style={{
          width: WHEEL,
          height: WHEEL,
          transform: `rotate(${angle}deg)`,
          transitionProperty: "transform",
          transitionDuration: `${ms}ms`,
          // A long, late-braking curve: fast out of the gate, and the last half-turn is what you
          // actually watch.
          transitionTimingFunction: "cubic-bezier(0.12, 0.72, 0.08, 1)",
        }}
      >
        {cards.map((c, i) => {
          const a = angles[i];
          const won = settled && c.id === winnerId;
          const label = clip(cardLabel(c), 13);
          // Room for the label is the ARC the wedge occupies where the text sits, not its angle:
          // a thin wedge on a big wheel can still be too tight for 9px type.
          const room = 2 * Math.PI * R * ((LABEL_IN + LABEL_OUT) / 2) * fractions[i];
          // On the left half the spoke runs right-to-left, so the text would read upside down.
          // Anchor it at the far end and turn it the other way instead.
          const flip = a.mid > 180;
          return (
            <g key={c.id}>
              <path
                d={wedgePath(a.start, a.end)}
                fill={fillOf(cardToken(c, i), false)}
                stroke={won ? "var(--accent)" : "var(--panel)"}
                strokeWidth={won ? 2.5 : 1}
                opacity={settled && !won ? 0.4 : 1}
                style={{ transitionProperty: "opacity", transitionDuration: "260ms" }}
              />
              {room >= 11 && (
                <text
                  x={flip ? R - R * LABEL_IN : R + R * LABEL_IN}
                  y={R}
                  transform={`rotate(${flip ? a.mid + 90 : a.mid - 90} ${R} ${R})`}
                  textAnchor={flip ? "end" : "start"}
                  dominantBaseline="middle"
                  fontSize={9}
                  fontWeight={500}
                  fill="var(--ink2)"
                  stroke="var(--panel)"
                  strokeWidth={2.5}
                  strokeLinejoin="round"
                  paintOrder="stroke"
                  opacity={settled && !won ? 0.4 : 1}
                >
                  {label}
                </text>
              )}
            </g>
          );
        })}
        <circle cx={R} cy={R} r={R - 0.5} fill="none" stroke="var(--border2)" strokeWidth={1} />
        <circle cx={R} cy={R} r={8} fill="var(--panel)" stroke="var(--border2)" strokeWidth={1} />
      </svg>
    </div>
  );
}

/** Straws: one per card, held in a fist, each with its card's name written down it. Unpulled they
 *  all show the same stub — that IS the game — and the pull reveals the winner's as the long one. */
function Straws({ cards, winnerId, spinning, settled, ms }: StageProps) {
  const revealed = spinning || settled;
  const winner = cards.findIndex((c) => c.id === winnerId);
  const heights = useMemo(() => strawHeights(cards.length, winner), [cards.length, winner]);
  return (
    <div
      role="img"
      aria-label={`${cards.length} straw${cards.length === 1 ? "" : "s"} in a fist`}
      className="relative flex items-end justify-center gap-1.5"
      style={{ height: STAGE }}
    >
      {/* The fist, behind the straws so every name still reads. */}
      <div
        aria-hidden="true"
        className="absolute bottom-3 left-1/2 h-11 w-[min(100%,14rem)] -translate-x-1/2 rounded-[var(--radius)] border"
        style={{
          background: "color-mix(in oklab, var(--st-track) 40%, var(--panel))",
          borderColor: "var(--border2)",
        }}
      />
      {cards.map((c, i) => {
        const isWinner = c.id === winnerId;
        const h = revealed ? heights[i] : 104;
        return (
          <div
            key={c.id}
            className="relative flex w-6 justify-center overflow-hidden rounded-t-full border border-b-0"
            style={{
              height: h,
              background: fillOf(cardToken(c, i), false),
              borderColor: isWinner && settled ? "var(--accent)" : "var(--border2)",
              borderWidth: isWinner && settled ? 2 : 1,
              transitionProperty: "height, border-color, border-width",
              transitionDuration: `${ms}ms`,
              transitionTimingFunction: "cubic-bezier(0.2, 0.85, 0.2, 1)",
            }}
          >
            {/* Written DOWN the straw from its tip, so the name is legible at every length and
                the short ones simply run out of room rather than being clipped mid-word. */}
            <span
              className="pt-2 text-[0.5rem] leading-none text-ink2"
              style={{
                writingMode: "vertical-rl",
                maxHeight: Math.max(0, h - 14),
                overflow: "hidden",
                whiteSpace: "nowrap",
                textOverflow: "ellipsis",
              }}
            >
              {cardLabel(c)}
            </span>
          </div>
        );
      })}
    </div>
  );
}

/** Paper in a box: the box is shaken, the slips jostle inside it, and then one is lifted out with
 *  its name on it. The shake is a keyframe animation rather than a transition because the point of
 *  it is the wobble in between, which two end states can't express. */
function Box({ cards, winnerId, spinning, settled, ms }: StageProps) {
  const out = spinning || settled;
  const index = cards.findIndex((c) => c.id === winnerId);
  const winner = index >= 0 ? cards[index] : undefined;
  const shake = Math.round(ms * 0.55);
  return (
    <div
      role="img"
      aria-label={`A box of ${cards.length} folded slip${cards.length === 1 ? "" : "s"}`}
      className="relative flex items-end justify-center"
      style={{ height: STAGE, width: STAGE }}
    >
      {/* The drawn slip, on its way out. Mounted ALWAYS, and merely invisible until there is one to
          lift: an element that first appears at its destination has nothing to travel from. */}
      <div
        aria-hidden="true"
        className="absolute left-1/2 flex w-28 -translate-x-1/2 justify-center rounded-[var(--radius-sm)] border px-1.5 py-1.5 shadow-sm"
        style={{
          bottom: out && winner ? 132 : 72,
          opacity: out && winner ? 1 : 0,
          background: winner ? fillOf(cardToken(winner, index), false) : "var(--panel)",
          borderColor: "var(--border2)",
          transitionProperty: "bottom, opacity",
          transitionDuration: `${Math.max(0, ms - shake)}ms`,
          // It comes out AFTER the shaking, not during it.
          transitionDelay: `${shake}ms`,
          transitionTimingFunction: "cubic-bezier(0.2, 0.8, 0.2, 1)",
        }}
      >
        <span className="truncate text-[0.5625rem] leading-tight text-ink2">
          {winner ? clip(cardLabel(winner), 20) : ""}
        </span>
      </div>

      <div
        className={`relative h-28 w-40 overflow-hidden rounded-[var(--radius-sm)] border border-border2 bg-surface ${
          spinning ? "pm-game-shake" : ""
        }`}
        style={{ animationDuration: `${shake}ms` }}
      >
        <div className="absolute inset-x-1 bottom-1 flex flex-wrap items-end justify-center gap-1">
          {cards.slice(0, 16).map((c, i) => (
            <div
              key={c.id}
              className={`h-5 w-4 rounded-[2px] border ${spinning ? "pm-game-jostle" : ""}`}
              style={{
                background: fillOf(cardToken(c, i), false),
                borderColor: "var(--border2)",
                animationDuration: `${Math.max(1, Math.round(shake / 3))}ms`,
                animationDelay: `${i * 35}ms`,
              }}
            />
          ))}
        </div>
      </div>
    </div>
  );
}

/**
 * Every card in the folder and where it stands this round — the readable answer to "what is still
 * in play", which is the part of the game a wheel can't state plainly.
 *
 * The bar above it holds the three things you reach for BETWEEN plays: whether the game takes turns
 * at all, whether a drawn card leaves the folder, and starting over. All three live here rather
 * than in the dice menu because they are about the round in front of you, not about which game this
 * folder is.
 */
function Legend({
  folder,
  cards,
  winnerId,
  weighted,
  onWeight,
  onReset,
  onAutoPopOut,
  onRepeat,
}: {
  folder: Widget;
  cards: Widget[];
  winnerId: string | null;
  /** Whether this game gives out shares — only the wheel does, so only the wheel offers to edit
   *  them. Putting a share control on a game that ignores it would be a lie in a dropdown. */
  weighted: boolean;
  onWeight: (childId: string, weight: number) => void;
  onReset: () => void;
  onAutoPopOut: (next: boolean) => void;
  onRepeat: (next: boolean) => void;
}) {
  const round = keepsRound(folder);
  const drawn = round ? (folder.spent ?? []).length : 0;
  return (
    <div className="shrink-0 border-t border-rule px-3 py-2">
      <div className="mb-1.5 flex flex-wrap items-center justify-between gap-x-3 gap-y-1">
        <p className="text-[0.625rem] uppercase tracking-wide text-ink4">
          {!round
            ? `${cards.length} in, every time`
            : drawn > 0
              ? `${cards.length - drawn} of ${cards.length} still in`
              : `${cards.length} in`}
        </p>
        <div className="flex items-center gap-3">
          <label
            className="flex items-center gap-1.5 text-[0.6875rem] text-ink3"
            title="On, a card waits its turn once it has been picked. Off, every card is in every play and the same one can come up twice running."
          >
            <span>Grey out what it picks</span>
            <Toggle
              ariaLabel="Grey out a card once it has been picked"
              checked={round}
              onChange={(next) => onRepeat(!next)}
            />
          </label>
          <label
            className="flex items-center gap-1.5 text-[0.6875rem] text-ink3"
            title="A drawn card goes straight to the board instead of staying here, greyed."
          >
            <span>Move the winner out</span>
            <Toggle
              ariaLabel="Move a drawn card out to the board"
              checked={folder.autoPopOut === true}
              onChange={onAutoPopOut}
            />
          </label>
          {drawn > 0 && (
            <Button variant="tertiary" size="sm" onClick={onReset}>
              Start the round over
            </Button>
          )}
        </div>
      </div>
      <ul className={`flex flex-wrap gap-1 overflow-auto ${weighted ? "max-h-24" : "max-h-16"}`}>
        {cards.map((c, i) => {
          const spent = isSpent(folder, c.id);
          const chip = (
            <>
              <span className="truncate">{cardLabel(c)}</span>
              {/* A literal tick, NOT `&check;`. JSX decodes the XHTML entity set and nothing more,
                  so an HTML5-only name like that one reaches the screen as its own source text. */}
              {spent && <span aria-hidden="true">✓</span>}
            </>
          );
          const tint = {
            background: fillOf(cardToken(c, i), spent),
            borderColor: "var(--border2)",
          };
          const shell = `flex max-w-[13rem] items-center gap-1 rounded-[var(--radius-sm)] border px-1.5 py-0.5 text-[0.6875rem] ${
            spent ? "text-ink4" : "text-ink3"
          } ${c.id === winnerId ? "ring-1 ring-accent" : ""}`;
          // Colour alone can't carry "already drawn" — the text says it too, for anyone who can't
          // see the dimming.
          const said = spent
            ? `${cardLabel(c)} — already drawn this round`
            : c.id === winnerId
              ? `${cardLabel(c)} — just drawn`
              : cardLabel(c);
          return (
            <li key={c.id}>
              {weighted ? (
                // A <label> wrapping the Select, so the card's name IS the control's name — no
                // second string to keep in step, and nothing announced as an unnamed dropdown.
                <label
                  className={shell}
                  style={tint}
                  title={`How big a share ${cardLabel(c)} gets`}
                >
                  <span className="sr-only">{said}, share of the wheel</span>
                  {chip}
                  <Select
                    compact
                    value={String(weightOf(c))}
                    onChange={(e) => onWeight(c.id, Number(e.currentTarget.value))}
                    className="ml-0.5 border-0 bg-transparent px-0.5 py-0 text-[0.6875rem]"
                  >
                    {WEIGHT_CHOICES.map((w) => (
                      <option key={w} value={String(w)}>
                        {w === 1 ? "even" : `${w}×`}
                      </option>
                    ))}
                  </Select>
                </label>
              ) : (
                <span aria-label={said} className={shell} style={tint}>
                  {chip}
                </span>
              )}
            </li>
          );
        })}
      </ul>
    </div>
  );
}

/**
 * The dice in the folder's top bar, and the list it opens: which game this folder plays, or none.
 *
 * "Just a folder" is a row of its own rather than a matter of pressing the running game again. A
 * toggle you can only find by re-selecting the thing you already selected is a riddle, and this is
 * the one control that undoes the most visible change the feature makes — the folder's own tile.
 *
 * `escapeClipping` is not optional here — a folder tile is `overflow-hidden` and the panel body is
 * `overflow-auto`, so an ordinary absolute popover is cut off; and inside the Overlay presentation
 * (a Modal) only the escaped panel's fixed z-index keeps it above the scrim.
 */
export function GameMenu({
  folder,
  onChange,
}: {
  folder: Widget;
  onChange: (patch: Partial<Widget>) => void;
}) {
  const current = folder.game;
  const on = folder.gameOn === true;
  const row = (active: boolean) =>
    `w-full rounded-[var(--radius-sm)] px-2 py-1.5 text-left ${
      active ? "bg-accent text-accent-ink" : "text-ink2 hover:bg-surface"
    }`;
  return (
    <Popover
      align="right"
      escapeClipping
      ariaLabel="Folder games"
      panelClassName="w-64"
      rootClassName="shrink-0"
      trigger={({ open, toggle }) => (
        <button
          type="button"
          data-help="pinboard-folder-game"
          onPointerDown={(e) => e.stopPropagation()}
          onClick={toggle}
          aria-expanded={open}
          title="Gamble your next task"
          aria-label="Gamble your next task"
          className="inline-flex min-h-[var(--tap-min,24px)] min-w-[var(--tap-min,24px)] shrink-0 items-center justify-center rounded-[var(--radius-sm)] text-ink4 hover:bg-surface hover:text-ink2"
        >
          <DiceGlyph />
        </button>
      )}
    >
      {({ close }) => (
        <div className="flex flex-col gap-2 p-2">
          <p className="px-1 text-[0.625rem] uppercase tracking-wide text-ink4">
            Let the folder pick
          </p>
          <ul className="flex flex-col gap-0.5">
            <li>
              <button
                type="button"
                aria-pressed={!on}
                onClick={() => {
                  // The game is remembered, only switched off: come back to the dice and the one
                  // you were playing is still the one that's ticked.
                  onChange({ gameOn: false });
                  close();
                }}
                className={row(!on)}
              >
                <span className="flex items-center gap-2 text-xs font-medium">
                  <FolderFaceGlyph />
                  Just a folder
                </span>
                <span
                  className={`mt-0.5 block text-[0.6875rem] ${!on ? "text-accent-ink" : "text-ink4"}`}
                >
                  No game. The tile opens the cards, the way every other folder does.
                </span>
              </button>
            </li>
            {GAME_KINDS.map((k) => (
              <li key={k}>
                <button
                  type="button"
                  aria-pressed={on && current === k}
                  onClick={() => {
                    // Picking a game is also how you switch it on — the one gesture the card asks
                    // for.
                    onChange({ game: k, gameOn: true });
                    close();
                  }}
                  className={row(on && current === k)}
                >
                  <span className="flex items-center gap-2 text-xs font-medium">
                    <GameGlyph game={k} />
                    {GAME_INFO[k].label}
                  </span>
                  <span
                    className={`mt-0.5 block text-[0.6875rem] ${
                      on && current === k ? "text-accent-ink" : "text-ink4"
                    }`}
                  >
                    {GAME_INFO[k].blurb}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}
    </Popover>
  );
}

/** The double dice the card asked for: the way into the games, on an opened folder's top bar. */
function DiceGlyph() {
  return (
    <svg
      viewBox="0 0 24 24"
      aria-hidden="true"
      className="h-4 w-4"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.6}
    >
      <rect x="3" y="8" width="11" height="11" rx="2" />
      <rect x="10" y="3" width="11" height="11" rx="2" />
      <circle cx="7" cy="12" r="1" fill="currentColor" stroke="none" />
      <circle cx="10.5" cy="15.5" r="1" fill="currentColor" stroke="none" />
      <circle cx="15.5" cy="8.5" r="1" fill="currentColor" stroke="none" />
    </svg>
  );
}

/** The plain-folder face, shown beside "Just a folder" so the way out of the games is pictured as
 *  what it gives you back. Small twin of PinboardView's own FolderGlyph. */
function FolderFaceGlyph() {
  return (
    <svg
      viewBox="0 0 24 24"
      aria-hidden="true"
      className="h-6 w-6"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.6}
    >
      <path
        d="M3 7a1 1 0 0 1 1-1h5l2 2h8a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V7Z"
        strokeLinejoin="round"
      />
    </svg>
  );
}

/** The tile face a game folder wears on the board, so you can tell at a glance which folder plays
 *  what without opening it. Hand-rolled like `FolderGlyph` — the app has no icon library. */
export function GameGlyph({ game }: { game: GameKind }) {
  const common = {
    viewBox: "0 0 24 24",
    className: "h-6 w-6",
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 1.6,
    "aria-hidden": true as const,
  };
  if (game === "roulette") {
    return (
      <svg {...common}>
        <circle cx="12" cy="12" r="8" />
        <circle cx="12" cy="12" r="1.6" fill="currentColor" stroke="none" />
        <path d="M12 4v4M12 16v4M4 12h4M16 12h4" strokeLinecap="round" />
      </svg>
    );
  }
  if (game === "straws") {
    return (
      <svg {...common}>
        <path d="M7 20V7M12 20V4M17 20v-9" strokeLinecap="round" />
      </svg>
    );
  }
  if (game === "coin") {
    return (
      <svg {...common}>
        <circle cx="12" cy="12" r="8" />
        <ellipse cx="12" cy="12" rx="3.2" ry="8" />
      </svg>
    );
  }
  if (game === "rps") {
    return <ThrowGlyph hand="scissors" className="h-6 w-6" />;
  }
  return (
    <svg {...common}>
      <path d="M4 10h16v9a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1v-9Z" strokeLinejoin="round" />
      <path d="M9 10V6l3 2 3-2v4" strokeLinejoin="round" />
    </svg>
  );
}

/** "Move this card out of the folder, back onto the board" — shown on folder-panel children, and on
 *  a game's winner, which is the same move for the same reason. Lives here rather than in
 *  PinboardView so both can reach it without that file importing this one back. */
export function PopOutButton({ onClick }: { onClick: () => void }) {
  return (
    <button
      type="button"
      onPointerDown={(e) => e.stopPropagation()}
      onClick={onClick}
      title="Move out to the board"
      aria-label="Move out to the board"
      className="inline-flex min-h-[var(--tap-min,24px)] min-w-[var(--tap-min,24px)] shrink-0 items-center justify-center rounded-[var(--radius-sm)] text-xs text-ink4 hover:bg-surface hover:text-ink2"
    >
      ⤴
    </button>
  );
}
