// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import { safeUrl, SCHEMA } from "./markdown";

// The markdown pipeline is PM's single sanitizing boundary for untrusted, ingested content. These lock
// the two pure pieces of that boundary so a refactor can't silently weaken it (T-07).

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
});
