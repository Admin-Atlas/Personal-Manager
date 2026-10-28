// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The reply to an invitation (#884, C14b): "Going?" with Yes, No and Maybe, as Google Calendar asks
// it. A reply is sent the moment it's chosen, as Google sends it, and Google tells the organiser,
// so nothing is asked first; an occurrence of a repeating event asks "This event" or "All events"
// before anything goes, with focus on Back so a second Enter can't choose (plan Q5). There is no
// "this and following": a guest can't split someone else's repeating event.
//
// The control says what the reply came to in its own line. An answer that comes after it has gone
// (the popover closed while the reply was on its way), and any answer that isn't a success, is also
// one of the calendar's notices, so it outlives the popover (C14 critique C2-M1). Where PM knows the
// note Google holds (the editor), the note can be added, changed or removed with the reply.
//
// It isn't a field: it never touches an editor's draft, never makes it dirty, and Save never sends
// it. It imports no write wrapper: the parent hands it `onReply`.

import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import type { Rsvp, RsvpScope, WriteOutcome } from "../../../lib/types";
import type { Reply } from "../../../lib/calendarEdit/guests";
import {
  asRsvp,
  NOTE_MAX,
  noteLength,
  replyHeld,
  RSVP_CHOICES,
  rsvpLabel,
  tidyNote,
} from "../../../lib/calendarEdit/rsvp";
import { replyText, type Tone } from "../../../lib/calendarEdit/editReasons";
import { Button, SegmentedControl, Textarea } from "../../ui";
import { noteReply } from "./useEventWrites";

interface Props {
  /** The reply Google holds now: the row's in the popover, the fresh copy's in the editor. */
  current: Reply | null;
  /** An occurrence of a repeating event: a reply asks "This event" or "All events" first. */
  repeating: boolean;
  /** Whose reply, when it isn't plainly the user's own: a calendar's name. */
  forWhom: string | null;
  /** The note Google holds. `null` where PM doesn't know it (the popover): no note box then. */
  note: string | null;
  /** The event's title, for a notice that says what the reply came to after the control has gone. */
  title: string;
  disabled?: boolean;
  /** Send it. The control says what it came to. */
  onReply: (reply: Rsvp, note: string | null, scope: RsvpScope | null) => Promise<WriteOutcome>;
}

/** A line's colour by its tone: the status tokens, already calibrated for text. */
const TONE_CLASS: Record<Tone, string> = {
  ok: "text-ink3",
  warn: "text-st-look",
  error: "text-st-due",
};

/** A button that waits for a reply on its way: unavailable, but never `disabled`, which would drop
 *  its focus. */
const WAITING = "aria-disabled:cursor-not-allowed";

