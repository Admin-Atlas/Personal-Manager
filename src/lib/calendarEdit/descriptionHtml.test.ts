// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The description codec (C19 §2): what a Google description shows, whether PM can write it back,
// how PM writes one, and when two are the same. The shared fixture is the one Rust reads too.

import { describe, expect, it } from "vitest";
import fixture from "./descriptionHtml.fixture.json";
import {
  EDITABLE_TAGS,
  MAX_DEPTH,
  docsEqual,
  isEmptyDoc,
  keptLink,
  linkFromTyped,
  linkTarget,
  looksLikeHtml,
  normalise,
  parseDescription,
  replacementDescription,
  replacementText,
  sameDescription,
  serialiseDescription,
  textLinks,
  type Block,
  type DescDoc,
  type DescNode,
  type Mark,
} from "./descriptionHtml";
import { KNOWN, LEGACY, asciiLower, isAlpha, isSpace } from "./descriptionTokens";

const t = (text: string): DescNode => ({ kind: "text", text });
const BR: DescNode = { kind: "br" };
const el =
  (kind: Mark | Block) =>
  (...children: DescNode[]): DescNode => ({ kind, children });
const b = el("b");
const i = el("i");
const u = el("u");
const p = el("p");
const ul = el("ul");
const ol = el("ol");
const li = el("li");
const a = (href: string, ...children: DescNode[]): DescNode => ({ kind: "a", href, children });

/** What a description shows, and whether PM can write it back. */
const read = (description: string) => {
  const parsed = parseDescription(description);
  return { doc: parsed.doc, supported: parsed.supported };
};

interface Row {
  description: string;
  html: boolean;
  markup?: boolean;
  pm_can_edit: boolean;
  supported: boolean;
  why: string;
}
const ROWS: Row[] = fixture.rows;

/** §5.1's `pm_can_edit`, the rule Rust builds in C19c, so the column Rust will read is right today:
 *  plain text, or formatting whose every tag is one PM writes, with no comment, declaration or tag
 *  the text ends inside. Laxer than the webview: it never looks at attributes, links or nesting. */
function pmCanEdit(s: string): boolean {
  if (!looksLikeHtml(s)) return true;
  let at = s.indexOf("<");
  while (at >= 0) {
    const next = s[at + 1] ?? "";
    if (next === "!" || next === "?") return false;
    const name = next === "/" ? at + 2 : at + 1;
    if (!isAlpha(s[name] ?? "")) {
      if (next === "/") return false;
      at = s.indexOf("<", at + 1);
      continue;
    }
    let j = name;
    while (j < s.length && !isSpace(s[j]) && s[j] !== "/" && s[j] !== ">") j++;
    if (!EDITABLE_TAGS.includes(asciiLower(s.slice(name, j)))) return false;
    // The tag's end, as the tokenizer reads it (C19 review 1): a quote opens a value only right
    // after "=" and whitespace; an unquoted value runs to whitespace or ">", and a "=" or a quote
    // inside it opens nothing.
    const end = tagEnd(s, j);
    if (end < 0) return false;
    at = s.indexOf("<", end);
  }
  return true;
}

/** Just past the ">" ending a tag whose name ends at `from`, or -1 if the text ends first. */
function tagEnd(s: string, from: number): number {
  const n = s.length;
  let i = from;
  for (;;) {
    while (i < n && (isSpace(s[i]) || s[i] === "/")) i++;
    if (i >= n) return -1;
    if (s[i] === ">") return i + 1;
    let j = s[i] === "=" ? i + 1 : i;
    while (j < n && !isSpace(s[j]) && s[j] !== "/" && s[j] !== ">" && s[j] !== "=") j++;
    i = j;
    let k = i;
    while (k < n && isSpace(s[k])) k++;
    if (s[k] !== "=") continue;
    k++;
    while (k < n && isSpace(s[k])) k++;
    if (k >= n) return -1;
    if (s[k] === '"' || s[k] === "'") {
      const close = s.indexOf(s[k], k + 1);
      if (close < 0) return -1;
      i = close + 1;
    } else {
      i = k;
      while (i < n && !isSpace(s[i]) && s[i] !== ">") i++;
    }
  }
}

/** C1-L2's strict-valid: what PM's own reader takes back as it is. `li` only in a list or at the top;
 *  no block in a paragraph, no link in a link; no empty or doubled mark; texts merged, no raw line
 *  break. Returns the first broken rule, or null. */
function broken(doc: DescDoc): string | null {
  const walk = (nodes: DescDoc, parent: string, inP: boolean, inLink: boolean): string | null => {
    for (let k = 0; k < nodes.length; k++) {
      const node = nodes[k];
      if (node.kind === "text") {
        if (node.text === "") return "an empty text";
        if (/[\r\n]/.test(node.text)) return "a raw line break";
        if (nodes[k + 1]?.kind === "text") return "two texts side by side";
        continue;
      }
      if (node.kind === "br") continue;
      if (node.kind === "li" && !["root", "ul", "ol"].includes(parent))
        return "an item outside a list";
      if (["p", "ul", "ol", "li"].includes(node.kind) && inP) return "a block in a paragraph";
      if (node.kind === "a" && inLink) return "a link in a link";
      const inline =
        node.kind === "a" || node.kind === "b" || node.kind === "i" || node.kind === "u";
      if (inline && node.children.length === 0) return "empty formatting";
      if (node.kind === parent && inline) return "a mark inside the same mark";
      if (node.kind === "a" && keptLink(node.href) !== node.href)
        return "an address the model can't hold";
      const found = walk(
        node.children,
        node.kind,
        node.kind === "p" || (inP && inline),
        inLink || node.kind === "a",
      );
      if (found !== null) return found;
    }
    return null;
  };
  return walk(doc, "root", false, false);
}

/** How deep the deepest element sits (a top-level one is 1; text inside it doesn't count). */
function depthOf(doc: DescDoc): number {
  let deepest = 0;
  const todo: [DescDoc, number][] = [[doc, 1]];
  while (todo.length > 0) {
    const [nodes, depth] = todo.pop() as [DescDoc, number];
    for (const node of nodes) {
      if (node.kind === "text" || node.kind === "br") continue;
      deepest = Math.max(deepest, depth);
      todo.push([node.children, depth + 1]);
    }
  }
  return deepest;
}

/** A seeded generator (mulberry32), so a failure names a document that can be made again. */
function random(seed: number): () => number {
  let s = seed;
  return () => {
    s = (s + 0x6d2b79f5) | 0;
    let x = Math.imul(s ^ (s >>> 15), 1 | s);
    x = (x + Math.imul(x ^ (x >>> 7), 61 | x)) ^ x;
    return ((x ^ (x >>> 14)) >>> 0) / 4294967296;
  };
}
const TEXTS = [
  "a",
  "word",
  " ",
  "  two  spaces ",
  // A no-break space (U+00A0), which an editor writes for a run of spaces.
  " ",
  "x & y",
  "<b>",
  "a < b > c",
  '"quoted"',
  "&amp;",
  "it's",
  "tab\there",
  "line\nbreak",
  "cr\r\nlf",
  "é ☕",
  "<https://example.com>",
  "&#39;",
];
const HREFS = [
  "https://example.com",
  "https://example.com/?a=1&b=2",
  'https://example.com/"q"<x>',
  "mailto:someone@example.com",
  "tel:+15550100",
  "HTTP://EXAMPLE.COM/x",
];
const KINDS = ["b", "i", "u", "a", "p", "ul", "ol", "li"] as const;

