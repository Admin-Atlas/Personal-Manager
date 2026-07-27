// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The `@` suggestion list for the chat composer (#276).
//
// Typing `@marketing` pins that tag for one query. That works whether or not anything suggests it —
// the backend parses the message it was sent — but a feature nobody can discover is a feature
// nobody uses, so the composer offers the tags that exist as soon as an `@` is typed.
//
// This is the first in-INPUT typeahead in the app: every other suggestion surface here either
// filters on the whole field value (CommandPalette) or hangs a native `<datalist>` off it
// (ProjectPicker, the reclassify field). Neither shape works for a token inside a longer message,
// so the ARIA is modelled on CommandPalette — the repo's one fully-ARIA'd combobox — rather than on
// the pickers: the TEXTAREA keeps focus and owns `aria-activedescendant`, and the list is a
// `role="listbox"` of `role="option"` buttons it points at.
//
// Deliberately NOT a Popover: the panel must sit against the composer without stealing focus or
// closing on the next keystroke, and Popover's escape-and-restore-focus behaviour is built for the
// opposite case.

import { useEffect, useRef } from "react";
import type { TagSummary } from "../lib/types";

export function MentionSuggest({
  items,
  active,
  listboxId,
  optionId,
  onPick,
  onHover,
}: {
  items: readonly TagSummary[];
  /** Index of the highlighted option — owned by the composer, which handles the arrow keys. */
  active: number;
  listboxId: string;
  optionId: (index: number) => string;
  onPick: (name: string) => void;
  onHover: (index: number) => void;
}) {
  const listRef = useRef<HTMLDivElement>(null);

  // Keep the highlighted option in view when the arrow keys walk past the visible window.
  useEffect(() => {
    listRef.current
      ?.querySelector(`[data-mention-index="${active}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [active]);

  if (items.length === 0) return null;

  return (
    <div
      ref={listRef}
      role="listbox"
      id={listboxId}
      aria-label="Tags"
      className="absolute bottom-full left-0 z-20 mb-1 max-h-56 w-72 overflow-y-auto rounded-[var(--radius-sm)] border border-border bg-surface py-1 shadow-lg"
    >
      {items.map((t, i) => (
        <button
          key={`${t.kind}:${t.name}`}
          id={optionId(i)}
          role="option"
          aria-selected={i === active}
          data-mention-index={i}
          type="button"
          // `onMouseDown` with preventDefault, not `onClick`: a click would blur the textarea first,
          // and the composer commits its draft on blur — the pick would land after the send.
          onMouseDown={(e) => {
            e.preventDefault();
            onPick(t.name);
          }}
          onMouseMove={() => onHover(i)}
          className={`flex w-full items-center gap-2 px-2 py-1 text-left text-xs ${
            i === active ? "bg-accent-soft text-accent-text" : "text-ink2"
          }`}
        >
          <span className="min-w-0 flex-1 truncate">{t.name}</span>
          {/* Which namespace this is — the only thing here that changes what picking it DOES, and
              the one thing that tells two same-named entries apart. Pinning a project reaches its
              files (including ones merely linked into it); pinning a label reaches whatever carries
              the label.

              The document count is deliberately NOT shown. It still orders the list — a tag you
              actually use comes before one you typed once — but on screen it was a number nobody
              was choosing by, competing for attention with the name and the kind, which are. */}
          <span className="shrink-0 text-[0.625rem] uppercase tracking-wide text-ink4">
            {t.kind === "project" ? "project" : "tag"}
          </span>
        </button>
      ))}
    </div>
  );
}
