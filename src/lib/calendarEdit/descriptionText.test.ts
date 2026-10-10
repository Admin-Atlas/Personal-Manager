// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { descriptionText } from "./descriptionText";

// Invisible or stand-in characters, spelled as code points so the test reads plainly.
const NBSP = String.fromCodePoint(0xa0);
const REPLACEMENT = String.fromCodePoint(0xfffd);

describe("descriptionText", () => {
  it.each([
    // What Google's own editor writes.
    ["Line one<br>Line two<br/>Line three<BR />", "Line one\nLine two\nLine three"],
    ["<b>Agenda</b><br><ul><li>One</li><li>Two</li></ul>", "Agenda\nOne\nTwo"],
    ['See <a href="https://example.com/a?b=1&amp;c=2">the doc</a>.', "See the doc."],
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
    // The description's own line breaks and spacing stay, as Google's view keeps them; CR LF and a
    // lone CR are line breaks, as the tokenizer reads them.
    ["one\ntwo  three", "one\ntwo  three"],
    ["<b>a</b>\r\n\r\n\r\n\r\nb", "a\n\nb"],
    ["<b>a</b>\rb", "a\nb"],
  ])("reads %j", (html, text) => {
    expect(descriptionText(html)).toBe(text);
  });

  it.each([
    ["a &amp; b &lt;c&gt; &quot;d&quot; &#39;e&#39; &apos;f&apos;", `a & b <c> "d" 'e' 'f'`],
    ["caf&eacute; M&uuml;nchen &szlig; &Aring; &yuml; &times; &divide;", "café München ß Å ÿ × ÷"],
    ["a&nbsp;b", `a${NBSP}b`],
    ["&ndash; &mdash; &hellip; &rsquo;s &euro;5 &trade;", "– — … ’s €5 ™"],
    // The rest of HTML 4, as Java's escapeHtml4 or PHP's htmlentities write it.
    ["x &le; 5 &rArr; pass &infin; &hearts;", "x ≤ 5 ⇒ pass ∞ ♥"],
    ["&Kappa;&alpha;&lambda;&eta; &thetasym; &lang;a&rang;", "Καλη ϑ ⟨a⟩"],
    ["&#233;&#xE9;&#XE9;&#x1F600;", "ééé😀"],
    ["&#65 &#x42", "A B"],
    ["&#0000000233;&#x00000000E9;&#000000065;", "ééA"],
    // Windows-1252's punctuation, as a browser reads the C1 codes.
    ["Don&#146;t &#147;go&#148; &#150; &#128;5 &#x99;", "Don’t “go” – €5 ™"],
    ["&#0;&#xD800;&#x110000;&#99999999999;", REPLACEMENT.repeat(4)],
    // Not references: left as written.
    [
      "a & b, &#; &#x; &foo; &constructor; &toString; &valueOf;",
      "a & b, &#; &#x; &foo; &constructor; &toString; &valueOf;",
    ],
    // Deliberately unlike a browser: HTML 5's own names, and the old forms without ";", stay as
    // written. Encoders write HTML 4's names with the ";", Google included.
    ["&colon; &check; &amp &copy 2026", "&colon; &check; &amp &copy 2026"],
  ])("decodes %j", (html, text) => {
    expect(descriptionText(html)).toBe(text);
  });

  // Decoded once, onto the output: escaped markup shows as text and is never read as a tag.
  it("never reads decoded text as markup", () => {
    expect(descriptionText("&lt;b&gt;not bold&lt;/b&gt; &lt;script&gt;x&lt;/script&gt;")).toBe(
      "<b>not bold</b> <script>x</script>",
    );
  });

  it.each([
    ["Hi<script>alert(1)</script> there<style>p { color: red }</style>!", "Hi there!"],
    ["a<SCRIPT type=x>b</script >c", "ac"],
    ["a<script>b</scriptx>c</script>d", "ad"],
    ["a<title>t</title>b<template><p>x</p></template>c", "abc"],
    // Nothing ends it: the rest stays hidden.
    ["a<style>b", "a"],
  ])("hides what never shows: %j", (html, text) => {
    expect(descriptionText(html)).toBe(text);
  });

  it.each([
    ["a<!-- <b>hidden</b> -->b", "ab"],
    ["a<!-->b", "ab"],
    ["a<!--->b", "ab"],
    ["a<!-- x --->b", "ab"],
    ["a<!-- x --!>b", "ab"],
    ["a<!-- b", "a"],
    ["<!DOCTYPE html><html><body>x</body></html>", "x"],
    ["<?xml version='1.0'?>x", "x"],
    ["</>x", "x"],
    ["</ 3>x", "x"],
  ])("drops comments and declarations: %j", (html, text) => {
    expect(descriptionText(html)).toBe(text);
  });

  it.each([
    // A ">" inside a quoted value doesn't end the tag; one outside quotes does.
    ["<a href=\"x>y\" title='p>q'>link</a>", "link"],
    ["<a href=x>y>link</a>", "y>link"],
    // A "<" that can't open a tag is text.
    ["1 < 2 and 3 <4 and <>", "1 < 2 and 3 <4 and <>"],
    // The text ends inside a tag (or a quoted value): it and the rest go.
    ["a <b", "a"],
    ['a <a href="x>text', "a"],
  ])("reads tags as the tokenizer does: %j", (html, text) => {
    expect(descriptionText(html)).toBe(text);
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
    '<a b="'.repeat(80_000),
  ])("stays linear on hostile input", (html) => {
    const started = performance.now();
    descriptionText(html);
    expect(performance.now() - started).toBeLessThan(1_000);
  });
});
