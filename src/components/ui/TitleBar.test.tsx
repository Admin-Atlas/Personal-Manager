// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// Escape leaves fullscreen, so a frameless fullscreen never traps the user, and stands down only for
// a dialog that owns the key: a modal one, one it was pressed inside, or an event's details (which
// close on Escape pressed anywhere). The calendar's quick create owns only a key pressed inside it
// and stays open while a title is typed, so beside it Escape still leaves fullscreen.

import { useRef } from "react";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useDialogLayer } from "../../lib/useDialogLayer";
import { Modal } from "./Modal";

const setFullscreen = vi.fn(() => Promise.resolve());

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    isMaximized: () => Promise.resolve(false),
    isFullscreen: () => Promise.resolve(true),
    setFullscreen,
    onResized: () => Promise.resolve(() => {}),
  }),
}));

vi.mock("@tauri-apps/api/app", () => ({
  getVersion: () => Promise.resolve("3.151.0-alpha"),
}));

vi.mock("./useEdgeResizeCursor", () => ({
  useEdgeResizeCursor: () => {},
}));

vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard" }),
}));

import { TitleBar } from "./TitleBar";

afterEach(() => {
  cleanup();
  setFullscreen.mockClear();
});

/** Quick create's shape: a dialog layer that leaves the page live (no `aria-modal`). */
function QuickCreate() {
  const ref = useRef<HTMLDivElement>(null);
  useDialogLayer(true, ref);
  return (
    <div ref={ref} role="dialog" aria-label="New event">
      <input aria-label="Title" />
    </div>
  );
}

/** Press Escape on `target`, and let the window's fullscreen be read. */
async function escape(target: Element) {
  fireEvent.keyDown(target, { key: "Escape" });
  await act(async () => {});
}

describe("TitleBar's Escape", () => {
  it("leaves fullscreen with no dialog open", async () => {
    render(<TitleBar />);
    await escape(document.body);
    expect(setFullscreen.mock.calls).toEqual([[false]]);
  });

  it("leaves fullscreen beside quick create, with focus on the page", async () => {
    render(
      <>
        <TitleBar />
        <QuickCreate />
        <button type="button">A day</button>
      </>,
    );
    await escape(screen.getByRole("button", { name: "A day" }));
    expect(setFullscreen.mock.calls).toEqual([[false]]);
  });

  it("stands down for a key pressed inside quick create", async () => {
    render(
      <>
        <TitleBar />
        <QuickCreate />
      </>,
    );
    await escape(screen.getByLabelText("Title"));
    expect(setFullscreen).not.toHaveBeenCalled();
  });

  it("stands down for a modal dialog, wherever the key is pressed", async () => {
    render(
      <>
        <TitleBar />
        <button type="button">A day</button>
        <Modal open onClose={() => {}} label="Settings">
          <button type="button">Save</button>
        </Modal>
      </>,
    );
    await escape(screen.getByRole("button", { name: "A day" }));
    await escape(screen.getByRole("button", { name: "Save" }));
    expect(setFullscreen).not.toHaveBeenCalled();
  });

  // An event's details aren't a dialog layer, and close on Escape pressed anywhere: the one key does
  // one thing.
  it("stands down for an event's details, wherever the key is pressed", async () => {
    render(
      <>
        <TitleBar />
        <button type="button">A day</button>
        <div role="dialog" aria-label="Dentist" />
      </>,
    );
    await escape(screen.getByRole("button", { name: "A day" }));
    expect(setFullscreen).not.toHaveBeenCalled();
  });
});
