// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The click-to-open detail popup for a calendar event (card 6 follow-up). PM is the calendar
// aggregator, so this surfaces everything the mirror holds — source calendar, when, busy/free,
// location, attendees + organiser, conferencing, recurrence, description, and any linked milestone /
// project / PM flag — plus buttons to open the event in its source calendar or its project. It's an
// in-place floating panel (the pinboard folder-popup pattern): fixed-position, clamped to the
// viewport, dismissed by click-outside or Escape. The description is the one piece of untrusted
// provider text, so it renders ONLY through the sanitising Markdown boundary.
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

import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import type { Calendar, CalendarEvent, Flag, Milestone } from "../../../lib/types";
import { eventFlags, openUrl } from "../../../lib/ipc";
import { formatClock, formatDateLocal } from "../../../lib/format";
import { parseLocal } from "../../../lib/calendar-layout";
import { useRestoreFocus } from "../../../lib/useRestoreFocus";
import { useDepth } from "../../../theme";
import { Button, IconButton } from "../../ui";
import { Markdown } from "../../../lib/markdown";

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
}

const MARGIN = 8;

/** A human "when" line: an all-day date (or range), or a date + start–end clock. */
function whenText(ev: CalendarEvent): string {
  const start = parseLocal(ev.start, ev.all_day);
  if (!start) return ev.start;
  if (ev.all_day) {
    const end = ev.end ? parseLocal(ev.end, true) : null;
    // All-day end is exclusive; show a range only when it spans more than the single start day.
    if (end && end.getTime() - 86_400_000 > start.getTime()) {
      const last = new Date(end.getTime() - 86_400_000);
      return `All day · ${formatDateLocal(start)} – ${formatDateLocal(last)}`;
    }
    return `All day · ${formatDateLocal(start)}`;
  }
  const end = ev.end ? parseLocal(ev.end, false) : null;
  const clock = end ? `${formatClock(start)}–${formatClock(end)}` : formatClock(start);
  return `${formatDateLocal(start)} · ${clock}`;
}

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

/** attendee response → a friendly label (Google/Graph terms). */
function responseLabel(v: string | null): string | null {
  switch (v) {
    case "accepted":
      return "accepted";
    case "declined":
      return "declined";
    case "tentative":
    case "tentativelyAccepted":
      return "maybe";
    case "needsAction":
    case "notResponded":
    case "none":
      return "no reply";
    case "organizer":
      return "organiser";
    default:
      return null;
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

export function CalendarEventPopover({
  event,
  anchor,
  calendar,
  color,
  milestone,
  onClose,
  onOpenProject,
}: Props) {
  const { showPower } = useDepth();
  const panelRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const [flags, setFlags] = useState<Flag[]>([]);

  // Position beside the anchor once measured: clamp horizontally, prefer below, flip above (or clamp
  // to the bottom) when there isn't room. Hidden until placed so it never flashes at 0,0.
  useLayoutEffect(() => {
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
    setPos({ left, top });
  }, [anchor]);

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

  // Move focus onto the panel once it has been PLACED. Keyed on `pos`, which is only set after the
  // measuring layout effect above: until then the panel is `visibility: hidden`, and a hidden
  // element cannot take focus.
  useEffect(() => {
    if (!pos) return;
    const el = panelRef.current;
    // Never steal focus back from a child the user has already reached.
    if (el && !el.contains(document.activeElement)) el.focus();
  }, [pos]);

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

  // Load any PM flags anchored on this event's UID.
  useEffect(() => {
    if (!event.uid) {
      setFlags([]);
      return;
    }
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

  const attendees = event.attendees ?? [];

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
          {event.recurring && (
            <div className="mt-0.5 text-ink4">
              Repeats{event.recurrence_summary ? ` · ${event.recurrence_summary}` : ""}
            </div>
          )}
        </Field>

        {event.show_as && <Field label="Shows as">{showAsLabel(event.show_as)}</Field>}
        {event.location && <Field label="Where">{event.location}</Field>}

        {event.organizer && <Field label="Organiser">{event.organizer}</Field>}

        {attendees.length > 0 && (
          <Field label="Guests">
            <ul className="flex flex-col gap-0.5">
              {attendees.map((a, i) => {
                const resp = responseLabel(a.response);
                return (
                  <li key={a.email ?? a.name ?? i} className="truncate">
                    {a.name ?? a.email ?? "(unknown)"}
                    {a.optional ? " (optional)" : ""}
                    {resp ? <span className="text-ink4"> — {resp}</span> : null}
                  </li>
                );
              })}
            </ul>
          </Field>
        )}

        {event.conference_url && (
          <Field label="Call">
            <button
              type="button"
              onClick={() => void openUrl(event.conference_url!)}
              className="truncate text-left text-accent-text hover:brightness-110"
            >
              Join the call →
            </button>
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
          <div className="mt-1 border-t border-border pt-2 text-xs text-ink2">
            <Markdown>{event.description}</Markdown>
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

      {/* Actions */}
      <div className="flex flex-wrap gap-2 border-t border-border px-3 py-2.5">
        {event.html_link && (
          <Button variant="secondary" size="sm" onClick={() => void openUrl(event.html_link!)}>
            {sourceLabel(calendar?.provider)}
          </Button>
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
      </div>
    </div>
  );
}
