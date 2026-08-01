// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// One row of a Settings section: a label on the left, the control that changes it on the right.
// Retyped 40 times across 10 files in three visual variants — and because the label was a bare
// `<span>` (or an orphan `<label>` with no `htmlFor`), NOTHING associated it with the control
// beside it. Concretely, before this existed: 13 SegmentedControl groups had no accessible name at
// all (`SegmentedControlProps` had no way to give one), 3 native `<select>`s were unnamed
// comboboxes, and 17 Toggles re-entered their name by hand — 7 of them announcing words that do
// not contain the visible label, which is a WCAG 2.5.3 Label-in-Name failure that breaks speech
// input ("click Models in use" does nothing).
//
// So the contract is: `label` is the ONLY place the text is written, and the association is a
// consequence of writing it. The row mints one id pair through `useFieldA11y` — the same core
// `Field` uses — and hands `controlProps` to the child as a function argument, so the call site
// spreads one object and cannot forget:
//
//   <SettingRow label="Map tab" helpId="settings-map-tab">
//     {(a11y) => <Toggle {...a11y} checked={mapVisible} onChange={setMapVisible} />}
//   </SettingRow>
//
// `aria-labelledby`, not `htmlFor`, is what does the naming — see `Field`'s header for why (most of
// these controls are not labelable elements). The `id` is handed down as well, so the ones that
// ARE labelable get both. Nothing regresses: none of these rows has click-to-focus today either.
//
// The row emits EXACTLY the markup the call sites emit today — no wrapper appears unless the
// content asks for one — so converting a site is a pure a11y change with no pixel to re-approve.
// The items-start/items-center split is driven by whether there IS a description, which is content,
// not an axis: this component takes no Depth input and must never fork layout.
//
// There is NO `className` prop, for the same reason `SectionLabel` has none. An earlier draft
// documented one "for spacing only", and that escape could never have worked: `cn()` is a plain
// joiner, so a caller's `mt-0` would be emitted ALONGSIDE this file's `mt-3`, and Tailwind emits
// margin utilities in ascending order — `mt-3` wins and the caller's intent is silently dead. That
// is not hypothetical; it is why four rows were left hand-written when the rest were converted.
// Spacing is the `spacing` variant, size and weight are the `emphasis` variant, and anything else
// is a new variant here rather than a class string racing this one at the call site.

import type { ReactNode } from "react";
import { cn } from "./cn";
import { useFieldA11y, type FieldA11y } from "./Field";

// SWAP, never layer — one complete margin string per value, and "none" contributes nothing at all.
const SPACING = {
  row: "mt-3",
  none: "",
} as const;

export interface SettingRowProps {
  /** The visible label AND the control's accessible name. Written once, here. */
  label: ReactNode;
  /** A second line under the label. Switches the row to top alignment, and is associated with the
   *  control via `aria-describedby`. */
  description?: ReactNode;
  /** Rendered immediately BEFORE the control on the same line — the inline ResetLink, a status
   *  chip. It shares the control's row, so it never disturbs the label's own layout. */
  aside?: ReactNode;
  /** Registry id for help mode. Lands as `data-help` on the row, because HelpOverlay resolves a
   *  hovered element with `closest("[data-help]")`. */
  helpId?: string;
  /** "default" is the ordinary row label; "strong" is the heavier `font-medium` treatment worn by
   *  the rows whose section is a single control (App lock, Duplicate check, Help mode). */
  emphasis?: "default" | "strong";
  /** Top margin. "row" (default) is the `mt-3` that separates a row from the row above it. "none"
   *  is for a row that is the FIRST child of its own section, where the section's own `pt-4` (or a
   *  card's padding) already supplies the space — adding `mt-3` there is a visible 12px. */
  spacing?: keyof typeof SPACING;
  /** Renders the control, spreading the passed ARIA props onto it so it is named by `label`. */
  children: (controlProps: FieldA11y["controlProps"]) => ReactNode;
}

export function SettingRow({
  label,
  description,
  aside,
  helpId,
  emphasis = "default",
  spacing = "row",
  children,
}: SettingRowProps) {
  const a11y = useFieldA11y({ description });
  const control = children(a11y.controlProps);

  const labelNode = (
    <span
      id={a11y.labelProps.id}
      // Swap, never layer: one complete string per emphasis.
      className={emphasis === "strong" ? "text-sm font-medium text-ink2" : "text-sm text-ink2"}
    >
      {label}
    </span>
  );

  return (
    <div
      data-help={helpId}
      className={cn(
        SPACING[spacing],
        "flex justify-between gap-3",
        description != null ? "items-start" : "items-center",
      )}
    >
      {description != null ? (
        <div className="min-w-0">
          {labelNode}
          <p {...a11y.descriptionProps} className="mt-1 text-xs text-ink4">
            {description}
          </p>
        </div>
      ) : (
        labelNode
      )}
      {aside != null ? (
        <div className="flex items-center gap-2">
          {aside}
          {control}
        </div>
      ) : (
        control
      )}
    </div>
  );
}
