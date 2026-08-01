// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Button's sizing contract. The defect this locks down is subtle and was invisible for months: 50
// call sites passed their own `px-*`/`py-*`/`text-*`, and because `cn()` is a plain joiner the
// classes did not replace the base — both survived, and Tailwind's emission order decided. Spacing
// is emitted ASCENDING, so the base `px-3 py-1.5` beat every `px-2`/`py-1` a call site asked for;
// font sizes are emitted alphabetically, so `text-xs` DID beat `text-sm`. Every compact button in
// PM was therefore a full-size box with shrunken type, and nothing failed.
//
// So the assertions here are COUNTS, not `toContain`. `toContain("px-2")` passes just as happily on
// a re-layered class list that also carries `px-3` — a count is the only shape that catches the
// regression this seam exists to prevent.
//
// `useTheme` is stubbed rather than wrapped in the real provider (which pulls in IPC) — the same
// stub ErrorBoundary.test.tsx and ConnectorItemRow.test.tsx use. `system` is mutable so the
// terminal branch can be exercised without a second mock.

import { render, cleanup } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const theme = vi.hoisted(() => ({ system: "slate" as "slate" | "editorial" | "terminal" }));
vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: theme.system, mode: "dark", accent: "mono", depth: "standard" }),
}));

import { Button, type ButtonSize } from "./Button";
import { Select } from "./Select";
import { TONE_MIX } from "./tone";

afterEach(() => {
  theme.system = "slate";
  cleanup();
});

/** Only the font-SIZE utilities — `text-ink2` and `disabled:text-faint` are colours and must not
 *  count. The `(?:^|\s)` anchor is what excludes the variant-prefixed forms. */
const TEXT_SIZE = /(?:^|\s)(text-(?:xs|sm|base|lg|xl|\[[^\]]+\]))/g;
const PX = /(?:^|\s)(px-[\w.[\]/-]+)/g;
const PY = /(?:^|\s)(py-[\w.[\]/-]+)/g;

function sizing(className: string) {
  const one = (re: RegExp) => [...className.matchAll(re)].map((m) => m[1]);
  return { px: one(PX), py: one(PY), text: one(TEXT_SIZE) };
}

const SIZES: ButtonSize[] = ["xs", "sm", "md", "lg"];

describe("Button sizing", () => {
  it.each(SIZES)("emits exactly one px / py / text-size at %s", (size) => {
    const { getByRole } = render(<Button size={size}>Go</Button>);
    const { px, py, text } = sizing(getByRole("button").className);
    expect(px).toHaveLength(1);
    expect(py).toHaveLength(1);
    expect(text).toHaveLength(1);
  });

  it("leaves the 200 unannotated call sites exactly where they were", () => {
    const { getByRole } = render(<Button>Go</Button>);
    expect(sizing(getByRole("button").className)).toEqual({
      px: ["px-3"],
      py: ["py-1.5"],
      text: ["text-sm"],
    });
  });

  it("matches Select's compact triple at sm, so the two primitives line up", () => {
    const { getByRole: getButton } = render(
      <Button size="sm" aria-label="Add">
        Add
      </Button>,
    );
    const btn = sizing(getButton("button").className);
    cleanup();
    const { getByRole: getSelect } = render(
      <Select compact aria-label="Zone">
        <option>UTC</option>
      </Select>,
    );
    const sel = sizing(getSelect("combobox").className);
    expect(btn).toEqual(sel);
  });

  it("keeps the tap-target floor at every size", () => {
    // WCAG 2.5.8. jsdom computes no layout, so this asserts the floor is still DECLARED — the
    // rendered height at xs is entirely --tap-min, which is why xs and sm share a height at
    // Standard density and differ only in horizontal padding.
    for (const size of SIZES) {
      cleanup();
      const { getByRole } = render(<Button size={size}>Go</Button>);
      const cls = getByRole("button").className;
      expect(cls).toContain("min-h-[var(--tap-min,24px)]");
      expect(cls).toContain("min-w-[var(--tap-min,24px)]");
    }
  });

  it("never lets size reach the DOM", () => {
    // <button> has no `size` attribute (only input/select do), so TypeScript cannot catch this one:
    // leaving `size` in `...rest` would type-check and quietly emit an invalid attribute.
    const { getByRole } = render(<Button size="sm">Go</Button>);
    expect(getByRole("button").hasAttribute("size")).toBe(false);
  });

  it("still brackets the label under the terminal System at the smallest size", () => {
    theme.system = "terminal";
    const { getByRole } = render(<Button size="xs">×</Button>);
    const btn = getByRole("button");
    expect(btn.textContent).toContain("[");
    expect(btn.textContent).toContain("]");
    expect(btn.className).toContain("font-mono");
    expect(btn.className).toContain("min-w-[var(--tap-min,24px)]");
  });
});

describe("Button danger variant", () => {
  it("takes its tint from the shared tone map, not a fifth hand-typed ratio", () => {
    const { getByRole } = render(<Button variant="danger">Delete</Button>);
    const cls = getByRole("button").className;
    // Spelled literally in Button.tsx because Tailwind extracts class names by scanning source
    // text; this is what stops the literal drifting away from tone.ts.
    expect(cls).toContain(`var(--st-due)_${TONE_MIX.fill}%`);
    expect(cls).toContain(`var(--st-due)_${TONE_MIX.fillHover}%`);
    expect(cls).not.toContain("#");
  });

  it("carries no inline style, so a call site never has to hand-tint a primary", () => {
    const { getByRole } = render(<Button variant="danger">Delete</Button>);
    expect(getByRole("button").getAttribute("style")).toBeNull();
  });

  it("sizes independently of its variant", () => {
    const { getByRole } = render(
      <Button variant="danger" size="sm">
        Delete
      </Button>,
    );
    const { px, py, text } = sizing(getByRole("button").className);
    expect(px).toEqual(["px-2"]);
    expect(py).toEqual(["py-1"]);
    expect(text).toEqual(["text-xs"]);
  });
});
