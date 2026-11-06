// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The link notice: what PM says when a link it handed to the OS didn't open (lib/linkFailure.ts), so
// a click on a mail link with no mail app set up isn't a click that does nothing. Mounted once per
// window, in main.tsx, outside App's gates, so it is there on every screen.
//
// Inside the topmost dialog while one is open. Settings is a dialog full of links, and a modal
// dialog shuts the rest of the page out three ways: its scrim takes a press (one on the notice
// closed Settings), its focus trap keeps Tab inside, and `aria-modal` leaves the rest of the page
// out of the accessibility tree in both engines. A notice outside it can't be pressed, tabbed to or
// heard. So the notice portals into the topmost open modal dialog (useDialogLayer's
// `useTopmostModalDialog`), as a Popover with `withinDialog` portals into its own: its × is then the
// dialog's last Tab stop, which the trap's wrap reaches. Still `fixed`, against the window: Modal's
// scrim and dialog have no transform, which would become its containing block, and the dialog's
// scrolling never clips a fixed child. Like that Popover it needs Modal's children to be elements: a
// bare string would make React replace the dialog's text, the notice with it.
//
// Its × hands focus to the dialog when it held it, so focus isn't left on nothing, outside the
// trap. Escape still closes the dialog, not the notice.
//
// Each move makes a new ×, so a dialog opened while the × had focus would restore focus to a × that
// is gone. The × is marked `data-focus-return`: the closing dialog's trap hands the name on, and the
// new × claims it once it is in its new place (useFocusTrap's `claimFocusReturn`), so focus comes
// back to the ×, in the window or in the dialog underneath. The × never holds focus as the notice
// moves: a dialog's trap has taken focus into it a commit before the notice follows, and a dialog
// that closes takes its × with it.
//
// `z-[60]`, the rung HelpOverlay and an escaping Popover use: in the window, over Modal's `z-50`
// scrim and under the right-click menu's `z-[70]`; inside a dialog, over all the dialog holds. What
// shares the rung and comes later in the page (a tooltip, an escaping popover, help mode's card)
// paints over it. Inside a dialog, so does everything in the window above the dialog's scrim (z-50):
// those, the menu, and the reader's missing-document notice, at z-50 and later in the page. It sits
// where that notice and the calendar's write notices sit (bottom centre), and can cover the foot of
// a tall dialog: one press on its × takes it down.
//
// It floats over the page like the reader's notice (lib/reader.tsx), so its tone is mixed into
// `--bg` (see Callout). The live region is always in the page, empty while there is nothing to say,
// so a screen reader hears each new line. It moves when a dialog opens or closes, a new region then:
// a line already up isn't said again. The words stay until dismissed or until the next link click: a
// mail link's carry the address, to be copied.

import { useLayoutEffect, useRef, type MouseEvent } from "react";
import { createPortal } from "react-dom";
import { dismissLinkFailure, useLinkFailure } from "../lib/linkFailure";
import { useTopmostModalDialog } from "../lib/useDialogLayer";
import { claimFocusReturn } from "../lib/useFocusTrap";
import { Callout, TONE_MIX, TONE_TOKEN } from "./ui";

/** The ×'s `data-focus-return` name. */
const RETURN_NAME = "link-notice-dismiss";

export function LinkNotice() {
  const text = useLinkFailure();
  const dialog = useTopmostModalDialog();
  const dismissRef = useRef<HTMLButtonElement>(null);
  // In its new place, the × takes back the focus of a dialog that was opened from it (see header).
  useLayoutEffect(() => {
    if (dismissRef.current) claimFocusReturn(RETURN_NAME, dismissRef.current);
  }, [dialog]);
  const dismiss = (e: MouseEvent<HTMLButtonElement>) => {
    // The × goes with the notice: if it held focus, the dialog takes it (Modal's has
    // `tabIndex={-1}`), so Tab stays in the trap. In the window there is no trap to keep.
    const held = document.activeElement === e.currentTarget;
    dismissLinkFailure();
    if (held && dialog?.isConnected) dialog.focus();
  };
  const notice = (
    <div
      role="status"
      className="pointer-events-none fixed bottom-4 left-1/2 z-[60] w-max max-w-[min(28rem,calc(100vw-2rem))] -translate-x-1/2"
    >
      {text !== null && (
        <Callout
          tone="warning"
          size="md"
          body="ink"
          live={false}
          className="pointer-events-auto flex items-start justify-between gap-3 text-ink2 shadow-lg"
          style={{
            background: `color-mix(in oklab, var(${TONE_TOKEN.warning}) ${TONE_MIX.surface}%, var(--bg))`,
          }}
        >
          <span className="min-w-0 [overflow-wrap:anywhere]">{text}</span>
          <button
            ref={dismissRef}
            type="button"
            onClick={dismiss}
            data-focus-return={RETURN_NAME}
            aria-label="Dismiss"
            className="shrink-0 text-ink4 hover:text-ink"
          >
            ×
          </button>
        </Callout>
      )}
    </div>
  );
  return dialog ? createPortal(notice, dialog) : notice;
}
