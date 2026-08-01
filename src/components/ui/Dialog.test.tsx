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

import { ConfirmDialog } from "./ConfirmDialog";
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

  it("tones the bar chrome's heading the same way, and defaults back to text-ink", () => {
    const { getByRole, rerender } = render(
      <Dialog open onClose={noop} chrome="bar" tone="danger" title="Remove this data?">
        <p>body</p>
      </Dialog>,
    );
    let heading = getByRole("heading", { level: 1 });
    expect(heading.style.color).toBe(`var(${TONE_TEXT_TOKEN.danger})`);

    rerender(
      <Dialog open onClose={noop} chrome="bar" title="Remove this data?">
        <p>body</p>
      </Dialog>,
    );
    heading = getByRole("heading", { level: 1 });
    expect(heading.style.color).toBe("");
    expect(heading.className).toContain("text-ink");
  });

  it("tones the heading and nothing else — the shell is chrome, not a message", () => {
    // A `danger` dialog must not tint its own surface the way a Callout does: the two remove-my-data
    // steps are ordinary cards carrying a red HEADING, and a red panel behind them would read as an
    // error that had already happened rather than a decision still being asked for.
    const { getByRole } = render(
      <Dialog open onClose={noop} tone="danger" title="Remove this data?">
        <p className="mt-2">body</p>
      </Dialog>,
    );
    const dialog = getByRole("dialog");
    expect(dialog.className).toContain("bg-surface");
    expect(dialog.getAttribute("style") ?? "").not.toContain("--st-due");
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

// ConfirmDialog is now a PRESET over Dialog's card chrome rather than a second copy of it. These
// assert the part that is still its own — the two-button shape and what `busy`/`danger` do — plus
// the one thing the rewrite must not have dropped on the way: the accessible name.
describe("ConfirmDialog", () => {
  it("is named by its title, through the preset", () => {
    const { getByRole } = render(
      <ConfirmDialog open title="Rebuild the index?" onConfirm={noop} onClose={noop}>
        Everything is re-read from your vault.
      </ConfirmDialog>,
    );
    expect(getByRole("dialog", { name: "Rebuild the index?" })).toBeTruthy();
    expect(getByRole("heading", { level: 2, name: "Rebuild the index?" })).toBeTruthy();
  });

  it("puts Cancel before Confirm, and wires each to its own handler", () => {
    const onConfirm = vi.fn();
    const onClose = vi.fn();
    const { getAllByRole, getByRole } = render(
      <ConfirmDialog
        open
        title="Disconnect?"
        confirmLabel="Disconnect"
        cancelLabel="Keep it"
        onConfirm={onConfirm}
        onClose={onClose}
      />,
    );
    expect(getAllByRole("button").map((b) => b.textContent)).toEqual(["Keep it", "Disconnect"]);

    fireEvent.click(getByRole("button", { name: "Keep it" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onConfirm).not.toHaveBeenCalled();

    fireEvent.click(getByRole("button", { name: "Disconnect" }));
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it("tints the confirm BUTTON for `danger`, not the heading", () => {
    // The destructive thing is the action. Tinting the title as well would double-count it — and
    // `Dialog tone="danger"` is reserved for the dialogs whose subject, not whose button, is the
    // danger (RemovePmData's two steps).
    const { getByRole } = render(
      <ConfirmDialog open title="Delete this preference?" danger onConfirm={noop} onClose={noop} />,
    );
    expect(getByRole("button", { name: "Confirm" }).className).toContain("--st-due");
    expect(getByRole("heading", { level: 2 }).style.color).toBe("");
  });

  it("blocks every exit while busy", () => {
    // Not just cosmetic: the action is already running, and an Escape that closed the dialog would
    // leave the user with no sight of an operation they cannot cancel.
    const onClose = vi.fn();
    const { getAllByRole, getByRole } = render(
      <ConfirmDialog open title="Removing…" busy onConfirm={noop} onClose={onClose} />,
    );
    expect(getAllByRole("button").every((b) => (b as HTMLButtonElement).disabled)).toBe(true);
    expect(getByRole("button", { name: "Working…" })).toBeTruthy();

    fireEvent.keyDown(window, { key: "Escape" });
    fireEvent.mouseDown(getByRole("dialog").parentElement!);
    expect(onClose).not.toHaveBeenCalled();
  });

  it("renders no body wrapper when it has no children", () => {
    const { getByRole } = render(
      <ConfirmDialog open title="Are you sure?" onConfirm={noop} onClose={noop} />,
    );
    // Title, then straight to the footer — an empty `mt-2` block would open a gap under the heading.
    const card = getByRole("heading", { level: 2 }).parentElement!;
    expect(card.children.length).toBe(2);
  });
});