export function RsvpControl({
  current,
  repeating,
  forWhom,
  note,
  title,
  disabled = false,
  onReply,
}: Props) {
  const labelId = useId();
  const stepId = useId();
  const noteId = useId();
  const hintId = useId();
  // The reply just sent, over the `current` it was sent against: it shows until the row says
  // otherwise, so a reload with Google's copy (or a change made in Google) wins (C14 critique C1-L5).
  const [shown, setShown] = useState<{ reply: Rsvp; over: Reply | null } | null>(null);
  // The note just sent, the same way over the `note` it replaced.
  const [sentNote, setSentNote] = useState<{ text: string; over: string | null } | null>(null);
  // An occurrence's question: the reply chosen and the note going with it.
  const [step, setStep] = useState<{ reply: Rsvp; note: string | null } | null>(null);
  const [sending, setSending] = useState(false);
  const [said, setSaid] = useState<{ tone: Tone; text: string } | null>(null);
  const [noteOpen, setNoteOpen] = useState(false);
  const [draft, setDraft] = useState("");
  const sendingRef = useRef(false);
  const mounted = useRef(true);
  const rootRef = useRef<HTMLDivElement>(null);
  const choicesRef = useRef<HTMLDivElement>(null);
  const backRef = useRef<HTMLButtonElement>(null);
  // Where focus goes once the reply buttons or the question are back on screen.
  const focusNext = useRef<"back" | Rsvp | null>(null);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  // The reply sent gives way for good once `current` moves on, whatever to, so a later change in
  // Google back to the answer it was sent over shows too (C1-L5); the note sent the same with `note`.
  useEffect(() => {
    if (shown !== null && shown.over !== current) setShown(null);
  }, [shown, current]);
  useEffect(() => {
    if (sentNote !== null && sentNote.over !== note) setSentNote(null);
  }, [sentNote, note]);

  useLayoutEffect(() => {
    const want = focusNext.current;
    if (want === null) return;
    focusNext.current = null;
    if (want === "back") {
      backRef.current?.focus();
      return;
    }
    const index = RSVP_CHOICES.findIndex((c) => c.value === want);
    choicesRef.current?.querySelectorAll("button")[index]?.focus();
  }, [step]);

  const reply = (shown !== null && shown.over === current ? shown.reply : null) ?? asRsvp(current);
  const noteNow = sentNote !== null && sentNote.over === note ? sentNote.text : note;
  /** Focus is in this control (so it may be moved within it, never taken from elsewhere). */
  const focused = () => rootRef.current?.contains(document.activeElement) ?? false;

  async function send(choice: Rsvp, text: string | null, scope: RsvpScope | null) {
    // One at a time: a second press while one is on its way sends nothing.
    if (sendingRef.current) return;
    sendingRef.current = true;
    setSending(true);
    setSaid(null);
    const against = current;
    const noteAgainst = note;
    let outcome: WriteOutcome;
    try {
      outcome = await onReply(choice, text, scope);
    } catch (e) {
      outcome = { outcome: "failed", message: e instanceof Error ? e.message : String(e) };
    } finally {
      sendingRef.current = false;
    }
    const success = outcome.outcome === "saved" || outcome.outcome === "no_change";
    if (!mounted.current) {
      noteReply(title, outcome, choice, scope);
      return;
    }
    setSending(false);
    setSaid(replyText(outcome, choice, scope));
    if (outcome.outcome === "needs_choice") {
      // The row became an occurrence of a repeating event: ask which ones, and send nothing yet.
      if (focused()) focusNext.current = "back";
      setStep({ reply: choice, note: text });
      return;
    }
    // A reply for all events that some occurrence kept its own answer against, or that PM couldn't
    // read back yet: this one may be among those that kept theirs, so what it holds is left to
    // `current` (the editor sets it from the days named, the popover reads Google's rows as they
    // land), never shown as the reply sent.
    if (replyHeld(outcome, scope, null)) {
      setShown({ reply: choice, over: against });
      if (text !== null) setSentNote({ text: tidyNote(text), over: noteAgainst });
    } else if (!success) {
      noteReply(title, outcome, choice, scope);
    }
    // Sent from the occurrence question (nothing closes it while the reply is on its way): back to
    // the reply buttons, focus on the one chosen if it was on the question.
    if (step !== null) {
      if (focused()) focusNext.current = choice;
      setStep(null);
    }
  }

  /** A reply button pressed: sent at once, or the occurrence question first. */
  function choose(value: Rsvp | "awaiting") {
    if (value === "awaiting" || value === reply) return;
    if (repeating) {
      focusNext.current = "back";
      setStep({ reply: value, note: null });
      return;
    }
    void send(value, null, null);
  }

  function back() {
    if (step === null) return;
    focusNext.current = step.reply;
    setStep(null);
  }

  /** Escape goes back to the reply buttons and stops there, so the popover or the editor stays
   *  open. While a reply is on its way it goes on up: closing then leaves a notice. */
  function onStepKey(e: KeyboardEvent<HTMLDivElement>) {
    if (e.key !== "Escape" || sending) return;
    e.stopPropagation();
    e.preventDefault();
    back();
  }

  function sendNote() {
    // While a reply is on its way Send note is only unavailable (it keeps focus), so it sends
    // nothing, and never swaps the occurrence question that reply was sent from.
    if (sendingRef.current) return;
    // Never so: Send note is disabled without a reply (or with too long a note).
    if (reply === null) return;
    if (repeating) {
      focusNext.current = "back";
      setStep({ reply, note: draft });
      return;
    }
    void send(reply, draft, null);
  }

  const tooLong = noteLength(draft) > NOTE_MAX;
  const removing = tidyNote(draft) === "" && (noteNow ?? "") !== "";
  const label = forWhom ? `Going? Replying for ${forWhom}.` : "Going?";

  return (
    <div ref={rootRef} className="flex flex-col gap-1.5">
      {step === null ? (
        <div className="flex flex-wrap items-center justify-between gap-2">
          <span id={labelId} className="text-xs text-ink2">
            {label}
          </span>
          <div ref={choicesRef}>
            <SegmentedControl<Rsvp | "awaiting">
              aria-labelledby={labelId}
              options={RSVP_CHOICES}
              // With no reply yet nothing is pressed, as in Google.
              value={reply ?? "awaiting"}
              onChange={choose}
              disabled={disabled}
              busy={sending}
            />
          </div>
        </div>
      ) : (
        <div
          role="group"
          aria-labelledby={stepId}
          onKeyDown={onStepKey}
          className="flex flex-wrap items-center gap-1.5"
        >
          <span id={stepId} className="mr-auto text-xs text-ink2">
            Reply {rsvpLabel(step.reply)} to which occurrences?
          </span>
          <Button
            size="xs"
            variant="secondary"
            className={WAITING}
            aria-disabled={sending || undefined}
            disabled={disabled}
            onClick={() => void send(step.reply, step.note, "this")}
          >
            This event
          </Button>
          <Button
            size="xs"
            variant="secondary"
            className={WAITING}
            aria-disabled={sending || undefined}
            disabled={disabled}
            onClick={() => void send(step.reply, step.note, "all")}
          >
            All events
          </Button>
          <Button
            ref={backRef}
            size="xs"
            variant="tertiary"
            className={WAITING}
            aria-disabled={sending || undefined}
            onClick={() => {
              if (!sending) back();
            }}
          >
            Back
          </Button>
        </div>
      )}
      {/* Always mounted, so a screen reader hears what the reply came to. */}
      <p
        role="status"
        className={`text-xs ${said && !sending ? TONE_CLASS[said.tone] : "text-ink3"}`}
      >
        {sending ? "Sending your reply…" : (said?.text ?? "")}
      </p>
      {note !== null &&
        (noteOpen ? (
          <div className="flex flex-col gap-1">
            <label htmlFor={noteId} className="text-xs text-ink2">
              Note
            </label>
            <Textarea
              id={noteId}
              rows={3}
              value={draft}
              aria-describedby={hintId}
              aria-invalid={tooLong || undefined}
              disabled={disabled}
              onChange={(e) => setDraft(e.target.value)}
            />
            <div id={hintId} className="text-xs">
              <p className="text-ink3">Everyone invited can see it.</p>
              {tooLong ? (
                <p className="text-st-due">A note can be up to 1,000 characters.</p>
              ) : reply === null ? (
                <p className="text-ink3">Choose Yes, No or Maybe first.</p>
              ) : null}
            </div>
            <div>
              <Button
                size="xs"
                variant="secondary"
                className={WAITING}
                aria-disabled={sending || undefined}
                disabled={disabled || reply === null || tooLong}
                onClick={sendNote}
              >
                {removing ? "Remove note" : "Send note"}
              </Button>
            </div>
          </div>
        ) : (
          <div>
            <Button
              size="xs"
              variant="tertiary"
              disabled={disabled}
              onClick={() => {
                setDraft(noteNow ?? "");
                setNoteOpen(true);
              }}
            >
              {noteNow ? "Change note" : "Add a note"}
            </Button>
          </div>
        ))}
    </div>
  );
}
