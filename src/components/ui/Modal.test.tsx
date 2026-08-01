// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The dialog shell's semantics, and the ratchet on its one remaining hole.
//
// Modal had no test file at all. Two of the things asserted here are new capability the overlay
// work needs before it can land: `placement="top"` (a search palette hangs from the top of the
// window, and passing `items-start` through className would leave `items-center` in the list too),
// and the topmost-dialog rule — without which one Escape inside Settings would fire the unsaved-
// changes guard AND close Settings, driving straight through the guard `requestClose` enforces.
//
// The matching source scan — every `<Modal>` in the tree carries a name, on a shrinking allow-list —
// lives with the other design rules in `src/theme/designGuards.test.ts`.

import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Modal } from "./Modal";

afterEach(cleanup);

const noop = () => {};

describe("Modal", () => {
  it("takes its accessible name from `label` when there is no heading to point at", () => {
    // PinboardView's folder board: its "title" is an editable <input>, not a heading, so
    // `labelledBy` has nothing correct to reference.
    const { getByRole } = render(
      <Modal open onClose={noop} label="Weekend plans">
        <input aria-label="Folder title" defaultValue="Weekend plans" />
      </Modal>,
    );
    expect(getByRole("dialog", { name: "Weekend plans" })).toBeTruthy();
  });

  it("prefers a heading via labelledBy", () => {
    const { getByRole } = render(
      <Modal open onClose={noop} labelledBy="t">
        <h2 id="t">Move conversation</h2>
      </Modal>,
    );
    expect(getByRole("dialog", { name: "Move conversation" })).toBeTruthy();
  });

  it("centres by default and swaps the whole padding string for placement='top'", () => {
    const { getByRole, rerender } = render(
      <Modal open onClose={noop} label="x">
        <p>body</p>
      </Modal>,
    );
    let scrim = getByRole("dialog").parentElement!;
    expect(scrim.className).toContain("items-center");
    expect(scrim.className).toContain("p-6");
    expect(scrim.className).not.toContain("items-start");

    rerender(
      <Modal open onClose={noop} label="x" placement="top">
        <p>body</p>
      </Modal>,
    );
    scrim = getByRole("dialog").parentElement!;
    expect(scrim.className).toContain("items-start");
    expect(scrim.className).toContain("pt-[12vh]");
    // Swap, not layer: `p-6` alongside `pt-[12vh]` would leave the winner to emission order.
    expect(scrim.className).not.toContain("items-center");
    expect(scrim.className).not.toMatch(/(?:^|\s)p-6(?:\s|$)/);
  });

  it("gives Escape to the topmost dialog only", () => {
    const outer = vi.fn();
    const inner = vi.fn();
    const { rerender } = render(
      <Modal open onClose={outer} label="Settings">
        <p>settings</p>
        <Modal open onClose={inner} label="Discard changes?">
          <p>guard</p>
        </Modal>
      </Modal>,
    );

    fireEvent.keyDown(window, { key: "Escape" });
    expect(inner).toHaveBeenCalledTimes(1);
    expect(outer).not.toHaveBeenCalled();

    // With the guard dismissed, the next Escape reaches the dialog behind it.
    rerender(
      <Modal open onClose={outer} label="Settings">
        <p>settings</p>
        <Modal open={false} onClose={inner} label="Discard changes?">
          <p>guard</p>
        </Modal>
      </Modal>,
    );
    fireEvent.keyDown(window, { key: "Escape" });
    expect(outer).toHaveBeenCalledTimes(1);
    expect(inner).toHaveBeenCalledTimes(1);
  });

  it("cannot be rendered without an accessible name", () => {
    // The point of the whole batch, and the reason `designGuards`' allow-list could be deleted:
    // this is a `tsc` failure now, not an audit finding. Enforced by the check gate's
    // `tsc --noEmit` — if the union were relaxed back to two optionals, the directive would go
    // unused and the typecheck would fail on THIS line.
    const unnamed = (
      // @ts-expect-error a Modal with neither labelledBy nor label is announced as bare "dialog"
      <Modal open onClose={noop}>
        <p>body</p>
      </Modal>
    );
    expect(unnamed).toBeTruthy();

    const bothNames = (
      // @ts-expect-error the two are exclusive: aria-labelledby wins outright, so an aria-label
      // alongside it is text nobody ever hears
      <Modal open onClose={noop} labelledBy="t" label="x">
        <h2 id="t">Move conversation</h2>
      </Modal>
    );
    expect(bothNames).toBeTruthy();
  });

  it("puts helpId on the dialog element, where help mode can outline it", () => {
    // Not on a wrapper inside: `.help-mode [data-help]:hover` draws the outline that tells you help
    // is there, and an element with no box of its own paints none. Omitted, it emits no attribute at
    // all rather than an empty one — `closest("[data-help]")` matches on presence.
    const { getByRole, rerender } = render(
      <Modal open onClose={noop} label="Command palette" helpId="command-palette">
        <p>body</p>
      </Modal>,
    );
    expect(getByRole("dialog").getAttribute("data-help")).toBe("command-palette");

    rerender(
      <Modal open onClose={noop} label="Command palette">
        <p>body</p>
      </Modal>,
    );
    expect(getByRole("dialog").hasAttribute("data-help")).toBe(false);
  });

  it("renders nothing when closed, and deregisters so a sibling dialog is topmost again", () => {
    const onClose = vi.fn();
    const { queryByRole } = render(
      <Modal open={false} onClose={onClose} label="x">
        <p>body</p>
      </Modal>,
    );
    expect(queryByRole("dialog")).toBeNull();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).not.toHaveBeenCalled();
  });
});
