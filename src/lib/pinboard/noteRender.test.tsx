// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment jsdom

import { describe, it, expect } from "vitest";
import { render } from "@testing-library/react";
import { Markdown } from "../markdown";
import { toRenderMarkdown } from "./notesMarkdown";

// `notesMarkdown.test.ts` proves what `toRenderMarkdown` EMITS. These prove what the renderer then
// DOES with it — the two halves had drifted: the transform was appending the two-space hard break
// correctly while the pipeline downstream dropped it, so a note typed across several lines rendered
// as one run-on paragraph. Rendering through the real <Markdown> boundary is the only way to catch
// that class of bug; a string assertion on the transform alone passes either way.
function html(note: string): string {
  const { container } = render(<Markdown>{toRenderMarkdown(note)}</Markdown>);
  return container.innerHTML;
}

describe("a note's own line breaks survive rendering", () => {
  it("keeps two typed lines on two lines", () => {
    expect(html("line one\nline two")).toContain("<br>");
  });

  it("still makes a blank line a paragraph break, not a doubled break", () => {
    const out = html("para one\n\npara two");
    expect(out.match(/<p>/g)?.length).toBe(2);
  });

  it("leaves list items alone — each already renders on its own line", () => {
    const out = html("- one\n- two");
    expect(out.match(/<li>/g)?.length).toBe(2);
    expect(out).not.toContain("<br>");
  });

  it("survives a pasted CRLF note", () => {
    // The \r used to land BETWEEN the text and the two-space hard break ("line one\r  \n"), which
    // stops being a hard break — the pair rendered as two separate paragraphs instead of one break.
    const out = html("line one\r\nline two");
    expect(out).toContain("<br>");
    expect(out.match(/<p>/g)?.length).toBe(1);
  });

  it("does not swallow a prose line written under a list", () => {
    // CommonMark lazy continuation: "- item\nplain after" folds the second line INTO the bullet,
    // so the break disappears and the item reads "item plain after". In this dialect a marker is
    // explicit, so an unmarked line means the author left the list.
    const out = html("- item\nplain after");
    expect(out).toContain("<li>item</li>");
    expect(out).toContain("<p>plain after</p>");
  });

  it("does not swallow a prose line written under a checklist", () => {
    const out = html("[] item\nplain after");
    expect(out).toContain("<p>plain after</p>");
    expect(out).toContain('type="checkbox"');
  });

  it("does not swallow a prose line written under a numbered list", () => {
    const out = html("1. item\nplain after");
    expect(out).toContain("<li>item</li>");
    expect(out).toContain("<p>plain after</p>");
  });

  it("keeps a roman run and its following prose in ONE paragraph", () => {
    // Roman items render as plain text (GFM has no roman list), so they are not a container and the
    // hard break already does the job — inserting a blank line there would split the note.
    const out = html("i. first\nand a note about it");
    expect(out.match(/<p>/g)?.length).toBe(1);
    expect(out).toContain("<br>");
  });
});

describe("toRenderMarkdown stays idempotent", () => {
  it("re-running the transform changes nothing", () => {
    for (const note of [
      "line one\nline two",
      "- item\nplain after",
      "[] todo\nplain after",
      "i. first\nnote",
      "para one\n\npara two",
    ]) {
      const once = toRenderMarkdown(note);
      expect(toRenderMarkdown(once)).toBe(once);
    }
  });

  it("renders the note's checkbox dialect as real task-list items", () => {
    const out = html("[] todo\n[x] done");
    expect(out.match(/type="checkbox"/g)?.length).toBe(2);
    expect(out).toContain("checked");
  });

  it("keeps the task-list classes the flush-indent CSS relies on", () => {
    // The `:has()` selectors in index.css are a fallback for renderers that drop these; on an
    // engine without `:has()` support these classes are the ONLY thing carrying the rule.
    const out = html("[] todo");
    expect(out).toContain("contains-task-list");
    expect(out).toContain("task-list-item");
  });
});
