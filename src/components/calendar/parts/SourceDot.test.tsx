// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// SourceDot's shape redundancy: with the colour-blind axis off it's always a plain circle (today's
// look), and with it on a source's slot picks a distinct clip-path shape — except slot 0 and overlays
// (no slot), which stay circles. useTheme is mocked so the axis can be flipped without a provider.

import { render, cleanup } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

let mockColorblind = false;
vi.mock("../../../theme", () => ({ useTheme: () => ({ colorblind: mockColorblind }) }));

import { SourceDot } from "./SourceDot";

afterEach(cleanup);

const dot = (props: { color: string; shapeIndex?: number }) =>
  render(<SourceDot {...props} />).container.querySelector("span")!;

describe("SourceDot shape redundancy", () => {
  it("is a plain circle when the axis is off, whatever the slot", () => {
    mockColorblind = false;
    const span = dot({ color: "#f00", shapeIndex: 2 });
    expect(span.className).toContain("rounded-full");
    expect(span.style.clipPath).toBe("");
  });

  it("takes a clip-path shape when the axis is on and a non-zero slot is given", () => {
    mockColorblind = true;
    const span = dot({ color: "#f00", shapeIndex: 2 });
    expect(span.className).not.toContain("rounded-full");
    expect(span.style.clipPath).toContain("polygon");
  });

  it("stays a circle for slot 0 and for overlays (no slot), even with the axis on", () => {
    mockColorblind = true;
    expect(dot({ color: "#f00", shapeIndex: 0 }).className).toContain("rounded-full");
    expect(dot({ color: "#f00" }).className).toContain("rounded-full");
  });
});
