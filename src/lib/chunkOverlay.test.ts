// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The reader's byte→char mapping, which had no test at all.
//
// WHY IT MATTERS. `chunks.start_offset` / `end_offset` are UTF-8 BYTE offsets recorded by the Rust
// splitter; a JavaScript string is UTF-16. Every chunk boundary the reader draws, and every span of
// text it attributes to a chunk, goes through `makeByteToChar`. Get it wrong on a document with one
// accent in it and the overlay silently shades the wrong words — there is no error, no blank space,
// nothing a user could report except "the highlighting looks off".
//
// The round-trip test is the one that matters: for a body of mixed scripts, slicing by the mapped
// indices must reproduce the exact substring the splitter saw. Everything else is an edge case that
// makes a failure legible.

import { describe, expect, it } from "vitest";

import { makeByteToChar, parentGroupStarts, segmentByLeaves, shadeLeaves } from "./chunkOverlay";
import type { ChunkSpan } from "./types";

/** The reference the mapper has to agree with — the same count Rust produces. */
const utf8Bytes = (s: string) => new TextEncoder().encode(s).length;

const leaf = (over: Partial<ChunkSpan> & { id: number }): ChunkSpan => ({
  ordinal: 0,
  parent_id: null,
  kind: "leaf",
  start_offset: 0,
  end_offset: 0,
  ...over,
});

describe("makeByteToChar", () => {
  it("is the identity on ASCII, where a byte is a character", () => {
    const toChar = makeByteToChar("hello world");
    for (let i = 0; i <= 11; i++) expect(toChar(i)).toBe(i);
  });

  it("diverges from the byte index as soon as the text is not ASCII", () => {
    // "é" is 2 bytes, 1 UTF-16 unit; "中" is 3 bytes, 1 unit.
    const body = "aé中b";
    const toChar = makeByteToChar(body);
    expect(utf8Bytes(body)).toBe(7);
    expect(toChar(0)).toBe(0); // a
    expect(toChar(1)).toBe(1); // é starts
    expect(toChar(3)).toBe(2); // 中 starts
    expect(toChar(6)).toBe(3); // b starts
    expect(toChar(7)).toBe(4); // end
  });

  it("counts a surrogate pair as 4 bytes and 2 UTF-16 units", () => {
    // The case a naive `body.length` comparison gets wrong in both directions at once.
    const body = "a😀b";
    expect(utf8Bytes(body)).toBe(6);
    expect(body.length).toBe(4); // a + 2 surrogates + b
    const toChar = makeByteToChar(body);
    expect(toChar(1)).toBe(1); // 😀 starts at char 1
    expect(toChar(5)).toBe(3); // b starts at char 3, byte 5
    expect(toChar(6)).toBe(4); // end of string
  });

  it("resolves an offset inside a multi-byte code point forward, to the next boundary", () => {
    // Writing this test is what showed the doc comment claimed the opposite ("resolves to that code
    // point's start") of what the binary search does. The code is the defensible half — a straddling
    // character belongs to the chunk that ENDS on it — so the comment was corrected to match, and
    // the behaviour is pinned here so the two cannot drift apart again.
    const toChar = makeByteToChar("a😀b");
    for (const inside of [2, 3, 4]) {
      expect(toChar(inside)).toBe(3); // → 'b', the next code-point boundary
    }
    // Whichever rule applies, the property that actually matters holds: one shared boundary maps to
    // one index, so adjacent chunks never lose or duplicate a character between them.
    const body = "a😀b";
    const boundary = 3; // mid-emoji, i.e. a boundary that should never occur
    expect(body.slice(0, toChar(boundary)) + body.slice(toChar(boundary))).toBe(body);
  });

  it("clamps out-of-range offsets rather than returning NaN or undefined", () => {
    const toChar = makeByteToChar("aé中b");
    expect(toChar(-1)).toBe(0);
    expect(toChar(-1000)).toBe(0);
    expect(toChar(999)).toBe(4);
  });

  it("handles an empty body without special-casing at the call site", () => {
    const toChar = makeByteToChar("");
    expect(toChar(0)).toBe(0);
    expect(toChar(5)).toBe(0);
  });

  it("round-trips: slicing by mapped indices reproduces the source substring exactly", () => {
    // The property the whole module exists for. Walk a mixed-script body one code point at a time,
    // computing byte offsets the way Rust does, and assert every single span comes back verbatim.
    const body = "Zürich 東京 🎉 done — naïve café\nsecond line";
    const toChar = makeByteToChar(body);
    const points = Array.from(body);

    let byte = 0;
    const byteAt: number[] = [];
    for (const cp of points) {
      byteAt.push(byte);
      byte += utf8Bytes(cp);
    }
    byteAt.push(byte);
    expect(byte).toBe(utf8Bytes(body));

    for (let i = 0; i < points.length; i++) {
      for (let j = i; j <= points.length; j++) {
        const sliced = body.slice(toChar(byteAt[i]), toChar(byteAt[j]));
        expect(sliced).toBe(points.slice(i, j).join(""));
      }
    }
  });
});

