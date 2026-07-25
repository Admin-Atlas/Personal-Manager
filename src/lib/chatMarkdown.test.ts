// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { CITE_HREF_PREFIX, citationTarget, linkCitations } from "./chatMarkdown";

describe("linkCitations", () => {
  it("rewrites an in-range marker to an in-page link keeping the literal label", () => {
    expect(linkCitations("As shown [1].", 2)).toBe("As shown [[1]](#pm-cite-1).");
  });

  it("leaves an out-of-range marker as plain text", () => {
    // The model sometimes cites a source that didn't survive the confidence gate.
    expect(linkCitations("As shown [3].", 2)).toBe("As shown [3].");
    expect(linkCitations("As shown [0].", 2)).toBe("As shown [0].");
  });

  it("rewrites every marker in a multi-line answer", () => {
    expect(linkCitations("- one [1]\n- two [2]", 2)).toBe(
      "- one [[1]](#pm-cite-1)\n- two [[2]](#pm-cite-2)",
    );
  });

  it("leaves markers inside a fenced code block alone", () => {
    const src = "before [1]\n```\nconst a = xs[1];\n```\nafter [1]";
    expect(linkCitations(src, 1)).toBe(
      "before [[1]](#pm-cite-1)\n```\nconst a = xs[1];\n```\nafter [[1]](#pm-cite-1)",
    );
  });

  it("handles a tilde fence and an unclosed fence", () => {
    expect(linkCitations("~~~\nxs[1]\n~~~\n[1]", 1)).toBe("~~~\nxs[1]\n~~~\n[[1]](#pm-cite-1)");
    // An unterminated fence mid-stream must not start linking code as citations.
    expect(linkCitations("[1]\n```\nxs[1]", 1)).toBe("[[1]](#pm-cite-1)\n```\nxs[1]");
  });

  it("leaves markers inside an inline code span alone", () => {
    expect(linkCitations("use `xs[1]` then see [1]", 1)).toBe(
      "use `xs[1]` then see [[1]](#pm-cite-1)",
    );
  });

  it("is a no-op when there is nothing to link", () => {
    expect(linkCitations("plain answer", 3)).toBe("plain answer");
    expect(linkCitations("cited [1] but ungrounded", 0)).toBe("cited [1] but ungrounded");
    expect(linkCitations("", 2)).toBe("");
  });

  it("does not disturb other markdown", () => {
    const src = "**bold** and _em_ and [a link](https://example.com) and [1]";
    expect(linkCitations(src, 1)).toBe(
      "**bold** and _em_ and [a link](https://example.com) and [[1]](#pm-cite-1)",
    );
  });
});

describe("citationTarget", () => {
  it("reads the source number out of one of our hrefs", () => {
    expect(citationTarget(`${CITE_HREF_PREFIX}2`, 3)).toBe(2);
  });

  it("rejects anything that isn't an in-range citation href", () => {
    expect(citationTarget("https://example.com", 3)).toBeNull();
    expect(citationTarget("#heading", 3)).toBeNull();
    expect(citationTarget(null, 3)).toBeNull();
    expect(citationTarget(undefined, 3)).toBeNull();
    // Out of range: a model-authored `#pm-cite-99` must not scroll to a source that isn't there.
    expect(citationTarget(`${CITE_HREF_PREFIX}99`, 3)).toBeNull();
    expect(citationTarget(`${CITE_HREF_PREFIX}x`, 3)).toBeNull();
  });
});
