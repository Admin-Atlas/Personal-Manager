// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The click-to-open detail popup for a calendar event (card 6 follow-up). PM is the calendar
// aggregator, so this surfaces everything the mirror holds — source calendar, when, busy/free,
// location, attendees + organiser, conferencing, recurrence, description, and any linked milestone /
// project / PM flag — plus buttons to open the event in its source calendar or its project, and, on
// an event the user is invited to (#884, C14b), "Going?" with Yes, No and Maybe. On a calendar
// shared with the user to view only, the line saying so offers to ask its owner for edit access
// (C20b): an email PM opens in the mail app for the user to send, never one PM sends. It's an
// in-place floating panel (the pinboard folder-popup pattern): fixed-position, clamped to the
// viewport, dismissed by click-outside or Escape. The description is the one piece of untrusted
// provider text, so it renders only through a sanitising boundary: `<Markdown>`, or, for a Google
// description with formatting, `RichDescription` (C19b). Its mail links open the mail app with the
// address alone (`useMailLink`), and a link that would open inside PM's own window does nothing.
//
// A NON-MODAL dialog, and staying one: it takes initial focus and hands focus back on Escape/Close
// (`useRestoreFocus`), but no focus trap and no `aria-modal` — the calendar behind it stays live and
// clicking another event is how you move between them. See the focus block in the body.
//
// Every action here is conditional on the event actually having somewhere to go: "Open in Project"
// only with a linked milestone, the source link only with an `html_link`. There is deliberately no
// "Open in Pinboard" — this popup opens for SYNCED events only (CalendarView routes its two
// first-party overlays, milestones and pinboard entries, straight to their own destination on click),
// so that button pointed at the Pinboard from every event that had nothing to do with it.

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type MutableRefObject,
  type ReactNode,
} from "react";
import type {
  AccessAsk,
  Calendar,
  CalendarEvent,
  Flag,
  Milestone,
  Rsvp,
  RsvpScope,
  WriteOutcome,
} from "../../../lib/types";
import {
  askConnectedText,
  askLabel,
  askText,
  JOIN_FAILED,
  OPEN_FAILED,
} from "../../../lib/calendarEdit/editReasons";
import { responseLabel, type Reply } from "../../../lib/calendarEdit/guests";
import { linkTarget, parseDescription } from "../../../lib/calendarEdit/descriptionHtml";
import { eventFlags, openUrl } from "../../../lib/ipc";
import { useRestoreFocus } from "../../../lib/useRestoreFocus";
import { useDepth } from "../../../theme";
import { eventColour } from "../../../theme/eventPalette";
import { Button, CopyLinkButton, IconButton } from "../../ui";
import { Markdown } from "../../../lib/markdown";
import { RsvpControl } from "../edit/RsvpControl";
import { RichDescription } from "./RichDescription";
import { stopLinkAuxClick, useMailLink } from "./useMailLink";
import { repeatsText, whenText } from "./whenText";

interface Props {
  event: CalendarEvent;
  /** The clicked element's on-screen rect, so the panel opens beside it. */
  anchor: DOMRect;
  /** The owning calendar (name + provider), when resolvable. */
  calendar: Calendar | null;
  /** The calendar's source colour. */
  color: string;
  /** A milestone linked to this event (by iCal UID), for the "Open in Project" action. */
  milestone: Milestone | null;
  onClose: () => void;
  onOpenProject?: (project: string) => void;
  /** What PM may do to the event (#884). Passed only by CalendarView; without it the popover is
   *  read-only, as FocusUpcoming shows it. */
  editing?: PopoverEditing;
  /** Filled with a way to close the panel from outside it (CalendarView's Ctrl+Z), handing focus
   *  back as Escape does. */
  closeRef?: MutableRefObject<(() => void) | null>;
  /** How the repeating event a Google row belongs to repeats, in Google Calendar's words, or null
   *  when PM can't say. Without it the line reads "Repeats" (C9b-2). */
  describeSeries?: (eventId: string) => Promise<string | null>;
}

