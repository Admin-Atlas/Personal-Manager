// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import { paddedPassphraseHint } from "./vaultPassphrase";

describe("paddedPassphraseHint", () => {
  it("stays silent when the typed passphrase carries no padding", () => {
    expect(paddedPassphraseHint("correct horse battery staple")).toBe("");
    expect(paddedPassphraseHint("")).toBe("");
  });

  it("treats internal spaces as part of the passphrase, not padding", () => {
    // A passphrase is exact bytes — only the ends were ever dropped, so a passphrase
    // that merely contains spaces must not be told to try it "without them".
    expect(paddedPassphraseHint("two words")).toBe("");
  });

  it("nudges when the typed passphrase is padded at either end", () => {
    expect(paddedPassphraseHint(" leading")).toContain("try it without");
    expect(paddedPassphraseHint("trailing ")).toContain("try it without");
    expect(paddedPassphraseHint("  both  ")).toContain("try it without");
    // Tabs and newlines pad too (a paste artefact is the likeliest way in).
    expect(paddedPassphraseHint("\tpasted\n")).toContain("try it without");
  });
});
