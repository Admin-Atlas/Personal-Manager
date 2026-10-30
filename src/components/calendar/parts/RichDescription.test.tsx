// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A Google description's formatting at the DOM (C19b): what the view draws from the codec's document,
// and that nothing hostile in a description survives as an element, an attribute or an address.
// markdown.render.test.tsx is the template: every assertion on a canary is "this did not survive".

import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  MAX_DEPTH,
  linkTarget,
  parseDescription,
  type DescNode,
} from "../../../lib/calendarEdit/descriptionHtml";
import { spyOnHtmlSinks } from "../../../lib/calendarEdit/htmlSinkSpy";
import { RichDescription } from "./RichDescription";

afterEach(cleanup);

/** The view of a description, as the pop-up draws one. */
function draw(description: string, onMail = vi.fn()) {
  const { container } = render(
    <RichDescription doc={parseDescription(description).doc} onMail={onMail} />,
  );
  return container.querySelector(".pm-rich") as HTMLElement;
}

describe("RichDescription", () => {
  it("draws_googles_formatting", () => {
    const view = draw(
      "<html-blob><b>Agenda</b><br><i>one</i> <u>two</u><ol><li>first</li><li>second</li></ol>" +
        '<ul><li>apple</li></ul><p>Notes</p><a href="https://example.com/x" target="_blank" id="ow1">site</a></html-blob>',
    );
    expect(view.querySelector("b")?.textContent).toBe("Agenda");
    expect(view.querySelector("i")?.textContent).toBe("one");
    expect(view.querySelector("u")?.textContent).toBe("two");
    expect(view.querySelector("br")).not.toBeNull();
    expect([...view.querySelectorAll("ol > li")].map((li) => li.textContent)).toEqual([
      "first",
      "second",
    ]);
    expect(view.querySelector("ul > li")?.textContent).toBe("apple");
    expect(view.querySelector("p")?.textContent).toBe("Notes");
    const link = view.querySelector("a");
    expect(link?.getAttribute("href")).toBe("https://example.com/x");
    // useExternalLinks opens it in the browser: target and rel are PM's, never Google's id.
    expect(link?.getAttribute("target")).toBe("_blank");
    expect(link?.getAttribute("rel")).toBe("noreferrer");
    expect(link?.hasAttribute("id")).toBe(false);
    expect(link?.textContent).toBe("site");
  });

  it.each([
    ["a script", "<b>a</b><script>alert(1)</script>"],
    ["an image that runs code", '<b>a</b><img src=x onerror="alert(1)">'],
    ["an iframe", '<b>a</b><iframe src="https://evil.example">x</iframe>'],
    ["a style", "<b>a</b><style>body{display:none}</style>"],
    ["a style attribute", '<b style="color: red; background: url(x)">a</b>'],
    ["a handler", '<b onclick="alert(1)">a</b>'],
    ["javascript:", '<a href="javascript:alert(1)">a</a>'],
    ["JAVASCRIPT:", '<a href="JAVASCRIPT:alert(1)">a</a>'],
    ["a leading space", '<a href=" javascript:alert(1)">a</a>'],
    ["a decoded reference", '<a href="&#106;avascript:alert(1)">a</a>'],
    ["a tab inside", '<a href="java&#9;script:alert(1)">a</a>'],
    ["data:", '<a href="data:text/html,x">a</a>'],
    ["vbscript:", '<a href="vbscript:x">a</a>'],
    ["protocol-relative", '<a href="//evil.example">a</a>'],
    ["backslashes", '<a href="\\\\evil.example">a</a>'],
    ["slash backslash", '<a href="/\\evil.example">a</a>'],
    ["relative", '<a href="/x">a</a>'],
    ["no scheme, not an address", '<a href="not an address">a</a>'],
    ["an object", '<b>a</b><object data="x"></object><embed src="x">'],
    ["a form", '<b>a</b><form action="https://evil.example"><input name="x"></form>'],
  ])("never draws %s", (_name, description) => {
    const view = draw(description);
    expect(view.querySelector("script, iframe, img, style, object, embed, form, input")).toBeNull();
    for (const element of view.querySelectorAll("*")) {
      for (const attr of element.getAttributeNames()) {
        expect(attr.startsWith("on") || attr === "style" || attr === "src").toBe(false);
      }
    }
    for (const link of view.querySelectorAll("a")) {
      const href = link.getAttribute("href") ?? "";
      expect(linkTarget(href)).toBe(href);
    }
    expect(view.textContent).not.toContain("alert(1)");
    expect(view.textContent).not.toContain("display:none");
    // The words stay.
    expect(view.textContent).toContain("a");
  });

  it("canaries keep their words and no link", () => {
    const view = draw(
      '<a href="javascript:alert(1)">Click me</a> and <a href="//evil.example">me</a>',
    );
    expect(view.querySelector("a")).toBeNull();
    expect(view.textContent).toBe("Click me and me");
  });

  it("mail_links_open_the_mail_app", () => {
    const onMail = vi.fn();
    const view = draw('<a href="mailto:ann@example.com?subject=Hi">Email us</a>', onMail);
    const link = view.querySelector("a");
    expect(link?.getAttribute("href")).toBe("mailto:ann@example.com?subject=Hi");
    // Not the browser's: no target, so useExternalLinks leaves it alone.
    expect(link?.hasAttribute("target")).toBe(false);
    // fireEvent says false when the default was prevented: PM's window never follows it.
    expect(fireEvent.click(link!)).toBe(false);
    expect(onMail).toHaveBeenCalledExactlyOnceWith("mailto:ann@example.com?subject=Hi");
  });

  it("a_phone_link_shows_as_its_words", () => {
    const view = draw('<b>Call <a href="tel:+15550100">the desk</a></b>');
    expect(view.querySelector("a")).toBeNull();
    expect(view.querySelector("b")?.textContent).toBe("Call the desk");
  });

  it("addresses_written_in_the_words_are_links", () => {
    const onMail = vi.fn();
    const view = draw(
      "<b>Join at https://example.com/j/1.</b> or mail ann@example.com, or www.example.org",
      onMail,
    );
    const links = [...view.querySelectorAll("a")];
    expect(links.map((a) => [a.textContent, a.getAttribute("href")])).toEqual([
      ["https://example.com/j/1", "https://example.com/j/1"],
      ["ann@example.com", "mailto:ann@example.com"],
      ["www.example.org", "https://www.example.org"],
    ]);
    // Inside the bold, with the full stop after it as words.
    expect(view.querySelector("b")?.textContent).toBe("Join at https://example.com/j/1.");
    expect(view.querySelector("b > a")).not.toBeNull();
    expect(links[0].getAttribute("target")).toBe("_blank");
    expect(fireEvent.click(links[1])).toBe(false);
    expect(onMail).toHaveBeenCalledExactlyOnceWith("mailto:ann@example.com");
  });

  it("words_inside_a_link_stay_its_words", () => {
    const view = draw('<a href="https://example.com">see www.example.org</a>');
    const links = view.querySelectorAll("a");
    expect(links).toHaveLength(1);
    expect(links[0].getAttribute("href")).toBe("https://example.com");
    expect(links[0].textContent).toBe("see www.example.org");
  });

  it("a_cut_description_shows_what_came_before", () => {
    const view = draw('<b>a</b> <i class="x');
    expect(view.querySelector("b")?.textContent).toBe("a");
    expect(view.querySelector("i")).toBeNull();
    expect(view.textContent).not.toContain("class");
  });

  it("an_address_in_angle_brackets_shows_and_links", () => {
    const view = draw("<b>Teams</b> Click here to join<https://example.com/j/1>");
    expect(view.textContent).toBe("Teams Click here to join<https://example.com/j/1>");
    expect(view.querySelector("a")?.getAttribute("href")).toBe("https://example.com/j/1");
  });

  it("draws_nothing_deeper_than_the_limit_but_its_words", () => {
    // Deeper than any document the codec makes, built by hand.
    let node: DescNode = { kind: "text", text: "deep" };
    for (let n = 0; n < MAX_DEPTH + 8; n++) node = { kind: "b", children: [node] };
    const { container } = render(<RichDescription doc={[node]} onMail={vi.fn()} />);
    let depth = 0;
    for (
      let at = container.querySelector(".pm-rich > b");
      at;
      at = at.querySelector(":scope > b")
    ) {
      depth++;
    }
    expect(depth).toBeLessThanOrEqual(MAX_DEPTH);
    expect(container.textContent).toBe("deep");
  });

  it("never_writes_html_into_the_page", () => {
    // Started before render and stopped before cleanup (critique C1-M8): the view writes as it
    // renders.
    const spy = spyOnHtmlSinks();
    try {
      draw(
        '<html-blob><b>Agenda</b><ul><li>one &amp; two</li></ul><a href="https://example.com">x</a>' +
          '<a href="mailto:a@example.com">m</a> see www.example.org<script>alert(1)</script></html-blob>',
      );
      draw("<table><tr><td>a</td><td>b</td></tr></table>");
    } finally {
      spy.restore();
    }
    expect(spy.sinks).toEqual([]);
    expect(spy.commands).toEqual([]);
  });

  // The spy above proves nothing unless it sees each sink when one is used.
  it("the_sink_spy_sees_every_sink", () => {
    const host = document.createElement("div");
    document.body.append(host);
    const shadow = document.createElement("div").attachShadow({ mode: "open" });
    const spy = spyOnHtmlSinks();
    try {
      host.innerHTML = "<b>x</b>";
      host.firstElementChild!.outerHTML = "<i>y</i>";
      shadow.innerHTML = "<b>z</b>";
      host.insertAdjacentHTML("beforeend", "<u>w</u>");
      document.createRange().createContextualFragment("<b>v</b>");
      new DOMParser().parseFromString("<b>u</b>", "text/html");
      document.write("");
      document.writeln("");
      document.execCommand("insertHTML");
    } finally {
      spy.restore();
      host.remove();
    }
    expect(spy.sinks).toEqual([
      "innerHTML",
      "outerHTML",
      "ShadowRoot innerHTML",
      "insertAdjacentHTML",
      "createContextualFragment",
      "parseFromString",
      "document.write",
      "document.writeln",
    ]);
    expect(spy.commands).toEqual(["insertHTML"]);
    // Restored: the stand-in is gone, and a sink used now isn't seen.
    expect(Object.getOwnPropertyDescriptor(document, "execCommand")).toBeUndefined();
    document.createElement("div").innerHTML = "<b>t</b>";
    expect(spy.sinks).toHaveLength(8);
  });
});
