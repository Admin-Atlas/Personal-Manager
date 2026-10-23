// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The calendar's notices about its own writes (#884): "Deleting “Dentist”. Undo", then how it ended;
// after a move (C16), where it is now, with an Undo.
// They float over the grid, bottom centre, like the reader's notice, so each mixes its tone into
// `--bg` (see Callout) rather than letting the events behind show through. The newest Undo is also
// Ctrl+Z (⌘Z on a Mac), which its tooltip says. A "this and following" change PM couldn't finish
// keeps its notice, with a button for each thing Google's state allows now, until it's settled; a
// new event PM can't tell was made keeps its Check again. After a "this and following" change
// lands, a milestone linked to it is offered a move to the new part (Move, or Keep it there), then
// Move back for a few seconds; Ctrl+Z never moves a milestone.
//
// They sit under an open dialog's scrim (z-50), so a notice that never goes can't cover an editor's
// Cancel and Save on a short window, outside its focus trap: the dialog is what the keyboard is in
// until it closes. And strictly over the floating briefing (z-[45], later in the page, so a tie
// would paint it on top) and the docked reader (z-40), or an Undo or a Check again could sit
// unseen and unclickable under a briefing parked at the foot of the window.

import type { FocusEvent } from "react";
import { IS_MAC } from "../../../lib/setupGuide";
import { Button, Callout, TONE_MIX, TONE_TOKEN, Tooltip } from "../../ui";
import type { SplitAction } from "../../../lib/types";
import {
  CALLOUT_TONE,
  keepLabel,
  moveBackLabel,
  moveLabel,
  SPLIT_ORDER,
  splitActionLabel,
} from "../../../lib/calendarEdit/editReasons";
import { latestUndo, type UndoKind, type WriteNotice } from "./useEventWrites";

const UNDO_KEY = IS_MAC ? "⌘Z" : "Ctrl+Z";

/** A move's button while it's on its way (a moment: it changes PM's own milestone only): it keeps
 *  focus and says it's unavailable, and the pointer says it can't be pressed. */
const WAITING = "aria-disabled:cursor-not-allowed";

interface Props {
  notices: readonly WriteNotice[];
  /** Press an Undo: a delete's (`kind` "delete") or a move's (C16a). */
  onUndo: (token: string, kind: UndoKind) => void;
  onDismiss: (id: number) => void;
  /** Settle a change PM couldn't finish (`token`, from its notice), naming its event `title`. */
  onSettle?: (token: string, action: SplitAction, title: string) => void;
  /** Check again whether a new event PM may already have made was (the notice `id`). */
  onCheckCreate?: (id: number) => void;
  /** Move the milestone a notice offers to the new part, or back (C9b-2). */
  onMove?: (noticeId: number) => void;
  onMoveBack?: (noticeId: number) => void;
  /** The pointer or focus came onto a notice with a move, or left it: Move back waits meanwhile. */
  onHold?: (noticeId: number, by: "pointer" | "focus", on: boolean) => void;
}

