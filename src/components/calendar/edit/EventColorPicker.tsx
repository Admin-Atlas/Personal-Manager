// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The editor's Colour (#884, C13): the event's calendar's colour, or one of Google Calendar's eleven,
// named as Google names them. A button showing the colour and its name opens the list, a listbox
// (C13 critique C2-L1): opening puts focus on the colour chosen, the arrow keys move through every
// one in order (Home and End to the ends), Enter or Space chooses and closes, a click chooses, and
// Tab or Escape closes; focus always goes back to the button. The chosen swatch has a ring and, on
// Google's colours, a check in the colour's own text colour, as Google draws it (C2-L2); a swatch's
// edge is `--ink4`, the contrast floor a control's edge needs, since its fill is anyone's colour. The
// name of the one under focus shows under the list too, since a swatch is only a colour.
//
// A colour Google holds that PM doesn't know stays as the choice, named as that, until another is
// picked. A Google Calendar label sets an event's colour in Google, and PM doesn't change labels, so
// on a labelled event the button says so and stays off (the editor's reason line says why).
// Event colours don't follow the colour-blind axis (F4): the names carry them.

import { useEffect, useRef, useState } from "react";
import type { ColourChoice } from "../../../lib/types";
import { eventColour, PICKER_ORDER } from "../../../theme/eventPalette";
import { Button, Popover, cn } from "../../ui";

interface Props {
  value: ColourChoice;
  /** The colour PM draws the event's calendar in (`colorOf`): "Calendar colour"'s swatch. */
  calendarColour: string;
  /** A Google Calendar label sets its colour: the button says so (and is off). */
  labelled: boolean;
  disabled: boolean;
  onChange: (next: ColourChoice) => void;
}

export const CALENDAR_COLOUR = "Calendar colour";
export const UNKNOWN_COLOUR = "A colour PM doesn't know";
export const SET_BY_LABEL = "Set by its Google Calendar label";

interface Item {
  key: string;
  choice: ColourChoice;
  name: string;
  fill: string;
  /** The text colour that reads on `fill`, for the check: Google's colours only. */
  ink: string | null;
  /** The current colour, which PM can't name: shown, never chosen again. */
  unknown?: true;
}

const isChosen = (item: Item, value: ColourChoice) =>
  item.choice.kind === value.kind &&
  (item.choice.kind !== "colour" || (value.kind === "colour" && item.choice.id === value.id));

/** A swatch: the fill as an inline colour (it's data, never a class), its edge `--ink4`. */
function Swatch({ fill, chosen, ink }: { fill: string; chosen: boolean; ink: string | null }) {
  return (
    <span
      aria-hidden="true"
      style={{ background: fill, color: ink ?? undefined }}
      className={cn(
        "inline-flex h-5 w-5 shrink-0 items-center justify-center rounded-full text-[0.625rem] shadow-[inset_0_0_0_1px_var(--ink4)]",
        chosen && "ring-2 ring-ink ring-offset-2 ring-offset-[var(--panel)]",
      )}
    >
      {chosen && ink ? "✓" : ""}
    </span>
  );
}

