// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The heading over a Settings section. One class string, retyped 27 times across 10 files (once as
// a file-local `SECTION_HEAD` const, 26 times as a literal), and 30 of those 32 rendered heads were
// `<label>` elements with no `htmlFor` — labels naming nothing.
//
// The consequence was not cosmetic. PM's Settings surface has exactly one `<h1>` and then, under
// it, a flat wall of text across ten tabs: screen-reader heading navigation and the rotor find zero
// landmarks, while 29 orphan `<label>`s are announced as form labels for controls that do not
// exist. So this element is a HEADING by default, and a `<label>` only where a label is genuinely
// what is meant.
//
// `as="h2"` is the default because `SettingsView` renders the only `<h1>` and no tab contains an
// `<h2>` — h2 gives an unbroken h1→h2 outline with no level skip. `as="h3"` is for a head already
// nested under a section's own h2 (DevPanel, under DevView's h1). `as="span"` is NOT a style
// choice: `Collapsible` renders its `title` inside a `<button>`, and a heading inside a button is
// invalid HTML that some screen readers flatten and no test in this repo would catch.
//
// `htmlFor` renders a real `<label>` instead of a heading, and is correct in exactly one case in
// the tree: a section that IS one control (AiModelsSettings' "Import AI memory", pointing at its
// textarea). It is a union with `as` because "a heading that is also a label" is not a thing.
//
// NO `className` prop, on purpose. `cn()` is a plain joiner, not tailwind-merge, so a call site's
// `text-sm` and this file's `text-xs` would both survive and stylesheet order would pick the
// winner. A call site that needs a difference gets a VARIANT here — which is also what stops the
// drift this file exists to end (the same role is already worn three different ways in the tree).

import type { ReactNode } from "react";
import { cn } from "./cn";

// The one copy of the recipe. `block` is applied separately so this exact substring is what every
// existing call site holds, and `SectionLabel.test.tsx` can assert it lives nowhere else.
const HEAD = "font-mono text-xs font-medium uppercase tracking-wide text-ink3";

interface SectionLabelBaseProps {
  children: ReactNode;
  /** A trailing affordance on the same line — a ResetLink, an item count, a Button, a StatusChip.
   *  Rendered right-aligned as a sibling of the head. Pass the conditional expression itself (e.g.
   *  `action={dirty && <ResetLink/>}`); a `false` still renders the row, matching the wrapper the
   *  call sites have unconditionally today. */
  action?: ReactNode;
  /** Cross-axis alignment of `action`. "center" (default), or "baseline" so a count sits on the
   *  head's own baseline (LocalAiSettings' two model counts). Only meaningful with `action`. */
  align?: "center" | "baseline";
}

export type SectionLabelProps = SectionLabelBaseProps &
  (
    | { as?: "h2" | "h3" | "span"; htmlFor?: never }
    /** Renders `<label htmlFor>` instead of a heading. ONLY when the section is one control. */
    | { htmlFor: string; as?: never }
  );

export function SectionLabel({
  children,
  action,
  align = "center",
  as = "h2",
  htmlFor,
}: SectionLabelProps) {
  // A `<span>` head sits inline inside a button; every other form is a block in its own right.
  const className = cn(as !== "span" && "block", HEAD);

  let head: ReactNode;
  if (htmlFor != null) {
    head = (
      <label htmlFor={htmlFor} className={className}>
        {children}
      </label>
    );
  } else if (as === "span") {
    head = <span className={className}>{children}</span>;
  } else if (as === "h3") {
    head = <h3 className={className}>{children}</h3>;
  } else {
    head = <h2 className={className}>{children}</h2>;
  }

  if (action === undefined) return head;

  return (
    <div
      className={cn(
        "flex justify-between gap-2",
        align === "baseline" ? "items-baseline" : "items-center",
      )}
    >
      {head}
      {action}
    </div>
  );
}
