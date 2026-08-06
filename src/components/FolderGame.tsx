// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The folder games — "gamble your next task".
//
// A folder can be handed a game (`lib/pinboard/game.ts` holds the rules). Its tile then plays
// instead of opening, and this is what it plays: a wheel, a fist of straws, or a box of folded
// slips, one per note inside. It lands on one of them. That note is the next thing you do.
//
// TWO RULES THIS FILE EXISTS TO HOLD:
//
// 1. THE OUTCOME IS DECIDED BEFORE THE ANIMATION STARTS, never by it. PM has two reduced-motion
//    signals and they fail in OPPOSITE directions — under the OS query the keyframes are never
//    emitted at all (so an `animationend` handler would never fire, and the game would hang
//    forever), while under the app's own "Reduced" setting they complete in 0.001ms (so the same
//    handler fires instantly and the result flashes past). Deciding first and treating the motion
//    as pure theatre is the only shape that behaves for everyone. `prefersReducedMotion()` is read
//    at click time — never cached, never from React context — and simply collapses the wait to zero.
//
// 2. THE RESULT IS ANNOUNCED, not merely drawn. A spinning wheel is a graphic; the answer to "what
//    should I do next" has to reach a screen reader too. The live region is mounted with the whole
//    surface rather than with the result, because an `aria-live` region only announces changes to a
//    region that ALREADY existed — one that appears alongside its own first message says nothing.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { CSSProperties } from "react";
import { Button, Popover, Toggle, VisuallyHidden } from "./ui";
import { prefersReducedMotion } from "../theme/motion";
import {
  cardLabel,
  draw,
  GAME_INFO,
  GAME_KINDS,
  isSpent,
  livePool,
  pool,
} from "../lib/pinboard/game";
import { TINT_PALETTE } from "../lib/pinboard/palette";
import type { GameKind, Widget } from "../lib/pinboard/types";

/** How long each game's theatre runs, in ms. Collapsed to 0 when motion is reduced. */
const SPIN_MS: Record<GameKind, number> = { roulette: 2600, straws: 1500, box: 1300 };

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
  onResetRound,
}: {
  folder: Widget;
  game: GameKind;
  onDraw: (childId: string) => void;
  onPopOut: (childId: string) => void;
  onResetRound: () => void;
}) {
  const [phase, setPhase] = useState<Phase>("idle");
  const [winnerId, setWinnerId] = useState<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const cards = useMemo(() => pool(folder), [folder]);
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
  }, [game]);

  const play = useCallback(() => {
    if (phase === "playing" || inPlay.length === 0) return;
    const winner = draw(inPlay, randomUnit());
    if (!winner) return;
    // Recorded NOW, not when the theatre finishes: the round is the honest part, and it must
    // survive the surface being closed (or PM being quit) halfway through a spin.
    setWinnerId(winner.id);
    onDraw(winner.id);
    const wait = prefersReducedMotion() ? 0 : SPIN_MS[game];
    if (wait === 0) {
      setPhase("done");
      return;
    }
    setPhase("playing");
    timer.current = setTimeout(() => setPhase("done"), wait);
  }, [phase, inPlay, game, onDraw]);

  const replay = useCallback(() => {
    setPhase("idle");
    setWinnerId(null);
  }, []);

  const winner = winnerId ? cards.find((c) => c.id === winnerId) : undefined;
  // The winner has left the folder (auto pop-out is on), so there is nothing to point at any more.
  const winnerGone = winnerId != null && !winner;
  const spinning = phase === "playing";
  const settled = phase === "done";

  return (
    <div className="flex h-full min-h-0 flex-col">
      {/* Mounted with the surface, not with the result — see the note at the top of this file. */}
      <VisuallyHidden role="status" aria-live="polite">
        {settled && winner ? `${info.label} chose ${cardLabel(winner)}.` : ""}
        {settled && winnerGone ? "Your card moved out to the board." : ""}
      </VisuallyHidden>

      <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 overflow-auto p-3">
        {cards.length === 0 ? (
          <EmptyGame />
        ) : (
          <>
            <Stage
              game={game}
              folder={folder}
              cards={cards}
              inPlay={inPlay}
              winnerId={winnerId}
              spinning={spinning}
              settled={settled}
            />
            <Verdict
              info={info}
              settled={settled}
              spinning={spinning}
              winner={winner}
              remaining={inPlay.length}
              onPlay={play}
              onAgain={replay}
              onPopOut={onPopOut}
            />
          </>
        )}
      </div>

      {cards.length > 0 && (
        <Legend
          folder={folder}
          cards={cards}
          winnerId={settled ? winnerId : null}
          onReset={onResetRound}
        />
      )}
    </div>
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
  remaining,
  onPlay,
  onAgain,
  onPopOut,
}: {
  info: { label: string; blurb: string; verb: string };
  settled: boolean;
  spinning: boolean;
  winner?: Widget;
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
        <p className="text-xs uppercase tracking-wide text-ink4">Do this next</p>
        <p className="max-w-xs text-sm font-medium text-ink">
          {winner ? cardLabel(winner) : "It moved out to your board."}
        </p>
        <div className="flex items-center gap-2">
          {winner && (
            <Button variant="secondary" size="sm" onClick={() => onPopOut(winner.id)}>
              Move out to the board
            </Button>
          )}
          <Button variant="tertiary" size="sm" onClick={onAgain} disabled={remaining === 0}>
            {remaining === 0 ? "That was the last one" : "Go again"}
          </Button>
        </div>
      </div>
    );
  }
  return (
    <div className="flex flex-col items-center gap-2 text-center">
      <p className="max-w-xs text-xs text-ink4">{info.blurb}</p>
      <Button size="sm" onClick={onPlay} disabled={remaining === 0}>
        {remaining === 0 ? "Nothing left to draw" : info.verb}
      </Button>
    </div>
  );
}

