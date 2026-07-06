// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The shared connector row — every Drive account, OneDrive account and local folder renders through
// this ONE component (part of X-D2's fold). This is the component-DOM half of the Wave-4 jsdom layer:
// it proves the harness renders a real connector component and pins the row's shape (title + a
// reachable dot vs. an "unreachable" badge, the meta line, and the Sync-now / Queued / action buttons).
// `useTheme` is stubbed so the row's <Button>s don't need the full ThemeProvider (which pulls in IPC).

import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ConnectorItemRow } from "./ConnectorItemRow";

vi.mock("../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal()),
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

describe("ConnectorItemRow", () => {
  it("renders a reachable account: title, meta, and a Sync-now button", () => {
    const onSync = vi.fn();
    render(
      <ConnectorItemRow
        title="me@gmail.com"
        reachable={true}
        meta="5 indexed"
        syncingThis={false}
        queued={false}
        syncDisabled={false}
        onSync={onSync}
        actionLabel="Disconnect"
        actionDisabled={false}
        onAction={vi.fn()}
      />,
    );
    expect(screen.getByText("me@gmail.com")).toBeTruthy();
    expect(screen.getByText("5 indexed")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Sync now" })).toBeTruthy();
    // No "unreachable" badge when reachable.
    expect(screen.queryByText("unreachable")).toBeNull();
  });

  it("shows the unreachable badge and a 'Queued' sync button", () => {
    render(
      <ConnectorItemRow
        title="me@outlook.com"
        reachable={false}
        badgeLabel="unreachable"
        meta="idle"
        syncingThis={false}
        queued={true}
        syncDisabled={true}
        onSync={vi.fn()}
        actionLabel="Disconnect"
        actionDisabled={false}
        onAction={vi.fn()}
      />,
    );
    expect(screen.getByText("me@outlook.com")).toBeTruthy();
    expect(screen.getByText("unreachable")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Queued" })).toBeTruthy();
  });
});
