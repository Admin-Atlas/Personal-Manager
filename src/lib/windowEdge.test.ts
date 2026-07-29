// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { edgeAt } from "./windowEdge";

// The cursor must appear exactly where tauri-runtime-wry's GTK handler will actually call
// `begin_resize_drag` — a resize cursor over a spot that does nothing is worse than none at all.
describe("edgeAt", () => {
  const W = 1000;
  const H = 800;
  const BAND = 5;

  it("reports no edge in the interior", () => {
    expect(edgeAt(500, 400, W, H, BAND)).toBeNull();
    expect(edgeAt(BAND + 1, BAND + 1, W, H, BAND)).toBeNull();
    expect(edgeAt(W - BAND - 1, H - BAND - 1, W, H, BAND)).toBeNull();
  });

  it("reports each side within the band", () => {
    expect(edgeAt(500, 0, W, H, BAND)).toBe("n");
    expect(edgeAt(500, H, W, H, BAND)).toBe("s");
    expect(edgeAt(0, 400, W, H, BAND)).toBe("w");
    expect(edgeAt(W, 400, W, H, BAND)).toBe("e");
  });

  it("prefers a corner over either of the sides forming it", () => {
    expect(edgeAt(0, 0, W, H, BAND)).toBe("nw");
    expect(edgeAt(W, 0, W, H, BAND)).toBe("ne");
    expect(edgeAt(0, H, W, H, BAND)).toBe("sw");
    expect(edgeAt(W, H, W, H, BAND)).toBe("se");
  });

  it("treats the band as inclusive, and just past it as interior", () => {
    expect(edgeAt(500, BAND, W, H, BAND)).toBe("n");
    expect(edgeAt(500, BAND + 0.5, W, H, BAND)).toBeNull();
    expect(edgeAt(W - BAND, 400, W, H, BAND)).toBe("e");
  });

  it("widens with the scale factor, since the native band is 5 x the GTK scale", () => {
    // At devicePixelRatio 2 the native border is 10 logical px, so 8px in is still an edge.
    expect(edgeAt(500, 8, W, H, 5)).toBeNull();
    expect(edgeAt(500, 8, W, H, 10)).toBe("n");
  });
});