/** Which game is drawn on the stage. */
function Stage(props: {
  game: GameKind;
  folder: Widget;
  cards: Widget[];
  inPlay: Widget[];
  winnerId: string | null;
  spinning: boolean;
  settled: boolean;
}) {
  if (props.game === "roulette") return <Wheel {...props} />;
  if (props.game === "straws") return <Straws {...props} />;
  return <Box {...props} />;
}

interface StageProps {
  folder: Widget;
  cards: Widget[];
  inPlay: Widget[];
  winnerId: string | null;
  spinning: boolean;
  settled: boolean;
}

const WHEEL = 168;
const R = WHEEL / 2;

/** One wedge path, drawn from the centre. */
function wedgePath(from: number, to: number): string {
  const p = (deg: number) => {
    const rad = ((deg - 90) * Math.PI) / 180;
    return `${R + R * Math.cos(rad)} ${R + R * Math.sin(rad)}`;
  };
  const large = to - from > 180 ? 1 : 0;
  return `M ${R} ${R} L ${p(from)} A ${R} ${R} 0 ${large} 1 ${p(to)} Z`;
}

/** The roulette wheel: a wedge per card still in play, spun to land the winner under the pointer. */
function Wheel({ inPlay, winnerId, spinning, settled }: StageProps) {
  const n = Math.max(inPlay.length, 1);
  const step = 360 / n;
  const index = inPlay.findIndex((c) => c.id === winnerId);
  // Four whole turns for the theatre, then whatever brings the winner's wedge to the top. Held
  // once the spin is over so the wheel doesn't snap back on re-render.
  const angle = index >= 0 && (spinning || settled) ? 360 * 4 - (index * step + step / 2) : 0;
  const style: CSSProperties = {
    transform: `rotate(${angle}deg)`,
    transitionProperty: "transform",
    transitionDuration: spinning ? `${SPIN_MS.roulette}ms` : "0ms",
    transitionTimingFunction: "cubic-bezier(0.15, 0.85, 0.2, 1)",
  };
  return (
    <div className="relative" style={{ width: WHEEL, height: WHEEL + 10 }}>
      {/* The pointer the wedge stops under. */}
      <svg
        viewBox="0 0 12 10"
        aria-hidden="true"
        className="absolute left-1/2 top-0 z-10 w-3 -translate-x-1/2 text-ink2"
        fill="currentColor"
      >
        <path d="M6 10 L0 0 L12 0 Z" />
      </svg>
      <svg
        viewBox={`0 0 ${WHEEL} ${WHEEL}`}
        role="img"
        aria-label={`A wheel with ${inPlay.length} wedge${inPlay.length === 1 ? "" : "s"}`}
        className="absolute bottom-0 h-[168px] w-[168px]"
        style={style}
      >
        {inPlay.map((c, i) => (
          <path
            key={c.id}
            d={wedgePath(i * step, (i + 1) * step)}
            fill={fillOf(cardToken(c, i), false)}
            stroke="var(--panel)"
            strokeWidth={1}
          />
        ))}
        <circle cx={R} cy={R} r={R - 0.5} fill="none" stroke="var(--border2)" strokeWidth={1} />
        <circle cx={R} cy={R} r={7} fill="var(--panel)" stroke="var(--border2)" strokeWidth={1} />
      </svg>
    </div>
  );
}