export function EventColorPicker({ value, calendarColour, labelled, disabled, onChange }: Props) {
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const optionRefs = useRef<(HTMLDivElement | null)[]>([]);
  const shown = open && !disabled;

  const items: Item[] = [];
  const own = value.kind === "colour" ? eventColour(value.id) : null;
  if (value.kind === "colour" && !own) {
    items.push({
      key: "unknown",
      choice: value,
      name: UNKNOWN_COLOUR,
      fill: calendarColour,
      ink: null,
      unknown: true,
    });
  }
  items.push({
    key: "calendar",
    choice: { kind: "calendar" },
    name: CALENDAR_COLOUR,
    fill: calendarColour,
    ink: null,
  });
  for (const id of PICKER_ORDER) {
    const c = eventColour(id);
    if (c)
      items.push({
        key: id,
        choice: { kind: "colour", id },
        name: c.name,
        fill: c.hex,
        ink: c.ink,
      });
  }
  const chosenAt = Math.max(
    0,
    items.findIndex((i) => isChosen(i, value)),
  );
  const [active, setActive] = useState(chosenAt);
  const current = items[chosenAt];
  const name = labelled ? SET_BY_LABEL : current.name;

  // Off while open (a save on its way): closed for good.
  useEffect(() => {
    if (disabled) setOpen(false);
  }, [disabled]);

  // Opened, focus goes to the colour chosen, after the panel's measuring frame (a hidden option
  // can't take focus). Only on opening: moving through the list mustn't pull focus back.
  useEffect(() => {
    if (!shown) return;
    const t = setTimeout(() => {
      const list = listRef.current;
      (
        list?.querySelector<HTMLElement>('[role="option"][aria-selected="true"]') ??
        list?.querySelector<HTMLElement>('[role="option"]')
      )?.focus();
    }, 0);
    return () => clearTimeout(t);
  }, [shown]);

  /** Close, with focus back on the button. */
  const done = () => {
    setOpen(false);
    triggerRef.current?.focus();
  };

  function move(to: number) {
    const i = Math.max(0, Math.min(items.length - 1, to));
    setActive(i);
    optionRefs.current[i]?.focus();
  }

  function choose(i: number) {
    const item = items[i];
    if (item.unknown) return;
    if (!isChosen(item, value)) onChange(item.choice);
    done();
  }

  return (
    <Popover
      open={shown}
      onOpenChange={(o) => {
        if (o) setActive(chosenAt);
        setOpen(o);
      }}
      escapeClipping
      withinDialog
      ariaLabel="Colours"
      panelClassName="max-w-xs"
      trigger={({ open: expanded, toggle }) => (
        <Button
          ref={triggerRef}
          variant="secondary"
          size="sm"
          aria-label={`Colour: ${name}`}
          aria-haspopup="listbox"
          aria-expanded={expanded}
          disabled={disabled}
          onClick={() => {
            // A click on a Mac leaves focus where it was; the button takes it, so Escape in the
            // list hands it back here.
            triggerRef.current?.focus();
            toggle();
          }}
        >
          <Swatch fill={own?.hex ?? calendarColour} chosen={false} ink={null} />
          <span className="truncate">{name}</span>
        </Button>
      )}
    >
      <div
        ref={listRef}
        role="listbox"
        aria-label="Colour"
        className="grid grid-cols-6 gap-1 p-1"
        onKeyDown={(e) => {
          switch (e.key) {
            case "ArrowRight":
            case "ArrowDown":
              e.preventDefault();
              move(active + 1);
              return;
            case "ArrowLeft":
            case "ArrowUp":
              e.preventDefault();
              move(active - 1);
              return;
            case "Home":
              e.preventDefault();
              move(0);
              return;
            case "End":
              e.preventDefault();
              move(items.length - 1);
              return;
            case "Enter":
            case " ":
              e.preventDefault();
              choose(active);
              return;
            case "Tab":
              // Out of the list to the button it hangs from, not wherever it sits in the dialog.
              e.preventDefault();
              done();
              return;
          }
        }}
      >
        {items.map((item, i) => {
          const chosen = isChosen(item, value);
          return (
            <div
              key={item.key}
              ref={(el) => {
                optionRefs.current[i] = el;
              }}
              role="option"
              aria-selected={chosen}
              aria-disabled={item.unknown ? true : undefined}
              aria-label={item.name}
              title={item.name}
              tabIndex={i === active ? 0 : -1}
              onFocus={() => setActive(i)}
              onClick={() => choose(i)}
              className={cn(
                "inline-flex h-8 w-8 items-center justify-center rounded-full",
                item.unknown ? "cursor-not-allowed" : "cursor-pointer hover:bg-surface",
              )}
            >
              <Swatch fill={item.fill} chosen={chosen} ink={item.ink} />
            </div>
          );
        })}
      </div>
      <p aria-hidden="true" className="px-2 pb-1 text-xs text-ink3">
        {items[active]?.name}
      </p>
    </Popover>
  );
}
