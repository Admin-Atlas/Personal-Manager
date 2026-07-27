// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The promises this surface makes about a paid, irreversible sweep (#580):
//
//   - it says what it will cost BEFORE anything is billed;
//   - nothing is written until the user accepts, and only the rows they left ticked;
//   - "this document ends up with no tags" is shown as a real outcome, not a blank gap.
//
// Each of those is invisible in the markup and expensive to get wrong: this pass rewrites tags
// across a whole library, through the vault, with no undo.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const retagScope = vi.fn();
const listTagProposals = vi.fn();
const commitRetag = vi.fn();
const discardTagProposals = vi.fn();
const proposeRetag = vi.fn();

vi.mock("../lib/ipc", () => ({
  retagScope: () => retagScope(),
  listTagProposals: () => listTagProposals(),
  commitRetag: (ids: number[]) => commitRetag(ids),
  discardTagProposals: () => discardTagProposals(),
  proposeRetag: (cb: (e: unknown) => void) => proposeRetag(cb),
}));

// The same stub the other component tests use: <Button> reaches for `useTheme`, and the real
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

beforeEach(() => {
  vi.clearAllMocks();
  retagScope.mockResolvedValue({ documents: 240, calls: 21 });
  listTagProposals.mockResolvedValue([]);
  commitRetag.mockResolvedValue(0);
  discardTagProposals.mockResolvedValue(undefined);
  proposeRetag.mockResolvedValue(undefined);
});

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

describe("TeachTags", () => {
  // This is a billable sweep over the whole library, not a local reshuffle. Someone must be able to
  // see what they are about to spend without starting it.
  it("states the cost before anything is run", async () => {
    render(<TeachTags />);
    await waitFor(() => expect(screen.getByText(/240 documents/)).toBeTruthy());
    expect(screen.getByText(/21 model calls/)).toBeTruthy();
    expect(proposeRetag).not.toHaveBeenCalled();
  });

  it("applies only the rows left ticked", async () => {
    listTagProposals.mockResolvedValue(ROWS);
    render(<TeachTags />);
    await waitFor(() => expect(screen.getByText("CV Standards")).toBeTruthy());

    fireEvent.click(screen.getByLabelText("Apply the new tags for CV Standards"));
    expect(screen.getByRole("button", { name: /Apply 2/ })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: /Apply 2/ }));
    await waitFor(() => expect(commitRetag).toHaveBeenCalledWith([1, 3]));
  });

  it("writes nothing until the user accepts", async () => {
    listTagProposals.mockResolvedValue(ROWS);
    render(<TeachTags />);
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
    render(<TeachTags />);
    await waitFor(() => expect(screen.getByText("Odd one out")).toBeTruthy());
    expect(screen.getAllByText("no tags").length).toBe(1);
  });

  it("shows the vocabulary as soon as the first call settles it", async () => {
    proposeRetag.mockImplementation(async (cb: (e: unknown) => void) => {
      cb({ type: "vocabulary", tags: ["invoice", "application"] });
      cb({ type: "progress", done: 12, total: 240 });
    });
    render(<TeachTags />);
    await waitFor(() => expect(screen.getByText(/240 documents/)).toBeTruthy());

    fireEvent.click(screen.getByRole("button", { name: /Re-tag my library/ }));
    await waitFor(() => expect(screen.getByText("invoice")).toBeTruthy());
    expect(screen.getByText("application")).toBeTruthy();
  });

  it("surfaces a failed pass instead of looking like it worked", async () => {
    proposeRetag.mockRejectedValue("no usable tag vocabulary");
    render(<TeachTags />);
    await waitFor(() => expect(screen.getByText(/240 documents/)).toBeTruthy());

    fireEvent.click(screen.getByRole("button", { name: /Re-tag my library/ }));
    await waitFor(() => expect(screen.getByRole("alert").textContent).toMatch(/vocabulary/));
  });
});
