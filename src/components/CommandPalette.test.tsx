// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The palette's dialog semantics, which it had been hand-rolling and getting wrong in one specific
// way: Escape was a React `onKeyDown` on the `<input>`, and the result rows are `tabIndex={-1}`, so
// the FIRST Tab moved focus out of the palette entirely — and from there a keydown on a background
// control never bubbled back to the input. Escape was dead, with the palette still on screen over a
// scrim, and the only way out was the mouse.
//
// Wearing `Modal` fixes that (its Escape is a window listener) and brings the rest of the shell with
// it: a `role="dialog"` with a name, and focus handed back to whatever opened the palette. Those are
// what these tests pin — not the fuzzy matcher, which is pure and belongs to its own unit.

import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const listConversations = vi.fn();
const listDocuments = vi.fn();
const listProjectOverviews = vi.fn();

vi.mock("../lib/ipc", () => ({
  listConversations: () => listConversations(),
  listDocuments: () => listDocuments(),
  listProjectOverviews: () => listProjectOverviews(),
}));

// The same stub the other component tests use: the palette's primitives reach for `useTheme`, and
// the real ThemeProvider pulls in IPC.
vi.mock("../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({
    system: "slate",
    mode: "dark",
    modePref: "system",
    modeSource: "system",
    accent: "mono",
    depth: "standard",
    autoLocation: "",
    teachVisible: true,
    mapVisible: true,
    setSystem: () => {},
    setModePref: () => {},
    setAccent: () => {},
    setDepth: () => {},
    setAutoLocation: () => {},
    setTeachVisible: () => {},
  }),
}));

import { CommandPalette } from "./CommandPalette";

const noop = () => {};

function renderPalette(onClose: () => void) {
  return render(
    <CommandPalette
      onClose={onClose}
      onOpenProject={noop}
      onOpenConversation={noop}
      onNavigate={noop}
      onOpenSettings={noop}
    />,
  );
}

/** Something outside the palette to hold focus — the position a single Tab used to land on. */
function outsideButton(label: string): HTMLButtonElement {
  const el = document.createElement("button");
  el.textContent = label;
  document.body.appendChild(el);
  return el;
}

beforeEach(() => {
  listConversations.mockResolvedValue([]);
  listDocuments.mockResolvedValue([]);
  listProjectOverviews.mockResolvedValue([]);
  // jsdom implements no layout, so it has no `scrollIntoView` at all; the palette keeps the
  // highlighted row in view on every arrow press. Same stub as ChatView.test.tsx.
  Element.prototype.scrollIntoView = vi.fn();
});

afterEach(() => {
  cleanup();
  document.body.innerHTML = "";
  vi.clearAllMocks();
});

describe("CommandPalette", () => {
  it("closes on Escape from a focus position that is NOT the input", async () => {
    // The regression this conversion exists for. Focus is deliberately moved off the input first:
    // with the old input-bound handler this Escape reached nothing at all.
    const onClose = vi.fn();
    renderPalette(onClose);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());

    const elsewhere = outsideButton("behind the palette");
    elsewhere.focus();
    expect(document.activeElement).toBe(elsewhere);

    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("still closes on Escape while the input holds focus", async () => {
    // The path that already worked must keep working — the fix removed the input's own branch.
    const onClose = vi.fn();
    const { getByRole } = renderPalette(onClose);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());

    const input = getByRole("combobox");
    expect(document.activeElement).toBe(input);

    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("keeps Tab on the query field instead of letting it leave", async () => {
    // The rows are `tabIndex={-1}` and selected via `aria-activedescendant`, so the query field is
    // the palette's ONLY tab stop — Tab should return to it, not walk out into the app behind the
    // scrim. (`useFocusTrap` had to learn that a tabindex="-1" button is not a stop; see its test.)
    const { getByRole } = renderPalette(noop);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());

    const input = getByRole("combobox");
    fireEvent.keyDown(input, { key: "Tab" });
    expect(document.activeElement).toBe(input);
  });

  it("is a named dialog, and the input keeps its own field name", async () => {
    // Two different names, both required: `aria-label` on a combobox names the FIELD, so it could
    // never have been the dialog's name. The palette had no dialog role at all before this.
    const { getByRole } = renderPalette(noop);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());

    expect(getByRole("dialog", { name: "Command palette" })).toBeTruthy();
    expect(
      getByRole("combobox", { name: "Search projects, files and conversations" }),
    ).toBeTruthy();
  });

  it("hands focus back to whatever opened it", async () => {
    // Closing used to drop focus on <body>: keyboard users landed at the top of the document.
    const opener = outsideButton("Open the palette");
    opener.focus();

    const { unmount } = renderPalette(noop);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());
    expect(document.activeElement).not.toBe(opener);

    unmount();
    expect(document.activeElement).toBe(opener);
  });

  it("keeps the help anchor on the panel itself", async () => {
    // Two things depend on where this sits. HelpOverlay resolves a hovered element with
    // `closest("[data-help]")`, so it must be an ANCESTOR of the rows and the input — and
    // `.help-mode [data-help]:hover` draws the outline that says help is available here, which
    // needs an element that actually paints a box. The dialog element is both.
    const { getByRole } = renderPalette(noop);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());

    expect(getByRole("dialog").getAttribute("data-help")).toBe("command-palette");
    expect(getByRole("combobox").closest("[data-help]")).toBe(getByRole("dialog"));
    expect(getByRole("listbox").closest("[data-help]")).toBe(getByRole("dialog"));
  });
});
