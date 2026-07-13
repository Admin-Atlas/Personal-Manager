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

// Whether the stored offsets index the shown body is now decided exactly, upstream: an index-only
// item's `fetch_index_only_body` reports an `aligned` flag from a content-hash identity check
// (commands.rs), and vault documents are aligned by construction (their body IS the string the
// splitter chunked). The old length-ratio heuristic here misjudged both no-offset docs and the
// cloud trim shift, so it was removed in favour of that exact signal.

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
 * Assign each leaf an alternating zebra shade (0/1) in document order, so **every** chunk reads as a
 * distinct band the whole way down. (Shading by parent group instead turns a document dominated by one
 * long heading-section — where every leaf shares a parent — into a single uniform band that looks like
 * the shading simply stops; the zebra doesn't.) Parent grouping is conveyed separately, by a heavier
 * divider at group boundaries — see [`parentGroupStarts`].
 */
export function shadeLeaves(segments: ChunkSegment[]): Map<number, 0 | 1> {
  const out = new Map<number, 0 | 1>();
  segments.forEach((seg, i) => out.set(seg.chunkId, (i % 2) as 0 | 1));
  return out;
}

/**
 * The leaves that begin a new parent group — the parent id differs from the previous leaf's, and a
 * parentless leaf (a single-leaf section) always stands alone. The overlay draws a heavier divider
 * before these so sibling groups stay legible while the zebra marks the individual leaves within them.
 */
export function parentGroupStarts(segments: ChunkSegment[]): Set<number> {
  const starts = new Set<number>();
  let prevParent: number | null = null;
  let first = true;
  for (const seg of segments) {
    // A group start on the first leaf, on any parentless leaf (each stands alone), or whenever the
    // parent id changes from the previous leaf.
    if (first || seg.parentId == null || seg.parentId !== prevParent) starts.add(seg.chunkId);
    prevParent = seg.parentId;
    first = false;
  }
  return starts;
}
