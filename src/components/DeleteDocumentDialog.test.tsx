// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The dialog's promise about what a delete does. This is worth a test rather than a read-through
// because being wrong in either direction is expensive: telling someone PM will delete their Google
// Drive file when it won't stops them using the feature, and telling them it won't when it does
// destroys work they can't get back.

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DeleteDocumentDialog } from "./DeleteDocumentDialog";
import type { Document } from "../lib/types";

vi.mock("../lib/ipc", () => ({ deleteDocument: vi.fn() }));

// Same stub the other component tests use: the dialog's <Button>s reach for `useTheme`, and the real
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

function doc(over: Partial<Document> = {}): Document {
  return {
    id: 1,
    title: "Q3 Report",
    source_path: null,
    ext: "md",
    byte_size: 10,
    chunk_count: 2,
    created_at: "2026-07-01T00:00:00Z",
    ingested_at: "2026-07-01T00:00:00Z",
    project: "Marketing",
    tags: [],
    importance: null,
    reviewed: true,
    last_activity: null,
    source_type: null,
    source_state: "ok",
    external_ref: null,
    source_id: null,
    source_parent_folder_id: null,
    source_parent_folder_name: null,
    ...over,
  } as Document;
}

// Without this each render stacks in the same document, so a phrase two cases share (the wording
// for a plain vault document and for a spreadsheet is the same) matches twice and a `queryByText`
// meant to prove ABSENCE finds an earlier test's copy. The repo idiom (DuplicatesPanel.test.tsx).
afterEach(cleanup);

function open(d: Document) {
  render(<DeleteDocumentDialog doc={d} onClose={() => {}} onDeleted={() => {}} />);
}

describe("DeleteDocumentDialog", () => {
  it("a vault document says PM's copy goes and the imported file does not", () => {
    open(doc());
    expect(screen.getByText(/Delete this file\?/)).toBeTruthy();
    expect(screen.getByText(/PM’s copy in your vault is removed/)).toBeTruthy();
    expect(screen.getByText(/file you imported from is left alone/)).toBeTruthy();
  });

  // The one that must never regress: PM does not delete from the provider.
  it("an index-only document promises the file at the source is untouched", () => {
    open(doc({ source_type: "index_only", source_id: "gdrive:abc" }));
    expect(screen.getByText(/only PM’s copy of the index is removed/)).toBeTruthy();
    expect(screen.getByText(/file itself is not touched/)).toBeTruthy();
  });

  // A photo and a spreadsheet are NOT pointers — PM wrote the Markdown behind them, and for a photo
  // it may be holding the image as well. Saying "your cloud account is not touched" about a
  // screenshot dragged off the desktop names an account that isn't involved, and hides the fact
  // that PM is about to delete something.
  it("a photo says both the read-out text and the kept image go", () => {
    open(doc({ source_type: "photo" }));
    expect(screen.getByText(/text it read out of it/)).toBeTruthy();
    expect(screen.getByText(/copy of the picture itself/)).toBeTruthy();
    expect(screen.queryByText(/cloud account/)).toBeNull();
  });

  it("a spreadsheet is treated as a document PM holds the file for, not a pointer", () => {
    open(doc({ source_type: "spreadsheet" }));
    expect(screen.getByText(/PM’s copy in your vault is removed/)).toBeTruthy();
    expect(screen.queryByText(/only PM’s copy of the index is removed/)).toBeNull();
  });

  // A chat routes to the conversation delete, so the dialog must not imply only a transcript goes.
  it("a chat document says the conversation goes too", () => {
    open(doc({ source_type: "chat" }));
    expect(screen.getByText(/Delete this chat\?/)).toBeTruthy();
    expect(screen.getByText(/removes the conversation and its messages too/)).toBeTruthy();
  });

  // `getAllBy*`: the phrase sits inside a <p> that is itself inside ConfirmDialog's body wrapper, so
  // an exact-node query matches more than one element.
  it("always states it can't be undone, and warns about existing citations", () => {
    open(doc());
    expect(screen.getAllByText(/can’t be undone from inside PM/).length).toBeGreaterThan(0);
    expect(
      screen.getAllByText(/Answers that already cited it will keep listing it/).length,
    ).toBeGreaterThan(0);
  });
});
