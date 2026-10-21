// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The editor's guest list, like Google Calendar's: add by address or from people already in your
// calendars, mark optional, take off, and the three guest permissions. Changes only the draft; the
// save says whether Google emails anyone.
//
// The box is a combobox in the CommandPalette's model: focus stays in it, the arrow keys move
// through the suggestions under it (in flow, so the editor's scroll never clips them), and nothing
// is picked until ArrowDown, so Enter adds what was typed (C12 critique C2-L3). Its Escape is its
// own while it has something to close or clear, never the editor's. A guest taken off and added back
// is Google's entry again, reply and all, so the save sends nothing for them.

import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type ClipboardEvent,
  type KeyboardEvent,
} from "react";
import {
  addressKey,
  guestSummary,
  isEmail,
  isRoomAddress,
  matchSuggestions,
  REPLY_GLYPH,
  replyOf,
  responseLabel,
  sameAddress,
  splitAddresses,
  type GuestSuggestion,
} from "../../../lib/calendarEdit/guests";
import { problemText } from "../../../lib/calendarEdit/editReasons";
import type { GuestDraft, GuestPerms } from "../../../lib/calendarEdit/eventDraft";
import { Button, IconButton, Input, VisuallyHidden } from "../../ui";

interface Props {
  guests: GuestDraft[];
  perms: GuestPerms;
  suggestions: readonly GuestSuggestion[];
  /** The account saving it: never suggested. */
  me: string | null;
  disabled: boolean;
  /** Google cut the list short: shown, not changeable. */
  omitted: boolean;
  onChange: (guests: GuestDraft[]) => void;
  onPerms: (perms: GuestPerms) => void;
  /** Text left in the box on blur that isn't an address, or null once it's cleared or added: the
   *  editor refuses to save meanwhile (the DateField `onRejectDate` pattern). */
  onReject: (text: string | null) => void;
}

/** How long "Already on the list." stays under the box. */
const NOTE_MS = 4000;

const ALREADY = "Already on the list.";
const CUT_SHORT = "Google didn't send this event's whole guest list, so PM can't change it.";

