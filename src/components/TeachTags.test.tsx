// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The promises this surface makes about bulk, irreversible tag writes (#580, #579):
//
//   - it says what a pass will cost BEFORE anything is billed;
//   - the proposed vocabulary is EDITABLE, and what gets used is what the user approved;
//   - nothing is written until they accept, and only the rows they left ticked;
//   - removing a tag everywhere confirms first and names the scale.
//
// Each is invisible in the markup and expensive to get wrong: these paths rewrite tags across a
// whole library, through the vault, with no undo.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const listTags = vi.fn();
const retagScope = vi.fn();
const listTagProposals = vi.fn();
const commitRetag = vi.fn();
const discardTagProposals = vi.fn();
const proposeRetagVocabulary = vi.fn();
const applyRetagVocabulary = vi.fn();
const deleteTag = vi.fn();
const renameTag = vi.fn();
// The re-tag pass now reports on the global `retag://progress` event and keeps its state in a
// backend snapshot, so the component reads both on mount. An ipc name missing from the factory
// below resolves to `undefined` and the mount effect throws, taking every test in this file with
// it — so these two are not optional extras.
const retagStatus = vi.fn();
const onRetagProgress = vi.fn();

vi.mock("../lib/ipc", () => ({
  listTags: () => listTags(),
  retagScope: () => retagScope(),
  listTagProposals: () => listTagProposals(),
  commitRetag: (ids: number[]) => commitRetag(ids),
  discardTagProposals: () => discardTagProposals(),
  proposeRetagVocabulary: () => proposeRetagVocabulary(),
  // No callback parameter any more: a per-call Channel is only heard by the component that made
  // it, and this one unmounts on a tab switch.
  applyRetagVocabulary: (v: string[]) => applyRetagVocabulary(v),
  retagStatus: () => retagStatus(),
  onRetagProgress: (h: (e: unknown) => void) => onRetagProgress(h),
  deleteTag: (name: string) => deleteTag(name),
  renameTag: (a: string, b: string) => renameTag(a, b),
}));

// The same stub the other component tests use: <Button>/<Input> reach for `useTheme`, and the real
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

import { TeachTags } from "./TeachTags";

afterEach(cleanup);

const ROWS = [
  {
    document_id: 1,
    title: "Chairs Info Sheet",
    current_tags: ["bimun", "chair"],
    proposed_tags: ["meeting-notes"],
  },
  {
    document_id: 2,
    title: "CV Standards",
    current_tags: ["cv", "placement"],
    proposed_tags: ["application"],
  },
  { document_id: 3, title: "Odd one out", current_tags: ["ammun"], proposed_tags: [] },
];

beforeEach(() => {
  vi.clearAllMocks();
  listTags.mockResolvedValue([
    { name: "tax", kind: "group", documents: 2 },
    { name: "taxes", kind: "group", documents: 7 },
    { name: "Sales", kind: "project", documents: 4 },
  ]);
  retagScope.mockResolvedValue({ documents: 240, calls: 21 });
  listTagProposals.mockResolvedValue([]);
  commitRetag.mockResolvedValue(0);
  discardTagProposals.mockResolvedValue(undefined);
  proposeRetagVocabulary.mockResolvedValue(["invoice", "application"]);
  applyRetagVocabulary.mockResolvedValue(undefined);
  retagStatus.mockResolvedValue({
    running: false,
    phase: null,
    processed: 0,
    total: null,
    started_at_ms: null,
    vocabulary: [],
    last_changed: null,
  });
  onRetagProgress.mockResolvedValue(() => {});
  deleteTag.mockResolvedValue(1);
  renameTag.mockResolvedValue(1);
});

async function ready() {
  render(<TeachTags />);
  await waitFor(() => expect(screen.getByText(/240 documents/)).toBeTruthy());
}

