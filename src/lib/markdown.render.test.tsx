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