/** Straws: one per card in play, revealed at their lengths — the winner's is the long one. */
function Straws({ inPlay, winnerId, spinning, settled }: StageProps) {
  const revealed = spinning || settled;
  return (
    <div
      role="img"
      aria-label={`${inPlay.length} straw${inPlay.length === 1 ? "" : "s"}`}
      className="flex h-[168px] items-end justify-center gap-1.5"
    >
      {inPlay.map((c, i) => {
        const isWinner = c.id === winnerId;
        // Unrevealed, every straw shows the same stub — that IS the game. Revealed, the winner is
        // tallest and the rest fall short by a stable, per-straw amount (no re-roll on re-render).
        const short = 46 + ((i * 37) % 58);
        const height = revealed ? (isWinner ? 156 : short) : 62;
        return (
          <div
            key={c.id}
            title={cardLabel(c)}
            className="w-3 rounded-t-[var(--radius-sm)] border border-b-0"
            style={{
              height,
              background: fillOf(cardToken(c, i), false),
              borderColor: "var(--border2)",
              transitionProperty: "height",
              transitionDuration: spinning ? `${SPIN_MS.straws}ms` : "0ms",
              transitionTimingFunction: "cubic-bezier(0.2, 0.8, 0.2, 1)",
            }}
          />
        );
      })}
    </div>
  );
}

/** Paper in a box: folded slips that jostle, and one that rises out. */
function Box({ inPlay, winnerId, spinning, settled }: StageProps) {
  const out = settled || spinning;
  return (
    <div
      role="img"
      aria-label={`A box of ${inPlay.length} folded slip${inPlay.length === 1 ? "" : "s"}`}
      className="relative flex h-[168px] w-[168px] items-end justify-center"
    >
      {/* The drawn slip, on its way out of the box. */}
      {inPlay.map((c, i) => {
        if (c.id !== winnerId) return null;
        return (
          <div
            key={c.id}
            className="absolute left-1/2 w-16 -translate-x-1/2 rounded-[var(--radius-sm)] border px-1 py-2"
            style={{
              bottom: out ? 118 : 40,
              opacity: out ? 1 : 0,
              background: fillOf(cardToken(c, i), false),
              borderColor: "var(--border2)",
              transitionProperty: "bottom, opacity",
              transitionDuration: spinning ? `${SPIN_MS.box}ms` : "0ms",
              transitionTimingFunction: "cubic-bezier(0.2, 0.8, 0.2, 1)",
            }}
          />
        );
      })}
      {/* The box, and the slips still in it. */}
      <div className="relative h-24 w-36 overflow-hidden rounded-[var(--radius-sm)] border border-border2 bg-surface">
        <div className="absolute inset-x-1 bottom-1 flex flex-wrap items-end justify-center gap-1">
          {inPlay.slice(0, 14).map((c, i) => (
            <div
              key={c.id}
              className="h-5 w-4 rounded-[2px] border"
              style={{ background: fillOf(cardToken(c, i), false), borderColor: "var(--border2)" }}
            />
          ))}
        </div>
      </div>
    </div>
  );
}