describe("your tags", () => {
  it("lists free-form tags with counts and leaves projects out", async () => {
    await ready();
    // By the rename control's title, not the bare text: the fold suggestion below names these tags
    // too, so a text query would match twice and pass for the wrong reason.
    expect(screen.getByTitle('Rename "tax" everywhere')).toBeTruthy();
    expect(screen.getByTitle('Rename "taxes" everywhere')).toBeTruthy();
    expect(screen.getByText("2")).toBeTruthy();
    // Projects have their own merge flow, with entities and aliases behind it.
    expect(screen.queryByTitle('Rename "Sales" everywhere')).toBeNull();
  });

  // Removing a tag rewrites vault files for every document carrying it, with no undo.
  it("confirms before removing a tag, and names the scale", async () => {
    await ready();
    fireEvent.click(screen.getByLabelText("Remove the tag tax from every document"));
    expect(screen.getByText(/comes off 2 documents/)).toBeTruthy();
    expect(deleteTag).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Remove it" }));
    await waitFor(() => expect(deleteTag).toHaveBeenCalledWith("tax"));
  });

  it("offers to fold a near-duplicate into the tag that has more documents", async () => {
    await ready();
    expect(screen.getByText("These look like the same tag")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Fold" }));
    // `taxes` (7) survives; `tax` (2) folds into it — folding the other way would rewrite more files
    // to reach the same place.
    await waitFor(() => expect(renameTag).toHaveBeenCalledWith("tax", "taxes"));
  });
});

describe("the re-tag pass", () => {
  it("states the cost before anything is run", async () => {
    await ready();
    expect(screen.getByText(/21 model calls/)).toBeTruthy();
    expect(proposeRetagVocabulary).not.toHaveBeenCalled();
    expect(applyRetagVocabulary).not.toHaveBeenCalled();
  });

  // The vocabulary is the one decision the whole pass turns on, and reviewing forty words is
  // seconds where reviewing its consequences is every proposal.
  it("suggests a vocabulary without labelling anything", async () => {
    await ready();
    fireEvent.click(screen.getByRole("button", { name: "Suggest tags" }));
    await waitFor(() => expect(screen.getByText("invoice")).toBeTruthy());
    expect(applyRetagVocabulary).not.toHaveBeenCalled();
  });

  it("labels from the EDITED vocabulary, not the suggested one", async () => {
    await ready();
    fireEvent.click(screen.getByRole("button", { name: "Suggest tags" }));
    await waitFor(() => expect(screen.getByText("invoice")).toBeTruthy());

    fireEvent.click(screen.getByLabelText("Drop application from the vocabulary"));
    const input = screen.getByLabelText("Add a tag to the vocabulary");
    fireEvent.change(input, { target: { value: "Receipts" } });
    fireEvent.keyDown(input, { key: "Enter" });

    fireEvent.click(screen.getByRole("button", { name: /Label my library/ }));
    await waitFor(() =>
      // Lowercased on the way in, matching how tags are stored everywhere else.
      expect(applyRetagVocabulary).toHaveBeenCalledWith(["invoice", "receipts"]),
    );
  });
});

describe("the proposals", () => {
  it("applies only the rows left ticked", async () => {
    listTagProposals.mockResolvedValue(ROWS);
    await ready();
    await waitFor(() => expect(screen.getByText("CV Standards")).toBeTruthy());

    fireEvent.click(screen.getByLabelText("Apply the new tags for CV Standards"));
    fireEvent.click(screen.getByRole("button", { name: /Apply 2/ }));
    await waitFor(() => expect(commitRetag).toHaveBeenCalledWith([1, 3]));
  });

  it("writes nothing until the user accepts", async () => {
    listTagProposals.mockResolvedValue(ROWS);
    await ready();
    await waitFor(() => expect(screen.getByText("Chairs Info Sheet")).toBeTruthy());
    expect(commitRetag).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    await waitFor(() => expect(discardTagProposals).toHaveBeenCalled());
    expect(commitRetag).not.toHaveBeenCalled();
  });

  // Clearing a one-off label is a real and often correct outcome. Rendering it as an empty gap
  // would read as a bug rather than as "this ends up with no tags".
  it("spells out a document that ends up with no tags", async () => {
    listTagProposals.mockResolvedValue(ROWS);
    await ready();
    await waitFor(() => expect(screen.getByText("Odd one out")).toBeTruthy());
    expect(screen.getAllByText("no tags").length).toBe(1);
  });

  it("surfaces a failed pass instead of looking like it worked", async () => {
    proposeRetagVocabulary.mockRejectedValue("no usable tag vocabulary");
    await ready();
    fireEvent.click(screen.getByRole("button", { name: "Suggest tags" }));
    await waitFor(() => expect(screen.getByRole("alert").textContent).toMatch(/vocabulary/));
  });
});
