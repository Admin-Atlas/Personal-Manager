// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Callout's contract, and each case is a net under a defect that was live in the tree:
//
// • ~27 dynamic error banners never announced, so a blind user got no feedback that an action had
//   failed. `danger` announcing BY DEFAULT is the fix, and the first three cases pin the asymmetry
//   (danger announces, info/warning stay quiet unless asked, and anything can opt out).
// • The danger tint had drifted into five rival ratios across ~45 hand-written blocks. The recipe
//   cases assert the rendered style comes from `tone.ts` and contains no hex, so a sixth ratio
//   cannot be typed into a call site's `style` and pass review unnoticed.
// • `cn()` is a plain joiner: if the primitive emitted its own margin, a caller's margin would not
//   replace it — both would survive and stylesheet order would pick. The margin case asserts the
//   primitive emits none at all, which is the only version of that guarantee that holds.

import { render, cleanup } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { Callout } from "./Callout";
import { TONE_MIX, TONE_TEXT_TOKEN, TONE_TOKEN, toneMix, type Tone } from "./tone";

afterEach(cleanup);

/** Any Tailwind margin utility, including the negative and per-axis forms. */
const MARGIN = /(^|\s)-?m[trblxyse]?-/;

describe("Callout live regions", () => {
  it("announces danger by default", () => {
    const { getByRole } = render(<Callout>Couldn't save</Callout>);
    expect(getByRole("alert").textContent).toBe("Couldn't save");
  });

  it("stays silent when danger opts out", () => {
    const { queryByRole } = render(<Callout live={false}>This cannot be undone</Callout>);
    expect(queryByRole("alert")).toBeNull();
    expect(queryByRole("status")).toBeNull();
  });

  it("keeps info and warning silent unless asked", () => {
    const { queryByRole } = render(
      <>
        <Callout tone="info">First sync takes a while</Callout>
        <Callout tone="warning">Your backup is older than the schedule</Callout>
      </>,
    );
    expect(queryByRole("alert")).toBeNull();
    expect(queryByRole("status")).toBeNull();
  });

  it("gives info and warning role=status when live is passed", () => {
    const { getAllByRole, queryByRole } = render(
      <>
        <Callout tone="info" live>
          Fell back to the cloud
        </Callout>
        <Callout tone="warning" live>
          Nearly out of room
        </Callout>
      </>,
    );
    expect(getAllByRole("status")).toHaveLength(2);
    // status, never alert — an interruption is reserved for something that went wrong.
    expect(queryByRole("alert")).toBeNull();
  });
});

describe("Callout tone recipe", () => {
  const tones: Tone[] = ["info", "warning", "danger"];

  it.each(tones)("takes %s's surface from the tone map alone", (tone) => {
    const { container } = render(
      <Callout tone={tone} live={false}>
        message
      </Callout>,
    );
    const el = container.firstElementChild as HTMLElement;
    expect(el.style.background).toBe(toneMix(TONE_TOKEN[tone], TONE_MIX.surface));
    expect(el.style.borderColor).toBe(toneMix(TONE_TOKEN[tone], TONE_MIX.border));
    expect(el.style.color).toBe(`var(${TONE_TEXT_TOKEN[tone]})`);
    // No hex anywhere: the mix resolves against whatever the four runtime axes made the token, so a
    // literal would freeze one System/Mode/Accent combination into the component.
    expect(el.getAttribute("style")).not.toContain("#");
  });

  it("emits one ratio per axis, not a second copy in the class list", () => {
    const { container } = render(<Callout>message</Callout>);
    const el = container.firstElementChild as HTMLElement;
    expect(el.className).not.toContain("color-mix");
    expect(el.className).not.toContain("st-due");
  });

  it("leaves the text inheriting when the body is neutral", () => {
    const { container } = render(
      <Callout body="ink" live={false}>
        prose and controls
      </Callout>,
    );
    const el = container.firstElementChild as HTMLElement;
    expect(el.style.color).toBe("");
    // The surface is still the tone's — only the message colour steps back.
    expect(el.style.background).toBe(toneMix(TONE_TOKEN.danger, TONE_MIX.surface));
  });

  it("lets a call site with a genuine reason override the surface", () => {
    const { container } = render(
      <Callout style={{ background: "var(--bg)" }}>floating over content</Callout>,
    );
    const el = container.firstElementChild as HTMLElement;
    expect(el.style.background).toBe("var(--bg)");
    // …without losing the rest of the recipe.
    expect(el.style.borderColor).toBe(toneMix(TONE_TOKEN.danger, TONE_MIX.border));
  });
});

describe("Callout layout", () => {
  it("emits no margin of its own, in either variant or size", () => {
    for (const variant of ["box", "strip"] as const) {
      for (const size of ["sm", "md"] as const) {
        cleanup();
        const { container } = render(
          <Callout variant={variant} size={size}>
            message
          </Callout>,
        );
        const el = container.firstElementChild as HTMLElement;
        expect(el.className).not.toMatch(MARGIN);
      }
    }
  });

  it("keeps the caller's spacing and placement alongside its own framing", () => {
    const { getByRole } = render(<Callout className="absolute left-4 top-4 z-10 mt-3">up</Callout>);
    const el = getByRole("alert");
    expect(el.className).toContain("mt-3");
    expect(el.className).toContain("absolute");
    expect(el.className).toContain("border");
  });

  it("swaps the framing per variant instead of layering it", () => {
    const { container: box } = render(<Callout variant="box">a</Callout>);
    const boxCls = (box.firstElementChild as HTMLElement).className;
    cleanup();
    const { container: strip } = render(<Callout variant="strip">b</Callout>);
    const stripCls = (strip.firstElementChild as HTMLElement).className;
    expect(boxCls).toContain("rounded-[var(--radius-sm)]");
    expect(stripCls).not.toContain("rounded-");
    expect(stripCls).toContain("border-b");
    // One padding pair each — a layered pair would leave the winner to stylesheet order.
    expect(boxCls.match(/(^|\s)px-/g)).toHaveLength(1);
    expect(stripCls.match(/(^|\s)px-/g)).toHaveLength(1);
  });

  it("renders a <p> when asked, so a div never lands inside a paragraph", () => {
    const { getByRole } = render(<Callout as="p">boom</Callout>);
    expect(getByRole("alert").tagName).toBe("P");
  });

  it("passes data-* attributes through for HelpOverlay", () => {
    const { getByRole } = render(<Callout data-help="vault-error">boom</Callout>);
    expect(getByRole("alert").getAttribute("data-help")).toBe("vault-error");
  });

  it("lets a site that already owns its live region keep authority", () => {
    // VaultUnlock spreads useFieldA11y's errorProps, which carry role + the describedby id.
    const { getByRole } = render(
      <Callout live={false} role="alert" id="vault-err">
        wrong passphrase
      </Callout>,
    );
    expect(getByRole("alert").id).toBe("vault-err");
  });
});