/** Every card in the folder and where it stands this round — the readable answer to "what is still
 *  in play", which is the part of the game a wheel can't state plainly. Doubles as the reset. */
function Legend({
  folder,
  cards,
  winnerId,
  onReset,
}: {
  folder: Widget;
  cards: Widget[];
  winnerId: string | null;
  onReset: () => void;
}) {
  const drawn = (folder.spent ?? []).length;
  return (
    <div className="shrink-0 border-t border-rule px-3 py-2">
      <div className="mb-1 flex items-center justify-between gap-2">
        <p className="text-[0.625rem] uppercase tracking-wide text-ink4">
          {drawn > 0 ? `${cards.length - drawn} of ${cards.length} still in` : `${cards.length} in`}
        </p>
        {drawn > 0 && (
          <Button variant="tertiary" size="sm" onClick={onReset}>
            Start the round over
          </Button>
        )}
      </div>
      <ul className="flex max-h-16 flex-wrap gap-1 overflow-auto">
        {cards.map((c, i) => {
          const spent = isSpent(folder, c.id);
          return (
            <li
              key={c.id}
              // Colour alone can't carry "already drawn" — the text says it too, for anyone who
              // can't see the dimming.
              aria-label={
                spent
                  ? `${cardLabel(c)} — already drawn this round`
                  : c.id === winnerId
                    ? `${cardLabel(c)} — just drawn`
                    : cardLabel(c)
              }
              className={`flex max-w-[11rem] items-center gap-1 rounded-[var(--radius-sm)] border px-1.5 py-0.5 text-[0.6875rem] ${
                spent ? "text-ink4" : "text-ink3"
              } ${c.id === winnerId ? "ring-1 ring-accent" : ""}`}
              style={{
                background: fillOf(cardToken(c, i), spent),
                borderColor: "var(--border2)",
              }}
            >
              <span className="truncate">{cardLabel(c)}</span>
              {spent && <span aria-hidden="true">&check;</span>}
            </li>
          );
        })}
      </ul>
    </div>
  );
}

/**
 * The dice in the folder's top bar, and the list it opens: which game this folder plays, whether
 * it plays one at all, and what happens to a card once it's drawn.
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
            {GAME_KINDS.map((k) => (
              <li key={k}>
                <button
                  type="button"
                  aria-pressed={on && current === k}
                  onClick={() => {
                    // Picking a game is also how you switch it on — the one gesture the card asks
                    // for. Picking the one already running turns it back off, so the dice is a way
                    // out as well as a way in.
                    const same = on && current === k;
                    onChange({ game: k, gameOn: !same });
                    close();
                  }}
                  className={`w-full rounded-[var(--radius-sm)] px-2 py-1.5 text-left ${
                    on && current === k ? "bg-accent text-accent-ink" : "text-ink2 hover:bg-surface"
                  }`}
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
          <label className="flex items-center justify-between gap-2 border-t border-rule px-1 pt-2 text-xs text-ink3">
            <span>
              Move the winner out
              <span className="mt-0.5 block text-[0.6875rem] text-ink4">
                A drawn card goes straight to the board instead of staying here, greyed.
              </span>
            </span>
            <Toggle
              ariaLabel="Move the winner out to the board"
              checked={folder.autoPopOut === true}
              onChange={(next) => onChange({ autoPopOut: next })}
            />
          </label>
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

/** The tile face a game folder wears on the board, so you can tell at a glance which folder plays
 *  what without opening it. Hand-rolled like `FolderGlyph` — the app has no icon library. */
export function GameGlyph({ game }: { game: GameKind }) {
  const common = {
    viewBox: "0 0 24 24",
    className: "h-6 w-6",
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 1.6,
  } as const;
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