/** Any tree of the model's nodes, nestings PM's reader would never make included. */
function generate(next: () => number, depth: number): DescNode[] {
  const pick = <T>(from: readonly T[]): T => from[Math.floor(next() * from.length)];
  const out: DescNode[] = [];
  const count = Math.floor(next() * 4);
  for (let k = 0; k < count; k++) {
    const r = next();
    if (depth >= 4 || r < 0.35) out.push(t(pick(TEXTS)));
    else if (r < 0.45) out.push(BR);
    else {
      const kind = pick(KINDS);
      const children = generate(next, depth + 1);
      out.push(kind === "a" ? a(pick(HREFS), ...children) : { kind, children });
    }
  }
  return out;
}
const DOCS: DescDoc[] = (() => {
  const next = random(19);
  return Array.from({ length: 500 }, () => generate(next, 0));
})();

describe("the shared fixture", () => {
  it("the_fixture_rows_read_as_the_fixture_says", () => {
    ROWS.forEach((row, k) => {
      const at = `F${k + 1}`;
      const parsed = parseDescription(row.description);
      expect(looksLikeHtml(row.description), at).toBe(row.html);
      expect(parsed.html, at).toBe(row.html);
      expect(parsed.supported, at).toBe(row.supported);
      if (row.markup !== undefined) expect(parsed.markup, at).toBe(row.markup);
      // The backend is never stricter than the webview.
      if (row.supported) expect(row.pm_can_edit, at).toBe(true);
    });
  });

  // Rust's pm_can_edit_unseen reads the same table, so it can't let through what the reader refuses.
  it("the_fixture_addresses_read_as_the_fixture_says", () => {
    for (const { href, kept } of fixture.hrefs) {
      expect(parseDescription(`<a href="${href}">x</a>`).supported, href).toBe(kept);
    }
  });

  it("the_legacy_names_are_the_fixtures", () => {
    expect([...LEGACY].sort()).toEqual([...fixture.legacy].sort());
  });

  it("the_fixture_pm_can_edit_column_follows_the_backend_rule", () => {
    ROWS.forEach((row, k) => {
      expect(pmCanEdit(row.description), `F${k + 1}`).toBe(row.pm_can_edit);
    });
  });

  it("the_editable_tags_are_the_fixtures", () => {
    expect([...EDITABLE_TAGS]).toEqual(fixture.tags);
  });

  // A tag PM writes is never read as words, whatever follows its name (review 3).
  it("the_editable_tags_are_all_known_names", () => {
    for (const tag of EDITABLE_TAGS) expect(KNOWN.has(tag), tag).toBe(true);
  });
});

describe("words in angle brackets (review 3)", () => {
  const shown = (doc: DescDoc): string =>
    JSON.stringify(doc)
      .match(/"text":"[^"]*"/g)!
      .join(" ");

  // What the reader builds a view from, were one of these ever beside formatting.
  it.each([
    ["Ask <José> about it", "<José>"],
    ["<b>Dial in</b>: <Room A, B>", "<Room A, B>"],
    ["<b>Ask</b> <John O'Brien> today", "<John O'Brien>"],
  ])("the_words_show_in_the_formatted_view: %j", (description, words) => {
    expect(shown(parseDescription(description).doc)).toContain(words);
  });

  it.each([
    '<a href="https://zoom.us/j/1";>Join</a>',
    '<a href="https://zoom.us/j/1", target="_blank">Join</a>',
  ])("a_link_with_a_stray_character_is_the_link_a_browser_draws: %j", (description) => {
    const parsed = parseDescription(description);
    expect(parsed.supported).toBe(false);
    expect(parsed.markup).toBe(true);
    expect(JSON.stringify(parsed.doc)).toContain('"href":"https://zoom.us/j/1"');
    expect(shown(parsed.doc)).toContain("Join");
    expect(shown(parsed.doc)).not.toContain("zoom");
  });

  it("a_never_shown_element_with_something_odd_in_its_tag_stays_never_shown", () => {
    const parsed = parseDescription("<style 1>p{}</style>x");
    expect(parsed.supported).toBe(false);
    expect(shown(parsed.doc)).not.toContain("p{}");
    expect(shown(parsed.doc)).toContain("x");
  });
});

