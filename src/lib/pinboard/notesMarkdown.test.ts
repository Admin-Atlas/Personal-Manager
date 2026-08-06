// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import {
  caretForRestore,
  countTasks,
  indentLines,
  listIndentBeforeCaret,
  outdentLines,
  toRenderMarkdown,
  toggleTaskAt,
} from "./notesMarkdown";

// F-52 regression: promoting a pinboard note to a real vault document ingests
// `toRenderMarkdown(text)`, not the raw shorthand dialect — so the ingested copy renders as
// standard GFM everywhere the note is read outside the board (reader, retrieval, chat citations)
// and the dialect markers ("[]", "." bullets, roman labels) don't get indexed as noise. These
// tests lock in that normalisation: the transform must actually change dialect notes (or the
// wiring would be a no-op) while leaving native markers and prose byte-for-byte.
describe("toggleTaskAt / countTasks — ticking a box in the RENDERED note", () => {
  it("flips the note dialect's own marker", () => {
    expect(toggleTaskAt("[] buy milk", 0)).toBe("[x] buy milk");
    expect(toggleTaskAt("[x] buy milk", 0)).toBe("[] buy milk");
    expect(toggleTaskAt("[ ] buy milk", 0)).toBe("[x] buy milk");
    expect(toggleTaskAt("[X] buy milk", 0)).toBe("[] buy milk");
  });

  it("flips a literal GFM task item too — it renders as a checkbox, so it must be tickable", () => {
    // `-` wins the marker alternation, so toRenderMarkdown passes "- [x] foo" through untouched and
    // the renderer makes a real task item out of it. Counting only the note dialect would map the
    // Nth rendered box to the wrong source line.
    expect(toggleTaskAt("- [ ] ship it", 0)).toBe("- [x] ship it");
    expect(toggleTaskAt("- [x] ship it", 0)).toBe("- [ ] ship it");
    expect(toggleTaskAt("* [ ] ship it", 0)).toBe("* [x] ship it");
  });

  it("indexes in rendered order across both dialects and skips non-task lines", () => {
    const note = "heading\n[] one\n- a bullet\n- [ ] two\n\n[x] three";
    expect(countTasks(note)).toBe(3);
    expect(toggleTaskAt(note, 1)).toContain("- [x] two");
    expect(toggleTaskAt(note, 1)).toContain("[] one"); // untouched
    expect(toggleTaskAt(note, 2)).toContain("[] three");
  });

  it("keeps indentation, content and every other line byte-for-byte", () => {
    expect(toggleTaskAt("  [] nested", 0)).toBe("  [x] nested");
    expect(toggleTaskAt("a\n[] b\nc", 0)).toBe("a\n[x] b\nc");
  });

  it("can force a state rather than flipping — the DOM event already knows the new value", () => {
    expect(toggleTaskAt("[x] done", 0, true)).toBe("[x] done");
    expect(toggleTaskAt("[] todo", 0, false)).toBe("[] todo");
  });

  it("returns null for an index that names no checkbox, so a stale click is a no-op", () => {
    expect(toggleTaskAt("[] only one", 1)).toBeNull();
    expect(toggleTaskAt("no boxes here", 0)).toBeNull();
    expect(toggleTaskAt("[] x", -1)).toBeNull();
    expect(countTasks("no boxes here")).toBe(0);
  });
});

