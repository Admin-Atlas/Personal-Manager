// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Scroll a textarea so the caret is visible after PM has moved it itself.
//
// A browser reveals the caret for you on an ordinary keystroke. It does NOT do so when a script
// assigns `selectionStart`, and the pinboard note assigns it constantly: pressing Enter inside a list
// is `preventDefault` → rewrite the text → put the caret on the new line, and Tab, the formatting
// buttons and undo/redo all do the same. On a note card, which is a few lines tall, that meant the
// bullet you had just created was below the fold and you typed into a line you could not see.
//
// `scrollTop` rather than `scrollIntoView`: the latter scrolls every ancestor as well, so revealing a
// line inside a note would also scroll the pinboard under it.
//
// The measurement is deliberately not "which line is the caret on, times the line height". A note
// card is narrow, so lines wrap, and a wrapped line makes that arithmetic wrong by however many rows
// it took. Instead the textarea is asked to measure itself: set its value to the text UP TO the
// caret, read `scrollHeight` (the height that text occupies at this exact width, wrapping and all),
// then put the value back. It costs one synchronous layout on a keypress and is exact.

/** How much room to leave below the caret's line, as a multiple of the line height. A caret pinned
 *  exactly to the bottom edge reads as "the box ends here" — a line of slack shows there is room to
 *  keep going, which is the thing being asked for. */
const PAD_LINES = 1;

export interface CaretRevealGeometry {
  /** Where the box is scrolled to now. */
  scrollTop: number;
  /** The visible height of the box. */
  clientHeight: number;
  /** Distance from the top of the CONTENT to the bottom of the caret's line. */
  caretBottom: number;
  /** One line's height, used both to find the top of the caret's line and to size the padding. */
  lineHeight: number;
  /** The largest scrollTop the box can actually reach. */
  maxScrollTop: number;
}

/**
 * The scroll offset that brings the caret's line into view, or the current one when it already is.
 *
 * Pure, so the arithmetic that decides what you can see is testable without a DOM. Scrolls the
 * minimum needed in either direction — a caret above the fold pulls the view up to its line, one
 * below pushes it down just far enough, and a caret already comfortably inside moves nothing, so
 * this can be called after every edit without fighting the person scrolling.
 */
export function nextScrollTop(geom: CaretRevealGeometry): number {
  const { scrollTop, clientHeight, caretBottom, lineHeight, maxScrollTop } = geom;
  const pad = lineHeight * PAD_LINES;
  const caretTop = caretBottom - lineHeight;

  const clamp = (v: number) => Math.max(0, Math.min(v, Math.max(0, maxScrollTop)));

  // Below the fold — scroll down until the line plus its slack is inside. Clamped, so a caret on the
  // very last line asks for padding that doesn't exist and simply lands at the bottom.
  if (caretBottom + pad > scrollTop + clientHeight) {
    return clamp(caretBottom + pad - clientHeight);
  }
  // Above it — scroll up to the line, again with slack, so it isn't flush against the top edge.
  if (caretTop - pad < scrollTop) return clamp(caretTop - pad);
  return scrollTop;
}

/**
 * Reveal the caret in `el`, if it isn't already visible.
 *
 * Restores the element's own value and selection before returning, so the swap used to measure is
 * never observable — it happens inside one synchronous block, well before the frame is painted, and
 * a controlled React input re-renders with the same string it already had.
 */
export function revealCaret(el: HTMLTextAreaElement): void {
  const caret = el.selectionEnd;
  if (caret == null) return;

  const value = el.value;
  const selectionStart = el.selectionStart;
  const before = value.slice(0, caret);

  // A trailing newline gives an empty last line, which some engines measure as no line at all — the
  // exact case here, since the caret usually sits at the start of a line just created. The sentinel
  // forces that line to be counted; it is removed again two statements later.
  el.value = before.endsWith("\n") ? `${before} ` : before;
  const caretBottom = el.scrollHeight;
  el.value = value;
  el.setSelectionRange(selectionStart, caret);

  const style = getComputedStyle(el);
  const parsed = parseFloat(style.lineHeight);
  // `line-height: normal` parses as NaN. Fall back to a conventional multiple of the font size
  // rather than to 0, which would make the padding vanish and the "is it above the fold" test read
  // every caret as already visible.
  const lineHeight = Number.isFinite(parsed) ? parsed : parseFloat(style.fontSize) * 1.2 || 16;

  el.scrollTop = nextScrollTop({
    scrollTop: el.scrollTop,
    clientHeight: el.clientHeight,
    caretBottom,
    lineHeight,
    maxScrollTop: el.scrollHeight - el.clientHeight,
  });
}
