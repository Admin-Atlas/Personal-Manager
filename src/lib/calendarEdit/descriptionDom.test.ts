// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The editor's box and the document model, both ways (C19 §6). Fixtures are built element by element
// with `h`, never by parsing, as the bridge itself is.

import { afterEach, describe, expect, it, vi } from "vitest";
import fixture from "./descriptionHtml.fixture.json";
import { docToNodes, nodesToDoc } from "./descriptionDom";
import {
  MAX_DEPTH,
  docsEqual,
  isEmptyDoc,
  normalise,
  parseDescription,
  serialiseDescription,
  type Block,
  type DescDoc,
  type DescNode,
  type Mark,
} from "./descriptionHtml";
import { spyOnHtmlSinks } from "./htmlSinkSpy";

afterEach(() => {
  vi.restoreAllMocks();
  document.body.replaceChildren();
});

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

/** An element, its attributes and children, made one by one. */
function h(tag: string, attrs: Record<string, string> = {}, ...children: (Node | string)[]) {
  const made = document.createElement(tag);
  for (const [name, value] of Object.entries(attrs)) made.setAttribute(name, value);
  made.append(...children);
  return made;
}

/** The editor's box holding `children`. */
const box = (...children: (Node | string)[]) => h("div", {}, ...children);

/** The box seeded from `doc`, as the editor seeds it. */
function seeded(doc: DescDoc): HTMLElement {
  const host = box();
  host.append(...docToNodes(doc, document));
  return host;
}

/** A seeded generator (mulberry32), so a failure names a case that can be made again. */
function random(seed: number): () => number {
  let s = seed;
  return () => {
    s = (s + 0x6d2b79f5) | 0;
    let x = Math.imul(s ^ (s >>> 15), 1 | s);
    x = (x + Math.imul(x ^ (x >>> 7), 61 | x)) ^ x;
    return ((x ^ (x >>> 14)) >>> 0) / 4294967296;
  };
}

describe("seeding the box", () => {
  it("seeds_each_kind_as_its_own_element", () => {
    const doc: DescDoc = [
      b(t("Agenda")),
      BR,
      i(t("one")),
      t(" "),
      u(t("two")),
      ol(li(t("first")), li(t("second"))),
      ul(li(t("apple"))),
      p(t("Notes")),
      a("https://example.com/x", t("site")),
    ];
    const nodes = docToNodes(doc, document);
    // Text goes in as a string, which `append` makes a text node (critique C1-L11).
    expect(nodes[3]).toBe(" ");
    const host = seeded(doc);
    const names = [...host.children].map((child) => child.localName);
    expect(names).toEqual(["b", "br", "i", "u", "ol", "ul", "p", "a"]);
    expect(host.querySelectorAll("ol > li")).toHaveLength(2);
    expect(host.querySelector("ul > li")?.firstChild).toBeInstanceOf(Text);
    const link = host.querySelector("a");
    expect(link?.getAttribute("href")).toBe("https://example.com/x");
    // No target: useExternalLinks opens only target="_blank" links, never one being edited.
    expect(link?.hasAttribute("target")).toBe(false);
    expect(link?.attributes).toHaveLength(1);
    for (const child of host.querySelectorAll("*")) {
      if (child.localName !== "a") expect(child.attributes).toHaveLength(0);
    }
  });

  it("writes_an_address_only_through_setAttribute_after_the_link_rule", () => {
    const set = vi.spyOn(Element.prototype, "setAttribute");
    // Built by hand: the reader never makes a link to an address its rule refuses.
    const host = seeded([
      a("https://example.com/?a=1&b=2", t("web")),
      a("mailto:someone@example.com", t("mail")),
      a("tel:+15550100", t("phone")),
      a("javascript:alert(1)", t("script")),
      a("//example.com", t("no scheme")),
      a("data:text/html,x", t("data")),
      a(" https://example.com", t("space")),
    ]);
    expect(set.mock.calls).toEqual([
      ["href", "https://example.com/?a=1&b=2"],
      ["href", "mailto:someone@example.com"],
      ["href", "tel:+15550100"],
    ]);
    expect(host.querySelectorAll("a")).toHaveLength(3);
    // A refused address leaves its words.
    expect(nodesToDoc(host)).toEqual([
      a("https://example.com/?a=1&b=2", t("web")),
      a("mailto:someone@example.com", t("mail")),
      a("tel:+15550100", t("phone")),
      t("scriptno schemedataspace"),
    ]);
  });

  it("text_that_looks_like_formatting_stays_text", () => {
    const words = '<b>x</b> &amp; <script>alert(1)</script> <img src=x onerror="y">';
    const host = seeded([t(words), b(t("<i>in bold</i>"))]);
    expect(host.querySelector("script, img, i")).toBeNull();
    expect(host.firstChild).toBeInstanceOf(Text);
    expect((host.firstChild as Text).data).toBe(words);
    expect(nodesToDoc(host)).toEqual([t(words), b(t("<i>in bold</i>"))]);
  });

  it("makes_nothing_deeper_than_the_limit_but_its_words", () => {
    let deep: DescNode = t("deep");
    for (let k = 0; k < 100; k++) deep = k % 2 === 0 ? b(deep, BR) : i(deep);
    const host = seeded([deep]);
    let depth = 0;
    for (
      let node: Element | null = host.firstElementChild;
      node && node.localName !== "br";
      node = node.firstElementChild
    ) {
      depth++;
    }
    expect(depth).toBeLessThanOrEqual(MAX_DEPTH);
    const doc = nodesToDoc(host);
    expect(JSON.stringify(doc)).toContain('"deep"');
  });
});

