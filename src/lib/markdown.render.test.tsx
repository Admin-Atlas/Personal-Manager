// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// H6. `markdown.test.ts` pins the pure pieces and the pipeline's shape; this drives the REAL
// component and asserts on the DOM it produces, because that is the only thing an attacker cares
// about. Every input here is what an ingested document body can legitimately contain — a captured
// web article, an email, a model-authored answer — and every assertion is "this did not survive".
//
// Kept apart from `markdown.test.ts` so the pure suite keeps its node environment (jsdom is opted
// into per file, see vitest.config.ts).

import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { parseDescription } from "./calendarEdit/descriptionHtml";
import { tokens } from "./calendarEdit/descriptionTokens";
import { Markdown } from "./markdown";

afterEach(cleanup);

function html(source: string): string {
  return render(<Markdown>{source}</Markdown>).container.innerHTML;
}

describe("the sanitizing boundary, end to end", () => {
  // The live canary for defence layer 1 (no raw-HTML parsing), and the only assertion here that a
  // single realistic regression can break.
  //
  // The hostile cases below are held up by several layers at once — the sanitizer strips a
  // <script> whether or not it was ever parsed — so none of them can tell you whether raw-HTML
  // parsing is still off. A BENIGN tag can: `<b>` is on the sanitizer's allow-list, so the only
  // thing standing between it and a real element is the absence of `rehype-raw`. Add that plugin
  // — the plausible one-line change to "make HTML in a captured web article render" — and this
  // fails while every hostile assertion below still passes.
  it("does not parse raw HTML at all, benign or otherwise", () => {
    const { container } = render(<Markdown>{"a <b>bold</b> word"}</Markdown>);
    expect(container.querySelector("b")).toBeNull();
    // Note what actually happens to it: the tags are DROPPED and their text kept, not escaped into
    // visible `<b>` characters. Worth pinning because it is the observable difference — a captured
    // web article loses its markup silently rather than showing it — and because it is what makes
    // the `querySelector` above a clean signal either way.
    expect(container.textContent?.trim()).toBe("a bold word");
  });

  it("drops a literal <script> in the source", () => {
    const out = html("Before\n\n<script>alert(1)</script>\n\nAfter");
    expect(out).not.toContain("<script");
    expect(out).not.toContain("alert(1)");
    // The surrounding prose still renders — this is a sanitizer, not a refusal.
    expect(out).toContain("Before");
    expect(out).toContain("After");
  });

  it("drops an inline event handler", () => {
    const out = html('<img src="x" onerror="alert(1)">');
    expect(out).not.toContain("onerror");
    expect(out).not.toContain("alert(1)");
  });

  it("drops an iframe", () => {
    const out = html('<iframe src="https://evil.example"></iframe>');
    expect(out).not.toContain("<iframe");
    expect(out).not.toContain("evil.example");
  });

  it("neutralises a javascript: link written as ordinary Markdown", () => {
    const { container } = render(<Markdown>{"[click me](javascript:alert(1))"}</Markdown>);
    expect(container.innerHTML).not.toContain("javascript:");
    // The text survives; only the destination is taken away, so the reader sees the words that
    // were written rather than a silently missing line.
    expect(container.textContent).toContain("click me");
    const href = container.querySelector("a")?.getAttribute("href");
    expect(href ?? "").toBe("");
  });

  it("neutralises a data: URL link", () => {
    const out = html("[x](data:text/html,<script>alert(1)</script>)");
    expect(out).not.toContain("data:text/html");
    expect(out).not.toContain("<script");
  });

  it("neutralises a protocol-relative link", () => {
    const { container } = render(<Markdown>{"[click](//evil.example/x)"}</Markdown>);
    const a = container.querySelector("a");
    expect(a).not.toBeNull();
    // The words survive; only the destination is taken away.
    expect(container.textContent).toContain("click");
    expect(a?.getAttribute("href")).toBe("");
    expect(container.innerHTML).not.toContain("evil.example");
    // Pin the href ONLY, not the attribute set: `rehype-external-links` special-cases a `//` prefix
    // as external and stamps target/rel BEFORE `urlTransform` empties the href, so this anchor still
    // carries `target="_blank"`. An empty href is a request for the current page, which the webview
    // swallows — it is not a re-opened hole, and it must not be "fixed" by asserting on the tag.
  });

  it("neutralises a protocol-relative image", () => {
    const { container } = render(<Markdown>{"![x](//evil.example/pixel.png)"}</Markdown>);
    const img = container.querySelector("img");
    expect(img).not.toBeNull();
    // React refuses to emit an empty `src` (it warns and omits the attribute), so the emptied URL
    // shows up as ABSENT rather than as `src=""`. Either way no request is made — assert the
    // effective value so this does not become a test of React's rendering choice.
    expect(img?.getAttribute("src") ?? "").toBe("");
    expect(container.innerHTML).not.toContain("evil.example");
    // The tripwire for the single most likely way this becomes live: today `img-src 'self' asset:
    // data: blob:` blocks the request anyway, so widening the CSP to render remote images would turn
    // an `![](//attacker/px.png)` in any ingested document into a per-open read receipt and IP
    // beacon. This assertion fails first if the URL guard is loosened alongside the CSP.
  });

  it("drops a style attribute and a <style> block", () => {
    const out = html('<div style="position:fixed;inset:0">covering</div>\n\n<style>*{}</style>');
    expect(out).not.toContain("<style");
    expect(out).not.toContain("position:fixed");
  });

  // The other direction: the boundary has to keep working, or someone will loosen it.
  it("still renders an ordinary link, and marks it for the external-link interceptor", () => {
    const { container } = render(<Markdown>{"[docs](https://example.com/x)"}</Markdown>);
    const a = container.querySelector("a");
    expect(a?.getAttribute("href")).toBe("https://example.com/x");
    expect(a?.getAttribute("target")).toBe("_blank");
    expect(a?.getAttribute("rel")).toContain("noreferrer");
  });

  it("still renders ordinary Markdown structure", () => {
    const out = html("# Title\n\n- one\n- two\n\n**bold**");
    expect(out).toContain("<h1");
    expect(out).toContain("<li");
    expect(out).toContain("<strong");
  });
});

