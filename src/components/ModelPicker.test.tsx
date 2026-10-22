// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// ModelPicker is a hand-rolled menu inside the Settings dialog (Settings › AI › "Add a model…"). Its
// Escape listener sits on `document` and the dialog's on `window`, so without stopping the key one
// Escape closed the menu AND Settings (or raised the unsaved-changes guard). It follows the Popover
// primitive's rule: a modal dialog over it owns the key, a non-modal layer only a key pressed in it.

import { useRef } from "react";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useDialogLayer } from "../lib/useDialogLayer";

vi.mock("../lib/ipc", () => ({
  listModels: () => Promise.resolve([]),
}));

// Same stub the other component tests use: the <Button>s reach for `useTheme`, and the real
// ThemeProvider pulls in IPC.
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
    setSystem: () => {},
    setModePref: () => {},
    setAccent: () => {},
    setDepth: () => {},
    setAutoLocation: () => {},
    setTeachVisible: () => {},
  }),
}));

import { ModelPicker } from "./ModelPicker";
import { Modal } from "./ui/Modal";

afterEach(cleanup);

describe("ModelPicker inside a dialog", () => {
  it("closes only itself on Escape", async () => {
    const onDialogClose = vi.fn();
    render(
      <Modal open onClose={onDialogClose} label="Settings">
        <ModelPicker value="" onChange={() => {}} triggerLabel="Add a model…" />
      </Modal>,
    );
    fireEvent.click(screen.getByText("Add a model…"));
    const search = await screen.findByPlaceholderText("Search models…");
    fireEvent.keyDown(search, { key: "Escape" });
    await waitFor(() => expect(screen.queryByPlaceholderText("Search models…")).toBeNull());
    expect(onDialogClose).not.toHaveBeenCalled();
    // With the menu closed, the next Escape reaches the dialog.
    fireEvent.keyDown(screen.getByText("Add a model…"), { key: "Escape" });
    expect(onDialogClose).toHaveBeenCalledTimes(1);
  });

  // The calendar's quick create stays open through a click in the sidebar, so it can sit behind
  // Settings: a non-modal layer, which owns only a key pressed inside it.
  it("closes only itself on Escape beside a non-modal layer", async () => {
    function Layer() {
      const ref = useRef<HTMLDivElement>(null);
      useDialogLayer(true, ref);
      return <div ref={ref} role="dialog" aria-label="New event" />;
    }
    const onDialogClose = vi.fn();
    render(
      <>
        <Layer />
        <Modal open onClose={onDialogClose} label="Settings">
          <ModelPicker value="" onChange={() => {}} triggerLabel="Add a model…" />
        </Modal>
      </>,
    );
    fireEvent.click(screen.getByText("Add a model…"));
    const search = await screen.findByPlaceholderText("Search models…");
    fireEvent.keyDown(search, { key: "Escape" });
    await waitFor(() => expect(screen.queryByPlaceholderText("Search models…")).toBeNull());
    expect(onDialogClose).not.toHaveBeenCalled();
    fireEvent.keyDown(screen.getByText("Add a model…"), { key: "Escape" });
    expect(onDialogClose).toHaveBeenCalledTimes(1);
  });

  // A modal dialog opened over the open menu (one of Settings' own questions) holds the keyboard.
  it("leaves Escape to a modal dialog opened over it", async () => {
    const onDialogClose = vi.fn();
    const onQuestionClose = vi.fn();
    const page = (asking: boolean) => (
      <Modal open onClose={onDialogClose} label="Settings">
        <ModelPicker value="" onChange={() => {}} triggerLabel="Add a model…" />
        {asking && (
          <Modal open onClose={onQuestionClose} label="Reset these settings?">
            <button type="button">Reset</button>
          </Modal>
        )}
      </Modal>
    );
    const { rerender } = render(page(false));
    fireEvent.click(screen.getByText("Add a model…"));
    await screen.findByPlaceholderText("Search models…");
    rerender(page(true));
    fireEvent.keyDown(screen.getByText("Reset"), { key: "Escape" });
    expect(onQuestionClose).toHaveBeenCalledTimes(1);
    expect(screen.queryByPlaceholderText("Search models…")).not.toBeNull();
    expect(onDialogClose).not.toHaveBeenCalled();
  });
});
