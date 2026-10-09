// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// ModelPicker is a hand-rolled menu inside the Settings dialog (Settings › AI › "Add a model…"). Its
// Escape listener sits on `document` and the dialog's on `window`, so without stopping the key one
// Escape closed the menu AND Settings (or raised the unsaved-changes guard).

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

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
});