describe("reading the box", () => {
  it("reads_what_webkit_and_chromium_make", () => {
    const cases: [string, HTMLElement, DescDoc][] = [
      [
        "Enter as div lines (WebKit, Chromium)",
        box("Line", h("div", {}, "Two"), h("div", {}, h("br")), h("div", {}, "Four")),
        [t("Line"), BR, t("Two"), BR, BR, t("Four")],
      ],
      ["a div that only holds a break", box(h("div", {}, h("br"))), [BR]],
      [
        "bold re-applied inside bold",
        box(h("b", {}, h("span", { style: "font-weight: bold;" }, "x"))),
        [b(t("x"))],
      ],
      ["a span left italic", box(h("span", { style: "font-style: italic;" }, "x")), [i(t("x"))]],
      [
        "a span left underlined",
        box(h("span", { style: "text-decoration: underline;" }, "x")),
        [u(t("x"))],
      ],
      ["a weight as a number", box(h("span", { style: "font-weight: 700;" }, "x")), [b(t("x"))]],
      ["a light weight", box(h("span", { style: "font-weight: 300;" }, "x")), [t("x")]],
      ["strong and em", box(h("strong", {}, "a"), h("em", {}, "b")), [b(t("a")), i(t("b"))]],
      ["font, unwrapped", box(h("font", { color: "red" }, "red")), [t("red")]],
      ["font with a weight", box(h("font", { style: "font-weight: bold" }, "x")), [b(t("x"))]],
      ["an image", box("a", h("img", { src: "https://example.com/x.png" }), "b"), [t("ab")]],
      ["a comment", box("a", document.createComment("note"), "b"), [t("ab")]],
      [
        "a script and a style",
        box("a", h("script", {}, "alert(1)"), h("style", {}, "b{}"), "b"),
        [t("ab")],
      ],
      ["a javascript: link", box(h("a", { href: "javascript:alert(1)" }, "x")), [t("x")]],
      ["a link without an address", box(h("a", {}, "x")), [t("x")]],
      [
        "a link with its address",
        box(h("a", { href: "https://example.com" }, "x")),
        [a("https://example.com", t("x"))],
      ],
      [
        "a heading's edges",
        box("a", h("h2", {}, "Title"), "b"),
        [t("a"), BR, t("Title"), BR, t("b")],
      ],
      // A list ends its line already: the line after it starts with no empty one between.
      [
        "a line after a list",
        box(h("ul", {}, h("li", {}, "a")), h("div", {}, "b")),
        [ul(li(t("a"))), t("b")],
      ],
      [
        "a list inside a paragraph",
        box(h("p", {}, "a", h("ul", {}, h("li", {}, "b")))),
        [p(t("a")), ul(li(t("b")))],
      ],
      ["an item inside bold", box(h("ul", {}, h("b", {}, h("li", {}, "x")))), [ul(li(b(t("x"))))]],
      [
        "a link inside a link",
        box(h("a", { href: "https://a.example" }, "x", h("a", { href: "https://b.example" }, "y"))),
        [a("https://a.example", t("xy"))],
      ],
      ["a raw line break in text", box("a\nb"), [t("a"), BR, t("b")]],
      // What an emptied box holds: a document that shows nothing, which a save writes as "".
      ["an emptied box", box(h("br")), [BR]],
    ];
    for (const [why, host, doc] of cases) {
      expect(nodesToDoc(host), why).toEqual(doc);
    }
  });

  it("reads_nothing_deeper_than_the_limit_but_its_words", () => {
    // Built from the top down: jsdom walks a whole subtree on each insertion of one.
    const host = box();
    let at: HTMLElement = host;
    for (let k = 0; k < 2000; k++) {
      const next = h(k % 3 === 0 ? "b" : "span");
      at.append(next);
      if (k % 7 === 0) at.append(h("br"));
      at = next;
    }
    at.append("deep");
    const doc = nodesToDoc(host);
    expect(JSON.stringify(doc)).toContain('"deep"');
    let depth = 0;
    for (let level: DescDoc = doc; level.length > 0; depth++) {
      const first = level.find((node) => node.kind !== "text" && node.kind !== "br");
      if (first === undefined) break;
      level = first.children;
    }
    expect(depth).toBeLessThanOrEqual(MAX_DEPTH);
    // Elements count, not only the ones that draw: below MAX_DEPTH of them, bold is its words, as
    // the codec and the view stop there too; what never shows is still dropped there.
    const spans = box();
    let inner: HTMLElement = spans;
    for (let k = 0; k < MAX_DEPTH + 8; k++) {
      const next = h("span");
      inner.append(next);
      inner = next;
    }
    inner.append(h("b", {}, "x", h("br")), h("script", {}, "alert(1)"), "y");
    expect(nodesToDoc(spans)).toEqual([t("x"), BR, t("y")]);
  });

  it("reads_text_without_textContent", () => {
    const content = vi.spyOn(Node.prototype, "textContent", "get");
    const markup = vi.spyOn(Element.prototype, "innerHTML", "get");
    // jsdom has no innerText; one that throws proves nothing reads it.
    Object.defineProperty(HTMLElement.prototype, "innerText", {
      configurable: true,
      get() {
        throw new Error("innerText was read");
      },
    });
    try {
      nodesToDoc(
        box(
          "plain ",
          h("b", {}, "bold"),
          h("a", { href: "https://example.com" }, "link"),
          h("ul", {}, h("li", {}, "item")),
          h("div", {}, h("span", { style: "font-style: italic" }, "line")),
        ),
      );
    } finally {
      Reflect.deleteProperty(HTMLElement.prototype, "innerText");
    }
    expect(content).not.toHaveBeenCalled();
    expect(markup).not.toHaveBeenCalled();
  });
});