export function WriteNotices({
  notices,
  onUndo,
  onDismiss,
  onSettle,
  onCheckCreate,
  onMove,
  onMoveBack,
  onHold,
}: Props) {
  if (notices.length === 0) return null;
  const newest = latestUndo(notices)?.token;
  // With two such notices up, each button names its event, so "Finish" isn't said twice alike.
  const partials = notices.filter((n) => n.split).length;
  const creates = notices.filter((n) => n.create).length;
  return (
    <div className="fixed bottom-4 left-1/2 z-[48] flex w-max max-w-md -translate-x-1/2 flex-col gap-2">
      {notices.map((n) => {
        const tone = CALLOUT_TONE[n.tone];
        // A milestone's move, offered or just made: one button that goes Move → Move back in place,
        // so focus on it stays put, and does nothing while either is on its way (aria-disabled,
        // never disabled, which would drop focus).
        const milestone = n.move ?? n.moveBack;
        const press = n.move ? onMove : onMoveBack;
        const hold =
          milestone && onHold
            ? {
                onPointerEnter: () => onHold(n.id, "pointer", true),
                onPointerLeave: () => onHold(n.id, "pointer", false),
                onFocus: () => onHold(n.id, "focus", true),
                onBlur: (e: FocusEvent<HTMLElement>) => {
                  if (!e.currentTarget.contains(e.relatedTarget as Node | null)) {
                    onHold(n.id, "focus", false);
                  }
                },
              }
            : {};
        return (
          <Callout
            key={n.id}
            tone={tone}
            size="md"
            body="ink"
            live
            // Only a notice with buttons beside × wraps them under its words; the rest keep ×
            // beside the text, as before.
            className={`flex items-center justify-between gap-3 text-ink2 shadow-lg${
              n.split || n.create || milestone ? " flex-wrap" : ""
            }`}
            style={{
              background: `color-mix(in oklab, var(${TONE_TOKEN[tone]}) ${TONE_MIX.surface}%, var(--bg))`,
            }}
            {...hold}
          >
            <span>{n.text}</span>
            <span className="flex shrink-0 items-center gap-2">
              {n.split &&
                onSettle &&
                SPLIT_ORDER.filter((a) => n.split!.offers.includes(a)).map((action) => {
                  const label = splitActionLabel(action);
                  return (
                    <Button
                      key={action}
                      variant="secondary"
                      size="xs"
                      aria-label={
                        partials > 1
                          ? `${label} (“${n.split!.title.trim() || "(no title)"}”)`
                          : undefined
                      }
                      disabled={n.settling}
                      onClick={() => onSettle(n.split!.token, action, n.split!.title)}
                    >
                      {label}
                    </Button>
                  );
                })}
              {n.create && onCheckCreate && (
                <Button
                  variant="secondary"
                  size="xs"
                  aria-label={
                    creates > 1
                      ? `Check again (“${n.create.sent.draft.summary.trim() || "(no title)"}”)`
                      : undefined
                  }
                  disabled={n.checking}
                  onClick={() => onCheckCreate(n.id)}
                >
                  Check again
                </Button>
              )}
              {milestone && press && (
                <Button
                  variant="secondary"
                  size="xs"
                  className={WAITING}
                  aria-label={n.move ? moveLabel(n.move) : moveBackLabel(milestone)}
                  aria-disabled={n.moving || undefined}
                  // A double-click on Move, or Enter held on it: the second click, or a repeat,
                  // lands here once it's Move back, and mustn't undo the move it made. The store
                  // also waits a moment before Move back takes a press, for a second Enter.
                  onKeyDown={(e) => {
                    if (e.repeat && e.key === "Enter") e.preventDefault();
                  }}
                  onClick={(e) => {
                    if (n.moving || (!n.move && e.detail > 1)) return;
                    press(n.id);
                  }}
                >
                  {n.move ? "Move" : "Move back"}
                </Button>
              )}
              {/* Saying no out loud, so × isn't the only way: nothing is sent. */}
              {n.move && onMove && (
                <Button
                  variant="tertiary"
                  size="xs"
                  className={WAITING}
                  aria-label={keepLabel(n.move)}
                  aria-disabled={n.moving || undefined}
                  onClick={() => {
                    if (!n.moving) onDismiss(n.id);
                  }}
                >
                  Keep it there
                </Button>
              )}
              {/* Every Undo is the same Tooltip > Button, so when the newest goes and an older one
                  becomes it, only its label changes: a focused button isn't swapped for a new one. */}
              {n.undoToken && (
                <Tooltip
                  label={n.undoToken === newest ? `Undo · ${UNDO_KEY}` : (n.undoLabel ?? "Undo")}
                >
                  <Button
                    variant="secondary"
                    size="xs"
                    aria-label={n.undoLabel}
                    aria-keyshortcuts={n.undoToken === newest ? "Control+Z Meta+Z" : undefined}
                    onClick={() => onUndo(n.undoToken!, n.undoKind ?? "delete")}
                  >
                    Undo
                  </Button>
                </Tooltip>
              )}
              <button
                type="button"
                onClick={() => onDismiss(n.id)}
                aria-label="Dismiss"
                className="text-ink4 hover:text-ink"
              >
                ×
              </button>
            </span>
          </Callout>
        );
      })}
    </div>
  );
}