describe("toRenderMarkdown — shorthand dialect → GFM (F-52)", () => {
  it("normalises checkbox markers to GFM task-list items", () => {
    // On the BULLET marker, so a checklist and the round bullets around it stay ONE list — a marker
    // change starts a new list in CommonMark, and two lists carry a `ul` margin between them.
    expect(toRenderMarkdown("[] buy milk")).toBe("* [ ] buy milk");
    expect(toRenderMarkdown("[x] done")).toBe("* [x] done");
    expect(toRenderMarkdown("[X] done")).toBe("* [x] done"); // case-folded to lowercase x
  });

  it("emits the two bullet kinds as two DIFFERENT GFM markers", () => {
    // The whole point: "." and "-" used to collapse to the same "- " item, so nothing downstream
    // could tell a round bullet from a dash point. The round one is NOT "-" any more — "-" is the
    // dash point's own input marker now, so the transform would read its own output back as dashes.
    expect(toRenderMarkdown(". first")).toBe("* first");
    expect(toRenderMarkdown("- first")).toBe("+ first");
  });

  it("keeps roman labels but appends a hard break so a run stays multi-line", () => {
    // Two trailing spaces = a Markdown hard break; without it GFM would merge the single newlines
    // into one paragraph and the "i."/"ii." labels would collapse together.
    expect(toRenderMarkdown("i. alpha\nii. beta")).toBe("i. alpha  \nii. beta  ");
  });

  it("preserves indentation on transformed markers", () => {
    expect(toRenderMarkdown("  . nested")).toBe("  * nested");
    expect(toRenderMarkdown("  - nested")).toBe("  + nested");
    expect(toRenderMarkdown("  [] task")).toBe("  * [ ] task");
  });

  it("leaves native list/quote markers byte-for-byte", () => {
    // "-" is deliberately no longer among them: it is the dash point's marker, and a dash point is
    // a different rendering from a plain bullet, so passing it through would BE the reported bug.
    const native = "* bullet\n+ dash\n1. one\n> quote";
    expect(toRenderMarkdown(native)).toBe(native);
  });

  it("leaves a literal GFM task alone whichever bullet carries it", () => {
    // countTasks and toggleTaskAt match the SOURCE line, so a rendered checkbox whose source
    // counterpart had moved would tick a different line than the one clicked.
    for (const line of ["- [x] done", "* [ ] todo", "+ [x] done"]) {
      expect(toRenderMarkdown(line)).toBe(line);
    }
  });

  it("gives plain prose lines a hard break so manual line breaks survive rendering (#394)", () => {
    // GFM folds a single newline into a space, so a note typed across several lines would render
    // as one line. Two trailing spaces make each newline a hard break, keeping the note's shape.
    expect(toRenderMarkdown("line one\nline two")).toBe("line one  \nline two  ");
    // Blank lines stay as paragraph breaks; an already-broken line isn't doubled (idempotent).
    expect(toRenderMarkdown("para one\n\npara two")).toBe("para one  \n\npara two  ");
    expect(toRenderMarkdown("already broken  ")).toBe("already broken  ");
  });

  it("actually changes a dialect note (the ingest wiring is not a no-op)", () => {
    const raw = "[] task one\n. a bullet\n- a dash point\ni. roman";
    const rendered = toRenderMarkdown(raw);
    expect(rendered).not.toBe(raw); // ingesting `rendered` differs from ingesting `raw`
    expect(rendered).toBe("* [ ] task one\n* a bullet\n+ a dash point\ni. roman  ");
  });

  it("is idempotent on its own OUTPUT alphabet", () => {
    // Which is what has to round-trip: this output is the copy that reaches the vault, so a second
    // pass appending another hard break would grow whitespace on every re-promote.
    for (const note of ["* a\n+ b\n\nsome prose", "- a\n. b\n[] c\nprose", "* [x] done\n* a"]) {
      const once = toRenderMarkdown(note);
      expect(toRenderMarkdown(once)).toBe(once);
    }
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

describe("caretForRestore — where the caret lands after an undo", () => {
  it("puts the caret where an undone insertion was, not at the end", () => {
    // Typed "bcd" into the middle, then undid it: the caret belongs where the text vanished from.
    expect(caretForRestore("abcdef", "aef")).toBe(1);
  });

  it("handles an undone deletion the same way", () => {
    expect(caretForRestore("aef", "abcdef")).toBe(4);
  });

  it("puts the caret at the end of an append that was undone", () => {
    expect(caretForRestore("hello world", "hello")).toBe(5);
  });

  it("bounds the suffix run so repeats can't be counted twice", () => {
    // The discriminating case: "aa" and "aaa" share a 2-char prefix AND a 2-char suffix, but "aaa"
    // is only 3 long. An unbounded suffix scan would over-count and put the caret at 1; the caret
    // belongs after the character that was added.
    expect(caretForRestore("aa", "aaa")).toBe(3);
    expect(caretForRestore("aaa", "aa")).toBe(2);
  });

  it("is the length itself when nothing is shared", () => {
    expect(caretForRestore("abc", "xyz")).toBe(3);
  });

  it("handles either side being empty", () => {
    expect(caretForRestore("", "abc")).toBe(3);
    expect(caretForRestore("abc", "")).toBe(0);
  });

  it("never returns a position outside the restored text", () => {
    for (const [from, to] of [
      ["", ""],
      ["a", "a"],
      ["abc", "abc"],
      ["aaaa", "aa"],
      ["x", "xxxxx"],
    ] as const) {
      const c = caretForRestore(from, to);
      expect(c).toBeGreaterThanOrEqual(0);
      expect(c).toBeLessThanOrEqual(to.length);
    }
  });
});

describe("fenced code is not a list — it is a picture of one", () => {
  // A `- ` line inside a ```diff block is a REMOVED line. Rewriting it to the dash-point marker
  // makes it read as an ADDED one, so the snippet says the opposite of what was pasted — and says it
  // in the vault copy too, since this output is what gets ingested.
  it("leaves a pasted diff meaning what it said", () => {
    const note = [
      "Patch to review:",
      "",
      "```diff",
      '- const BULLET = "-";',
      '+ const BULLET = "*";',
      "```",
    ].join("\n");
    const out = toRenderMarkdown(note).split("\n");
    expect(out).toContain('- const BULLET = "-";');
    expect(out).toContain('+ const BULLET = "*";');
  });

  it("leaves a YAML list, and every other marker, exactly as typed", () => {
    // Not only the dash: the round-bullet, checkbox and roman branches all rewrite a line by its
    // first token, and all of them are wrong inside a fence. The hard-break and blank-line rules
    // are too — two trailing spaces in a code block are two characters of code.
    const body = [
      "- name: build",
      ". not a bullet",
      "[] not a checkbox",
      "i. not roman",
      "plain line",
    ];
    const note = ["```yaml", ...body, "```"].join("\n");
    expect(toRenderMarkdown(note)).toBe(note);
  });

  it("still transforms the note either side of the fence", () => {
    const out = toRenderMarkdown(["- before", "```", "- inside", "```", "- after"].join("\n"));
    expect(out.split("\n")).toEqual(["+ before", "```", "- inside", "```", "+ after"]);
  });

  it("is idempotent over a note containing a fence", () => {
    // The pass runs on every render AND on every ingest, so a note that is edited twice must not
    // drift. Anything that grows on a second run corrupts the filed document, not just the view.
    const note = [
      "intro",
      "",
      "```sh",
      "- ls -la",
      "",
      "  indented output",
      "```",
      "",
      "outro",
    ].join("\n");
    const once = toRenderMarkdown(note);
    expect(toRenderMarkdown(once)).toBe(once);
  });

  it("respects the fence rules that decide where a block ends", () => {
    // A backtick fence with a backtick in its info string does not open one (that is what keeps
    // inline code from swallowing the rest of a note); only a fence of the same character and at
    // least the same length closes; and an unclosed fence runs to the end.
    expect(toRenderMarkdown("``` `- x`\n- y").split("\n")).toEqual(["``` `- x`  ", "+ y"]);
    expect(toRenderMarkdown("~~~~\n- a\n~~~\n- b\n~~~~\n- c").split("\n")).toEqual([
      "~~~~",
      "- a",
      "~~~",
      "- b",
      "~~~~",
      "+ c",
    ]);
    expect(toRenderMarkdown("```\n- a\n- b").split("\n")).toEqual(["```", "- a", "- b"]);
  });

  it("does not offer a checkbox inside a fence, so the real ones stay in step", () => {
    // The renderer emits a fenced checkbox as literal text with no `<input>`. Counting it here would
    // make every real box after it one out of step, and a click would tick a different line.
    const note = ["[] real one", "```", "[] not tickable", "```", "[] real two"].join("\n");
    expect(countTasks(note)).toBe(2);
    expect(toggleTaskAt(note, 1)).toBe(
      ["[] real one", "```", "[] not tickable", "```", "[x] real two"].join("\n"),
    );
    expect(toggleTaskAt(note, 2)).toBeNull();
  });
});