export function GuestField({
  guests,
  perms,
  suggestions,
  me,
  disabled,
  omitted,
  onChange,
  onPerms,
  onReject,
}: Props) {
  const id = useId();
  const listId = `${id}-suggestions`;
  const rejectId = `${id}-refused`;
  const summaryId = `${id}-summary`;
  const optionId = (i: number) => `${id}-option-${i}`;
  const permId = (p: keyof GuestPerms) => `${id}-${p}`;

  const [text, setTextState] = useState("");
  // The box's text as the handlers read it: a Tab's add and the blur after it are one keypress.
  const textRef = useRef("");
  const setText = (next: string) => {
    textRef.current = next;
    setTextState(next);
  };
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const [note, setNote] = useState<string | null>(null);
  const noteTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [rejected, setRejected] = useState<string | null>(null);
  const rejectedRef = useRef<string | null>(null);
  // Entries taken off, by address: added back, each is Google's entry again.
  const removed = useRef(new Map<string, GuestDraft>());

  useEffect(
    () => () => {
      if (noteTimer.current !== null) clearTimeout(noteTimer.current);
    },
    [],
  );

  const locked = disabled || omitted;
  const offered = useMemo(
    () => (me ? suggestions.filter((s) => !sameAddress(s.email, me)) : suggestions),
    [suggestions, me],
  );
  const candidates = locked
    ? []
    : matchSuggestions(
        offered,
        text,
        guests.map((g) => g.email),
      );
  const showing = open && candidates.length > 0;
  const summary = guestSummary(guests);

  /** Tell the editor what the box refuses, only when that changes. */
  function reject(value: string | null) {
    if (rejectedRef.current === value) return;
    rejectedRef.current = value;
    setRejected(value);
    onReject(value);
  }

  function flash(words: string) {
    if (noteTimer.current !== null) clearTimeout(noteTimer.current);
    setNote(words);
    noteTimer.current = setTimeout(() => {
      noteTimer.current = null;
      setNote(null);
    }, NOTE_MS);
  }

  /** Add each of `people` not already on the list, in one change. */
  function add(people: readonly { email: string; name: string | null }[]) {
    const next = [...guests];
    let repeated = false;
    for (const p of people) {
      const email = p.email.trim();
      if (next.some((g) => sameAddress(g.email, email))) {
        repeated = true;
        continue;
      }
      const back = removed.current.get(addressKey(email));
      next.push(
        back
          ? { ...back }
          : {
              email,
              name: p.name,
              response: null,
              optional: false,
              organizer: false,
              self: false,
              room: isRoomAddress(email),
            },
      );
    }
    if (repeated) flash(ALREADY);
    if (next.length > guests.length) onChange(next);
  }

  /** Add the addresses in `typed` and leave the rest in the box, refused; true when it held any. */
  function commit(typed: string): boolean {
    const pieces = splitAddresses(typed);
    const good = pieces.filter(isEmail);
    const rest = pieces.filter((p) => !isEmail(p)).join(" ");
    if (good.length > 0) add(good.map((email) => ({ email, name: null })));
    if (good.length > 0 || rest === "") setText(rest);
    reject(rest === "" ? null : good.length > 0 ? rest : typed.trim());
    setOpen(false);
    setActive(-1);
    return good.length > 0;
  }

  function pick(s: GuestSuggestion) {
    add([{ email: s.email, name: s.name }]);
    setText("");
    reject(null);
    setOpen(false);
    setActive(-1);
  }

  function remove(g: GuestDraft) {
    removed.current.set(addressKey(g.email), g);
    onChange(guests.filter((x) => x !== g));
  }

  function onKeyDown(e: KeyboardEvent<HTMLInputElement>) {
    const typed = textRef.current;
    switch (e.key) {
      case "ArrowDown":
      case "ArrowUp": {
        const n = candidates.length;
        if (n === 0) return;
        e.preventDefault();
        const down = e.key === "ArrowDown";
        if (!showing) {
          setOpen(true);
          setActive(down ? 0 : n - 1);
        } else {
          setActive((a) => (down ? (a + 1) % n : a <= 0 ? n - 1 : a - 1));
        }
        return;
      }
      case "Enter":
        // Never the editor's: Enter here adds, or says why it can't.
        e.preventDefault();
        if (showing && active >= 0 && active < candidates.length) pick(candidates[active]);
        else if (typed.trim() !== "") commit(typed);
        return;
      case "Tab":
        // Added, then on to the next control: never trapped.
        if (typed.trim() !== "") commit(typed);
        return;
      case ",":
      case ";":
        if (isEmail(typed)) {
          e.preventDefault();
          commit(typed);
        }
        return;
      case "Escape":
        if (showing) {
          e.stopPropagation();
          setOpen(false);
          setActive(-1);
        } else if (typed !== "") {
          e.stopPropagation();
          setText("");
          reject(null);
        }
        return;
    }
  }

  function onPaste(e: ClipboardEvent<HTMLInputElement>) {
    const pieces = splitAddresses(e.clipboardData.getData("text"));
    const good = pieces.filter(isEmail);
    // Nothing to add: an ordinary paste (a name to search by, say).
    if (good.length === 0) return;
    e.preventDefault();
    add(good.map((email) => ({ email, name: null })));
    const rest = pieces.filter((p) => !isEmail(p));
    setText([textRef.current.trim(), ...rest].filter((p) => p !== "").join(" "));
    setOpen(false);
    setActive(-1);
  }

  function onBlur() {
    const typed = textRef.current;
    setOpen(false);
    setActive(-1);
    if (typed.trim() === "") reject(null);
    else commit(typed);
  }

  const fixed = (g: GuestDraft) => g.organizer || g.self;

  return (
    <fieldset
      className="flex min-w-0 flex-col gap-2"
      aria-describedby={summary ? summaryId : undefined}
    >
      <legend className="text-sm text-ink2">Guests</legend>
      {summary && (
        <p id={summaryId} className="text-xs text-ink4">
          {summary}
        </p>
      )}
      <Input
        value={text}
        disabled={locked}
        aria-label="Add guests"
        placeholder="Add guests"
        role="combobox"
        aria-autocomplete="list"
        aria-expanded={showing}
        aria-controls={listId}
        aria-activedescendant={showing && active >= 0 ? optionId(active) : undefined}
        aria-invalid={rejected !== null ? true : undefined}
        aria-describedby={rejected !== null ? rejectId : undefined}
        onChange={(e) => {
          setText(e.target.value);
          setOpen(true);
          setActive(-1);
          if (e.target.value.trim() === "") reject(null);
        }}
        onKeyDown={onKeyDown}
        onPaste={onPaste}
        onBlur={onBlur}
      />
      {/* In flow under the box. Always in the tree, so the box's `aria-controls` names it. */}
      <ul
        role="listbox"
        id={listId}
        aria-label="Suggestions"
        hidden={!showing}
        className="rounded-[var(--radius-sm)] border border-ink4 py-1"
      >
        {candidates.map((s, i) => (
          <li
            key={s.email}
            id={optionId(i)}
            role="option"
            aria-selected={showing && i === active}
            // A press adds it without taking focus from the box.
            onMouseDown={(e) => {
              e.preventDefault();
              pick(s);
            }}
            onMouseMove={() => setActive(i)}
            className={`cursor-pointer px-2 py-1 ${
              showing && i === active ? "bg-accent-soft" : "hover:bg-surface"
            }`}
          >
            <span className="block truncate text-sm text-ink2">{s.name ?? s.email}</span>
            {s.name && <span className="block truncate text-xs text-ink4">{s.email}</span>}
          </li>
        ))}
      </ul>
      {rejected !== null && (
        <p id={rejectId} className="text-xs text-st-due">
          {problemText({ kind: "bad_guest", email: rejected })}
        </p>
      )}
      {/* Always mounted, so "Already on the list." is heard as it appears. */}
      <div role="status" className="text-xs text-ink4">
        {note}
      </div>
      {guests.length > 0 && (
        <ul aria-label="Guests" className="flex flex-col gap-1">
          {guests.map((g) => {
            const who = g.name?.trim() || g.email;
            const reply = replyOf(g.response);
            const said = responseLabel(g.response);
            return (
              <li key={addressKey(g.email)} className="flex min-w-0 items-center gap-2">
                <span aria-hidden="true" className="w-4 shrink-0 text-center text-xs text-ink3">
                  {reply ? REPLY_GLYPH[reply] : ""}
                </span>
                {said && <VisuallyHidden>{said}</VisuallyHidden>}
                <span className="flex min-w-0 flex-1 items-baseline gap-2">
                  <span className="truncate text-sm text-ink2" title={g.email}>
                    {who}
                  </span>
                  {g.name?.trim() && <span className="truncate text-xs text-ink4">{g.email}</span>}
                  {g.organizer && <span className="shrink-0 text-xs text-ink4">Organiser</span>}
                  {g.optional && fixed(g) && (
                    <span className="shrink-0 text-xs text-ink4">Optional</span>
                  )}
                  {(g.room || isRoomAddress(g.email)) && (
                    <span className="shrink-0 text-xs text-ink4">Room</span>
                  )}
                </span>
                {!fixed(g) && (
                  <>
                    {/* One name whatever its state, which aria-pressed carries (C12 critique
                        C2-L2); pressed, it's drawn as the selected choice. */}
                    <Button
                      variant={g.optional ? "secondary" : "tertiary"}
                      size="xs"
                      aria-pressed={g.optional}
                      aria-label={`Optional, ${who}`}
                      disabled={locked}
                      onClick={() =>
                        onChange(guests.map((x) => (x === g ? { ...x, optional: !x.optional } : x)))
                      }
                    >
                      Optional
                    </Button>
                    <IconButton
                      label={`Remove ${who}`}
                      variant="subtle"
                      disabled={locked}
                      onClick={() => remove(g)}
                    >
                      ×
                    </IconButton>
                  </>
                )}
              </li>
            );
          })}
        </ul>
      )}
      <fieldset className="flex flex-col gap-1">
        <legend className="text-xs text-ink3">Guest permissions</legend>
        <span className="flex items-center gap-2 text-xs text-ink2">
          <input
            id={permId("modify")}
            type="checkbox"
            checked={perms.modify}
            disabled={locked}
            // As Google's: guests who may change the event may also invite others and see the list.
            onChange={(e) =>
              onPerms(
                e.target.checked
                  ? { modify: true, invite_others: true, see_others: true }
                  : { ...perms, modify: false },
              )
            }
          />
          <label htmlFor={permId("modify")}>Modify event</label>
        </span>
        <span className="flex items-center gap-2 text-xs text-ink2">
          <input
            id={permId("invite_others")}
            type="checkbox"
            checked={perms.invite_others}
            disabled={locked || perms.modify}
            onChange={(e) => onPerms({ ...perms, invite_others: e.target.checked })}
          />
          <label htmlFor={permId("invite_others")}>Invite others</label>
        </span>
        <span className="flex items-center gap-2 text-xs text-ink2">
          <input
            id={permId("see_others")}
            type="checkbox"
            checked={perms.see_others}
            disabled={locked || perms.modify}
            onChange={(e) => onPerms({ ...perms, see_others: e.target.checked })}
          />
          <label htmlFor={permId("see_others")}>See guest list</label>
        </span>
      </fieldset>
      {omitted && <p className="text-xs text-ink4">{CUT_SHORT}</p>}
    </fieldset>
  );
}
