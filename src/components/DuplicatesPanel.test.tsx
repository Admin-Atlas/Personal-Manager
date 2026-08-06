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
const dismissDuplicatePair = vi.fn();
const duplicateSnapshot = vi.fn();
const restoreDuplicateDismissals = vi.fn();
const documentLocations = vi.fn();
const openReader = vi.fn();

vi.mock("../lib/ipc", () => ({
  scanDuplicates: () => scanDuplicates(),
  deleteDocument: (id: number) => deleteDocument(id),
  // Explicit factory: a name missing here resolves to `undefined` and the click that calls it
  // throws, so a new ipc import has to be added in the same change that uses it.
  dismissDuplicatePair: (a: number, b: number) => dismissDuplicatePair(a, b),
  duplicateSnapshot: () => duplicateSnapshot(),
  restoreDuplicateDismissals: () => restoreDuplicateDismissals(),
  documentLocations: (id: number) => documentLocations(id),
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

import { useState } from "react";

import { DuplicatesPanel } from "./DuplicatesPanel";
import type { Document, DuplicateReport } from "../lib/types";

afterEach(cleanup);

/** The panel as the Documents view mounts it: the report lives OUTSIDE, because the toolbar button
 *  shows its count whether or not the panel is open. `seed` stands in for a background check that
 *  already ran. */
function Panel({ seed = null }: { seed?: DuplicateReport | null }) {
  const [report, setReport] = useState<DuplicateReport | null>(seed);
  return <DuplicatesPanel report={report} onReport={setReport} onClose={() => {}} />;
}

function doc(id: number, title: string, source_type: Document["source_type"] = "vault"): Document {
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
    source_modified_at: null,
    source_parent_folder_id: null,
    source_parent_folder_name: null,
    source_folder_path: null,
    source_author: null,
    source_last_modified_by: null,
    source_created_at: null,
    source_size_bytes: null,
    pm_refreshed_at: null,
  };
}

const BOTH_SIGNALS = {
  a: doc(1, "Contract Acme"),
  b: doc(2, "Contract Acme (copy)"),
  same_opening: true,
  similarity: 0.99,
};

/** A report in the shape the backend returns, so a test states only what it is about. */
function report(
  pairs: (typeof BOTH_SIGNALS)[],
  extra: Partial<DuplicateReport> = {},
): DuplicateReport {
  return {
    scanned: 120,
    pairs,
    similarity_skipped: false,
    similarity_limit: 5000,
    dismissed: 0,
    checked_at: "2026-08-03T09:00:00Z",
    incremental: false,
    ...extra,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  scanDuplicates.mockResolvedValue(report([BOTH_SIGNALS]));
  // Both resolving actions prune the pair in the backend and the panel re-reads, so the default
  // snapshot is the report AFTER the only pair has gone. Component state no longer hides anything:
  // that was the defect — it died with the component, and the tab router unmounts this view.
  duplicateSnapshot.mockResolvedValue(report([]));
  deleteDocument.mockResolvedValue(undefined);
  documentLocations.mockResolvedValue([]);
  dismissDuplicatePair.mockResolvedValue(undefined);
  restoreDuplicateDismissals.mockResolvedValue(undefined);
});

const CHECK = "Check the whole library";

async function scanned() {
  render(<Panel />);
  fireEvent.click(screen.getByRole("button", { name: CHECK }));
  await waitFor(() => expect(screen.getByText("Contract Acme")).toBeTruthy());
}

describe("the scan", () => {
  it("runs only when asked", () => {
    render(<Panel />);
    expect(scanDuplicates).not.toHaveBeenCalled();
  });

  it("shows what the background check already found, without asking again", async () => {
    // The whole return on checking after a sync (#711): opening the panel reads a result rather
    // than starting a 13-second wait. A panel that re-scanned on mount would throw that away.
    render(
      <Panel
        seed={{
          scanned: 120,
          pairs: [BOTH_SIGNALS],
          similarity_skipped: false,
          similarity_limit: 5000,
          dismissed: 0,
          checked_at: "2026-08-03T09:00:00Z",
          incremental: true,
        }}
      />,
    );
    expect(screen.getByText("Contract Acme")).toBeTruthy();
    expect(scanDuplicates).not.toHaveBeenCalled();
    // …and it says what it did NOT look at, for the same reason a half-run scan does.
    expect(screen.getByText(/covers what has arrived since PM last looked/)).toBeTruthy();
  });

  it("says why a pair was flagged, in words that carry the confidence", async () => {
    await scanned();
    expect(screen.getByText(/start identically/)).toBeTruthy();
  });

  it("explains how matching works once, in the panel, not once per pair", async () => {
    await scanned();
    // It describes the CHECK, so it is identical on every row; repeated per card it read as a note
    // about that pair and pushed the two documents down every card.
    expect(screen.getAllByText(/compares what is inside a document/)).toHaveLength(1);
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
      checked_at: "2026-08-03T09:00:00Z",
      incremental: false,
    });
    render(<Panel />);
    fireEvent.click(screen.getByRole("button", { name: CHECK }));
    await waitFor(() => expect(screen.getByText(/compared openings only/)).toBeTruthy());
  });

  it("surfaces a failure instead of looking like an empty library", async () => {
    scanDuplicates.mockRejectedValue("the index is being rebuilt");
    render(<Panel />);
    fireEvent.click(screen.getByRole("button", { name: CHECK }));
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
    // Re-scanning after every removal would make clearing three duplicates take three full sweeps —
    // so the backend prunes the pair and this re-reads the snapshot, which is one cheap read.
    await scanned();
    fireEvent.click(screen.getAllByRole("button", { name: "Remove this one" })[0]);
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(screen.queryByText("Contract Acme (copy)")).toBeNull());
    expect(screen.getByText(/Nothing looks duplicated/)).toBeTruthy();
    expect(scanDuplicates).toHaveBeenCalledTimes(1);
  });

  it("takes the removal from the backend snapshot, not from component state", async () => {
    // The regression: a deleted document was hidden in a local `removed` set that died with the
    // component. The tab router unmounts this view, so coming back re-rendered a card — with live
    // Open and "Remove this one" buttons — for a row that no longer existed.
    await scanned();
    fireEvent.click(screen.getAllByRole("button", { name: "Remove this one" })[0]);
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(deleteDocument).toHaveBeenCalledWith(1));
    await waitFor(() => expect(duplicateSnapshot).toHaveBeenCalledTimes(1));
  });
});