/** The popover's editing actions. */
export interface PopoverEditing {
  /** Offer Edit (something about the event may change). */
  canEdit: boolean;
  /** Offer Delete. */
  canDelete: boolean;
  /** Delete is "Remove": it takes the user's copy of someone else's event off their calendar
   *  (C14d). */
  removes?: boolean;
  /** Offer Duplicate (C18c): a copy the user can change before it's created, on any calendar. */
  canDuplicate: boolean;
  /** Why the event can't be changed, or a part of it, when that's so; a line under the actions. */
  reason: string | null;
  onEdit: () => void;
  onDelete: () => void;
  onDuplicate: () => void;
  /** Copies the user can reply from, the one shown first (C14b). Empty or absent: no reply strip. */
  rsvp?: readonly PopoverRsvp[];
  /** On a calendar shared with you to view only: whom to ask for edit access, and how (C20b). Shown
   *  with the reason line, so only where there is one. */
  ask?: PopoverAsk | null;
}

/** Asking a view-only calendar's owner for edit access (C20b, plan Q10). */
export interface PopoverAsk {
  owner: string;
  /** The owner is one of the user's own Google accounts in PM: a sentence on how to share it from
   *  there, and no email. */
  connected: boolean;
  /** This account's address, for the words. */
  own: string | null;
  /** The calendar's name, for the words. */
  calendar: string;
  /** Opens the email in the mail app (the backend decides to whom, and what it says). */
  onAsk: () => Promise<AccessAsk>;
}

/** One copy of the event the user can reply from. */
export interface PopoverRsvp {
  /** The copy's row id: a React key. */
  key: string;
  current: Reply | null;
  repeating: boolean;
  forWhom: string | null;
  onReply: (reply: Rsvp, scope: RsvpScope | null) => Promise<WriteOutcome>;
}

const MARGIN = 8;

/** busy/free/tentative/oof/elsewhere → a friendly label. */
function showAsLabel(v: string): string {
  switch (v) {
    case "free":
      return "Free";
    case "tentative":
      return "Tentative";
    case "oof":
      return "Out of office";
    case "elsewhere":
      return "Working elsewhere";
    default:
      return "Busy";
  }
}

/** Where the rest of a description PM's copy cut short is. */
function restOfDescription(provider: string | undefined): string {
  switch (provider) {
    case "google":
      return "The rest is in Google Calendar.";
    case "outlook":
    case "microsoft":
      return "The rest is in Outlook.";
    default:
      return "The rest is in the calendar it comes from.";
  }
}

/** "Open in Google" / "Open in Outlook" / a generic label from the provider. */
function sourceLabel(provider: string | undefined): string {
  switch (provider) {
    case "google":
      return "Open in Google Calendar";
    case "outlook":
    case "microsoft":
      return "Open in Outlook";
    default:
      return "Open in calendar";
  }
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex gap-2 text-xs">
      <span className="w-20 shrink-0 font-mono uppercase tracking-wide text-ink4">{label}</span>
      <div className="min-w-0 flex-1 text-ink2">{children}</div>
    </div>
  );
}

/** The reason line of an event on a calendar shared with you to view only, with the ask for edit
 *  access after it (C20b): an inline button that opens an email in the mail app, and a status line
 *  under it saying what came of that. The status line is there, empty, from the start: a live region
 *  that appears together with its words isn't announced (critique C2-L2). Keyed by the event, so a
 *  re-pointed pop-up never shows the last event's answer. A long address wraps anywhere. */
function AskLine({ reason, ask }: { reason: string; ask: PopoverAsk }) {
  const [asking, setAsking] = useState(false);
  const [said, setSaid] = useState("");
  const onClick = () => {
    // aria-disabled, not disabled, so focus stays on the button while it waits.
    if (asking) return;
    setAsking(true);
    // Emptied first, as the mail links' line is: a second try that ends the same way is said again
    // (a live region whose words don't change announces nothing) and is seen to have happened.
    setSaid("");
    // Anything the command refuses with comes back as the backend's own words.
    void ask
      .onAsk()
      .then(
        (result) => askText(result, ask.calendar),
        (e: unknown) =>
          askText(
            { outcome: "failed", message: typeof e === "string" ? e : String(e) },
            ask.calendar,
          ),
      )
      .then((text) => {
        setSaid(text);
        setAsking(false);
      });
  };
  return (
    <>
      <p>
        {reason}{" "}
        <button
          type="button"
          aria-disabled={asking || undefined}
          onClick={onClick}
          className="border-0 bg-transparent p-0 text-left align-baseline text-accent-text underline decoration-dotted underline-offset-2 transition [overflow-wrap:anywhere] hover:brightness-110 motion-reduce:transition-none"
        >
          {askLabel(ask.owner)}
        </button>
      </p>
      <p role="status" className="text-ink3 [overflow-wrap:anywhere]">
        {said}
      </p>
    </>
  );
}