describe("reading", () => {
  it.each<[string, DescDoc]>([
    ["<html-blob>Line<br>Line two</html-blob>", [t("Line"), BR, t("Line two")]],
    ["<html-blob><u></u><u></u>Hi</html-blob>", [t("Hi")]],
    ["<strong><b>Both</b></strong> <em>it</em>", [b(t("Both")), t(" "), i(t("it"))]],
    [
      '<span style="background-color: var(--textfield-surface); color: var(--on-surface);">x</span>',
      [t("x")],
    ],
    [
      '<a href="https://example.com/?a=1&amp;b=2" target="_blank" id="ow12" __is_owner="true">site</a>',
      [a("https://example.com/?a=1&b=2", t("site"))],
    ],
    [
      '<a href="https://www.google.com/url?q=https://example.com/a&amp;sa=D&amp;usg=REDACTED">agenda</a>',
      [a("https://www.google.com/url?q=https://example.com/a&sa=D&usg=REDACTED", t("agenda"))],
    ],
    [
      "<b>Agenda</b><br><ul><li>One</li><li>Two</li></ul>",
      [b(t("Agenda")), BR, ul(li(t("One")), li(t("Two")))],
    ],
    [
      '<a href="www.example.org/path">PM site</a>',
      [a("https://www.example.org/path", t("PM site"))],
    ],
    ['<a href="tel:+15550100">call</a>', [a("tel:+15550100", t("call"))]],
    [
      '<a href="https://example.com" rel="noopener" lang="en" data-x="1" aria-label="Site">x</a>',
      [a("https://example.com", t("x"))],
    ],
    ["<p>One</p><p><br></p><p>Three</p>", [p(t("One")), p(BR), p(t("Three"))]],
    ["<ul><li>a</li><ul><li>b</li></ul></ul>", [ul(li(t("a")), ul(li(t("b"))))]],
    ["<ol><li>a<ol><li>b</li></ol></li></ol>", [ol(li(t("a"), ol(li(t("b")))))]],
    ["<span>&nbsp;</span>", [t(" ")]],
    ["<b/>x", [b(t("x"))]],
    ["a</br>b<br/>c", [t("a"), BR, t("b"), BR, t("c")]],
    ["<B>Upper</B><BR>", [b(t("Upper"))]],
    ["<b>&lt;b&gt;</b> Tom &amp; Jerry", [b(t("<b>")), t(" Tom & Jerry")]],
  ])("google_shapes_read_as_google_shows_them: %j", (description, doc) => {
    expect(read(description)).toEqual({ doc, supported: true });
  });

  it.each<[string, DescDoc]>([
    // What WebKit and Chromium make of Enter (V3).
    [
      "Line<div>Two</div><div><br></div><div>Four</div>",
      [t("Line"), BR, t("Two"), BR, BR, t("Four")],
    ],
    ["<div>a</div><div>b</div>", [t("a"), BR, t("b")]],
    ["a<br><div>b</div>", [t("a"), BR, t("b")]],
    ["<div><b>a</b></div><div><p>b</p></div>c", [b(t("a")), BR, p(t("b")), t("c")]],
  ])("engine_lines_read_as_lines: %j", (description, doc) => {
    expect(read(description)).toEqual({ doc, supported: true });
  });

  // C19 review 1: a line ended inside formatting has ended (Bold, Enter, Enter, as both engines
  // and Google's own copies leave it), whitespace between lines draws nothing, and a line inside
  // formatting breaks after words before it. Each is drawn this way by Chromium and WebKitGTK.
  it.each<[string, DescDoc]>([
    ["<b>a</b><div><b><br></b></div><div>c</div>", [b(t("a")), BR, b(BR), t("c")]],
    ["<i>a</i><div><i><br></i></div><div><i>c</i></div>", [i(t("a")), BR, i(BR), i(t("c"))]],
    ["<div>a</div> <div>b</div>", [t("a"), BR, t(" b")]],
    ["a<i><div>x</div></i>", [t("a"), i(BR, t("x"), BR)]],
    ["<p><b>x<br></b><br></p>", [p(b(t("x"), BR), BR)]],
    ['<div style="font-weight:bold">x<br></div>y', [b(t("x"), BR), t("y")]],
  ])("a_line_that_ended_inside_formatting_has_ended: %j", (description, doc) => {
    const parsed = read(description);
    expect(parsed.doc).toEqual(doc);
    // And a save writes back what it read.
    expect(read(serialiseDescription(parsed.doc)).doc).toEqual(doc);
  });

  // A reference PM leaves as written, that a browser draws as a character, would be written back
  // as the code (C19 review 1); and a carriage return written as one draws as a space.
  it.each<[string, DescDoc]>([
    ["<b>&check;</b>", [b(t("&check;"))]],
    ["<b>&comma;</b>", [b(t("&comma;"))]],
    ["<b>&copy 2026</b>", [b(t("&copy 2026"))]],
    ["<b>&notes</b>", [b(t("&notes"))]],
    ["x &foo; y", [t("x &foo; y")]],
  ])("references_a_browser_may_draw_refuse: %j", (description, doc) => {
    expect(read(description)).toEqual({ doc, supported: false });
  });

  it("references_no_browser_draws_stay_supported", () => {
    expect(read("<b>Q&A, R&D, AT&T &#;</b>").supported).toBe(true);
    expect(read('<a href="https://x.example?a=1&amp;b=2">y</a>').supported).toBe(true);
    expect(read('<a href="https://x.example?a=1&amp">y</a>').supported).toBe(false);
  });

  it.each(["<b>a&#13;b</b>", "<b>a&#xD;b</b>", "a&#13;b"])(
    "a_carriage_return_written_as_a_reference_refuses: %j",
    (description) => {
      expect(read(description).supported).toBe(false);
    },
  );

  // Review round 1 of PR 1: display only, the read-only view draws it as a browser does.
  it("a_carriage_return_written_as_a_reference_shows_as_a_space", () => {
    expect(read("<b>a&#13;b</b>").doc).toEqual([b(t("a b"))]);
    expect(read("a&#xD;b").doc).toEqual([t("a b")]);
  });

  it("raw_newlines_are_lines_except_between_blocks", () => {
    // Shown as Google is thought to show them, but not written back until E11-1's H9 says so.
    expect(read("<b>a</b>\nb")).toEqual({ doc: [b(t("a")), BR, t("b")], supported: false });
    expect(read("<ul>\n<li>x</li>\n</ul>\n<p>y</p>")).toEqual({
      doc: [ul(li(t("x"))), p(t("y"))],
      supported: false,
    });
    // Plain lines are lines, CR LF and CR included.
    expect(read("a\nb\r\nc\rd")).toEqual({
      doc: [t("a"), BR, t("b"), BR, t("c"), BR, t("d")],
      supported: true,
    });
    // Runs of spaces are kept as written, and written back so.
    expect(read("<b>two  spaces</b>")).toEqual({ doc: [b(t("two  spaces"))], supported: true });
  });

  it.each<[string, DescDoc, boolean]>([
    ["font-weight: bold", [b(t("x"))], true],
    ["font-weight:600", [b(t("x"))], true],
    ["font-weight: 900", [b(t("x"))], true],
    ["font-weight: bolder", [b(t("x"))], true],
    ["FONT-WEIGHT: Bold !important", [b(t("x"))], true],
    ["font-weight: bold!important;", [b(t("x"))], true],
    ["font-weight: 950", [b(t("x"))], false],
    ["font-style: italic", [i(t("x"))], true],
    ["font-style: oblique", [i(t("x"))], true],
    ["text-decoration: underline", [u(t("x"))], true],
    ["text-decoration-line: underline", [u(t("x"))], true],
    ["text-decoration: underline wavy", [t("x")], false],
    ["font-weight: 700; font-style: italic; text-decoration: underline", [b(i(u(t("x"))))], true],
    ["color: var(--on-surface)", [t("x")], true],
    ["background-color: var( --textfield-surface )", [t("x")], true],
    ["color: red", [t("x")], false],
    ["color: var(--x, red)", [t("x")], false],
    ["font-size: var(--x)", [t("x")], false],
    ["margin: 0", [t("x")], false],
    ["font-weight", [t("x")], false],
    [";;", [t("x")], true],
    // A no-break space is not CSS's whitespace, so this declaration means nothing to a browser.
    ["font-weight: bold", [t("x")], false],
  ])("styles_read_as_marks_or_refuse: %j", (style, doc, supported) => {
    expect(read(`<span style="${style}">x</span>`)).toEqual({ doc, supported });
  });

  it.each<[string, DescDoc]>([
    // Google Docs' paste wrapper.
    ['<b style="font-weight: normal" id="docs-internal-guid-1">plain</b>', [t("plain")]],
    ['<b>a<span style="font-weight: 400">b</span>c</b>', [b(t("a")), t("b"), b(t("c"))]],
    ['<i>a<span style="font-style: normal">b</span></i>', [i(t("a")), t("b")]],
    ['<b><p style="font-weight: normal">x</p>y</b>', [p(t("x")), b(t("y"))]],
    // Bold inside plain is bold again.
    ['<span style="font-weight: normal"><b>x</b></span>', [b(t("x"))]],
    // An underline draws through a child that turns its own off, as CSS draws it; its own goes.
    ['<u>a<span style="text-decoration: none">b</span></u>', [u(t("ab"))]],
    ['<u style="text-decoration: none">x</u>', [t("x")]],
  ])("turning_formatting_off_shows_plain_and_refuses: %j", (description, doc) => {
    expect(read(description)).toEqual({ doc, supported: false });
  });

  it.each([
    "<script>alert(1)</script>after",
    "<SCRIPT src=x></SCRIPT>after",
    "<img src=x onerror=alert(1)>after",
    '<iframe src="https://example.com">x</iframe>after',
    "<style>b { color: red }</style>after",
    '<svg><a href="javascript:alert(1)">link</a></svg>after',
    '<a href="javascript:alert(1)">link</a>after',
    '<a href="JAVASCRIPT:alert(1)">link</a>after',
    '<a href=" javascript:alert(1)">link</a>after',
    '<a href="&#106;avascript:alert(1)">link</a>after',
    '<a href="java&#9;script:alert(1)">link</a>after',
    '<a href="data:text/html,x">link</a>after',
    '<a href="vbscript:x">link</a>after',
    '<a href="//evil.example/x">link</a>after',
    '<a href="\\\\evil.example/x">link</a>after',
    '<a href="/\\evil.example/x">link</a>after',
    '<a href="/relative">link</a>after',
    '<a href="example">link</a>after',
    '<a href="https:evil.example">link</a>after',
    '<a href="https://example.com" onclick="steal()">link</a>after',
    '<span style="color: red; background: url(https://evil.example/x)">after</span>',
  ])("canaries_fail_closed_and_keep_no_link: %j", (description) => {
    const parsed = parseDescription(description);
    expect(parsed.supported).toBe(false);
    const shown = JSON.stringify(parsed.doc);
    expect(shown).toContain("after");
    expect(shown).not.toMatch(/alert|color: red|steal/);
    if (description.includes("link</a>") && !description.includes("onclick")) {
      expect(parsed.doc).toEqual([t("linkafter")]);
    }
  });

  it("a_link_with_an_attribute_that_does_something_keeps_its_address_but_refuses", () => {
    // What shows is still the link; PM just won't write the description back.
    expect(read('<a href="https://example.com" onclick="steal()">link</a>')).toEqual({
      doc: [a("https://example.com", t("link"))],
      supported: false,
    });
  });

  it("items_inside_formatting_are_lifted_out_and_refuse", () => {
    expect(read("<b><li>x</li></b>")).toEqual({ doc: [li(b(t("x")))], supported: false });
    expect(read('<a href="https://example.com"><li>x</li></a>')).toEqual({
      doc: [li(a("https://example.com", t("x")))],
      supported: false,
    });
    // An item closes the one it is in through formatting, as browsers do.
    expect(read("<ul><li><b>a<li>b</li></b></li></ul>")).toEqual({
      doc: [ul(li(b(t("a"))), li(t("b")))],
      supported: false,
    });
    // Through an element that draws nothing, the same, and nothing is lost.
    expect(read("<ul><li><span>a<li>b</li></ul>").doc).toEqual([ul(li(t("a")), li(t("b")))]);
  });

  it("misnested_and_unclosed_elements_read_as_browsers_close_them", () => {
    expect(read("<b>a<i>b</b>c</i>")).toEqual({
      doc: [b(t("a"), i(t("b"))), t("c")],
      supported: false,
    });
    expect(read("<p>a<ul><li>b</li></ul></p>")).toEqual({
      doc: [p(t("a")), ul(li(t("b")))],
      supported: false,
    });
    expect(read('<a href="https://a.example">x<a href="https://b.example">y</a></a>')).toEqual({
      doc: [a("https://a.example", t("x")), a("https://b.example", t("y"))],
      supported: false,
    });
    expect(read("a</b>c")).toEqual({ doc: [t("ac")], supported: false });
    // Unclosed at the end is what browsers do too.
    expect(read("<ul><li>a<li>b")).toEqual({ doc: [ul(li(t("a")), li(t("b")))], supported: true });
  });

  it("other_blocks_and_cells_read_as_lines_and_refuse", () => {
    expect(
      read(
        "<h2>Title</h2>Body<table><tr><td>Dial-in</td><td>+1 555</td></tr><tr><td>PIN</td></tr></table>",
      ),
    ).toEqual({
      doc: [t("Title"), BR, t("Body"), BR, t("Dial-in +1 555"), BR, t("PIN")],
      supported: false,
    });
    expect(read("a<hr>b")).toEqual({ doc: [t("a"), BR, t("b")], supported: false });
    // A cell's formatted words are owed the gap too; one ending in a space has it already.
    expect(read("<table><tr><td><b>a</b></td><td>b </td><td>c</td></tr></table>")).toEqual({
      doc: [b(t("a")), t(" b c")],
      supported: false,
    });
  });

  it("addresses_in_angle_brackets_show_as_written_and_refuse", () => {
    expect(read("Click here to join<https://example.com/j/1>")).toEqual({
      doc: [t("Click here to join<https://example.com/j/1>")],
      supported: false,
    });
    expect(read("Organiser: Ann <ann@example.com>")).toEqual({
      doc: [t("Organiser: Ann <ann@example.com>")],
      supported: false,
    });
    expect(read("<b>x</b> <https://example.com>").doc).toEqual([
      b(t("x")),
      t(" <https://example.com>"),
    ]);
  });

  it("a_description_cut_inside_a_tag_shows_what_came_before", () => {
    expect(read('<b>Agenda</b> one <a href="https://exa')).toEqual({
      doc: [b(t("Agenda")), t(" one ")],
      supported: false,
    });
  });

  it("nesting_deeper_than_the_limit_is_unwrapped", () => {
    const parsed = parseDescription("<b><i>".repeat(1_000) + "x");
    expect(parsed.supported).toBe(false);
    expect(depthOf(parsed.doc)).toBeLessThanOrEqual(MAX_DEPTH);
    expect(JSON.stringify(parsed.doc)).toContain('"x"');
    // Style marks count towards it too.
    const styled = parseDescription(
      '<span style="font-weight: bold; font-style: italic; text-decoration: underline"><i>'.repeat(
        200,
      ) + "y",
    );
    expect(depthOf(styled.doc)).toBeLessThanOrEqual(MAX_DEPTH);
    // Twelve elements drawing three marks each are 36 deep: past the limit, so PM can't write it
    // back, though far fewer elements than the limit are open.
    const marked = parseDescription(
      '<b style="font-style: italic; text-decoration: underline">'.repeat(12) + "x",
    );
    expect(marked.supported).toBe(false);
    expect(depthOf(marked.doc)).toBeLessThanOrEqual(MAX_DEPTH);
  });
});

