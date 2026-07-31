// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The one-time sweep's UI contract. DELETE THIS FILE with the component (card #651's follow-up).
//
// What is worth pinning here is not the layout — it is the SILENCE. This banner is uninvited and it
// sits above a permanent delete, so every path that should show nothing must show nothing: a
// refusal, an empty plan, a failed scan. A regression in any of those puts a delete button in front
// of someone PM has no business interrupting, and in the refusal case the list would be exactly the
// files it must not touch.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { OrphanSweepBanner } from "./OrphanSweepBanner";

const scanOrphanFiles = vi.fn();
const deleteOrphanFiles = vi.fn();
const dismissOrphanSweep = vi.fn();

vi.mock("../lib/ipc", () => ({
  scanOrphanFiles: () => scanOrphanFiles(),
  deleteOrphanFiles: (paths: string[]) => deleteOrphanFiles(paths),
  dismissOrphanSweep: () => dismissOrphanSweep(),
}));

// The same stub the other component tests use: every <Button> reaches for `useTheme`, and the real
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

beforeEach(() => {
  vi.clearAllMocks();
  deleteOrphanFiles.mockResolvedValue(0);
  dismissOrphanSweep.mockResolvedValue(undefined);
});

afterEach(cleanup);

const plan = (orphans: string[]) => ({ orphans, refusal: null });

describe("OrphanSweepBanner", () => {
  it("offers the sweep when there are leftovers, leading with the rebuild consequence", async () => {
    scanOrphanFiles.mockResolvedValue(plan(["photos/a.png.pmenc", "b.md.pmenc"]));
    render(<OrphanSweepBanner />);

    // "2 files" and the harm — not "some files are using disk space", which would not be worth a
    // dialog over a permanent delete.
    expect(await screen.findByText(/2 files/)).toBeTruthy();
    expect(screen.getByText(/bring them back as documents/)).toBeTruthy();
  });

  it("shows nothing at all when the vault is clean", async () => {
    scanOrphanFiles.mockResolvedValue(plan([]));
    const { container } = render(<OrphanSweepBanner />);
    await waitFor(() => expect(scanOrphanFiles).toHaveBeenCalled());
    expect(container.textContent).toBe("");
  });

  it("shows nothing when the backend refuses — the list would be the files it must not delete", async () => {
    // A refused plan carries no orphans by construction, but the component must not render an
    // "0 files" banner either. Silence is the contract.
    scanOrphanFiles.mockResolvedValue({ orphans: [], refusal: { kind: "no_documents" } });
    const { container } = render(<OrphanSweepBanner />);
    await waitFor(() => expect(scanOrphanFiles).toHaveBeenCalled());
    expect(container.textContent).toBe("");
  });

  it("shows nothing when the scan itself fails", async () => {
    // An uninvited cleanup that cannot run should leave no trace. Surfacing an error here would
    // interrupt someone about a job they never asked for.
    scanOrphanFiles.mockRejectedValue(new Error("vault locked"));
    const { container } = render(<OrphanSweepBanner />);
    await waitFor(() => expect(scanOrphanFiles).toHaveBeenCalled());
    expect(container.textContent).toBe("");
  });

  it("lists every file and warns about the backup before anything can be deleted", async () => {
    scanOrphanFiles.mockResolvedValue(plan(["photos/a.png.pmenc", "b.md.pmenc"]));
    render(<OrphanSweepBanner />);
    fireEvent.click(await screen.findByRole("button", { name: "Review them" }));

    expect(screen.getByText("photos/a.png.pmenc")).toBeTruthy();
    expect(screen.getByText("b.md.pmenc")).toBeTruthy();
    expect(screen.getByText(/Back up your vault first/)).toBeTruthy();
    expect(screen.getByText(/cannot be undone from inside PM/)).toBeTruthy();
    expect(deleteOrphanFiles).not.toHaveBeenCalled();
  });

  it("deletes only after the explicit confirm, and reports how many went", async () => {
    scanOrphanFiles.mockResolvedValue(plan(["photos/a.png.pmenc", "b.md.pmenc"]));
    deleteOrphanFiles.mockResolvedValue(2);
    render(<OrphanSweepBanner />);

    fireEvent.click(await screen.findByRole("button", { name: "Review them" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete 2 files" }));

    await waitFor(() =>
      expect(deleteOrphanFiles).toHaveBeenCalledWith(["photos/a.png.pmenc", "b.md.pmenc"]),
    );
    expect(await screen.findByText(/Removed 2 leftover files/)).toBeTruthy();
  });

  it("cancelling the dialog deletes nothing and leaves the offer standing", async () => {
    scanOrphanFiles.mockResolvedValue(plan(["b.md.pmenc"]));
    render(<OrphanSweepBanner />);

    fireEvent.click(await screen.findByRole("button", { name: "Review them" }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));

    expect(deleteOrphanFiles).not.toHaveBeenCalled();
    expect(dismissOrphanSweep).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Review them" })).toBeTruthy();
  });

  it("'Not now' records the dismissal so the banner cannot come back", async () => {
    scanOrphanFiles.mockResolvedValue(plan(["b.md.pmenc"]));
    const { container } = render(<OrphanSweepBanner />);

    fireEvent.click(await screen.findByRole("button", { name: "Not now" }));

    await waitFor(() => expect(dismissOrphanSweep).toHaveBeenCalled());
    expect(deleteOrphanFiles).not.toHaveBeenCalled();
    expect(container.textContent).toBe("");
  });

  it("keeps the banner and shows the error when the delete fails", async () => {
    // The backend re-plans and refuses if the vault moved under us. That must read as "nothing was
    // deleted", not as a silent success.
    scanOrphanFiles.mockResolvedValue(plan(["b.md.pmenc"]));
    deleteOrphanFiles.mockRejectedValue(new Error("the vault changed since it was scanned"));
    render(<OrphanSweepBanner />);

    fireEvent.click(await screen.findByRole("button", { name: "Review them" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete 1 file" }));

    expect(await screen.findByRole("alert")).toBeTruthy();
    expect(screen.getByText(/the vault changed since it was scanned/)).toBeTruthy();
  });

  it("says 'file' not 'files' for a single leftover", async () => {
    scanOrphanFiles.mockResolvedValue(plan(["b.md.pmenc"]));
    render(<OrphanSweepBanner />);
    expect(await screen.findByText(/1 file\b/)).toBeTruthy();
  });
});