export function CalendarEventPopover({
  event,
  anchor,
  calendar,
  color,
  milestone,
  onClose,
  onOpenProject,
  editing,
  closeRef,
  describeSeries,
}: Props) {
  const { showPower } = useDepth();
  // The colour the event was given in Google, if any.
  const own = eventColour(event.color_id);
  const panelRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const [flags, setFlags] = useState<Flag[]>([]);
  // How a Google occurrence's repeating event repeats, kept with the event it was asked for.
  const [seriesWords, setSeriesWords] = useState<{ id: string; words: string } | null>(null);
  // Shown only while that event is: the keyboard path re-points this mounted panel at another chip
  // (see the focus block), and the last event's words must never show on the next one, not even
  // for one paint. Until they come, and when they can't, the line reads "Repeats".
  const words = seriesWords?.id === event.id ? seriesWords.words : null;

  // Position beside the anchor once measured: clamp horizontally, prefer below, flip above (or clamp
  // to the bottom) when there isn't room. Hidden until placed so it never flashes at 0,0. Measured
  // again when the repeat line's words or the flags arrive, since either can make the panel taller,
  // and one flipped above its chip would then grow over it; a move only when the numbers change.
  const place = useCallback(() => {
    const el = panelRef.current;
    if (!el) return;
    const w = el.offsetWidth;
    const h = el.offsetHeight;
    const left = Math.max(MARGIN, Math.min(anchor.left, window.innerWidth - w - MARGIN));
    let top = anchor.bottom + 6;
    if (top + h + MARGIN > window.innerHeight) {
      const above = anchor.top - 6 - h;
      top = above >= MARGIN ? above : Math.max(MARGIN, window.innerHeight - h - MARGIN);
    }
    setPos((p) => (p && p.left === left && p.top === top ? p : { left, top }));
  }, [anchor]);
  useLayoutEffect(() => place(), [place, words, flags.length]);
  // And whenever anything else changes its size: the reply strip's occurrence question and its
  // lines come and go inside it (C14 critiques C1-L4, C2-L3). jsdom has no ResizeObserver.
  useEffect(() => {
    const el = panelRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const watch = new ResizeObserver(() => place());
    watch.observe(el);
    return () => watch.disconnect();
  }, [place]);
  const placed = pos !== null;

  // Focus handling for a NON-MODAL dialog. This panel is deliberately not a `Modal`: there is no
  // scrim, the calendar behind it stays live, and clicking a different event dismisses this one and
  // opens that one — so a focus TRAP and `aria-modal` would both be wrong, and this file's own test
  // asserts that they stay absent. What it does owe is the other half: it declares `role="dialog"`
  // and never moved focus into itself, so a keyboard user who opened it with Enter was still on the
  // chip, with the panel rendered LAST in the calendar's DOM — reaching its own Close / "Join the
  // call" buttons meant tabbing through every remaining event in the grid.
  //
  // The opener is keyed to `anchor`, not to mount: the mouse path unmounts the panel between events
  // (the outside-mousedown dismissal below fires first), but the keyboard path re-points the mounted
  // instance at a new chip, and a mount-only capture would hand focus back to the first chip of the
  // session. See `useRestoreFocus`.
  const restoreFocus = useRestoreFocus(true, anchor);

  // Escape and the Close button leave focus nowhere, so they hand it back. An outside click has
  // already moved focus to whatever was clicked, so it deliberately does not.
  const dismiss = useCallback(() => {
    restoreFocus();
    onClose();
  }, [restoreFocus, onClose]);

  // Closed from outside (Ctrl+Z bringing an event back reflows the grid under the panel): focus goes
  // back to the chip if it was in the panel, which is about to go; if it's already back in the grid,
  // it stays where it is.
  useEffect(() => {
    if (!closeRef) return;
    closeRef.current = () => {
      if (panelRef.current?.contains(document.activeElement)) restoreFocus();
      onClose();
    };
    return () => {
      closeRef.current = null;
    };
  }, [closeRef, restoreFocus, onClose]);

  // Move focus onto the panel once it has been PLACED, and again when it's re-pointed at another
  // chip. Keyed on `placed`, which is only true after the measuring layout effect above: until then
  // the panel is `visibility: hidden`, and a hidden element cannot take focus. Never on a later
  // move: the repeat line's words arriving re-place the panel, and focus the user has since taken
  // elsewhere (Tab out to the grid) must stay there.
  useEffect(() => {
    if (!placed) return;
    const el = panelRef.current;
    // Never steal focus back from a child the user has already reached.
    if (el && !el.contains(document.activeElement)) el.focus();
  }, [anchor, placed]);

  // Dismiss on Escape or a click/tap outside the panel.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") dismiss();
    };
    const onDown = (e: MouseEvent) => {
      if (panelRef.current && !panelRef.current.contains(e.target as Node)) onClose();
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("mousedown", onDown);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("mousedown", onDown);
    };
  }, [dismiss, onClose]);

  // Load any PM flags anchored on this event's UID. The last event's go first, whatever this one's
  // read comes to: re-pointed at another chip, a read that fails mustn't leave them up.
  useEffect(() => {
    setFlags([]);
    if (!event.uid) return;
    let alive = true;
    eventFlags(event.uid)
      .then((f) => {
        if (alive) setFlags(f);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [event.uid]);

  // How a Google occurrence's repeating event repeats, in Google Calendar's words: asked once per
  // event shown (C9b-2). A feed's row has its own words and never asks.
  useEffect(() => {
    if (!event.recurring || !event.series_id || !describeSeries) return;
    const id = event.id;
    let alive = true;
    describeSeries(id)
      .then((said) => {
        const text = said?.trim();
        if (alive && text) setSeriesWords({ id, words: text });
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [event.id, event.recurring, event.series_id, describeSeries]);

  const attendees = event.attendees ?? [];

  // A Google description draws its formatting when it has some Google writes (C19b, critique C1-H1);
  // any other keeps the Markdown path, as Outlook's and a feed's always do (DECISIONS 10-10: their
  // "<https://…>" addresses are links there). Read once per description, not per render.
  const google = calendar?.provider === "google";
  const rich = useMemo(() => {
    if (!google || !event.description) return null;
    const parsed = parseDescription(event.description);
    return parsed.markup ? parsed.doc : null;
  }, [google, event.description]);
  const mail = useMailLink(event.id);
  // Join the call pressed and the browser didn't open: said under it, for this event only (a
  // re-pointed pop-up never shows another's), Copy link being beside it.
  const [joinFailed, setJoinFailed] = useState<string | null>(null);
  const joinCall = (link: string) => {
    setJoinFailed(null);
    const on = event.id;
    openUrl(link).catch(() => setJoinFailed(on));
  };
  // "Open in Google Calendar" likewise, said under the actions.
  const [openFailed, setOpenFailed] = useState<string | null>(null);
  const openSource = (link: string) => {
    setOpenFailed(null);
    const on = event.id;
    openUrl(link).catch(() => setOpenFailed(on));
  };
  // A Markdown description's mail links open as the formatted view's do (whose own handler has
  // already taken its own), and a link that would load inside PM's window (a relative one, or one
  // `safeUrl` emptied) does nothing. Web links are `useExternalLinks`', left to it.
  const onDescriptionClick = (e: ReactMouseEvent) => {
    const anchor = e.target instanceof Element ? e.target.closest("a") : null;
    if (anchor === null || e.defaultPrevented) return;
    const href = anchor.getAttribute("href") ?? "";
    if (anchor.target === "_blank" && /^https?:\/\//i.test(href)) return;
    e.preventDefault();
    const target = linkTarget(href);
    if (target !== null && /^mailto:/i.test(target)) mail.open(target);
  };

  // A `role="dialog"` with a blank accessible name is announced as an unnamed dialog, and the
  // heading below would render empty beside it. Every producer of `summary` does already substitute
  // something — Google's parse writes this exact string, the milestone overlay always appends
  // " · <project>", and the pinboard overlay falls back to "deadline" — but that is three unconnected
  // guarantees, one of them in Rust, with nothing pinning them. A fourth source (an ICS import, a new
  // overlay) inherits the naming rule for free by landing here instead.
  const title = event.summary.trim() || "(no title)";

  return (
    <div
      ref={panelRef}
      role="dialog"
      // No `aria-modal`: the calendar behind this stays live and Tab must be able to leave.
      aria-label={title}
      tabIndex={-1}
      // Its own help in the calendar view, on the panel rather than the grid's (C20b). Without
      // `editing` (Focus › Upcoming) the help is the agenda's around it.
      data-help={editing ? "calendar-event" : undefined}
      className="fixed z-50 flex max-h-[75vh] w-[340px] flex-col overflow-hidden rounded-[var(--radius)] border border-border2 bg-panel shadow-2xl focus:outline-none"
      style={{
        left: pos?.left ?? anchor.left,
        top: pos?.top ?? anchor.bottom + 6,
        visibility: pos ? "visible" : "hidden",
      }}
    >
      {/* Header */}
      <div className="flex items-start justify-between gap-2 border-b border-border px-3 py-2.5">
        <div className="min-w-0">
          <h2 className="break-words font-head text-sm font-semibold text-ink">{title}</h2>
          <div className="mt-0.5 flex items-center gap-1.5 text-xs text-ink4">
            <span
              className="inline-block h-2.5 w-2.5 shrink-0 rounded-full"
              style={{ background: color }}
            />
            <span className="truncate">{calendar?.name ?? "Calendar"}</span>
          </div>
        </div>
        <IconButton label="Close" onClick={dismiss} className="shrink-0">
          ×
        </IconButton>
      </div>

      {/* Body */}
      <div className="flex flex-col gap-2 overflow-y-auto px-3 py-3">
        <Field label="When">
          <div>{whenText(event)}</div>
          {event.recurring && <div className="mt-0.5 text-ink4">{words ?? repeatsText(event)}</div>}
        </Field>

        {event.show_as && <Field label="Shows as">{showAsLabel(event.show_as)}</Field>}
        {own && (
          // The colour the event was given in Google, by name: what carries it for anyone who can't
          // tell the fill's hue (the colour-blind axis doesn't recolour it).
          <Field label="Colour">
            <span className="inline-flex items-center gap-1.5">
              <span
                aria-hidden
                className="inline-block h-2.5 w-2.5 shrink-0 rounded-full border border-ink4"
                style={{ background: own.hex }}
              />
              {own.name}
            </span>
          </Field>
        )}
        {event.location && <Field label="Where">{event.location}</Field>}

        {event.organizer && <Field label="Organiser">{event.organizer}</Field>}

        {attendees.length > 0 && (
          <Field label="Guests">
            <ul className="flex flex-col gap-0.5">
              {attendees.map((a, i) => {
                const resp = responseLabel(a.response);
                return (
                  <li key={a.email ?? a.name ?? i} className="flex min-w-0 gap-1">
                    {/* A long address gives way, never the answer after it. */}
                    <span className="truncate" title={a.email ?? undefined}>
                      {a.name ?? a.email ?? "(unknown)"}
                      {a.optional ? " (optional)" : ""}
                    </span>
                    {resp ? <span className="shrink-0 text-ink4">— {resp}</span> : null}
                  </li>
                );
              })}
            </ul>
          </Field>
        )}

        {event.conference_url && (
          <Field label="Call">
            <span className="flex min-w-0 items-center gap-2">
              <button
                type="button"
                onClick={() => joinCall(event.conference_url!)}
                className="truncate text-left text-accent-text hover:brightness-110"
              >
                Join the call →
              </button>
              {/* For joining from another browser, or sending the link on. */}
              <CopyLinkButton
                url={event.conference_url}
                to="the call"
                size="xs"
                className="shrink-0"
              />
            </span>
            {/* Always there (empty, it takes no room), so a screen reader hears it. */}
            <p role="status" className="text-st-due">
              {joinFailed === event.id ? JOIN_FAILED : ""}
            </p>
          </Field>
        )}

        {milestone && (
          <Field label="Project">
            <span className="text-ink">{milestone.project_name}</span>
            {milestone.label ? <span className="text-ink4"> · {milestone.label}</span> : null}
          </Field>
        )}

        {flags.length > 0 && (
          <Field label="Flags">
            <ul className="flex flex-col gap-0.5">
              {flags.map((f) => (
                <li key={f.id} className="truncate text-ink2">
                  {f.type.replace(/-/g, " ")}
                </li>
              ))}
            </ul>
          </Field>
        )}

        {event.description && (
          // It hears clicks on its links only, which the keyboard sends as links do.
          <div
            className="pm-event-desc mt-1 border-t border-border pt-2 text-xs text-ink2"
            onClick={onDescriptionClick}
            // A middle-click is never a click: the Markdown path's links need the guard too.
            onAuxClick={stopLinkAuxClick}
          >
            {rich !== null ? (
              <RichDescription doc={rich} onMail={mail.open} />
            ) : (
              <Markdown plainLines>{event.description}</Markdown>
            )}
            {event.description_cut && (
              <p className="mt-1 text-ink3">{restOfDescription(calendar?.provider)}</p>
            )}
            {/* Always there (empty, it takes no room), so a screen reader hears it when a mail link
                didn't open; selectable, so the address can be copied. */}
            <p role="status" className="text-st-due">
              {mail.failure ?? ""}
            </p>
          </div>
        )}

        {/* Power-depth provenance: status, visibility, recurrence UID, timestamps. */}
        {showPower && (
          <div className="mt-1 flex flex-col gap-1 border-t border-border pt-2 text-[0.6875rem] text-ink4">
            {event.status && <div>Status: {event.status}</div>}
            {event.visibility && <div>Visibility: {event.visibility}</div>}
            {event.uid && <div className="break-all">UID: {event.uid}</div>}
            {event.updated && <div>Updated: {event.updated}</div>}
          </div>
        )}
      </div>

      {/* "Going?", at the foot of the card as Google puts it: one per copy the user can reply
          from (an invitation mirrored on two of their calendars names each). No note here: PM
          doesn't know the note Google holds until the editor reads it (C14 spec Q2). */}
      {editing?.rsvp && editing.rsvp.length > 0 && (
        <div className="flex flex-col gap-2.5 border-t border-border px-3 py-2.5">
          {editing.rsvp.map((copy) => (
            <RsvpControl
              key={copy.key}
              current={copy.current}
              repeating={copy.repeating}
              forWhom={copy.forWhom}
              note={null}
              title={title}
              onReply={(reply, _note, scope) => copy.onReply(reply, scope)}
            />
          ))}
        </div>
      )}

      {/* Actions */}
      <div className="flex flex-wrap gap-2 border-t border-border px-3 py-2.5">
        {event.html_link && (
          <>
            <Button variant="secondary" size="sm" onClick={() => openSource(event.html_link!)}>
              {sourceLabel(calendar?.provider)}
            </Button>
            <CopyLinkButton url={event.html_link} to="this event" />
          </>
        )}
        {milestone && onOpenProject && (
          <Button
            variant="tertiary"
            size="sm"
            onClick={() => onOpenProject(milestone.project_name)}
          >
            Open in Project
          </Button>
        )}
        {/* Duplicate first of the event's own actions, taking the row's push to the right from
            whichever comes first (critique C2-L6). */}
        {editing?.canDuplicate && (
          <Button
            variant="tertiary"
            size="sm"
            className="ml-auto"
            onClick={() => {
              // As Edit below: focus back to the chip, for what opens next to return it there.
              restoreFocus();
              editing.onDuplicate();
            }}
          >
            Duplicate
          </Button>
        )}
        {editing?.canEdit && (
          <Button
            variant="primary"
            size="sm"
            className={editing.canDuplicate ? undefined : "ml-auto"}
            onClick={() => {
              // As Delete below: focus back to the chip, for the editor to return it there.
              restoreFocus();
              editing.onEdit();
            }}
          >
            Edit
          </Button>
        )}
        {editing?.canDelete && (
          <Button
            variant="danger"
            size="sm"
            className={editing.canEdit || editing.canDuplicate ? undefined : "ml-auto"}
            onClick={() => {
              // Back to the event's chip first, so the delete dialog that opens next returns
              // focus there when it closes, rather than to this button, which is about to go.
              restoreFocus();
              editing.onDelete();
            }}
          >
            {editing.removes ? "Remove" : "Delete"}
          </Button>
        )}
      </div>
      {/* Always there with the link (empty, it takes no room), so a screen reader hears it. */}
      {event.html_link && (
        <p
          role="status"
          className={`px-3 text-xs text-st-due${openFailed === event.id ? " pb-2.5" : ""}`}
        >
          {openFailed === event.id ? OPEN_FAILED : ""}
        </p>
      )}
      {editing?.reason && (
        <div className="border-t border-border px-3 py-2 text-[0.6875rem] leading-relaxed text-ink4">
          {editing.ask && !editing.ask.connected ? (
            <AskLine key={event.id} reason={editing.reason} ask={editing.ask} />
          ) : (
            <p>
              {editing.reason}
              {/* The owner is one of the user's own accounts: how to share it from there. */}
              {editing.ask && (
                <span className="[overflow-wrap:anywhere]">
                  {" "}
                  {askConnectedText(editing.ask.owner, editing.ask.own)}
                </span>
              )}
            </p>
          )}
        </div>
      )}
    </div>
  );
}