describe("both ways", () => {
  /** Any DOM an engine could leave in the box: the model's elements and others, nested any way. */
  function tree(next: () => number, depth: number): (Node | string)[] {
    const pick = <T>(from: readonly T[]): T => from[Math.floor(next() * from.length)];
    const out: (Node | string)[] = [];
    const count = Math.floor(next() * 4);
    for (let k = 0; k < count; k++) {
      const r = next();
      if (depth >= 5 || r < 0.3) {
        out.push(pick(["a", "word", " ", "x & y", "<b>", '"q"', " ", "line\nbreak", "&amp;"]));
      } else if (r < 0.38) out.push(h("br"));
      else if (r < 0.42) out.push(document.createComment("c"));
      else {
        const tag = pick([
          "b",
          "strong",
          "i",
          "em",
          "u",
          "p",
          "ul",
          "ol",
          "li",
          "div",
          "span",
          "font",
          "a",
          "h2",
          "blockquote",
          "img",
          "script",
        ]);
        const attrs: Record<string, string> =
          tag === "a"
            ? {
                href: pick([
                  "https://example.com",
                  "mailto:a@example.com",
                  "tel:+1555",
                  "javascript:x",
                  "",
                ]),
              }
            : tag === "span" && next() < 0.5
              ? {
                  style: pick([
                    "font-weight: bold",
                    "font-style: italic",
                    "text-decoration: underline",
                    "color: red",
                  ]),
                }
              : {};
        out.push(h(tag, attrs, ...tree(next, depth + 1)));
      }
    }
    return out;
  }

  it("what_the_editor_writes_it_can_read_again", () => {
    const next = random(1919);
    for (let k = 0; k < 300; k++) {
      const doc = nodesToDoc(box(...tree(next, 0)));
      const written = serialiseDescription(doc);
      const back = parseDescription(written);
      expect(back.supported, `tree ${k}`).toBe(true);
      if (isEmptyDoc(doc)) expect(written, `tree ${k}`).toBe("");
      else expect(docsEqual(back.doc, doc), `tree ${k}`).toBe(true);
    }
  });

  it("seeding_then_reading_gives_the_same_document", () => {
    // Every description PM can write back, seeded and read with nothing typed, is the document it
    // was: an editor opened and left alone sends nothing (C19 §2.6, §7.8).
    const supported = fixture.rows.filter((row) => row.supported);
    expect(supported.length).toBeGreaterThan(30);
    supported.forEach((row, k) => {
      const doc = parseDescription(row.description).doc;
      expect(nodesToDoc(seeded(doc)), `supported row ${k + 1}`).toEqual(doc);
    });
    // And every document the reader would make, from any tree of the model's nodes.
    const next = random(7);
    const pick = <T>(from: readonly T[]): T => from[Math.floor(next() * from.length)];
    const gen = (depth: number): DescNode[] =>
      Array.from({ length: Math.floor(next() * 4) }, (): DescNode => {
        const r = next();
        if (depth >= 4 || r < 0.35) return t(pick(["a", " ", "x & y", "<b>", " ", "tab\there"]));
        if (r < 0.45) return BR;
        const kind = pick(["b", "i", "u", "a", "p", "ul", "ol", "li"] as const);
        const children = gen(depth + 1);
        return kind === "a"
          ? a(pick(["https://example.com", "mailto:a@example.com", "tel:+15550100"]), ...children)
          : { kind, children };
      });
    for (let k = 0; k < 300; k++) {
      const doc = normalise(gen(0));
      expect(nodesToDoc(seeded(doc)), `document ${k}`).toEqual(doc);
    }
  });

  it("never_writes_html_into_the_page", () => {
    const spy = spyOnHtmlSinks();
    try {
      const host = seeded(
        parseDescription(
          '<html-blob><b>Agenda</b><ul><li>one &amp; two</li></ul><a href="https://example.com">x</a>' +
            "&lt;script&gt;alert(1)&lt;/script&gt;<script>alert(2)</script></html-blob>",
        ).doc,
      );
      document.body.append(host);
      nodesToDoc(host);
    } finally {
      spy.restore();
    }
    expect(spy.sinks).toEqual([]);
    expect(spy.commands).toEqual([]);
  });
});