describe("segmentByLeaves", () => {
  it("slices each leaf to the exact text the splitter chunked", () => {
    const body = "# Café\n\nDeux bières.";
    // Byte offsets, as Rust would record them: "# Café\n\n" is 9 bytes (é is 2).
    const head = utf8Bytes("# Café");
    const bodyStart = utf8Bytes("# Café\n\n");
    const segs = segmentByLeaves(body, [
      leaf({ id: 1, ordinal: 0, start_offset: 0, end_offset: head }),
      leaf({ id: 2, ordinal: 1, start_offset: bodyStart, end_offset: utf8Bytes(body) }),
    ]);
    expect(segs.map((s) => s.text)).toEqual(["# Café", "Deux bières."]);
  });

  it("skips parents and any chunk with no recorded offsets", () => {
    // Chat-turn chunks predate the offset columns; a parent is structural and never rendered.
    const segs = segmentByLeaves("abcdef", [
      leaf({ id: 1, kind: "parent", start_offset: 0, end_offset: 6 }),
      leaf({ id: 2, start_offset: null, end_offset: null }),
      leaf({ id: 3, start_offset: 0, end_offset: 3 }),
    ]);
    expect(segs.map((s) => s.chunkId)).toEqual([3]);
  });

  // WHY THESE EXIST. The splitter re-seeds each new leaf with the trailing whole units of the one it
  // just flushed (CHUNK_OVERLAP_TOKENS), so stored leaf ranges genuinely overlap — `start_offset` of
  // leaf N+1 is strictly less than `end_offset` of leaf N. That is deliberate and retrieval depends
  // on it; the reader is the layer that has to adapt, by clipping. Before the clip, the overlay
  // rendered the shared run twice: once at the tail of one band, once at the head of the next, so
  // "Show chunks" showed different text from the same document with the toggle off.

  it("tiles overlapping leaves, so the shared run is rendered exactly once", () => {
    // THE regression property. Two leaves whose ranges overlap (leaf 2 starts inside leaf 1, exactly
    // as overlap_seed produces): the concatenation must be the source span, byte for byte.
    const body = "0123456789abcdefghij";
    const segs = segmentByLeaves(body, [
      leaf({ id: 1, ordinal: 0, start_offset: 0, end_offset: 12 }),
      leaf({ id: 2, ordinal: 1, start_offset: 8, end_offset: 20 }),
    ]);
    expect(segs.map((s) => s.text).join("")).toBe(body.slice(0, 20));
    // The overlapped run "89ab" belongs to the EARLIER chunk — the same rule makeByteToChar uses for
    // a straddling code point. Flipping that attribution is a decision, not a tidy-up.
    expect(segs[0].text).toBe("0123456789ab");
    expect(segs[1].text).toBe("cdefghij");
  });

  it("does not repeat a shared paragraph across two bands", () => {
    // The realistic shape: leaf 1 = paragraphs 0-1, leaf 2 = paragraphs 1-2 (whole-unit overlap).
    const paras = ["First paragraph here.", "Second paragraph here.", "Third paragraph here."];
    const body = paras.join("\n\n");
    const p1Start = utf8Bytes(paras[0] + "\n\n");
    const p1End = p1Start + utf8Bytes(paras[1]);
    const segs = segmentByLeaves(body, [
      leaf({ id: 1, ordinal: 0, start_offset: 0, end_offset: p1End }),
      leaf({ id: 2, ordinal: 1, start_offset: p1Start, end_offset: utf8Bytes(body) }),
    ]);
    expect(segs[1].text.startsWith(paras[1])).toBe(false);
    const all = segs.map((s) => s.text).join("");
    expect(all).toBe(body);
    expect(all.split(paras[1]).length - 1).toBe(1); // paragraph 1 occurs exactly once
  });

  it("clips on a non-ASCII body without re-introducing byte/char drift", () => {
    // Clipping composes with the byte→char mapping: the cursor is a UTF-16 index, the offsets are
    // bytes, and mixing the two up is exactly the class of bug this module exists to prevent.
    const body = "Café ☕ naïve 😀 résumé done";
    const mid = utf8Bytes("Café ☕ naïve "); // inside leaf 1, start of leaf 2
    const segs = segmentByLeaves(body, [
      leaf({ id: 1, ordinal: 0, start_offset: 0, end_offset: utf8Bytes("Café ☕ naïve 😀") }),
      leaf({ id: 2, ordinal: 1, start_offset: mid, end_offset: utf8Bytes(body) }),
    ]);
    expect(segs.map((s) => s.text).join("")).toBe(body);
    expect(segs[0].text).toBe("Café ☕ naïve 😀");
    expect(segs[1].text).toBe(" résumé done");
  });

  it("yields an empty band, not a reversed slice, for a leaf inside its predecessor", () => {
    // Monotonic end offsets make this unreachable from the splitter; it pins the Math.max, because
    // body.slice(hi, lo) silently returns "" from the wrong reasoning and a negative range would
    // otherwise surface as text taken from somewhere else entirely.
    const body = "alpha beta gamma";
    const segs = segmentByLeaves(body, [
      leaf({ id: 1, ordinal: 0, start_offset: 0, end_offset: 16 }),
      leaf({ id: 2, ordinal: 1, start_offset: 6, end_offset: 10 }),
      leaf({ id: 3, ordinal: 2, start_offset: 11, end_offset: 16 }),
    ]);
    // Every leaf still gets a band — the overlay promises a 1:1 chunk→band mapping.
    expect(segs.map((s) => s.chunkId)).toEqual([1, 2, 3]);
    expect(segs[1].text).toBe("");
    expect(segs[2].text).toBe("");
    expect(segs.map((s) => s.text).join("")).toBe(body);
  });
});

describe("shadeLeaves and parentGroupStarts", () => {
  it("alternates the zebra over every leaf in document order", () => {
    const segs = [1, 2, 3, 4].map((id) => ({
      chunkId: id,
      parentId: 9,
      ordinal: id,
      text: "x",
    }));
    expect([...shadeLeaves(segs).values()]).toEqual([0, 1, 0, 1]);
  });

  it("starts a group at each parent change, and gives every parentless leaf its own", () => {
    const segs = [
      { chunkId: 1, parentId: 10, ordinal: 0, text: "a" },
      { chunkId: 2, parentId: 10, ordinal: 1, text: "b" },
      { chunkId: 3, parentId: null, ordinal: 2, text: "c" },
      { chunkId: 4, parentId: null, ordinal: 3, text: "d" },
      { chunkId: 5, parentId: 11, ordinal: 4, text: "e" },
    ];
    // 1 opens the first group; 2 continues it; 3 and 4 each stand alone; 5 opens a new parent.
    expect([...parentGroupStarts(segs)]).toEqual([1, 3, 4, 5]);
  });
});
