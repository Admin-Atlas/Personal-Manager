// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// SectionLabel's contract: a Settings section head is a HEADING, not an orphan `<label>`. The
// assertion that pins the actual defect is "omitting htmlFor renders no <label> at all" — 29 of the
// 32 heads in the tree are labels naming nothing, which is what a screen reader announces them as.

import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { SectionLabel } from "./SectionLabel";

afterEach(cleanup);

describe("SectionLabel", () => {
  it("renders a level-2 heading by default", () => {
    const { getByRole } = render(<SectionLabel>Appearance</SectionLabel>);
    expect(getByRole("heading", { level: 2, name: "Appearance" })).toBeTruthy();
  });

  it("renders a level-3 heading with as='h3'", () => {
    const { getByRole } = render(<SectionLabel as="h3">Signals</SectionLabel>);
    expect(getByRole("heading", { level: 3, name: "Signals" })).toBeTruthy();
  });

  it("renders NO heading with as='span' — a heading inside Collapsible's button is invalid HTML", () => {
    const { queryByRole, getByText } = render(<SectionLabel as="span">Sources</SectionLabel>);
    expect(queryByRole("heading")).toBeNull();
    expect(getByText("Sources").tagName).toBe("SPAN");
  });

  it("renders a <label> for that control, and no heading, when htmlFor is given", () => {
    const { container, queryByRole } = render(
      <SectionLabel htmlFor="ai-memory-paste">Import AI memory</SectionLabel>,
    );
    const label = container.querySelector("label");
    expect(label?.getAttribute("for")).toBe("ai-memory-paste");
    expect(queryByRole("heading")).toBeNull();
  });

  it("renders NO label element when htmlFor is omitted", () => {
    const { container } = render(<SectionLabel>Backup passphrase</SectionLabel>);
    expect(container.querySelector("label")).toBeNull();
  });

  it("renders `action` as a sibling of the head, aligned to centre by default", () => {
    const { getByRole, getByText } = render(
      <SectionLabel action={<button>Reset</button>}>Contrast</SectionLabel>,
    );
    const head = getByRole("heading", { level: 2 });
    const action = getByText("Reset");
    expect(head.parentElement).toBe(action.parentElement);
    expect(head.parentElement?.className).toContain("items-center");
    expect(head.parentElement?.className).not.toContain("items-baseline");
  });

  it("aligns `action` on the baseline with align='baseline'", () => {
    const { getByRole } = render(
      <SectionLabel action={<span>12</span>} align="baseline">
        Recommended models
      </SectionLabel>,
    );
    const wrapper = getByRole("heading", { level: 2 }).parentElement;
    expect(wrapper?.className).toContain("items-baseline");
    expect(wrapper?.className).not.toContain("items-center");
  });

  it("renders no wrapper row when there is no action", () => {
    const { container, getByRole } = render(<SectionLabel>Search</SectionLabel>);
    expect(getByRole("heading", { level: 2 }).parentElement).toBe(container);
  });

  it("wears the one section-head recipe, with `block` only when it is not a span", () => {
    const { getByRole } = render(<SectionLabel>Motion</SectionLabel>);
    expect(getByRole("heading", { level: 2 }).className).toBe(
      "block font-mono text-xs font-medium uppercase tracking-wide text-ink3",
    );
    cleanup();
    const { getByText } = render(<SectionLabel as="span">Motion</SectionLabel>);
    expect(getByText("Motion").className).toBe(
      "font-mono text-xs font-medium uppercase tracking-wide text-ink3",
    );
  });
});

// The regression net that matters more than the unit cases above — "this class string has exactly
// one home" — is a source scan, and it lives with the other design rules in
// `src/theme/designGuards.test.ts`.
