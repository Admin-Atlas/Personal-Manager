// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { spaceOf, type BoardSpace } from "./grid";

/** The cells a board's scroller has room for: its own box, less its padding as rendered.
 *
 *  The box, not `clientWidth`: that's the box less whatever scrollbar is showing, so the board's size
 *  would depend on the scrollbars it causes (see `spaceOf`). The padding as rendered, not a constant:
 *  it's in rem, so it grows with Settings' Text size. A fixed 48px (p-6 at the default size) left the
 *  board up to 7px too big for its scroller at Large, where p-6 is 27.6px a side, and the scrollbars
 *  came up at most window sizes though nothing was out of view (#1038). */
export function measureSpace(el: HTMLElement): BoardSpace {
  const box = el.getBoundingClientRect();
  const s = getComputedStyle(el);
  const px = (v: string) => parseFloat(v) || 0;
  return spaceOf(
    box.width - px(s.paddingLeft) - px(s.paddingRight),
    box.height - px(s.paddingTop) - px(s.paddingBottom),
    0,
  );
}
