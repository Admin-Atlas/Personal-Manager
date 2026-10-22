// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The calendar's notices about its own writes (#884): "Deleting “Dentist”. Undo", then how it ended.
// They float over the grid, bottom centre, like the reader's notice, so each mixes its tone into
// `--bg` (see Callout) rather than letting the events behind show through. The newest Undo is also
// Ctrl+Z (⌘Z on a Mac), which its tooltip says. A "this and following" change PM couldn't finish
// keeps its notice, with a button for each thing Google's state allows now, until it's settled.

import { IS_MAC } from "../../../lib/setupGuide";
import { Button, Callout, TONE_MIX, TONE_TOKEN, Tooltip } from "../../ui";
import type { SplitAction } from "../../../lib/types";
import { CALLOUT_TONE, SPLIT_ORDER, splitActionLabel } from "../../../lib/calendarEdit/editReasons";
import { latestUndo, type WriteNotice } from "./useEventWrites";

const UNDO_KEY = IS_MAC ? "⌘Z" : "Ctrl+Z";

interface Props {
  notices: readonly WriteNotice[];
  onUndo: (token: string) => void;
  onDismiss: (id: number) => void;
  /** Settle a change PM couldn't finish (`token`, from its notice), naming its event `title`. */
  onSettle?: (token: string, action: SplitAction, title: string) => void;
}

export function WriteNotices({ notices, onUndo, onDismiss, onSettle }: Props) {
  if (notices.length === 0) return null;
  const newest = latestUndo(notices);
  // With two such notices up, each button names its event, so "Finish" isn't said twice alike.
  const partials = notices.filter((n) => n.split).length;
  return (
    <div className="fixed bottom-4 left-1/2 z-50 flex w-max max-w-md -translate-x-1/2 flex-col gap-2">
      {notices.map((n) => {
        const tone = CALLOUT_TONE[n.tone];
        return (
          <Callout
            key={n.id}
            tone={tone}
            size="md"
            body="ink"
            live
            // Only a notice with settle buttons wraps them under its words; the rest keep × beside
            // the text, as before.
            className={`flex items-center justify-between gap-3 text-ink2 shadow-lg${
              n.split ? " flex-wrap" : ""
            }`}
            style={{
              background: `color-mix(in oklab, var(${TONE_TOKEN[tone]}) ${TONE_MIX.surface}%, var(--bg))`,
            }}
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
                    onClick={() => onUndo(n.undoToken!)}
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
