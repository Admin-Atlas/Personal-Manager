// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { isLetterKey } from "./letterKey";

describe("isLetterKey", () => {
  it("matches the letter the layout types, wherever the key sits", () => {
    expect(isLetterKey({ key: "z", code: "KeyZ" }, "z")).toBe(true); // QWERTY
    expect(isLetterKey({ key: "z", code: "KeyY" }, "z")).toBe(true); // German QWERTZ
    expect(isLetterKey({ key: "z", code: "KeyW" }, "z")).toBe(true); // French AZERTY
    expect(isLetterKey({ key: "Z", code: "KeyZ" }, "z")).toBe(true); // with Shift
  });

  it("doesn't match another letter on the same key", () => {
    expect(isLetterKey({ key: "y", code: "KeyZ" }, "z")).toBe(false); // QWERTZ's Y
    expect(isLetterKey({ key: "w", code: "KeyZ" }, "z")).toBe(false); // AZERTY's W
  });

  it("falls back to the physical key when the layout types no Latin letter", () => {
    expect(isLetterKey({ key: "я", code: "KeyZ" }, "z")).toBe(true); // Russian
    expect(isLetterKey({ key: "ζ", code: "KeyZ" }, "z")).toBe(true); // Greek
    expect(isLetterKey({ key: "н", code: "KeyY" }, "z")).toBe(false);
    expect(isLetterKey({ key: "Unidentified", code: "KeyZ" }, "z")).toBe(true);
  });
});
