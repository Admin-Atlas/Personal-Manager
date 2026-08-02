// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Toggle's contract, and the regression that made it worth pinning: the knob must re-pick its
// token when `checked` changes, because the surface underneath it changes at the same moment.
//
// The knob used to be an unconditional `bg-accent-ink`. That token is calibrated ONLY against the
// accent fill it sits on when the switch is ON; when OFF the knob sits on `--surface` instead and
// the token carries no contract there at all. Under the `mono` (Eigengrau) accent `--accent-ink`
// and `--bg` are the same literal, so the OFF knob was drawn in the page background — 1.00:1
// against the page and 1.04:1 against its own track, on the default slate + dark + mono install.

import { render, cleanup } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { Toggle } from "./Toggle";

afterEach(cleanup);

/** The knob is the innermost span: button > track span > knob span. */
function knobOf(btn: HTMLElement): HTMLElement {
  const knob = btn.querySelector("span > span");
  if (!knob) throw new Error("Toggle rendered no knob");
  return knob as HTMLElement;
}

describe("Toggle", () => {
  it("paints the OFF knob in a neutral role, never the accent's ink", () => {
    const { getByRole } = render(<Toggle checked={false} onChange={() => {}} ariaLabel="Sync" />);
    const knob = knobOf(getByRole("switch", { name: "Sync" }));
    // `ink4` is the lowest neutral role `boost()` floors at 4.5:1 against bg/panel/surface at
    // every Contrast level (contrast.test.ts pins that), so the OFF knob inherits a real floor
    // instead of depending on whichever accent the user picked.
    expect(knob.className).toContain("bg-ink4");
    expect(knob.className).not.toContain("bg-accent-ink");
  });

  it("paints the ON knob in the accent's ink, which is the surface it now sits on", () => {
    const { getByRole } = render(<Toggle checked onChange={() => {}} ariaLabel="Sync" />);
    const knob = knobOf(getByRole("switch", { name: "Sync" }));
    expect(knob.className).toContain("bg-accent-ink");
    expect(knob.className).not.toContain("bg-ink4");
  });

  it("moves the track with the knob, so the pair is always re-picked together", () => {
    const { getByRole, rerender } = render(
      <Toggle checked={false} onChange={() => {}} ariaLabel="Sync" />,
    );
    const track = () => getByRole("switch", { name: "Sync" }).querySelector("span") as HTMLElement;
    expect(track().className).toContain("bg-surface");
    rerender(<Toggle checked onChange={() => {}} ariaLabel="Sync" />);
    expect(track().className).toContain("bg-accent");
  });

  it("outlines the track in `--ink4` in BOTH states, the only token that clears 3:1 everywhere", () => {
    // A switch is a UI component, so its boundary owes 3:1 (WCAG 1.4.11). The fill never gave one —
    // `--surface` on `--panel` measures 1.03–1.16:1 — and neither did the two obvious outlines:
    // `--border2` is 1.42–1.84:1 at the default Contrast (2.82–4.29:1 even at `high`), and an ON
    // track outlined in `--accent` draws a line in the colour underneath it. Both shipped and both
    // read as no outline at all. `--ink4` measures 4.77–6.76:1 against surface AND panel at every
    // level; contrast.test.ts sweeps that across every System × Mode × Accent so this class cannot
    // drift back onto a token that only looks like an edge.
    //
    // ONE class, outside the checked branch, is the point: the outline must not depend on the
    // accent, because the accent is exactly what fails in light mode (down to 1.36:1).
    const trackOf = (el: HTMLElement) => el.querySelector("span") as HTMLElement;
    const off = render(<Toggle checked={false} onChange={() => {}} ariaLabel="Sync" />);
    const offTrack = trackOf(off.getByRole("switch", { name: "Sync" }));
    expect(offTrack.className).toContain("shadow-[inset_0_0_0_1px_var(--ink4)]");
    cleanup();
    const on = render(<Toggle checked onChange={() => {}} ariaLabel="Sync" />);
    const onTrack = trackOf(on.getByRole("switch", { name: "Sync" }));
    // An inset SHADOW, not a border: a border would eat a pixel off the padding box that the
    // knob's `left`/`top` resolve against, shrinking its inset from 2px to 1px and landing the two
    // states on different device-pixel boundaries at a fractional DPR.
    expect(onTrack.className).toContain("shadow-[inset_0_0_0_1px_var(--ink4)]");
    expect(onTrack.className).not.toContain("var(--accent)]");
  });

  it("keeps its own disabled alpha — it is the switch's only inert cue", () => {
    // Unlike Button, Toggle has no disabled colour branch, so the wrapper opacity cannot be
    // dropped the way Button's `disabled:opacity-40` was: without it a disabled switch renders
    // pixel-identical to an enabled one. DESIGN_TOKENS.md §7 records this asymmetry.
    const { getByRole } = render(
      <Toggle checked={false} onChange={() => {}} ariaLabel="Sync" disabled />,
    );
    const btn = getByRole("switch", { name: "Sync" });
    expect(btn.className).toContain("opacity-50");
    expect(btn.className).toContain("cursor-not-allowed");
  });
});
