// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// Settings → Storage. The tab draws what the backend reports, so each test hands it a report and
// checks what the rows offer: photo text recognition installs from its row (and an update's stale
// copy is repaired there, with Remove still beside it), a removal asks first in that component's
// own words, and a library something still needs keeps a greyed Remove and a pill that jumps to
// what must go first. Every action goes through the backend and the inventory is read again after,
// so the rows show what is really left on disk.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { StorageComponent, StorageReport } from "../lib/types";

const listStorageComponents = vi.fn();
const removeStorageComponent = vi.fn();
const installOptionalOcr = vi.fn();
vi.mock("../lib/ipc", () => ({
  listStorageComponents: () => listStorageComponents(),
  removeStorageComponent: (id: string) => removeStorageComponent(id),
  installOptionalOcr: () => installOptionalOcr(),
  onOcrInstall: () => Promise.resolve(() => {}),
}));

// The primitives reach for `useTheme`; the real provider pulls in IPC.
vi.mock("../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { StorageSettings } from "./StorageSettings";

const row = (over: Partial<StorageComponent>): StorageComponent => ({
  id: "ocr",
  label: "Photo text recognition (rapidocr)",
  detail: "Reads text out of dropped photos and screenshots (OCR), on-device.",
  size_bytes: 100,
  approximate: false,
  child: false,
  status: "removable",
  blockers: [],
  manage: null,
  note: null,
  ...over,
});

const report = (...components: StorageComponent[]): StorageReport => ({
  total_bytes: components.reduce((n, c) => n + c.size_bytes, 0),
  components,
});

const tsne = row({
  id: "openTSNE",
  label: "Enhanced map layout (openTSNE)",
  detail: "Sharper, tighter clusters for the semantic Map (t-SNE).",
  manage: { label: "Turn on/off in Memory map", tab: "general" },
});

const sklearnHeld = row({
  id: "scikit-learn",
  label: "scikit-learn",
  detail: "Used by the enhanced layout. Includes joblib and threadpoolctl.",
  child: true,
  status: "blocked",
  blockers: [{ label: "Remove the enhanced layout first", anchor: "openTSNE" }],
});

/** The row's own box, so a button is found on the row it belongs to. */
const rowBox = (container: HTMLElement, id: string) =>
  within(container.querySelector<HTMLElement>(`#storage-${id}`)!);

beforeEach(() => {
  for (const f of [listStorageComponents, removeStorageComponent, installOptionalOcr])
    f.mockReset();
});
afterEach(cleanup);

describe("installing photo text recognition", () => {
  it("installs from its row, shows the download, then reads the inventory again", async () => {
    listStorageComponents.mockResolvedValue(
      report(row({ status: "installable", approximate: true })),
    );
    let finish = () => {};
    installOptionalOcr.mockReturnValue(new Promise<void>((resolve) => (finish = resolve)));
    render(<StorageSettings onNavigate={() => {}} />);

    fireEvent.click(await screen.findByRole("button", { name: "Install" }));
    expect(installOptionalOcr).toHaveBeenCalledTimes(1);
    const busy = screen.getByRole("button", { name: "Installing…" }) as HTMLButtonElement;
    expect(busy.disabled).toBe(true);
    expect(
      screen.getByRole("progressbar", { name: "Downloading photo text recognition" }),
    ).toBeTruthy();

    finish();
    await waitFor(() => expect(listStorageComponents).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.queryByRole("progressbar")).toBeNull());
  });

  it("shows a failed install and leaves the row as it was", async () => {
    listStorageComponents.mockResolvedValue(
      report(row({ status: "installable", approximate: true })),
    );
    installOptionalOcr.mockRejectedValue("could not download photo text recognition");
    render(<StorageSettings onNavigate={() => {}} />);

    fireEvent.click(await screen.findByRole("button", { name: "Install" }));
    expect(await screen.findByText(/could not download photo text recognition/)).toBeTruthy();
    await waitFor(() =>
      expect((screen.getByRole("button", { name: "Install" }) as HTMLButtonElement).disabled).toBe(
        false,
      ),
    );
    expect(listStorageComponents).toHaveBeenCalledTimes(1);
  });

  // An update can leave the old packages on disk but not working. Installing over them repairs it,
  // and Remove stays beside it so someone offline can still get the space back.
  it("repairs a stale copy from its row, with Remove still on offer", async () => {
    listStorageComponents.mockResolvedValue(report(row({ status: "stale" })));
    installOptionalOcr.mockResolvedValue(undefined);
    render(<StorageSettings onNavigate={() => {}} />);

    fireEvent.click(await screen.findByRole("button", { name: "Install again" }));
    expect(screen.getByRole("button", { name: "Remove" })).toBeTruthy();
    expect(installOptionalOcr).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(listStorageComponents).toHaveBeenCalledTimes(2));
  });
});

describe("removing a component", () => {
  it("asks first in the component's own words, and removes only once confirmed", async () => {
    listStorageComponents.mockResolvedValue(report(tsne));
    removeStorageComponent.mockResolvedValue(undefined);
    render(<StorageSettings onNavigate={() => {}} />);

    fireEvent.click(await screen.findByRole("button", { name: "Remove" }));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText("Remove the enhanced layout?")).toBeTruthy();
    expect(within(dialog).getByText(/returns to the basic \(PCA\) layout/)).toBeTruthy();
    expect(removeStorageComponent).not.toHaveBeenCalled();

    fireEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(removeStorageComponent).toHaveBeenCalledWith("openTSNE"));
    await waitFor(() => expect(listStorageComponents).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("leaves the component alone when the confirm is cancelled", async () => {
    listStorageComponents.mockResolvedValue(report(tsne));
    render(<StorageSettings onNavigate={() => {}} />);

    fireEvent.click(await screen.findByRole("button", { name: "Remove" }));
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(removeStorageComponent).not.toHaveBeenCalled();
  });

  it("shows the backend's reason when it refuses a removal", async () => {
    listStorageComponents.mockResolvedValue(report(tsne));
    removeStorageComponent.mockRejectedValue("Remove the enhanced layout first");
    render(<StorageSettings onNavigate={() => {}} />);

    fireEvent.click(await screen.findByRole("button", { name: "Remove" }));
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Remove" }));
    expect(await screen.findByText("Remove the enhanced layout first")).toBeTruthy();
  });
});

describe("the removal cascade", () => {
  it("greys Remove on a library something still needs, and its pill jumps to that row", async () => {
    listStorageComponents.mockResolvedValue(report(tsne, sklearnHeld));
    const { container } = render(<StorageSettings onNavigate={() => {}} />);
    await screen.findByText("scikit-learn");

    const held = rowBox(container, "scikit-learn");
    // Installed and held, not missing: the chip says so, or it reads as a download still to come.
    expect(held.getByText("Installed — in use")).toBeTruthy();
    expect((held.getByRole("button", { name: "Remove" }) as HTMLButtonElement).disabled).toBe(true);

    const dependent = container.querySelector<HTMLElement>("#storage-openTSNE")!;
    dependent.scrollIntoView = vi.fn();
    fireEvent.click(held.getByRole("button", { name: "Remove the enhanced layout first →" }));
    expect(dependent.scrollIntoView).toHaveBeenCalledTimes(1);
  });

  it("links a row managed elsewhere to the tab that owns it", async () => {
    listStorageComponents.mockResolvedValue(report(tsne));
    const onNavigate = vi.fn();
    render(<StorageSettings onNavigate={onNavigate} />);

    fireEvent.click(await screen.findByRole("button", { name: "Turn on/off in Memory map →" }));
    expect(onNavigate).toHaveBeenCalledWith("general");
  });
});
