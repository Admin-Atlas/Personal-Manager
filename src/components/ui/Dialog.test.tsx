// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Dialog's contract, and the whole reason it exists: `getByRole("dialog", { name })` RESOLVES.
// That query returns nothing for 12 of PM's 19 dialogs today — including "Remove this data?",
// "Final confirmation", "Delete <project>" and "Remove this tag?" — because `role="dialog"` shipped
// with no accessible name. Here the name is a consequence of passing `title`, which is required.

import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

// Dialog's bar chrome renders a <Button>, which reaches for `useTheme`; the real provider pulls in
// IPC. Same stub the other component tests use.
vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { Dialog } from "./Dialog";
import { TONE_TEXT_TOKEN } from "./tone";

afterEach(cleanup);

const noop = () => {};

describe("Dialog", () => {
  it("names the card chrome from its title", () => {
    const { getByRole } = render(
      <Dialog open onClose={noop} title="Remove this tag?">
        <p className="mt-2">body</p>
      </Dialog>,
    );
    expect(getByRole("dialog", { name: "Remove this tag?" })).toBeTruthy();
    expect(getByRole("heading", { level: 2, name: "Remove this tag?" })).toBeTruthy();
  });

  it("names the bar chrome from its title", () => {
    const { getByRole } = render(
      <Dialog open onClose={noop} chrome="bar" title="What's New">
        <p>body</p>
      </Dialog>,
    );
    expect(getByRole("dialog", { name: "What's New" })).toBeTruthy();
    expect(getByRole("heading", { level: 1, name: "What's New" })).toBeTruthy();
  });

  it("mints a distinct heading id per instance", () => {
    const { getAllByRole } = render(
      <>
        <Dialog open onClose={noop} title="First">
          <p>a</p>
        </Dialog>
        <Dialog open onClose={noop} title="Second">
          <p>b</p>
        </Dialog>
      </>,
    );
    const [a, b] = getAllByRole("dialog");
    const first = a.getAttribute("aria-labelledby");
    const second = b.getAttribute("aria-labelledby");
    expect(first).toBeTruthy();
    expect(first).not.toBe(second);
  });

  it("closes on Escape and on a scrim mousedown, but not on a click inside", () => {
    const onClose = vi.fn();
    const { getByRole, getByText } = render(
      <Dialog open onClose={onClose} title="Delete project">
        <p className="mt-2">body</p>
      </Dialog>,
    );
    const dialog = getByRole("dialog");
    const scrim = dialog.parentElement!;

    fireEvent.mouseDown(getByText("body"));
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.mouseDown(dialog);
    expect(onClose).not.toHaveBeenCalled();

    fireEvent.mouseDown(scrim);
    expect(onClose).toHaveBeenCalledTimes(1);

    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it("takes its danger heading colour from the one tone map, and writes no hex", () => {
    const { getByRole } = render(
      <Dialog open onClose={noop} tone="danger" title="Remove this data?">
        <p className="mt-2">body</p>
      </Dialog>,
    );
    const heading = getByRole("heading", { level: 2 });
    expect(heading.style.color).toBe(`var(${TONE_TEXT_TOKEN.danger})`);
    expect(heading.style.color).not.toContain("#");
    // Swapped, not layered — a surviving `text-ink` would race the inline colour.
    expect(heading.className).not.toContain("text-ink");
  });

  it("gives the card chrome no Close affordance of its own", () => {
    // One of the remove-my-data steps is deliberately undismissable; a shell-supplied Close button
    // would hand back the exit it exists to withhold.
    const { queryByRole } = render(
      <Dialog open onClose={noop} title="Final confirmation">
        <p className="mt-2">body</p>
      </Dialog>,
    );
    expect(queryByRole("button")).toBeNull();
  });

  it("gives the bar chrome a Close button whose label is configurable", () => {
    const onClose = vi.fn();
    const { getByRole } = render(
      <Dialog open onClose={onClose} chrome="bar" title="Share this vault" closeLabel="Cancel">
        <p>body</p>
      </Dialog>,
    );
    fireEvent.click(getByRole("button", { name: "Cancel" }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("renders eyebrow, subtitle and footer where each chrome puts them", () => {
    const { getByText } = render(
      <Dialog
        open
        onClose={noop}
        title="Compressed"
        subtitle="The older turns were folded in."
        eyebrow="Step 2 of 4"
        footer={<button>Undo</button>}
      >
        <p className="mt-2">body</p>
      </Dialog>,
    );
    expect(getByText("Step 2 of 4")).toBeTruthy();
    expect(getByText("The older turns were folded in.")).toBeTruthy();
    expect(getByText("Undo").parentElement?.className).toContain("justify-end");
  });

  it("passes sizing through Modal's own seams, never through className", () => {
    // The WhatsNew trap: a rival max-h-* passed via className leaves BOTH in the list and lets
    // stylesheet order pick, which silently turned an 80vh dialog into an 85vh one.
    const { getByRole } = render(
      <Dialog
        open
        onClose={noop}
        chrome="bar"
        title="What's New"
        heightClassName="max-h-[80vh]"
        widthClassName="max-w-3xl"
      >
        <p>body</p>
      </Dialog>,
    );
    const cls = getByRole("dialog").className;
    expect(cls).toContain("max-h-[80vh]");
    expect(cls).not.toContain("max-h-[85vh]");
    expect(cls).toContain("max-w-3xl");
    expect(cls).not.toContain("max-w-lg");
    // The bar chrome still gets its own column layout on top.
    expect(cls).toContain("flex flex-col");
  });

  it("cannot be rendered without an accessible name", () => {
    // Type-level, and therefore enforced by the `tsc --noEmit` in the check gate rather than by a
    // lint rule nobody runs. If `title` ever became optional, this line would stop erroring and the
    // test would fail — which is the point.
    const withoutTitle = (
      // @ts-expect-error a Dialog with no title has no accessible name and must not compile
      <Dialog open onClose={noop}>
        <p>body</p>
      </Dialog>
    );
    expect(withoutTitle).toBeTruthy();
  });
});
