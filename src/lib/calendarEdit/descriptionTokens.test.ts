// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The description tokenizer (C19 §1): what a Google description's characters are, read as the HTML
// tokenizer reads them wherever that changes what shows. descriptionText.test.ts, left as it was
// before the tokenizer existed, proves the readers built on it read as the old scanner did.

import { describe, expect, it } from "vitest";
import { descriptionText } from "./descriptionText";
import { tokens, type Token } from "./descriptionTokens";

const start = (name: string, attrs: [string, string][] = [], selfClosing = false): Token => ({
  kind: "start",
  name,
  attrs: attrs.map(([n, value]) => ({ name: n, value })),
  selfClosing,
});
const end = (name: string): Token => ({ kind: "end", name });
const text = (t: string): Token => ({ kind: "text", text: t });
const literal = (t: string): Token => ({ kind: "literal", text: t });
const COMMENT: Token = { kind: "comment" };
const CUT: Token = { kind: "cut" };
/** U+212A, spelled as a code point so the test reads plainly (it looks like "K"). */
const KELVIN = String.fromCodePoint(0x212a);

describe("tokens", () => {
  it("reads_tags_with_their_attributes", () => {
    expect(tokens('<A HREF="https://x/?a=1&amp;b=2" id=ow12 __is_owner="true">x</A>')).toEqual([
      start("a", [
        ["href", "https://x/?a=1&b=2"],
        ["id", "ow12"],
        ["__is_owner", "true"],
      ]),
      text("x"),
      end("a"),
    ]);
    // Single quotes, spaces around "=", a name with no value, and an end tag's attributes dropped.
    expect(tokens("<p  data-x = 'one two' hidden></p class=x>")).toEqual([
      start("p", [
        ["data-x", "one two"],
        ["hidden", ""],
      ]),
      end("p"),
    ]);
  });

  it("attributes_need_no_space_between_them", () => {
    // Google's own anchors are written so (V3); browsers read two attributes.
    expect(tokens('<a id="ow1"__is_owner="true">')).toEqual([
      start("a", [
        ["id", "ow1"],
        ["__is_owner", "true"],
      ]),
    ]);
  });

  it("a_quote_opens_a_value_only_after_equals", () => {
    expect(tokens('<a "x>y">')).toEqual([start("a", [['"x', ""]]), text('y">')]);
    expect(tokens('<a b=c"d>e">')).toEqual([start("a", [["b", 'c"d']]), text('e">')]);
    // A name that starts with "=" keeps it, so its quote opens no value either (HTML).
    expect(tokens('<a ="x>y">')).toEqual([start("a", [['="x', ""]]), text('y">')]);
  });

  it("a_gt_in_a_quoted_value_stays_in_its_tag", () => {
    expect(tokens("<b title=\"x>y\" data-q='p>q'>a</b>")).toEqual([
      start("b", [
        ["title", "x>y"],
        ["data-q", "p>q"],
      ]),
      text("a"),
      end("b"),
    ]);
  });

  it("a_repeated_attribute_keeps_the_first", () => {
    expect(tokens('<a href="one" HREF="two">')).toEqual([start("a", [["href", "one"]])]);
  });

  it("a_slash_closes_an_element_only_directly_before_its_end", () => {
    expect(tokens("<br/><br /><br/ ><b/>")).toEqual([
      start("br", [], true),
      start("br", [], true),
      start("br", [], false),
      start("b", [], true),
    ]);
    // In an unquoted value the slash is the value's.
    expect(tokens("<a href=x/>")).toEqual([start("a", [["href", "x/"]])]);
  });

  it("never_shown_elements_skip_their_content", () => {
    expect(tokens("<script><b>x</b></script>y")).toEqual([
      start("script"),
      end("script"),
      text("y"),
    ]);
    expect(tokens("a<STYLE type=x>p { color: red }</style >b<title>t")).toEqual([
      text("a"),
      start("style", [["type", "x"]]),
      end("style"),
      text("b"),
      start("title"),
      end("title"),
    ]);
  });

  it("comments_and_declarations_are_one_token_each", () => {
    expect(
      tokens("a<!-- x -->b<!-->c<!--->d<!-- y --!>e<!DOCTYPE html>f<?xml?>g</ 3>h</>i<!-- open"),
    ).toEqual([
      text("a"),
      COMMENT,
      text("b"),
      COMMENT,
      text("c"),
      COMMENT,
      text("d"),
      COMMENT,
      text("e"),
      COMMENT,
      text("f"),
      COMMENT,
      text("g"),
      COMMENT,
      text("h"),
      COMMENT,
      text("i"),
      COMMENT,
    ]);
  });

  it("a_lt_that_opens_nothing_is_text", () => {
    expect(tokens("1 < 2 and 3 <4 and <> <")).toEqual([text("1 < 2 and 3 <4 and <> <")]);
  });

  it("text_ending_inside_a_tag_is_cut", () => {
    expect(tokens("a <b")).toEqual([text("a "), CUT]);
    expect(tokens('a <a href="x>text')).toEqual([text("a "), CUT]);
    expect(tokens("<b>x</b")).toEqual([start("b"), text("x"), CUT]);
    expect(tokens("<a href=")).toEqual([CUT]);
  });

  it("references_decode_once", () => {
    expect(tokens("&amp;lt;b&amp;gt; &lt;i&gt;")).toEqual([text("&lt;b&gt; <i>")]);
    expect(tokens('<a title="&amp;quot;">')).toEqual([start("a", [["title", "&quot;"]])]);
  });

  it("line_endings_are_line_feeds", () => {
    expect(tokens("a\r\nb\rc<b title='x\r\ny'>")).toEqual([
      text("a\nb\nc"),
      start("b", [["title", "x\ny"]]),
    ]);
  });

  it("addresses_in_angle_brackets_are_words", () => {
    expect(tokens("See <https://example.com/j/1>, mail <ann@example.com>")).toEqual([
      text("See "),
      literal("<https://example.com/j/1>"),
      text(", mail "),
      literal("<ann@example.com>"),
    ]);
    expect(tokens("<MAILTO:a@b.example><http://x?a=1&amp;b=2><a@b.com>")).toEqual([
      literal("<MAILTO:a@b.example>"),
      literal("<http://x?a=1&b=2>"),
      literal("<a@b.com>"),
    ]);
    // Whitespace before the ">" makes it a tag again, and a tag holding "@" in a value is a tag.
    expect(tokens("<https://x y>")).toEqual([
      start("https:", [
        ["x", ""],
        ["y", ""],
      ]),
    ]);
    expect(tokens('<a href="mailto:x@y">m</a>')).toEqual([
      start("a", [["href", "mailto:x@y"]]),
      text("m"),
      end("a"),
    ]);
    // Outlook's "<o:p>" has a colon but no address: an element.
    expect(tokens("<o:p></o:p>")).toEqual([start("o:p"), end("o:p")]);
    // An end tag never is one: browsers draw nothing for it.
    expect(tokens("</https://x></ann@example.com>")).toEqual([
      end("https:"),
      end("ann@example.com"),
    ]);
  });

  it("the_editors_text_keeps_addresses_in_angle_brackets", () => {
    // descriptionText.ts, rebuilt on these tokens, shows them as words rather than dropping them as
    // an element named "https:" (its own tests, left as they were, have no such case).
    expect(descriptionText("<b>Join</b> <https://example.com/j/1> or mail <ann@example.com>")).toBe(
      "Join <https://example.com/j/1> or mail <ann@example.com>",
    );
  });

  it("names_lower_case_only_ascii_letters", () => {
    // The Kelvin sign lower-cases to "k" in JavaScript but not in HTML's tokenizer.
    expect(tokens(`<BLOC${KELVIN}QUOTE>`)).toEqual([start(`bloc${KELVIN}quote`)]);
  });

  // One pass that never steps back: inputs built to make a backtracking reader quadratic finish
  // quickly. (500 KB is far past Google's description limit.)
  it.each([
    "<".repeat(500_000),
    "<a".repeat(250_000),
    "<!--".repeat(125_000),
    "<!--" + "-".repeat(500_000),
    "<!-- -->".repeat(60_000),
    "<div>".repeat(100_000),
    "&a".repeat(250_000),
    "&#1".repeat(160_000),
    "<script>" + "</scrip".repeat(70_000),
    '<a b="'.repeat(100_000),
    "&#".repeat(100_000),
    "<x@".repeat(100_000),
    "<a ".repeat(100_000) + ">",
    "<a@b".repeat(100_000) + ">",
    "<https:".repeat(100_000),
  ])("reading_is_linear (%#)", (html) => {
    const started = performance.now();
    tokens(html);
    expect(performance.now() - started).toBeLessThan(1_000);
  });
});
