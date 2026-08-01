// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The design system's rules, asserted against the source tree rather than against a rendered
// component. A render test proves a primitive behaves; only a scan proves the tree still USES it —
// and every defect in this batch was a rule that held in one file and had been retyped in twenty.
//
// ONE file for all of these, deliberately. Three separate investigations each proposed their own
// scanner; six scanners is six things to discover, and a rule nobody can find is a rule nobody
// keeps. Append a `describe` here instead.
//
// HOW IT READS THE TREE: `import.meta.glob(… ?raw)`, not `node:fs`. The frontend tsconfig carries
// no `@types/node` — by design, so a Node API cannot quietly appear in shipped UI code — and adding
// it to enable a test would widen the ambient types of the entire app. Vite already inlines the
// sources at transform time, which needs nothing new.
//
// THE ONE RULE THAT IS NOT HERE: the token-existence guard lives in `scripts/design-tokens.test.mjs`
// instead, and it is not a taste call. Its authority is `@theme inline` in `src/index.css` — the
// block that decides which colour utilities Tailwind emits at all — and Vitest stubs every CSS
// import to an EMPTY STRING (`test.css` defaults to false), through `?raw`, `as: "raw"` and a glob
// alike. A guard that cannot read the file it is about would have had to check a proxy instead. The
// `scripts/**/*.test.mjs` lane is plain Node, is outside the frontend tsconfig, and is already
// collected by the same `just frontend-test` (`check-files-in-place.mjs:141`).
//
// THE ALLOW-LIST PATTERN, and why no rule below uses it any more: each rule shipped carrying the
// set of files that still broke it — a to-do list CI kept honest, asserted in BOTH directions so a
// fixed entry failed as stale and the list could only ever shrink. Both lists are now empty, so
// each rule states its final form directly and there is no longer a line to add yourself to. A new
// rule that lands mid-conversion should bring the two-directional helper back with it rather than
// asserting only that nothing NEW offends, which is how an allow-list rots into a lie.

import { describe, expect, it } from "vitest";

const RAW = import.meta.glob("../**/*.{ts,tsx}", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

/** Repo-relative paths → contents, tests excluded (a test may legitimately quote what it forbids). */
const SOURCES: ReadonlyArray<readonly [string, string]> = Object.entries(RAW)
  .map(([key, text]) => [`src/${key.replace(/^\.\.\//, "")}`, text] as const)
  .filter(([path]) => !/\.test\.tsx?$/.test(path))
  .sort(([a], [b]) => a.localeCompare(b));

function filesContaining(needle: string): string[] {
  return SOURCES.filter(([, text]) => text.includes(needle)).map(([path]) => path);
}

// ---------------------------------------------------------------------------------------------
// The Settings section head. One class string, 27 hand-written copies across 10 files, and 30 of
// the 32 rendered heads were `<label>` elements naming nothing. `ui/SectionLabel.tsx` owns it now.
// ---------------------------------------------------------------------------------------------

const HEAD_RECIPE = "font-mono text-xs font-medium uppercase tracking-wide text-ink3";

// The ten-file allow-list this rule shipped with is GONE, not emptied: all 32 rendered heads now go
// through `SectionLabel`, so the rule states its final form directly. A file that retypes the recipe
// fails on the equality below — there is no longer a line to add yourself to.
describe("the section-head recipe has one home", () => {
  const offenders = filesContaining(HEAD_RECIPE);

  it("is written in SectionLabel.tsx and nowhere else", () => {
    expect(offenders).toEqual(["src/components/ui/SectionLabel.tsx"]);
  });
});

// ---------------------------------------------------------------------------------------------
// Every dialog has an accessible name. `role="dialog"` with neither `aria-labelledby` nor
// `aria-label` is announced as bare "dialog" — which is what 12 of PM's 19 dialogs did, including
// "Remove this data?", "Final confirmation", "Delete <project>" and "Remove this tag?".
//
// The nine-file allow-list this rule shipped with is GONE, not emptied: `ModalProps` is now
// `ModalBaseProps & ({ labelledBy } | { label })`, so an unnamed `<Modal>` no longer compiles and
// the primary enforcement is `tsc`, not this scan. What is left here is the backstop that survives
// the type being loosened — if the union is ever relaxed back to two optionals, the offending call
// site still fails HERE rather than shipping silent. Going through `Dialog` cannot offend at all:
// its `title` is required and it wires the name itself.
// ---------------------------------------------------------------------------------------------

/** The opening `<Modal …>` tag only — brace-depth aware, so a `>` inside an arrow function in an
 *  attribute does not end it early and a `label=` on a CHILD element cannot be mistaken for one. */
function openingTag(src: string, start: number): string {
  let depth = 0;
  for (let i = start; i < src.length; i++) {
    const ch = src[i];
    if (ch === "{") depth++;
    else if (ch === "}") depth--;
    else if (ch === ">" && depth === 0) return src.slice(start, i + 1);
  }
  return src.slice(start);
}

describe("every dialog has an accessible name", () => {
  const offenders = SOURCES.filter(([path, text]) => {
    if (path === "src/components/ui/Modal.tsx" || path === "src/components/ui/Dialog.tsx")
      return false;
    return [...text.matchAll(/<Modal[\s>]/g)].some((m) => {
      const tag = openingTag(text, m.index);
      return !/\blabelledBy\b/.test(tag) && !/\blabel\s*=/.test(tag);
    });
  }).map(([path]) => path);

  it("finds the dialogs it is meant to be scanning", () => {
    // A scan of an empty set passes, and after the conversion nearly every dialog reaches Modal
    // through `Dialog`. Pin that the matcher still sees the ones that remain — `Dialog` itself,
    // and the folder board that names itself with `label`.
    const withModals = SOURCES.filter(([, t]) => /<Modal[\s>]/.test(t)).map(([p]) => p);
    expect(withModals).toContain("src/components/ui/Dialog.tsx");
    expect(withModals).toContain("src/components/PinboardView.tsx");
  });

  it("has no unnamed <Modal> anywhere in the tree", () => {
    expect(offenders).toEqual([]);
  });
});