describe("links", () => {
  it.each([
    "https://example.com",
    "http://example.com/a?b=1&c=2#d",
    "HTTPS://EXAMPLE.COM",
    "mailto:someone@example.com",
    "MailTo:someone@example.com?subject=x",
  ])("linkTarget_takes_only_absolute_web_and_mail_addresses: %j", (href) => {
    expect(linkTarget(href)).toBe(href);
  });

  it.each([
    "",
    "https:example.com",
    "http:/example.com",
    "//example.com",
    "\\\\example.com",
    "/\\example.com",
    "/relative",
    "./relative",
    "#top",
    "example.com",
    "javascript:alert(1)",
    "JAVASCRIPT:alert(1)",
    "data:text/html,x",
    "vbscript:x",
    "ftp://example.com",
    "tel:+15550100",
    " https://example.com",
    "https://example.com ",
    "https://exa mple.com",
    "https://example.com/\u0000",
    "https://example.com/\u0085",
    // Ending in a no-break space (U+00A0), then in a line separator (U+2028).
    "https://example.com/ ",
    "https://example.com/ ",
  ])("linkTarget refuses %j", (href) => {
    expect(linkTarget(href)).toBeNull();
  });

  it("keptLink_also_keeps_phone_numbers", () => {
    expect(keptLink("https://example.com")).toBe("https://example.com");
    expect(keptLink("tel:+1(555)010-0,1;2#*")).toBe("tel:+1(555)010-0,1;2#*");
    expect(keptLink("TEL:5550100")).toBe("TEL:5550100");
    for (const refused of ["tel:", "tel:+", "tel:555 0100", "tel:555a", "sms:5550100", "tel:%35"]) {
      expect(keptLink(refused), refused).toBeNull();
    }
  });

  it.each<[string, string]>([
    ["example.com", "https://example.com"],
    ["  www.example.org/path?a=1#b  ", "https://www.example.org/path?a=1#b"],
    ["sub-domain.example.co.uk", "https://sub-domain.example.co.uk"],
    ["name@example.com", "mailto:name@example.com"],
    ["first.last+tag@mail.example.com", "mailto:first.last+tag@mail.example.com"],
    ["https://example.com/x", "https://example.com/x"],
    ["mailto:x@example.com", "mailto:x@example.com"],
    ["example.com/a@b", "https://example.com/a@b"],
  ])("typed_links_become_addresses_or_nothing: %j", (typed, address) => {
    expect(linkFromTyped(typed)).toBe(address);
  });

  it.each([
    "",
    "   ",
    "javascript:alert(1)",
    "//example.com",
    "two words",
    "example",
    "example..com",
    ".example.com",
    "a@b@example.com",
    "@example.com",
    "name@example",
    "name%40x@example.com",
    "https:example.com",
  ])("linkFromTyped refuses %j", (typed) => {
    expect(linkFromTyped(typed)).toBeNull();
  });

  it("a_typed_address_past_2000_characters_is_refused", () => {
    expect(linkFromTyped("x".repeat(1_996) + ".com")).toBe(`https://${"x".repeat(1_996)}.com`);
    expect(linkFromTyped("x".repeat(1_997) + ".com")).toBeNull();
  });

  it("links_stored_without_their_scheme_read_in_full", () => {
    expect(read('<a href="example.com">x</a>')).toEqual({
      doc: [a("https://example.com", t("x"))],
      supported: true,
    });
    expect(read('<a href="name@example.com">x</a>')).toEqual({
      doc: [a("mailto:name@example.com", t("x"))],
      supported: true,
    });
    // An address it can't read is its words.
    expect(read('<a href="example">x</a>')).toEqual({ doc: [t("x")], supported: false });
    expect(read('<a href=" example.com">x</a>')).toEqual({ doc: [t("x")], supported: false });
    expect(read("<a>x</a>")).toEqual({ doc: [t("x")], supported: false });
  });
});

