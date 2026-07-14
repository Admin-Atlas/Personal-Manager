// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import {
  indentLines,
  listIndentBeforeCaret,
  outdentLines,
  toRenderMarkdown,
} from "./notesMarkdown";

// F-52 regression: promoting a pinboard note to a real vault document ingests
// `toRenderMarkdown(text)`, not the raw shorthand dialect — so the ingested copy renders as
// standard GFM everywhere the note is read outside the board (reader, retrieval, chat citations)
// and the dialect markers ("[]", "." bullets, roman labels) don't get indexed as noise. These
// tests lock in that normalisation: the transform must actually change dialect notes (or the
// wiring would be a no-op) while leaving native markers and prose byte-for-byte.
describe("toRenderMarkdown — shorthand dialect → GFM (F-52)", () => {
  it("normalises checkbox markers to GFM task-list items", () => {
    expect(toRenderMarkdown("[] buy milk")).toBe("- [ ] buy milk");
    expect(toRenderMarkdown("[x] done")).toBe("- [x] done");
    expect(toRenderMarkdown("[X] done")).toBe("- [x] done"); // case-folded to lowercase x
  });

  it("maps '.' bullets to '-' bullets (GFM has no distinct dot marker)", () => {
    expect(toRenderMarkdown(". first")).toBe("- first");
  });

  it("keeps roman labels but appends a hard break so a run stays multi-line", () => {
    // Two trailing spaces = a Markdown hard break; without it GFM would merge the single newlines
    // into one paragraph and the "i."/"ii." labels would collapse together.
    expect(toRenderMarkdown("i. alpha\nii. beta")).toBe("i. alpha  \nii. beta  ");
  });

  it("preserves indentation on transformed markers", () => {
    expect(toRenderMarkdown("  . nested")).toBe("  - nested");
    expect(toRenderMarkdown("  [] task")).toBe("  - [ ] task");
  });

  it("leaves native markers and non-marker prose byte-for-byte", () => {
    const native = "- bullet\n1. one\n> quote\njust prose, not a list";
    expect(toRenderMarkdown(native)).toBe(native);
  });

  it("actually changes a dialect note (the ingest wiring is not a no-op)", () => {
    const raw = "[] task one\n. a bullet\ni. roman";
    const rendered = toRenderMarkdown(raw);
    expect(rendered).not.toBe(raw); // ingesting `rendered` differs from ingesting `raw`
    expect(rendered).toBe("- [ ] task one\n- a bullet\ni. roman  ");
  });

  it("is idempotent on already-native content", () => {
    const native = "- a\n- b\n\nsome prose";
    expect(toRenderMarkdown(toRenderMarkdown(native))).toBe(toRenderMarkdown(native));
  });
});

// Tab / Shift+Tab / Backspace indent behaviour in the note editor. One level = two spaces, so a
// Tab-indented checkbox nests under the item above once rendered; continueList (tested via the app)
// then carries that indent to the items typed after it.
describe("indentLines / outdentLines", () => {
  it("indents the caret line by two spaces and moves the caret past them", () => {
    const r = indentLines("[] task", 3, 3);
    expect(r.text).toBe("  [] task");
    expect(r.selStart).toBe(5);
    expect(r.selEnd).toBe(5);
  });

  it("indents every non-blank line a selection covers, leaving blank lines alone", () => {
    const v = "[] a\n\n[] b";
    const r = indentLines(v, 0, v.length);
    expect(r.text).toBe("  [] a\n\n  [] b");
  });

  it("outdents the caret line by one level (caret clamps to the line start)", () => {
    const r = outdentLines("  [] task", 2, 2);
    expect(r.text).toBe("[] task");
    expect(r.selStart).toBe(0);
  });

  it("removes a single leading tab as one indent level", () => {
    expect(outdentLines("\t[] task", 1, 1).text).toBe("[] task");
  });

  it("is a no-op on an already-flush line", () => {
    const r = outdentLines("[] task", 3, 3);
    expect(r.text).toBe("[] task");
    expect(r.selStart).toBe(3);
  });

  it("round-trips indent → outdent", () => {
    const indented = indentLines("[] task", 0, 0);
    expect(outdentLines(indented.text, 2, 2).text).toBe("[] task");
  });
});

describe("listIndentBeforeCaret — Backspace-outdents inside a list item's indent", () => {
  it("reports the indent length when the caret sits within an indented list item's indent", () => {
    expect(listIndentBeforeCaret("  [] task", 2)).toBe(2);
    expect(listIndentBeforeCaret("  [] task", 1)).toBe(2);
  });

  it("returns null for a flush item, non-list prose, or a caret past the indent", () => {
    expect(listIndentBeforeCaret("[] task", 0)).toBeNull(); // flush → nothing to outdent
    expect(listIndentBeforeCaret("  plain text", 2)).toBeNull(); // indented, but not a list item
    expect(listIndentBeforeCaret("  [] task", 5)).toBeNull(); // caret is in the content, not the indent
  });
});
