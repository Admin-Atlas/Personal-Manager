// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import rehypeSanitize, { defaultSchema } from "rehype-sanitize";
import rehypeExternalLinks from "rehype-external-links";
import { REHYPE_PLUGINS, safeUrl, SCHEMA } from "./markdown";

// The markdown pipeline is PM's single sanitizing boundary for untrusted, ingested content. These lock
// the two pure pieces of that boundary so a refactor can't silently weaken it (T-07), plus the SHAPE
// of the pipeline itself (H6). Behaviour through the real renderer is in `markdown.render.test.tsx`;
// the two halves are deliberately separate so the pure suite keeps its node environment.

describe("safeUrl", () => {
  it("keeps http(s) / mailto absolute URLs", () => {
    expect(safeUrl("https://example.com/x")).toBe("https://example.com/x");
    expect(safeUrl("http://example.com")).toBe("http://example.com");
    expect(safeUrl("mailto:a@b.co")).toBe("mailto:a@b.co");
  });

  it("keeps relative and in-page targets", () => {
    expect(safeUrl("#anchor")).toBe("#anchor");
    expect(safeUrl("/path")).toBe("/path");
    expect(safeUrl("./rel")).toBe("./rel");
    expect(safeUrl("../up")).toBe("../up");
  });

  it("neutralises a hostile or non-allowlisted scheme to empty", () => {
    expect(safeUrl("javascript:alert(1)")).toBe("");
    expect(safeUrl("data:text/html,<script>alert(1)</script>")).toBe("");
    expect(safeUrl("vbscript:msgbox('x')")).toBe("");
    expect(safeUrl("file:///etc/passwd")).toBe("");
  });

  it("is case-insensitive on the scheme", () => {
    expect(safeUrl("HTTPS://example.com")).toBe("HTTPS://example.com");
    expect(safeUrl("JavaScript:alert(1)")).toBe("");
  });
});

describe("SCHEMA", () => {
  it("extends the default anchor allowlist with target + rel", () => {
    expect(SCHEMA.attributes.a).toContain("target");
    expect(SCHEMA.attributes.a).toContain("rel");
  });

  it("never allows a <script> tag (the GitHub default omits it)", () => {
    expect(SCHEMA.tagNames ?? []).not.toContain("script");
  });

  // The assertion above reads hast-util-sanitize's own allow-list, because SCHEMA never sets
  // `tagNames` of its own — it would only catch PM literally appending "script". This is the
  // PM-side pin: the ONLY thing we widen is `attributes.a`. Everything else, `protocols` above all
  // (the list that makes a `javascript:` href die at the sanitizer even if urlTransform were
  // bypassed), has to stay exactly the library default.
  it("widens the default schema in exactly one place and no other", () => {
    const differing = Object.keys(defaultSchema).filter(
      (k) =>
        SCHEMA[k as keyof typeof SCHEMA] !== defaultSchema[k as keyof typeof defaultSchema] &&
        k !== "attributes",
    );
    expect(differing).toEqual([]);
    expect(Object.keys(SCHEMA).sort()).toEqual(Object.keys(defaultSchema).sort());

    const attrs = SCHEMA.attributes as Record<string, unknown>;
    const defaults = (defaultSchema.attributes ?? {}) as Record<string, unknown>;
    const changedAttrs = Object.keys(defaults).filter((k) => attrs[k] !== defaults[k]);
    expect(changedAttrs).toEqual(["a"]);
    expect(Object.keys(attrs).sort()).toEqual(Object.keys(defaults).sort());
  });
});

// H6. Order is the security property, and it is not visible from any behavioural test: a plugin
// added AFTER the sanitizer gets whatever it emits straight into the DOM. Adding `rehype-raw` — the
// plausible one-line change to "make HTML in a captured web article render" — is exactly the shape
// of edit these exist to stop passing silently.
describe("the rehype pipeline", () => {
  const entry = (i: number) => REHYPE_PLUGINS[i] as unknown[];

  it("runs the sanitizer LAST", () => {
    const last = entry(REHYPE_PLUGINS.length - 1);
    expect(last[0]).toBe(rehypeSanitize);
  });

  it("hands the sanitizer PM's schema, not a fresh one", () => {
    const last = entry(REHYPE_PLUGINS.length - 1);
    expect(last[1]).toBe(SCHEMA);
  });

  // Strict on purpose. A new rehype plugin is a decision about where it sits relative to the
  // sanitizer, so it should have to come here and say so rather than be appended quietly.
  it("holds exactly the two plugins it is meant to", () => {
    expect(REHYPE_PLUGINS).toHaveLength(2);
    expect(entry(0)[0]).toBe(rehypeExternalLinks);
  });
});