describe("normalising", () => {
  it("text_merges_and_whitespace_between_blocks_goes", () => {
    expect(
      normalise([t("a"), t(""), t("b"), p(t("x")), t(" \n "), ul(t(" "), li(t("y")), t("\t"))]),
    ).toEqual([t("ab"), p(t("x")), ul(li(t("y")))]);
    // Whitespace that only comes before a paragraph, or is alone in a list, goes too.
    expect(normalise([t(" "), p(t("x")), ul(t(" \n"))])).toEqual([p(t("x")), ul()]);
    // Empty formatting goes; the same mark inside itself is one mark.
    expect(normalise([b(), t("a"), u(i()), b(b(t("x")), t("y"))])).toEqual([t("a"), b(t("xy"))]);
  });

  it("a_break_that_ends_a_line_after_its_words_goes", () => {
    // "a<br>" draws one line, so it is "a".
    expect(normalise([t("a"), BR])).toEqual([t("a")]);
    expect(normalise([p(t("a"), BR), li(b(t("b")), BR)])).toEqual([p(t("a")), li(b(t("b")))]);
    // "a<br><br>" draws an empty line under the words (what Enter at the end leaves), and a break
    // after a list starts one: both stay, so a save keeps the line Google shows.
    expect(normalise([t("a"), BR, BR])).toEqual([t("a"), BR, BR]);
    expect(normalise([li(t("b"), BR, BR)])).toEqual([li(t("b"), BR, BR)]);
    expect(normalise([ul(li(t("x"))), BR])).toEqual([ul(li(t("x"))), BR]);
    expect(read("a<div><br></div>")).toEqual({ doc: [t("a"), BR, BR], supported: true });
    // An empty line on its own draws, and so do breaks before words.
    expect(normalise([p(BR)])).toEqual([p(BR)]);
    expect(normalise([BR, BR])).toEqual([BR, BR]);
    expect(normalise([BR, t("a")])).toEqual([BR, t("a")]);
    // Inside formatting, a break stays.
    expect(normalise([b(t("a"), BR)])).toEqual([b(t("a"), BR)]);
    // A line that ended inside formatting has ended: the break after it draws an empty line, and
    // stays (C19 review 1). After words in formatting it goes; whitespace shows nothing.
    expect(normalise([li(t("x"), b(BR), BR)])).toEqual([li(t("x"), b(BR), BR)]);
    expect(normalise([p(b(t("x"), BR), BR)])).toEqual([p(b(t("x"), BR), BR)]);
    expect(normalise([t("x"), i(b(BR)), BR])).toEqual([t("x"), i(b(BR)), BR]);
    expect(normalise([b(t("x")), BR])).toEqual([b(t("x"))]);
    expect(normalise([t("a"), BR, t(" "), BR])).toEqual([t("a"), BR, t(" "), BR]);
  });

  it("nestings_the_reader_never_makes_are_made_ones_it_does", () => {
    expect(normalise([b(li(t("x")))])).toEqual([li(b(t("x")))]);
    expect(normalise([ul(b(t("a"), li(t("x")), t("c")))])).toEqual([
      ul(b(t("a")), li(b(t("x"))), b(t("c"))),
    ]);
    expect(normalise([li(t("a"), li(t("b")), t("c"))])).toEqual([
      li(t("a")),
      li(t("b")),
      li(t("c")),
    ]);
    expect(normalise([p(t("a"), ul(li(t("b"))), t("c"))])).toEqual([
      p(t("a")),
      ul(li(t("b"))),
      p(t("c")),
    ]);
    expect(normalise([p(b(t("a"), ul(li(t("b"))), t("c")))])).toEqual([
      p(b(t("a"))),
      b(ul(li(t("b")))),
      p(b(t("c"))),
    ]);
    expect(normalise([p(p(t("a")))])).toEqual([p(t("a"))]);
    expect(normalise([a("https://a.example", t("x"), b(a("https://b.example", t("y"))))])).toEqual([
      a("https://a.example", t("x"), b(t("y"))),
    ]);
    expect(normalise([b(li(b(t("x"))))])).toEqual([li(b(t("x")))]);
  });

  it("normalising_twice_changes_nothing", () => {
    DOCS.forEach((doc, k) => {
      const once = normalise(doc);
      expect(normalise(once), `document ${k}`).toEqual(once);
      expect(broken(once), `document ${k}`).toBeNull();
    });
  });

  it("nothing_deeper_than_the_limit_survives_but_its_text", () => {
    let deep: DescNode = t("x");
    for (let k = 0; k < 100_000; k++) deep = k % 2 === 0 ? b(deep, BR) : i(deep);
    const doc = normalise([deep]);
    expect(depthOf(doc)).toBeLessThanOrEqual(MAX_DEPTH);
    expect(JSON.stringify(doc)).toContain('"x"');
    // The other walks take any depth without recursing.
    expect(isEmptyDoc([deep])).toBe(false);
    expect(docsEqual([deep], [deep])).toBe(true);
    expect(serialiseDescription([deep]).length).toBeGreaterThan(100_000);
  });
});

