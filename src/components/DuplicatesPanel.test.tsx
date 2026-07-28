// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The promises this surface makes about a feature whose two signals BOTH produce false pairs (#282):
//
//   - it says why each pair was flagged, and "starts identically" and "reads alike" read differently;
//   - removing acts on ONE named document, after a confirmation that says what happens to it;
//   - a connected-account document is never described as being deleted from the provider;
//   - a scan that could only run half its method says so instead of reporting a clean library.
//
// Each is invisible in the markup and expensive to get wrong: this is a surface that invites people
// to delete their own documents.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const scanDuplicates = vi.fn();
const deleteDocument = vi.fn();
const openReader = vi.fn();

vi.mock("../lib/ipc", () => ({
  scanDuplicates: () => scanDuplicates(),
  deleteDocument: (id: number) => deleteDocument(id),
}));

vi.mock("../lib/reader", () => ({
  useReader: () => ({ openReader, current: null }),
}));

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

import { DuplicatesPanel } from "./DuplicatesPanel";

afterEach(cleanup);

function doc(id: number, title: string, source_type = "vault") {
  return {
    id,
    title,
    source_path: null,
    ext: "md",
    byte_size: 10,
    chunk_count: 3,
    created_at: null,
    ingested_at: "2026-07-01T10:00:00Z",
    project: "Sales",
    linked_projects: [],
    tags: [],
    importance: null,
    reviewed: false,
    last_activity: null,
    source_type,
    source_state: "ok",
    source_id: null,
    external_ref: null,
    stored_summary: null,
    source_modified_at: null,
    source_account: null,
    source_parent_folder_id: null,
    source_parent_folder_name: null,
  };
}

const BOTH_SIGNALS = {
  a: doc(1, "Contract Acme"),
  b: doc(2, "Contract Acme (copy)"),
  same_opening: true,
  similarity: 0.99,
};

beforeEach(() => {
  vi.clearAllMocks();
  scanDuplicates.mockResolvedValue({
    scanned: 120,
    pairs: [BOTH_SIGNALS],
    similarity_skipped: false,
    similarity_limit: 5000,
  });
  deleteDocument.mockResolvedValue(undefined);
});

async function scanned() {
  render(<DuplicatesPanel />);
  fireEvent.click(screen.getByRole("button", { name: "Check for duplicates" }));
  await waitFor(() => expect(screen.getByText("Contract Acme")).toBeTruthy());
}

describe("the scan", () => {
  it("runs only when asked", () => {
    render(<DuplicatesPanel />);
    expect(scanDuplicates).not.toHaveBeenCalled();
  });

  it("says why a pair was flagged, in words that carry the confidence", async () => {
    await scanned();
    expect(screen.getByText(/start identically and read the same/)).toBeTruthy();
  });

  it("distinguishes a similarity-only pair from an identical opening", async () => {
    // The two signals are not equally strong. Wording them the same way would teach the user to
    // trust the weaker one as much as the stronger, which is how a flag becomes a delete.
    scanDuplicates.mockResolvedValue({
      scanned: 120,
      pairs: [{ ...BOTH_SIGNALS, same_opening: false, similarity: 0.975 }],
      similarity_skipped: false,
      similarity_limit: 5000,
    });
    await scanned();
    expect(screen.getByText(/read very alike, though they don't start the same way/)).toBeTruthy();
  });

  // "No duplicates" from a scan that skipped half its method is a claim PM has not earned.
  it("admits when only half the method ran", async () => {
    scanDuplicates.mockResolvedValue({
      scanned: 9000,
      pairs: [],
      similarity_skipped: true,
      similarity_limit: 5000,
    });
    render(<DuplicatesPanel />);
    fireEvent.click(screen.getByRole("button", { name: "Check for duplicates" }));
    await waitFor(() => expect(screen.getByText(/compared openings only/)).toBeTruthy());
  });

  it("surfaces a failure instead of looking like an empty library", async () => {
    scanDuplicates.mockRejectedValue("the index is being rebuilt");
    render(<DuplicatesPanel />);
    fireEvent.click(screen.getByRole("button", { name: "Check for duplicates" }));
    await waitFor(() => expect(screen.getByRole("alert").textContent).toMatch(/rebuilt/));
    expect(screen.queryByText(/Nothing looks duplicated/)).toBeNull();
  });
});

describe("removing one side", () => {
  it("confirms first, and removes only the document named", async () => {
    await scanned();
    const buttons = screen.getAllByRole("button", { name: "Remove this one" });
    expect(buttons.length).toBe(2);
    fireEvent.click(buttons[1]); // the SECOND document
    expect(deleteDocument).not.toHaveBeenCalled();
    // Twice on screen now: its own card, AND the confirmation naming it. The confirmation naming the
    // document is the whole guard — "remove the duplicate" would leave the user guessing which side.
    expect(screen.getAllByText("Contract Acme (copy)").length).toBe(2);

    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(deleteDocument).toHaveBeenCalledWith(2));
    expect(deleteDocument).toHaveBeenCalledTimes(1);
  });

  // PM must never suggest it deletes from a connected account — it removes its own pointer.
  it("never describes a connected-account file as being deleted", async () => {
    scanDuplicates.mockResolvedValue({
      scanned: 4,
      pairs: [{ ...BOTH_SIGNALS, b: doc(2, "Contract Acme (Drive)", "index_only") }],
      similarity_skipped: false,
      similarity_limit: 5000,
    });
    await scanned();
    fireEvent.click(screen.getAllByRole("button", { name: "Remove this one" })[1]);
    expect(screen.getByText(/stays where it is in your connected account/)).toBeTruthy();
    expect(screen.queryByText(/file in your vault goes too/)).toBeNull();
  });

  it("drops the pair from the list once one side is gone", async () => {
    // Re-scanning after every removal would make clearing three duplicates take three full sweeps.
    await scanned();
    fireEvent.click(screen.getAllByRole("button", { name: "Remove this one" })[0]);
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(screen.queryByText("Contract Acme (copy)")).toBeNull());
    expect(screen.getByText(/Nothing looks duplicated/)).toBeTruthy();
    expect(scanDuplicates).toHaveBeenCalledTimes(1);
  });
});
