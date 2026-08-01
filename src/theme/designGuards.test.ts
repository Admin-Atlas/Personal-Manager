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
// THE ALLOW-LIST PATTERN: each rule below carries the set of files that still break it. That set is
// a to-do list CI keeps honest, and it may only ever SHRINK. A new file breaking the rule fails
// immediately; an entry that has been fixed fails as stale. Converting a file means deleting its
// line, which is the smallest possible reminder that the conversion is not finished.

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
function expectOnly(offenders: string[], allowed: Set<string>) {
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

/** Files still hand-writing the section-head recipe, pending the settings-markup conversion. */
const UNCONVERTED_HEADS = new Set([
  "src/components/BackupSettings.tsx",
  "src/components/ConnectorsSettings.tsx",
  "src/components/SettingsView.tsx",
  "src/components/dev/DevPanel.tsx",
  "src/components/localai/LocalAiSettings.tsx",
  "src/components/settings/AccessibilitySettings.tsx",
  "src/components/settings/AiModelsSettings.tsx",
  "src/components/settings/DeveloperSettings.tsx",
  "src/components/settings/GeneralSettings.tsx",
  "src/components/settings/SearchSettings.tsx",
]);

describe("the section-head recipe has one home", () => {
  const offenders = filesContaining(HEAD_RECIPE);

  it("is written in SectionLabel.tsx", () => {
    expect(offenders).toContain("src/components/ui/SectionLabel.tsx");
  });

  it("is written nowhere else except the files still awaiting conversion", () => {
    expectOnly(
      offenders.filter((f) => f !== "src/components/ui/SectionLabel.tsx"),
      UNCONVERTED_HEADS,
    );
  });
});

// ---------------------------------------------------------------------------------------------
// Every dialog has an accessible name. `role="dialog"` with neither `aria-labelledby` nor
// `aria-label` is announced as bare "dialog" — which is what 12 of PM's 19 dialogs did, including
// "Remove this data?", "Final confirmation", "Delete <project>" and "Remove this tag?".
//
// This is the ratchet standing in for a required-prop union: `ModalProps` cannot demand a name
// until every existing call site has one, so until then a NEW unnamed `<Modal>` fails HERE. Going
// through `Dialog` needs no entry — its `title` is required and it wires the name itself.
// ---------------------------------------------------------------------------------------------

/** Files still opening a `<Modal>` with neither `labelledBy` nor `label`. */
const UNNAMED_DIALOGS = new Set([
  "src/components/ContextMeter.tsx",
  "src/components/DeleteProjectDialog.tsx",
  "src/components/MergeProjectDialog.tsx",
  "src/components/PinboardView.tsx",
  "src/components/RebuildProgress.tsx",
  "src/components/RemovePmData.tsx",
  "src/components/TeachPreferences.tsx",
  "src/components/TeachTags.tsx",
  "src/components/TeachView.tsx",
]);

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
    // A scan of an empty set passes. Pin that the matcher still sees PM's dialogs at all.
    const withModals = SOURCES.filter(([, t]) => /<Modal[\s>]/.test(t)).length;
    expect(withModals).toBeGreaterThan(10);
  });

  it("has no unnamed <Modal> outside the known, shrinking allow-list", () => {
    expectOnly(offenders, UNNAMED_DIALOGS);
  });
});