describe("writing", () => {
  it("writing_escapes_the_four_characters", () => {
    expect(serialiseDescription([b(t(`a & b < c > d "e" it's`))])).toBe(
      "<b>a &amp; b &lt; c &gt; d &quot;e&quot; it's</b>",
    );
    expect(serialiseDescription([a('https://example.com/?a=1&b="x"<y>', t("l"))])).toBe(
      '<a href="https://example.com/?a=1&amp;b=&quot;x&quot;&lt;y&gt;">l</a>',
    );
  });

  it("writes_googles_subset_and_nothing_else", () => {
    expect(
      serialiseDescription([
        b(t("A")),
        BR,
        i(t("b")),
        u(t("c")),
        p(t("d")),
        ul(li(t("e"))),
        ol(li(t("f"))),
        a("mailto:x@example.com", t("g")),
      ]),
    ).toBe(
      '<b>A</b><br><i>b</i><u>c</u><p>d</p><ul><li>e</li></ul><ol><li>f</li></ol><a href="mailto:x@example.com">g</a>',
    );
    // A link the model could never hold is written as its words.
    expect(serialiseDescription([b(a("javascript:alert(1)", t("x")))])).toBe("<b>x</b>");
  });

  it("a_description_with_no_formatting_is_written_as_plain_lines", () => {
    expect(serialiseDescription([t("a"), BR, t("b")])).toBe("a\nb");
    expect(serialiseDescription([t("Tom & Jerry, x < y")])).toBe("Tom & Jerry, x < y");
    expect(serialiseDescription(parseDescription("<html-blob>Hi<br>there</html-blob>").doc)).toBe(
      "Hi\nthere",
    );
    expect(serialiseDescription(parseDescription("Tom &amp; Jerry").doc)).toBe("Tom & Jerry");
  });

  it("words_that_look_like_formatting_are_written_so_they_stay_words", () => {
    for (const doc of [
      [t("<b>bold</b>")],
      [t("&amp;")],
      [t("a <b"), BR, t("c>")],
      [t("x&sup2;")],
    ]) {
      const written = serialiseDescription(doc);
      expect(looksLikeHtml(written)).toBe(true);
      // Escaped words are references, so the pop-up draws them through the codec too (C19b).
      expect(parseDescription(written)).toEqual({ doc, supported: true, html: true, markup: true });
    }
    expect(serialiseDescription([t("<b>bold</b>")])).toBe("&lt;b&gt;bold&lt;/b&gt;");
    expect(serialiseDescription([t("a <b"), BR, t("c>")])).toBe("a &lt;b<br>c&gt;");
  });

  it("an_empty_description_is_written_as_nothing", () => {
    for (const doc of [
      [],
      [BR],
      parseDescription("<div><br></div>").doc,
      [ul(li())],
      // A space, then a no-break space (U+00A0).
      [t(" ")],
      [t(" ")],
      [p(BR)],
      [b(t(" \t"))],
    ]) {
      expect(isEmptyDoc(doc)).toBe(true);
      expect(serialiseDescription(doc)).toBe("");
    }
    expect(isEmptyDoc([p(), t(".")])).toBe(false);
  });

  it("writing_then_reading_gives_the_same_document", () => {
    let written = 0;
    DOCS.forEach((doc, k) => {
      const d = normalise(doc);
      if (isEmptyDoc(d)) return;
      written++;
      const back = parseDescription(serialiseDescription(d));
      expect(back.supported, `document ${k}`).toBe(true);
      expect(back.doc, `document ${k}`).toEqual(d);
    });
    // The generator makes mostly documents with something to show.
    expect(written).toBeGreaterThan(300);
  });
});

describe("comparing", () => {
  it.each<[string, string, boolean]>([
    ["<html-blob>a<br>b</html-blob>", "a\nb", true],
    ["<strong>x</strong>", "<b>x</b>", true],
    ["<em><b>x</b></em>", "<i><b>x</b></i>", true],
    ["a\r\nb", "a\nb", true],
    ['<a href="example.com">x</a>', '<a href="https://example.com" target="_blank">x</a>', true],
    ["<b>x</b>", "x", false],
    ["<b>x</b>", "<i>x</i>", false],
    ['<a href="https://a.example">x</a>', '<a href="https://b.example">x</a>', false],
    ["<table><tr><td>a</td></tr></table>", "a", false],
    ["<table><tr><td>a</td></tr></table>", "<table><tr><td>b</td></tr></table>", false],
    ["<table><tr><td>a</td></tr></table>", "<table><tr><td>a</td></tr></table>", true],
    // Both show nothing (C1-M2).
    ["<br>", "", true],
    ["<p><br></p>", "", true],
    ["<html-blob><br></html-blob>", "", true],
    ["\n", "", true],
    [" ", "", true],
    ["<b> </b>", "<div><br></div>", true],
    // One PM can't write back compares by text: clearing an image still clears it.
    ['<img src="https://example.com/x.png">', "", false],
  ])("the_same_description_written_two_ways_is_the_same: %j %j", (x, y, same) => {
    expect(sameDescription(x, y)).toBe(same);
    expect(sameDescription(y, x)).toBe(same);
  });
});

