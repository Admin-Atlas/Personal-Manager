// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The shared connector row — every Drive account, OneDrive account and local folder renders through
// this ONE component (part of X-D2's fold). This is the component-DOM half of the Wave-4 jsdom layer:
// it proves the harness renders a real connector component and pins the row's shape (title + a
// reachable dot vs. an "unreachable" badge, the meta line, and the Sync-now / Queued / action buttons).
// `useTheme` is stubbed so the row's <Button>s don't need the full ThemeProvider (which pulls in IPC).

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
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

// The split control's menu is a `Popover`, which watches its trigger to stay anchored. jsdom
// implements no layout and ships no ResizeObserver, and where the panel lands is not what these
// tests are about — the same stub Composer.test.tsx uses, for the same reason.
beforeEach(() => {
  // `globals` is off in vitest.config.ts, so RTL's auto-cleanup never runs and renders otherwise
  // ACCUMULATE across tests in a file — which is how a second row's split control turned
  // `getByRole` ambiguous rather than failing honestly. The repo idiom (BackupSettings, ChatView,
  // CalendarEventPopover all do this).
  cleanup();
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});

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

  // --- the split control (#727) ------------------------------------------------------------------
  //
  // "Sync now" rides a delta cursor and usually costs one request returning an empty page;
  // re-indexing costs a full listing of the account. The split is what keeps the cheap action one
  // click and makes the expensive one deliberate.

  const base = {
    title: "me@gmail.com",
    reachable: true,
    meta: "5 indexed",
    syncingThis: false,
    queued: false,
    syncDisabled: false,
    actionLabel: "Disconnect",
    actionDisabled: false,
  };

  it("renders no split control for a connector with nothing to re-index", () => {
    // The local-folder connector keeps no delta cursor, so it passes no `onReindex` — and an empty
    // menu beside every row would be worse than no menu at all.
    render(<ConnectorItemRow {...base} onSync={vi.fn()} onAction={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "More sync options" })).toBeNull();
  });

  it("offers Re-index everything behind the split control", () => {
    const onReindex = vi.fn();
    render(
      <ConnectorItemRow {...base} onSync={vi.fn()} onAction={vi.fn()} onReindex={onReindex} />,
    );
    // Folded away until asked for — that is the point of the split.
    expect(screen.queryByRole("button", { name: "Re-index everything" })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "More sync options" }));
    fireEvent.click(screen.getByRole("button", { name: "Re-index everything" }));
    expect(onReindex).toHaveBeenCalledTimes(1);
  });

  it("explains why re-indexing is unavailable instead of just grey", () => {
    // A disabled item with no explanation is the thing users report as broken. The backend refuses a
    // re-index mid-sync (the running pass would write a fresh cursor and undo the clear), so the row
    // has to say the same thing the command would.
    const onReindex = vi.fn();
    render(
      <ConnectorItemRow
        {...base}
        onSync={vi.fn()}
        onAction={vi.fn()}
        onReindex={onReindex}
        reindexDisabled
        reindexBlockedReason="Available once the current sync finishes."
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "More sync options" }));
    const item = screen.getByRole("button", { name: "Re-index everything" });
    expect(item.hasAttribute("disabled")).toBe(true);
    expect(screen.getByText("Available once the current sync finishes.")).toBeTruthy();

    fireEvent.click(item);
    expect(onReindex).not.toHaveBeenCalled();
  });
});