describe("keeping both", () => {
  it("records the decision so the pair stops being re-offered", async () => {
    // The third answer. Before this the only choices were "delete one" or "leave it and be asked
    // again on every scan forever" — the report is recomputed from scratch each time and wrote
    // nothing back, which is why a rebuild appeared to bring the same duplicates back.
    await scanned();
    fireEvent.click(screen.getByRole("button", { name: "Keep both" }));
    await waitFor(() => expect(dismissDuplicatePair).toHaveBeenCalledWith(1, 2));
    // …and it leaves the list, without a re-scan.
    await waitFor(() => expect(screen.queryByText("Contract Acme")).toBeNull());
    expect(scanDuplicates).toHaveBeenCalledTimes(1);
  });

  it("takes the decision from the backend snapshot, and the hidden count with it", async () => {
    // The regression this closes: `dismiss_duplicate_pair` persisted the decision, but nothing ever
    // reached the cached report the Documents view re-reads on mount. `absorb` only appends, so the
    // pair survived every later sweep — a tab switch re-offered a decision the user had already
    // made, and with `dismissed` still 0 there was not even a "you chose to keep this" line to
    // explain it. Both now come back from the backend, which is what makes them survive a remount.
    duplicateSnapshot.mockResolvedValue(report([], { dismissed: 1 }));
    await scanned();
    fireEvent.click(screen.getByRole("button", { name: "Keep both" }));
    await waitFor(() => expect(duplicateSnapshot).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.getByText(/1 pair you chose to keep/)).toBeTruthy());
    expect(scanDuplicates).toHaveBeenCalledTimes(1);
  });

  it("says how many are hidden and offers them back", async () => {
    // Never a silent narrowing: a result the user cannot see the shape of is the same defect as a
    // scan that skipped half its method and reported a clean sweep.
    scanDuplicates.mockResolvedValue({
      scanned: 120,
      pairs: [],
      similarity_skipped: false,
      similarity_limit: 5000,
      dismissed: 3,
      checked_at: "2026-08-03T09:00:00Z",
      incremental: false,
    });
    render(<Panel />);
    fireEvent.click(screen.getByRole("button", { name: CHECK }));
    await waitFor(() => expect(screen.getByText(/3 pairs you chose to keep/)).toBeTruthy());

    fireEvent.click(screen.getByRole("button", { name: "Show them again" }));
    await waitFor(() => expect(restoreDuplicateDismissals).toHaveBeenCalled());
  });
});