describe("drawing (C19b)", () => {
  // Whether the pop-up draws a Google description through the codec, or keeps Markdown (critique
  // C1-H1): any element but a name with a ":" (review 1: Markdown drops a block of HTML it doesn't
  // know, words and all), or a reference. An address in angle brackets is no element.
  it.each([
    ["<b>Agenda</b>", true],
    ["<span>x</span>", true],
    ["a<br>b", true],
    ["line</p>", true],
    ["<html-blob>x</html-blob>", true],
    ["<table><tr><td>a</td></tr></table>", true],
    ["<h2>Title</h2>", true],
    ["x<script>alert(1)</script>", true],
    ["Tom &amp; Jerry", true],
    ["it&#39;s", true],
    ["Click here to join<https://example.com/j/1>", false],
    ["Ann <ann@example.com>", false],
    ["<o:p>Room 4</o:p>", false],
    ['<font color="red">x</font>', true],
    ['<img src="https://example.com/x.png">', true],
    ["<center>Hello</center>", true],
    ["<html><body>Hello there</body></html>", true],
    ["<form>Sign up</form>", true],
    ["<details><summary>x</summary>y</details>", true],
    ['<font color="red">\nHello\n</font>\nafter', true],
    ["See <www.example.com> for details", false],
    ["Dial in: <Room 4>", false],
    ["Contact <first.last> please", false],
    ["<example.com>", false],
    ["<b>Dial in</b>: <Room 4>", true],
    ["Ask <José> about it", false],
    ["Meet at <Building A, Room B> today", false],
    ["<Ask Bob!> first", false],
    ['Meet <Room "A">', false],
    ["Contact <John Smith, PM> please", false],
    ["Ask <John O'Brien> today", false],
    ["Dial in: <Room A, B>", false],
    ["See <Room A/B>", false],
    ["see <your name here?>", false],
    ["Call <Ann Lee, Bob Ray> now", false],
    ["x </José> y", false],
    ["a < b and c > d", false],
    ["a<!-- note -->b", false],
    ["", false],
  ])("markup_is_formatting_google_writes: %j", (description, markup) => {
    expect(parseDescription(description).markup).toBe(markup);
  });

  const pieces = (text: string) => textLinks(text).map((p) => (p.href ? [p.text, p.href] : p.text));

  it("addresses_in_the_words_become_links_without_their_punctuation", () => {
    expect(pieces("See https://example.com/a. Then")).toEqual([
      "See ",
      ["https://example.com/a", "https://example.com/a"],
      ". Then",
    ]);
    expect(pieces("mail ann@example.com, or www.example.org!")).toEqual([
      "mail ",
      ["ann@example.com", "mailto:ann@example.com"],
      ", or ",
      ["www.example.org", "https://www.example.org"],
      "!",
    ]);
    expect(pieces("(https://example.com) [www.example.org]")).toEqual([
      "(",
      ["https://example.com", "https://example.com"],
      ") [",
      ["www.example.org", "https://www.example.org"],
      "]",
    ]);
    // A bracket the address opens is its own.
    expect(pieces("https://en.example.org/wiki/Foo_(bar)")).toEqual([
      ["https://en.example.org/wiki/Foo_(bar)", "https://en.example.org/wiki/Foo_(bar)"],
    ]);
    expect(pieces("https://example.com/a[1]")).toEqual([
      ["https://example.com/a[1]", "https://example.com/a[1]"],
    ]);
    expect(pieces("“https://example.com” and 'ann@example.com';")).toEqual([
      "“",
      ["https://example.com", "https://example.com"],
      "” and '",
      ["ann@example.com", "mailto:ann@example.com"],
      "';",
    ]);
    // A run ends at a space of any kind (a no-break space here), "<", ">" and '"'.
    const nbsp = String.fromCharCode(0xa0);
    expect(pieces(`<https://example.com/j/1>"www.example.org"${nbsp}https://example.net`)).toEqual([
      "<",
      ["https://example.com/j/1", "https://example.com/j/1"],
      '>"',
      ["www.example.org", "https://www.example.org"],
      `"${nbsp}`,
      ["https://example.net", "https://example.net"],
    ]);
    // Every closing mark goes, and every opening one before.
    for (const mark of [".", ",", ";", ":", "!", "?", "'", "’", "”", ")", "]"]) {
      expect(pieces(`http://example.com${mark}`), mark).toEqual([
        ["http://example.com", "http://example.com"],
        mark,
      ]);
    }
    for (const mark of ["(", "[", "'", "‘", "“"]) {
      expect(pieces(`${mark}www.example.org`), mark).toEqual([
        mark,
        ["www.example.org", "https://www.example.org"],
      ]);
    }
    expect(pieces("HTTPS://EXAMPLE.COM mailto:ann@example.com?subject=x")).toEqual([
      ["HTTPS://EXAMPLE.COM", "HTTPS://EXAMPLE.COM"],
      " ",
      ["mailto:ann@example.com?subject=x", "mailto:ann@example.com?subject=x"],
    ]);
  });

  it.each([
    "e.g. example.com and notes.txt",
    "@channel and a@b and @example.com",
    "https:example.com",
    "http:/example.com",
    "javascript:alert('@x.com')",
    "(javascript:alert(1)//www.example.org)",
    "wwwexample.org and www-example.org",
    "plain words only",
    "",
  ])("words_that_aren't_an_address_stay_words: %j", (text) => {
    expect(textLinks(text).every((p) => p.href === null)).toBe(true);
    expect(
      textLinks(text)
        .map((p) => p.text)
        .join(""),
    ).toBe(text);
  });
});

describe("replacing (C19d)", () => {
  /** "Replace with plain text…" on a description Google holds. */
  const replaced = (description: string) => replacementText(parseDescription(description).doc);

  // The editor's old read-only text (descriptionText.ts, gone in C19d) read these; its cases moved
  // here with what that reader left out: a list's items now start "• " or "1. ", and a link's
  // address follows its words (Q2).
  it.each([
    // What Google's own editor writes.
    ["Line one<br>Line two<br/>Line three<BR />", "Line one\nLine two\nLine three"],
    ["<b>Agenda</b><br><ul><li>One</li><li>Two</li></ul>", "Agenda\n• One\n• Two"],
    [
      'See <a href="https://example.com/a?b=1&amp;c=2">the doc</a>.',
      "See the doc (https://example.com/a?b=1&c=2).",
    ],
    ["<p>First</p><p>Second</p>", "First\nSecond"],
    ["<h2>Title</h2><div>Body</div >", "Title\nBody"],
    ["Line one<br><br>Line three", "Line one\n\nLine three"],
    ["  <p>padded</p>  ", "padded"],
    // What other editors write: a line opens a block, and edges that meet break once.
    ["Line one<div>Line two</div><div>Line three</div>", "Line one\nLine two\nLine three"],
    ["<div><p>a</p></div><p>b</p>", "a\nb"],
    ["<p>a</p><p></p><p></p><p>b</p>", "a\nb"],
    [
      "<table><tr><td>Dial-in</td><td>+1 555</td></tr><tr><td>PIN</td></tr></table>",
      "Dial-in +1 555\nPIN",
    ],
    ["<table>\n<tr><td>a</td>\n<td>b</td></tr></table>", "a\nb"],
    // The description's own line breaks and spacing stay; CR LF and a lone CR are line breaks, and
    // three or more break as two.
    ["one\ntwo  three", "one\ntwo  three"],
    ["<b>a</b>\r\n\r\n\r\n\r\nb", "a\n\nb"],
    ["<b>a</b>\rb", "a\nb"],
    // An address written in angle brackets is words, kept as written (critique C1-H1).
    [
      "<b>Join</b> <https://example.com/j/1> or mail <ann@example.com>",
      "Join <https://example.com/j/1> or mail <ann@example.com>",
    ],
  ])("replacement_text_keeps_words_lines_lists_and_addresses: %j", (description, text) => {
    expect(replaced(description)).toBe(text);
  });

  it.each([
    // Lists: numbered from 1 in each list, two spaces more per level, an item's later lines under
    // its words.
    ["<ol><li>x</li><li>y</li></ol><ol><li>z</li></ol>", "1. x\n2. y\n1. z"],
    [
      "<ul><li>a<ul><li>b</li><li>c<ol><li>d</li></ol></li></ul></li><li>e</li></ul>",
      "• a\n  • b\n  • c\n    1. d\n• e",
    ],
    ["<ul><li>a<br>b</li></ul>after", "• a\n  b\nafter"],
    ["<li>loose</li>", "• loose"],
    // Links: the address after the words, unless the words are the address, as written or typed.
    ['<a href="https://example.com/x">https://example.com/x</a>', "https://example.com/x"],
    ['<a href="example.com">example.com</a>', "example.com"],
    ['<a href="mailto:ann@example.com">Ann</a>', "Ann (ann@example.com)"],
    ['<a href="mailto:ann@example.com">ann@example.com</a>', "ann@example.com"],
    ['<a href="tel:+15550100">Call us</a>', "Call us (+15550100)"],
    [
      '<ul><li><a href="https://example.com"><b>site</b></a> notes</li></ul>',
      "• site (https://example.com) notes",
    ],
    // What PM can't write back keeps its words: a link it can't open is words alone.
    ['<a href="javascript:alert(1)">x</a> and <font color="red">red</font>', "x and red"],
    ["<b>a</b><script>alert(1)</script>", "a"],
    ["", ""],
  ])("replacing_keeps_lists_and_link_addresses_as_words: %j", (description, text) => {
    expect(replaced(description)).toBe(text);
  });

  // C19 review 1: what Replace saves reads back as the very words it showed, never as live
  // formatting; an address in angle brackets stays as written (critique C1-H1).
  it.each([
    ['<pre>&lt;div class="x"&gt;hello&lt;/div&gt;</pre>', '<div class="x">hello</div>'],
    ['<font color="red">a &lt;name&gt; b</font>', "a <name> b"],
    ["<font color=red>Tom &amp;amp; Jerry</font>", "Tom &amp; Jerry"],
    ["<font color=red>&lt;script&gt;alert(1)&lt;/script&gt;</font>", "<script>alert(1)</script>"],
    [
      "<h2>Notes</h2>&lt;img src=x onerror=alert(1)&gt; after",
      "Notes\n<img src=x onerror=alert(1)> after",
    ],
    ["<h2>Use &lt;b&gt;bold&lt;/b&gt; tags</h2>", "Use <b>bold</b> tags"],
    ["<table><tr><td>Ann &lt;ann@example.com&gt;</td></tr></table>", "Ann <ann@example.com>"],
    ["<h2>x &lt; y</h2><p>plain</p>", "x < y\nplain"],
  ])("what_replace_saves_reads_back_as_its_words: %j", (description, words) => {
    const saved = replacementDescription(description);
    const back = parseDescription(saved);
    expect(replacementText(back.doc)).toBe(words);
    expect(back.supported).toBe(true);
    // Never live markup: no tag but a break, as typed words that look like formatting are written.
    expect(saved.replace(/<br>/g, "")).not.toMatch(/<[a-z/!?]/i);
  });

  it("what_replace_saves_keeps_an_address_in_angle_brackets_as_written", () => {
    expect(
      replacementDescription("<table><tr><td>Join <https://example.com/j/1></td></tr></table>"),
    ).toBe("Join <https://example.com/j/1>");
    // Written escaped, it stays escaped.
    expect(replacementDescription("<h2>Join &lt;https://example.com/j/1&gt;</h2>")).toBe(
      "Join &lt;https://example.com/j/1&gt;",
    );
    // With other words that would read otherwise, all of it goes escaped: the words are kept.
    expect(replacementDescription("<h2>&lt;b&gt; <https://example.com></h2>")).toBe(
      "&lt;b&gt; &lt;https://example.com&gt;",
    );
    // Plain words stay plain lines.
    expect(replacementDescription("<h2>a</h2>b & c")).toBe("a\nb & c");
  });

  it("replacing_what_is_deeper_than_the_limit_keeps_its_words", () => {
    let doc: DescDoc = [t("deep")];
    for (let k = 0; k < MAX_DEPTH * 2; k++) doc = [b(...doc)];
    expect(replacementText([ul(li(t("a")), li(...doc))])).toBe("• a\n• deep");
  });
});

