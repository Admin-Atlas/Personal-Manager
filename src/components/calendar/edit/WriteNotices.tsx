// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The calendar's notices about its own writes (#884): "Deleting “Dentist”. Undo", then how it ended.
// They float over the grid, bottom centre, like the reader's notice, so each mixes its tone into
// `--bg` (see Callout) rather than letting the events behind show through.

import { Button, Callout, TONE_MIX, TONE_TOKEN, type Tone as CalloutTone } from "../../ui";
import type { Tone } from "../../../lib/calendarEdit/editReasons";
import type { WriteNotice } from "./useEventWrites";

const CALLOUT_TONE: Record<Tone, CalloutTone> = {
  ok: "info",
  warn: "warning",
  error: "danger",
};

interface Props {
  notices: readonly WriteNotice[];
  onUndo: (token: string) => void;
  onDismiss: (id: number) => void;
}

export function WriteNotices({ notices, onUndo, onDismiss }: Props) {
  if (notices.length === 0) return null;
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
            className="flex items-center justify-between gap-3 text-ink2 shadow-lg"
            style={{
              background: `color-mix(in oklab, var(${TONE_TOKEN[tone]}) ${TONE_MIX.surface}%, var(--bg))`,
            }}
          >
            <span>{n.text}</span>
            <span className="flex shrink-0 items-center gap-2">
              {n.undoToken && (
                <Button
                  variant="secondary"
                  size="xs"
                  aria-label={n.undoLabel}
                  onClick={() => onUndo(n.undoToken!)}
                >
                  Undo
                </Button>
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
