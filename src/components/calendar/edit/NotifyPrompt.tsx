// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The email question on its own, for a write that asks nothing else (#884, C12): a one-off's save or
// delete, or a new event. Google's "Send update emails to guests?" with its Back, Don't send and Send
// as PM draws it: the question as the title, who would get what, two radios with nothing picked, and
// Save (Delete, Create) off until one is. Focus starts on Back, so Enter on open goes back and never
// answers (C12 spec §0.5 row 2). It sends nothing itself: the caller saves with the answer.

import { useRef, useState } from "react";
import { notifyOf, notifyQuestion, type NotifyKind } from "../../../lib/calendarEdit/guests";
import type { GuestNotice, Notify } from "../../../lib/types";
import { Callout, ConfirmDialog } from "../../ui";
import { NotifyChoice } from "./NotifyChoice";

interface Props {
  notice: GuestNotice;
  kind: NotifyKind;
  /** The address Google sends from (the account). */
  from: string | null;
  /** The backend asked: who Google would email isn't what the editor counted. */
  again: boolean;
  onSend: (n: Notify) => void;
  onBack: () => void;
}

const CONFIRM: Record<NotifyKind, string> = { save: "Save", delete: "Delete", create: "Create" };

/** Rendered only while open (the caller mounts it), so every question starts unanswered. */
export function NotifyPrompt({ notice, kind, from, again, onSend, onBack }: Props) {
  const [value, setValue] = useState<Notify | null>(null);
  // Once: a second press before the caller closes it sends nothing (C12 critique C1).
  const sent = useRef(false);
  return (
    <ConfirmDialog
      open
      title={notifyQuestion(notice, kind)}
      cancelLabel="Back"
      confirmLabel={CONFIRM[kind]}
      danger={kind === "delete"}
      confirmDisabled={value === null}
      initialFocus="cancel"
      onConfirm={() => {
        if (value === null || sent.current) return;
        sent.current = true;
        onSend(notifyOf(value));
      }}
      onClose={onBack}
    >
      {/* Neutral: the backend knows only that its numbers differ, not why (C12 critique C2-L6),
          nor whether the user chose before: the editor asks nothing when it counts nobody, so
          this can be the first question. */}
      {again && (
        <Callout tone="warning" size="md" className="mt-1">
          PM checked with Google before saving, and these are the numbers it found.
        </Callout>
      )}
      <NotifyChoice
        notice={notice}
        kind={kind}
        scope={null}
        from={from}
        value={value}
        onChange={setValue}
        disabled={false}
        legend="hidden"
      />
    </ConfirmDialog>
  );
}