describe("costs", () => {
  it.each([
    ["one run of openers", "(".repeat(200_000) + "https://example.com"],
    ["one run of closers", "https://example.com" + ")".repeat(200_000)],
    ["many addresses", " https://example.com/a".repeat(20_000)],
    ["a long address", "https://example.com/" + "a".repeat(200_000)],
    ["many at signs", "a@".repeat(100_000)],
  ])("finding addresses in words is linear: %s", (_name, text) => {
    const started = performance.now();
    textLinks(text);
    expect(performance.now() - started).toBeLessThan(1_000);
  });

  it.each([
    ["nested bold", "<b>".repeat(100_000) + "x"],
    ["paragraphs", "<p>".repeat(100_000)],
    ["an open address", '<a href="' + "x".repeat(100_000)],
    ["references", "&amp;".repeat(50_000)],
    ["unmatched ends", "<x>".repeat(100_000) + "</y>".repeat(100_000)],
    ["a paragraph of spans and lines", "<p>" + "<span>".repeat(100_000) + "<div>".repeat(100_000)],
    ["items in formatting", "<b><li>x".repeat(50_000)],
    ["links in links", '<a href="https://example.com">x'.repeat(50_000)],
    ["plain off and on", '<b><span style="font-weight: normal">x</span>'.repeat(30_000)],
    ["lines", "<div>a</div>".repeat(50_000)],
    ["breaks", "a" + "<br>".repeat(100_000)],
    ["nested lists", "<ul><li>a".repeat(50_000)],
    ["links in items", '<ol><li><a href="https://example.com">x</a></li></ol>'.repeat(20_000)],
    // C19 review 1: a table's every cell asked how the words so far end.
    ["cells", "<table><tr>" + "<td>xxxxxxxxxxxxxxxxxxxxxxxx</td>".repeat(25_000)],
    ["cell ends", "<table><tr><td>" + "x</td>".repeat(100_000)],
    // C19 review 1: one list of children past the engine's limit for arguments.
    ["breaks in bold", "<b><b>" + "<br>".repeat(140_000)],
    ["breaks in a paragraph", "<p>" + "<br>".repeat(140_000)],
    ["items", "<ul>" + "<li>a</li>".repeat(140_000)],
    // A line's edge after many empty marks.
    ["empty marks, then lines", "a" + "<b></b>".repeat(50_000) + "<div>x</div>".repeat(50_000)],
  ])("reading_is_linear: %s", (_name, description) => {
    // Each pass timed on its own, so a busy machine running every suite at once keeps the margin a
    // quadratic reader would blow through many times over.
    const took = (work: () => unknown) => {
      const started = performance.now();
      work();
      return performance.now() - started;
    };
    let doc: DescDoc = [];
    expect(took(() => (doc = parseDescription(description).doc))).toBeLessThan(1_000);
    expect(took(() => serialiseDescription(doc))).toBeLessThan(1_000);
    expect(took(() => replacementText(doc))).toBeLessThan(1_000);
    // Two reads: the texts differ, so both are parsed.
    expect(took(() => sameDescription(description, description + " "))).toBeLessThan(2_000);
  });

  it.each(["<a".repeat(250_000), "&#1".repeat(160_000), "&a".repeat(250_000), "<".repeat(500_000)])(
    "telling formatting from plain text is linear (%#)",
    (description) => {
      const started = performance.now();
      looksLikeHtml(description);
      expect(performance.now() - started).toBeLessThan(1_000);
    },
  );
});

describe("telling formatting from plain text", () => {
  it.each([
    "<b>Agenda</b>",
    "line one<br>line two",
    'see <a href="https://x">this</a>',
    "</p>",
    "fish &amp; chips",
    "it&#39;s",
    "it&#x27;s",
    "it&#X27;s",
    "x&sup2;",
    "a&nbsp;b",
    "&thetasym;",
  ])("formatted: %j", (description) => {
    expect(looksLikeHtml(description)).toBe(true);
  });

  it.each([
    "a < b and c > d",
    "x > y and a <b",
    "Q&A at 3",
    "5<6",
    "just notes",
    "",
    "café ☕ <",
    "&#;",
    "&#x;",
    "&abcdefghi;",
    "& amp;",
    "<>",
    "< b>",
  ])("plain: %j", (description) => {
    expect(looksLikeHtml(description)).toBe(false);
  });
});
