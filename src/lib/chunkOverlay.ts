// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Byte→char conversion for the chunk-boundary overlay, done ONCE in one place.
//
// `chunks.start_offset`/`end_offset` are BYTE offsets into the UTF-8 document body the splitter chunked;
// `char_count` is a CHARACTER count; a JS string is UTF-16. Slicing the body with the raw byte numbers
// would drift on any non-ASCII content (emoji, CJK, accented text). This module maps byte offsets to
// UTF-16 string indices by walking the body's code points once, then slices — so a segment's text is
// exactly the source substring the splitter saw.

import type { ChunkSpan } from "./types";

/** UTF-8 byte length of a single code point (no allocation, unlike TextEncoder per char). */
function utf8Len(codePoint: number): number {
  if (codePoint < 0x80) return 1;
  if (codePoint < 0x800) return 2;
  if (codePoint < 0x10000) return 3;
  return 4;
}

/**
 * Build a byte-offset → UTF-16-index mapper for `body`, precomputed in a single pass so many lookups
 * share the work. Offsets are expected to land on code-point boundaries (chunk edges are block-aligned);
 * an offset that falls inside a multi-byte code point resolves to that code point's start, and an
 * out-of-range offset clamps to 0 or the string length.
 */
export function makeByteToChar(body: string): (byteOffset: number) => number {
  const codePoints = Array.from(body); // splits on code points, not UTF-16 units
  const byteStart = new Array<number>(codePoints.length + 1);
  const charStart = new Array<number>(codePoints.length + 1);
  let bytes = 0;
  let chars = 0;
  for (let i = 0; i < codePoints.length; i++) {
    byteStart[i] = bytes;
    charStart[i] = chars;
    bytes += utf8Len(codePoints[i].codePointAt(0)!);
    chars += codePoints[i].length; // 1 for BMP, 2 for a surrogate pair
  }
  byteStart[codePoints.length] = bytes; // == total UTF-8 byte length
  charStart[codePoints.length] = chars; // == body.length (UTF-16 units)

  return (byteOffset: number): number => {
    if (byteOffset <= 0) return 0;
    if (byteOffset >= bytes) return chars;
    // First code point whose start byte is >= byteOffset (binary search).
    let lo = 0;
    let hi = codePoints.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (byteStart[mid] < byteOffset) lo = mid + 1;
      else hi = mid;
    }
    return charStart[lo];
  };
}

/**
 * Whether the stored leaf offsets plausibly index THIS body — a guard for index-only documents, whose
 * offsets are computed against the live body fetched at index time. In the steady state that body
 * matches what we fetch now, but after a rebuild-from-manifest an index-only item is re-embedded from
 * its ~500-char summary, so the offsets index a fragment far shorter than the freshly fetched full
 * body (or, on a summary fallback, far longer than the summary shown). Either way they don't line up.
 * The leaves tile the body, so the largest end offset should sit near the body's own byte length; if
 * it's wildly off, the offsets belong to a different string and the overlay would be misleading. Vault
 * documents always pass (their body IS the exact string the splitter chunked).
 */
export function offsetsAlignToBody(body: string, leaves: ChunkSpan[]): boolean {
  let bodyBytes = 0;
  for (const ch of body) bodyBytes += utf8Len(ch.codePointAt(0)!);
  if (bodyBytes === 0) return false;
  let maxEnd = 0;
  for (const c of leaves) {
    if (c.kind === "leaf" && c.end_offset != null) maxEnd = Math.max(maxEnd, c.end_offset);
  }
  if (maxEnd === 0) return false;
  // The last leaf should end near the body's end: not far short of it (summary-length offsets over a
  // full body) and not past it (full-body offsets over a summary fallback).
  return maxEnd >= bodyBytes * 0.5 && maxEnd <= bodyBytes * 1.05;
}

/** One rendered segment: a leaf chunk's source text plus the grouping needed to shade it by parent. */
export interface ChunkSegment {
  chunkId: number;
  parentId: number | null;
  ordinal: number;
  text: string;
}

/**
 * Slice the document body into ordered per-leaf segments using the stored byte offsets. Only leaves with
 * real offsets are included (chat-turn chunks predate the offset columns and are skipped). The caller
 * decides how to shade parent groups; `parentId` is carried through for that.
 */
export function segmentByLeaves(body: string, leaves: ChunkSpan[]): ChunkSegment[] {
  const toChar = makeByteToChar(body);
  return leaves
    .filter((c) => c.kind === "leaf" && c.start_offset != null && c.end_offset != null)
    .map((c) => ({
      chunkId: c.id,
      parentId: c.parent_id,
      ordinal: c.ordinal,
      text: body.slice(toChar(c.start_offset as number), toChar(c.end_offset as number)),
    }));
}

/**
 * Assign each segment an alternating shade bucket (0/1) by parent group, in first-appearance order, so
 * adjacent parent groups read as distinct bands. A parentless leaf is its own group.
 */
export function shadeBuckets(segments: ChunkSegment[]): Map<number, 0 | 1> {
  const groupIndex = new Map<number, number>();
  let next = 0;
  const out = new Map<number, 0 | 1>();
  for (const seg of segments) {
    const key = seg.parentId ?? -seg.chunkId; // parentless leaf → unique negative key
    let idx = groupIndex.get(key);
    if (idx === undefined) {
      idx = next++;
      groupIndex.set(key, idx);
    }
    out.set(seg.chunkId, (idx % 2) as 0 | 1);
  }
  return out;
}
