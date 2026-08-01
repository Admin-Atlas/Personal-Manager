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
// THE ALLOW-LIST PATTERN: a rule that lands mid-conversion ships carrying the set of sites that
// still break it — a to-do list CI keeps honest, asserted in BOTH directions so a fixed entry fails
// as STALE and the list can only ever shrink. The two conversion allow-lists that shipped with the
// first two rules are gone, not emptied: those rules state their final form directly. The third
// rule (`text-faint`) has a PERMANENT allow-list rather than a shrinking one — the sites on it are
// correct and are meant to stay — but it is asserted the same way, because "these five and no
// others" is exactly as perishable a claim as "these ten, for now".

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

/** Both directions at once: nothing outside the allow-list breaks the rule, and no allow-list entry
 *  has quietly been fixed. Without the second half the list rots into a lie. */
function expectOnly(offenders: string[], allowed: ReadonlySet<string>) {
  expect({
    unexpected: offenders.filter((f) => !allowed.has(f)),
    stale: [...allowed].filter((f) => !offenders.includes(f)),
  }).toEqual({ unexpected: [], stale: [] });
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

// ---------------------------------------------------------------------------------------------
// `faint` is DECORATIVE. It is the one neutral role with no contrast floor at any Contrast level —
// `CONTRAST_SHIFT` (tokens.ts) leaves it alone at `aa`, so it renders between 1.67:1 (terminal /
// dark, on `--surface`) and 3.08:1, well under the 4.5:1 that `contrast.test.ts` holds every text
// role to. It was nevertheless carrying 51 sites of real informational text: section headings,
// empty states, file paths on error screens, citation indices, model provenance, live RAM
// readouts, a stack trace the user is asked to copy.
//
// The token itself did NOT move, and that is the decision, not a shortcut. Lifting `faint` to
// 4.5:1 lands it on top of `ink4` (4.73–4.98:1 after the `aa` boost) and collapses a tier of the
// ink ramp — a worse outcome than the bug. So the text moved instead: the text ramp is
// `ink`→`ink4`, and `faint` now means what its name says.
//
// What legitimately remains is separators, a placeholder glyph, and disabled controls. WCAG 1.4.3
// exempts text that is part of an INACTIVE user-interface component, which is what all four
// `Button` entries are — and `Button`'s base already applies `disabled:opacity-40`, so no token
// choice makes the drawn colour compliant there anyway.
//
// The count is part of the key. A FIFTH `disabled:text-faint` inside Button.tsx is exactly the
// regression this rule is for, and a file-level allow-list would wave it through. Tests are
// excluded from SOURCES, so `Button.test.tsx` quoting the class in a comment does not offend.
// ---------------------------------------------------------------------------------------------

const DECORATIVE_FAINT: ReadonlySet<string> = new Set([
  // The "·" standing in for a day with no events — the empty section above it already says so.
  "src/components/calendar/terminal/TerminalAgenda.tsx ×1",
  // The " · " between key/value pairs; its own parent is already `text-ink4`.
  "src/components/dev/DevRaw.tsx ×1",
  // `disabled:text-faint` on primary / secondary / tertiary / danger.
  "src/components/ui/Button.tsx ×4",
]);

describe("`text-faint` is decorative, never informational text", () => {
  const offenders = SOURCES.filter(([, text]) => text.includes("text-faint")).map(
    ([path, text]) => `${path} ×${text.split("text-faint").length - 1}`,
  );

  it("appears only at the sites that are genuinely decorative or disabled", () => {
    expectOnly(offenders, DECORATIVE_FAINT);
  });
});

// ---------------------------------------------------------------------------------------------
// Every `<label>` labels something. A `<label>` with no `htmlFor`, no wrapped control and no
// spread association is not a weak label — it is announced as a form label for a control that does
// not exist, and the control beside it falls back to its PLACEHOLDER for a name. PM's most
// consequential input announced as "sk-or-… , password", and lost even that on the first keystroke.
//
// 22 of them shipped: 7 were section headings wearing the wrong element, 4 were control rows that
// had not reached `SettingRow`, and 11 were genuine label/control pairs now wired through
// `useFieldA11y`. This is the rule that stops the 23rd — `tsc` cannot see it, because `htmlFor` is
// optional on a `<label>` by definition.
// ---------------------------------------------------------------------------------------------

/** JSX elements that count as "this label wraps its control", so implicit association applies. */
const LABELABLE =
  /<(input|select|textarea|Input|Select|Textarea|Toggle|SegmentedControl|DateField)[\s/>]/;

/** The `<label>`'s children — nesting-aware, so an inner `<label>` cannot close the outer one. */
function elementBody(src: string, afterOpeningTag: number): string {
  let depth = 1;
  let i = afterOpeningTag;
  while (i < src.length) {
    const open = src.indexOf("<label", i);
    const close = src.indexOf("</label>", i);
    if (close === -1) return src.slice(afterOpeningTag);
    if (open !== -1 && open < close) {
      depth++;
      i = open + 6;
    } else {
      depth--;
      if (depth === 0) return src.slice(afterOpeningTag, close);
      i = close + 8;
    }
  }
  return src.slice(afterOpeningTag);
}

describe("every <label> is associated with a control", () => {
  const offenders: string[] = [];
  for (const [path, text] of SOURCES) {
    const lines = text.split("\n");
    for (const m of text.matchAll(/<label[\s>]/g)) {
      const lineNo = text.slice(0, m.index).split("\n").length;
      // Prose in a doc comment writes `<label>` too ("returns the failures as `<label>: <error>`").
      // Skipping comment-leading lines is enough and cannot hide a real one: JSX never begins a
      // line with `*` or `//`. A `<label` sharing a line with a trailing comment would fail here —
      // which is the safe direction for a guard to be wrong in.
      const lead = lines[lineNo - 1].trimStart();
      if (lead.startsWith("//") || lead.startsWith("*") || lead.startsWith("/*")) continue;
      const tag = openingTag(text, m.index);
      const named = /\bhtmlFor\b/.test(tag) || /\{\.\.\./.test(tag);
      const wraps =
        !/\/>$/.test(tag) &&
        (() => {
          const body = elementBody(text, m.index + tag.length);
          return LABELABLE.test(body) || body.includes("{children}");
        })();
      if (!named && !wraps) offenders.push(`${path}:${lineNo}`);
    }
  }

  it("finds the labels it is meant to be scanning", () => {
    // An empty scan passes, so pin that the matcher still sees real ones — the two files that
    // deliberately spread `labelProps` rather than writing `htmlFor` by hand.
    const withLabels = SOURCES.filter(([, t]) => /<label[\s>]/.test(t)).map(([p]) => p);
    expect(withLabels).toContain("src/components/ui/Field.tsx");
    expect(withLabels).toContain("src/components/VaultUnlock.tsx");
  });

  it("has no orphan <label> anywhere in the tree", () => {
    expect(offenders).toEqual([]);
  });
});
