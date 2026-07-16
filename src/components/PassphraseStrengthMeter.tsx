// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The reusable passphrase strength meter (M-4). Every passphrase-CREATION input renders one so the
// user gets live feedback that matches the backend floor exactly — it scores through the same zxcvbn
// model via the `score_passphrase` command, never a separate frontend heuristic. Purely advisory: the
// command-layer `validate_passphrase_strength` is the real gate. NEVER wire it into an unlock/restore
// input, where an old, weak-but-valid passphrase must still be accepted.

import { useEffect, useRef, useState } from "react";

import { scorePassphrase } from "../lib/ipc";
import type { PassphraseScore } from "../lib/types";

const SCORE_LABEL = ["Very weak", "Weak", "Fair", "Good", "Strong"];
// Tone per score, using design-system status tokens (no raw colour). Weak → due, fair → look,
// good/strong → quick.
const SEGMENT_TONE = ["bg-st-due", "bg-st-due", "bg-st-look", "bg-st-quick", "bg-st-quick"];

/**
 * Live strength feedback for a passphrase being chosen. Debounces scoring, renders a four-segment
 * bar + label + the strongest zxcvbn hint, and reports each result via `onScored` so the parent can
 * gate its submit button (`score.acceptable`).
 */
export function PassphraseStrengthMeter({
  passphrase,
  onScored,
}: {
  passphrase: string;
  onScored?: (score: PassphraseScore | null) => void;
}) {
  const [score, setScore] = useState<PassphraseScore | null>(null);
  // Hold `onScored` in a ref so a fresh inline callback each render doesn't re-fire the effect.
  const onScoredRef = useRef(onScored);
  onScoredRef.current = onScored;

  useEffect(() => {
    if (passphrase.length === 0) {
      setScore(null);
      onScoredRef.current?.(null);
      return;
    }
    let cancelled = false;
    const timer = setTimeout(() => {
      scorePassphrase(passphrase)
        .then((s) => {
          if (cancelled) return;
          setScore(s);
          onScoredRef.current?.(s);
        })
        .catch(() => {
          // A scoring hiccup shouldn't block the user — the backend floor still enforces on submit.
        });
    }, 150);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [passphrase]);

  if (!score) return null;

  // Same order as the backend floor: padding first (it's the actionable half, and being told
  // "too short" or "too guessable" would send the user the wrong way), then length, then zxcvbn.
  const hint = score.padded
    ? "Can't start or end with a space — remove it. Spaces inside are fine."
    : score.too_short
      ? "Use at least 10 characters."
      : (score.warning ?? score.suggestions[0] ?? null);

  return (
    <div className="flex flex-col gap-1" aria-live="polite">
      <div className="flex items-center gap-2">
        <div className="flex flex-1 gap-1">
          {[0, 1, 2, 3].map((i) => (
            <div
              key={i}
              className={`h-1 flex-1 rounded-full ${
                i < score.score ? SEGMENT_TONE[score.score] : "bg-border2"
              }`}
            />
          ))}
        </div>
        <span className={`text-xs ${score.acceptable ? "text-st-quick" : "text-ink4"}`}>
          {SCORE_LABEL[score.score]}
        </span>
      </div>
      {!score.acceptable && hint && <p className="text-xs text-ink4">{hint}</p>}
    </div>
  );
}
