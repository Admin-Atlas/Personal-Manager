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
  const { container } = render(<Markdown dashLists>{toRenderMarkdown(note)}</Markdown>);
  return container.innerHTML;
}

/** The same note through the DEFAULT boundary — what every other surface sees, and what the vault
 *  copy is read back through in the reader. */
function plainHtml(note: string): string {
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
    const out = html(". one\n. two");
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

// The two bullet kinds. Both stay REAL list items — nesting and hanging indent are the whole reason
// not to render a dash point as prose with a dash typed in front of it — while staying
// distinguishable enough for CSS to give them different markers.
describe("round bullets and dash points render as two kinds of list", () => {
  it("makes a '.' line a plain bullet list and a '-' line a dash list", () => {
    const dots = html(". one\n. two");
    expect(dots.match(/<li>/g)?.length).toBe(2);
    expect(dots).not.toContain("pm-dash-list");

    const dashes = html("- one\n- two");
    expect(dashes.match(/<li>/g)?.length).toBe(2);
    expect(dashes).toContain("pm-dash-list");
  });

  it("keeps them as SEPARATE lists when a note mixes them", () => {
    // Changing the bullet character starts a new list in CommonMark, which is what makes a per-list
    // class the right granularity: a list is homogeneous by construction.
    const out = html(". bullet\n- dash");
    expect(out.match(/<ul/g)?.length).toBe(2);
    expect(out.match(/pm-dash-list/g)?.length).toBe(1);
  });

  it("tags a NESTED dash list too", () => {
    // A nested list's position starts at its indentation, so reading the character AT the offset
    // instead of scanning past the whitespace would leave every nested list unmarked.
    const out = html(". parent\n  - nested dash");
    expect(out).toContain("pm-dash-list");
    expect(out.match(/<ul/g)?.length).toBe(2);
  });

  it("leaves a literal GFM task alone, so the tick-by-index mapping still holds", () => {
    // countTasks/toggleTaskAt match the SOURCE line. A rendered checkbox with no source counterpart
    // in the same order would tick a different line than the one clicked.
    const out = html("- [x] done\n- [ ] todo");
    expect(out.match(/type="checkbox"/g)?.length).toBe(2);
    expect(out).not.toContain("pm-dash-list");
  });

  it("renders as an ORDINARY bullet everywhere the opt-in is off", () => {
    // Which is what the vault copy is read through: "+" is standard GFM, so a promoted note stays
    // portable and no other surface's Markdown is restyled by this.
    const out = plainHtml("- one\n- two");
    expect(out.match(/<li>/g)?.length).toBe(2);
    expect(out).not.toContain("pm-dash-list");
  });

  it("puts a bullet and a checkbox in ONE list, and marks the non-task item", () => {
    // The shape the flush-checklist CSS has to cope with. `ul.contains-task-list` gets its left
    // padding zeroed so a checklist sits at the note's edge, and a plain bullet's disc is drawn in
    // exactly that padding — so a non-task sibling silently lost its marker and read as a stray line
    // of prose. The rule that gives it back keys on `li:not(.task-list-item)`, which only works if
    // remark-gfm marks the task item and leaves the bullet unmarked, in the same list.
    const { container } = render(<Markdown dashLists>{toRenderMarkdown(". a\n[] b")}</Markdown>);
    const lists = container.querySelectorAll("ul");
    expect(lists).toHaveLength(1);
    expect(lists[0].className).toContain("contains-task-list");
    const items = lists[0].querySelectorAll(":scope > li");
    expect(items).toHaveLength(2);
    expect(items[0].className).not.toContain("task-list-item");
    expect(items[1].className).toContain("task-list-item");
  });

  it("re-running the transform over a dash list changes nothing", () => {
    // "+" is in MARKER_RE for exactly this: without it the emitted line reads as prose on the next
    // pass and grows a second hard break every time — and this output is what reaches the vault.
    for (const note of ["- one\n- two", ". bullet\n- dash\nplain after"]) {
      const once = toRenderMarkdown(note);
      expect(toRenderMarkdown(once)).toBe(once);
    }
  });
});