// An event's description is plain text typed line by line (`plainLines`): a dashed divider under the
// dial-in lines of a pasted invite must not turn them into a heading, nor an indented line into code.
// The default surface keeps reading them as Markdown does.
describe("plain typed lines (an event's description)", () => {
  it("reads a divider under text as a divider, not a heading underline", () => {
    const { container } = render(
      <Markdown plainLines>{"Meeting ID: 1\nPasscode: 2\n---\nOne tap"}</Markdown>,
    );
    expect(container.querySelector("h1, h2")).toBeNull();
    expect(container.querySelector("hr")).not.toBeNull();
    expect(container.querySelector("p")?.textContent).toBe("Meeting ID: 1\nPasscode: 2");
    const equals = render(<Markdown plainLines>{"Notes\n===\nbody"}</Markdown>).container;
    expect(equals.querySelector("h1, h2")).toBeNull();
  });

  // A plain-text Google event takes this path (parseDescription's `markup`, C19 review 2).
  it("keeps words in angle brackets that are no tag", () => {
    for (const [line, words] of [
      ["See <www.example.com> for details", "www.example.com"],
      ["Dial in: <Room 4>", "<Room 4>"],
      ["Contact <first.last> please", "<first.last>"],
    ]) {
      const { container } = render(<Markdown plainLines>{line}</Markdown>);
      expect(container.textContent).toContain(words);
    }
  });

  // Review round 3 of PR 1: the description's reader calls a name it doesn't know an element only
  // where this drops the tag too, so the flip to the formatted view loses no words. The two readers
  // are held to each other, for what each is.
  const asWords = (line: string) => tokens(line).some((token) => token.kind === "literal");
  it.each([
    ["Ask <José> about it", "José"],
    ["Meet at <Building A, Room B> today", "Building A, Room B"],
    ["<Ask Bob!> first", "Ask Bob!"],
    ['Meet <Room "A">', 'Room "A"'],
    ["Contact <John Smith, PM> please", "John Smith, PM"],
    ["Ask <John O'Brien> today", "John O'Brien"],
    ["Dial in: <Room A, B>", "Room A, B"],
    ["See <Room A/B>", "Room A/B"],
    ["see <your name here?>", "your name here?"],
    ["Call <Ann Lee, Bob Ray> now", "Ann Lee, Bob Ray"],
    ["x <Room / > y", "Room / "],
    ['x <Room a="b"c> y', 'Room a="b"c'],
    ["x <Room a=b`c> y", "Room a=b"],
    ["x <Room a=> y", "Room a="],
    ["x <Room =x> y", "Room =x"],
    ["x </José> y", "José"],
    ["x </Room 4> y", "Room 4"],
    ["x </first.last> y", "first.last"],
    ["x <first.last> y", "first.last"],
    ["x </Room A> y", "Room A"],
    // Words after a name that opens a block of HTML, anywhere but a line's start.
    ["x <Option 2> y", "Option 2"],
    ["<Option/B> Thursday", "Option/B"],
    ["<Option, B> Thursday", "Option, B"],
  ])("shows what the description's reader calls words: %j", (line, words) => {
    const { container } = render(<Markdown plainLines>{line}</Markdown>);
    expect(container.textContent).toContain(words);
    expect(asWords(line)).toBe(true);
    expect(parseDescription(line).markup).toBe(false);
  });

  // Review round 5 of PR 1: a comment or declaration alone on its lines is hidden by both readers, so
  // nothing is at risk and the rest keeps Markdown's reading.
  it.each([
    ["Team lunch\n\n<!-- booking 42 -->", "Team lunch", "booking"],
    ["Agenda\n<!-- internal note -->\nNext steps", "Next steps", "internal"],
    ["<!-- a\nnote -->\nHello", "Hello", "note"],
    ["   <!-- note -->\nHello", "Hello", "note"],
    ["    <!-- note -->\nHello", "Hello", "note"],
    ["- <!-- note -->\nHello", "Hello", "note"],
    ["> <!-- note -->\nHello", "Hello", "note"],
    ['<?xml version="1.0"?>\nHello', "Hello", "xml"],
    ["<!DOCTYPE note>\nHello", "Hello", "note"],
    ["<![CDATA[ note ]]>\nHello", "Hello", "note"],
  ])("keeps Markdown for what both readers hide: %j", (line, words, hidden) => {
    const { container } = render(<Markdown plainLines>{line}</Markdown>);
    expect(container.textContent).toContain(words);
    expect(container.textContent).not.toContain(hidden);
    expect(parseDescription(line).markup).toBe(false);
  });

  it("a comment alone on a line leaves the rest to Markdown's reading", () => {
    const line = "Agenda:\n- one\n- **two**\n<!-- c -->";
    const { container } = render(<Markdown plainLines>{line}</Markdown>);
    expect(container.querySelector("li strong")?.textContent).toBe("two");
    expect(parseDescription(line).markup).toBe(false);
  });

  // Review round 4 of PR 1: a line Markdown reads as a block of HTML goes whole, so the formatted view
  // draws the description, whose words these hold.
  it.each([
    // Where a browser ends it and Markdown reads on, or words follow on its last line (round 5).
    ["<!-- a --!> Thursday -->", "Thursday"],
    ["<!-- a --!>\nThursday\n-->", "Thursday"],
    ["<? a > Thursday ?>", "Thursday"],
    ["<![CDATA[ a > Thursday ]]>", "Thursday"],
    ["<? a >\nThursday ?>", "Thursday"],
    ["<![CDATA[ a >\nThursday ]]>", "Thursday"],
    ["<!-- a\nb --> Thursday", "Thursday"],
    // A list ends the comment for Markdown, and the block that opens then takes the next line.
    ["- <!-- a\n<Option 2> -->\nThursday", "Thursday"],
    ["<Option 2> Thursday 3pm\nOption 3 Friday", "Thursday 3pm"],
    ["<Form A, B> please sign\nby Friday", "please sign"],
    ["<Details A, B> Thursday", "Thursday"],
    ["<Center 4> Thursday", "Thursday"],
    ["<Menu 4> Thursday", "Thursday"],
    ["<Summary 1> Thursday", "Thursday"],
    ["<textarea 1> Thursday", "Thursday"],
    ["</Option 2> Thursday", "Thursday"],
    ["Options:\n<Option 2> Thursday", "Thursday"],
    ["   <Option 2> Thursday", "Thursday"],
    ["    <Option 2> Thursday", "Thursday"],
    ["\t<Option 2> Thursday", "Thursday"],
    ["- <Option 2> Thursday", "Thursday"],
    ["1. <Option 2> Thursday", "Thursday"],
    ["> <Option 2> Thursday", "Thursday"],
    // With no ">" it's no tag to the reader either: plain lines, that Markdown still drops.
    ["<Option 2 Thursday", "Thursday"],
    ["<p is a letter", "is a letter"],
    ["<!-- note --> Thursday", "Thursday"],
    ["<? note ?> Thursday", "Thursday"],
    ["<!DOCTYPE note> Thursday", "Thursday"],
    ["<![CDATA[ note ]]> Thursday", "Thursday"],
  ])("draws through the formatted view what Markdown drops: %j", (line, words) => {
    const { container } = render(<Markdown plainLines>{line}</Markdown>);
    expect(container.textContent).not.toContain(words);
    const parsed = parseDescription(line);
    expect(parsed.markup).toBe(true);
    expect(JSON.stringify(parsed.doc)).toContain(words);
  });

  // Every element's name, whichever list it's on: whatever Markdown drops with its line, whatever the
  // renderer's next version drops, the reader flips for.
  it.each(
    (
      "a abbr acronym address applet area article aside audio b base basefont bdi bdo bgsound big " +
      "blink blockquote body br button canvas caption center cite code col colgroup command data " +
      "datalist dd del details dfn dialog dir div dl dt em embed fieldset figcaption figure font " +
      "footer form frame frameset h1 h2 h3 h4 h5 h6 head header hgroup hr html i iframe image img " +
      "input ins isindex kbd keygen label legend li link listing main map mark marquee math menu " +
      "menuitem meta meter multicol nav nextid nobr noembed noframes noscript object ol optgroup " +
      "option output p param picture plaintext pre progress q rb rp rt rtc ruby s samp script " +
      "search section select slot small source spacer span strike strong style sub summary sup svg " +
      "table tbody td template textarea tfoot th thead time title tr track tt u ul var video wbr " +
      "xmp"
    ).split(" "),
  )("the description's reader flips for a line Markdown drops: <%s 4>", (name) => {
    const line = `<${name} 4> Thursday`;
    const { container } = render(<Markdown plainLines>{line}</Markdown>);
    if (!container.textContent?.includes("Thursday")) {
      expect(parseDescription(line).markup).toBe(true);
    }
  });

  it.each([
    "x <Room> y",
    "x <Room A> y",
    "x <Room/> y",
    "x <Room A /> y",
    "x <Room _a> y",
    "x <Room :a.b-1> y",
    'x <Room a="b>c"> y',
    "x <Room a='b' c = d e> y",
    "x </Room> y",
    "x </Room > y",
  ])("drops what the description's reader calls an element: %j", (line) => {
    const { container } = render(<Markdown plainLines>{line}</Markdown>);
    expect(container.textContent?.trim()).toBe("x  y");
    expect(asWords(line)).toBe(false);
  });

  it("keeps an indented line as text", () => {
    const { container } = render(<Markdown plainLines>{"Agenda\n\n    item one"}</Markdown>);
    expect(container.querySelector("pre, code")).toBeNull();
    expect(container.textContent).toContain("item one");
  });

  it("leaves the default surface as Markdown reads it", () => {
    expect(html("Title\n---")).toContain("<h2");
  });
});
