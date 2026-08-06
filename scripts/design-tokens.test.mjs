// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The token system's own guard: every colour utility and every `var(--…)` in `src/` names something
// that actually exists.
//
// WHY THIS EXISTS. Tailwind v4 emits NOTHING for an unrecognised utility — no warning, no build
// failure, no runtime error, no console noise. `text-ink1` sat on the duplicate-document title in
// `DuplicatesPanel` and `hover:bg-bg2` on the radio rows in `DeleteProjectDialog`. The first was
// invisible (preflight's `button { color: inherit }` happened to paint the intended colour anyway,
// so it would have broken the day any wrapper set a colour); the second silently removed the hover
// highlight from a row that still said `cursor-pointer`. Neither could be seen in a diff, a build
// log or a screenshot. A token name one character off a real one is indistinguishable from a
// working one until someone looks — which is the definition of a rule that needs a machine.
//
// The two edits that motivated this are one word each. The rule is the deliverable.
//
// WHY IT LIVES IN scripts/ AND NOT src/theme/designGuards.test.ts. The authority on which colour
// utilities exist is the `@theme inline` block in `src/index.css`; anything else is a proxy. Vitest
// stubs CSS imports to an empty string (`test.css` defaults to false) — verified through `?raw`,
// `as: "raw"` and `import.meta.glob` alike — so a test inside `src/` cannot read that block at all,
// and `node:fs` is unavailable there because the frontend tsconfig deliberately carries no
// `@types/node`. This lane is plain Node, sits outside that tsconfig, and is already collected by
// the same `just frontend-test` run (registered at `check-files-in-place.mjs:141`), so it needs no
// justfile recipe, no `pr.yml` step and no ci-membership row.
//
// WHY NOT ESLINT. `eslint-plugin-tailwindcss`'s `no-custom-classname` is not v4-ready — v4 has no JS
// config to introspect, the theme lives in CSS — and it would add a devDependency for a job that is
// ~60 lines here. That is the wrong trade against the repo's lean-deps rule (I-18).
//
// SCOPE, deliberately. Only PM's OWN role namespace is checked. `text-stone-400` and `bg-red-500`
// still pass, because the default Tailwind palette stays available through the reskin transition
// (see the `@theme` comment in index.css); banning it is a separate and much larger decision.
// Inside the namespace where this bug class lives, the check is exact: `ink5`, `border3`,
// `st-urgent` and `accent-softer` all fail here.

import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";

import { DENSITY_VARS, themeVars } from "../src/theme/tokens.ts";
import { ACCENTS, MONO_ACCENT } from "../src/theme/profiles.ts";

const repoRoot = fileURLToPath(new URL("..", import.meta.url));
const CSS = readFileSync(join(repoRoot, "src/index.css"), "utf8").replace(/\r\n/g, "\n");

/** Every `.ts`/`.tsx` under `src/`, tests excluded — a test may legitimately quote what it forbids. */
function sources(dir = join(repoRoot, "src")) {
  const out = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) out.push(...sources(full));
    else if (/\.tsx?$/.test(entry.name) && !/\.test\.tsx?$/.test(entry.name))
      out.push([relative(repoRoot, full).replace(/\\/g, "/"), readFileSync(full, "utf8")]);
  }
  return out;
}
const SOURCES = sources().sort(([a], [b]) => a.localeCompare(b));

// ---------------------------------------------------------------------------------------------
// The two authorities: what Tailwind generates utilities for, and what gets written at runtime.
// ---------------------------------------------------------------------------------------------

const themeBlock = /@theme inline\s*\{([\s\S]*?)\n\}/.exec(CSS)?.[1] ?? "";
const rootBlock = /:root\s*\{([\s\S]*?)\n\}/.exec(CSS)?.[1] ?? "";

/** `--color-X: var(--Y)` → X ↦ Y. X is the utility suffix (`text-X`); Y is the property it reads. */
const THEME_COLORS = new Map(
  [...themeBlock.matchAll(/--color-([\w-]+)\s*:\s*var\(\s*(--[\w-]+)\s*\)/g)].map((m) => [
    m[1],
    m[2],
  ]),
);

/** The custom properties the bootstrap `:root` declares (first paint, before React mounts). */
const ROOT_PROPS = new Set([...rootBlock.matchAll(/^\s*(--[\w-]+)\s*:/gm)].map((m) => m[1]));

/** Everything `applyTheme` writes onto <html> at runtime. Taken from the code that WRITES it, not a
 *  regex over it, so a renamed token is caught by construction. Both `themeVars` branches are
 *  sampled: the `mono` Eigengrau sentinel writes its accent roles down a separate path. */
const RUNTIME_PROPS = new Set([
  ...Object.keys(themeVars("editorial", "dark", ACCENTS.editorial[0])),
  ...Object.keys(themeVars("terminal", "light", MONO_ACCENT)),
  ...Object.keys(DENSITY_VARS.standard),
  ...Object.keys(DENSITY_VARS.comfortable),
  "--font-scale", // applyTheme sets this one directly, outside both maps
]);

// ---------------------------------------------------------------------------------------------
// Rule 1 — every colour utility resolves to a token `@theme inline` defines.
// ---------------------------------------------------------------------------------------------

/** A colour name's family: its first dash-segment with trailing digits dropped
 *  (`ink2` → `ink`, `st-due` → `st`, `accent-soft` → `accent`). */
function family(name) {
  return name.split("-")[0].replace(/\d+$/, "");
}

/** The families PM owns, derived from the theme itself rather than hand-listed. Whole-SEGMENT
 *  comparison is what keeps this free of false positives: `text-start` has family `start`, not
 *  `st`, and `bg-blend-multiply` has family `blend`, so neither is read as a token reference. */
const PM_FAMILIES = new Set([...THEME_COLORS.keys()].map(family));

/** Utility prefixes that take a colour. Longest-first so `text-` can never be matched as `to-`. */
const COLOR_PREFIXES = [
  "placeholder",
  "decoration",
  "divide",
  "outline",
  "shadow",
  "stroke",
  "accent",
  "border",
  "caret",
  "ring",
  "fill",
  "from",
  "text",
  "via",
  "bg",
  "to",
].sort((a, b) => b.length - a.length);

const CLASS_TOKEN = new RegExp(
  `^(?:[\\w@\\-./%()[\\]]+:)*(${COLOR_PREFIXES.join("|")})-([\\w-]+)(?:/\\d+)?$`,
);

/** Every whitespace-separated token of every string/template literal in a file. Literals only, never
 *  raw text — otherwise a doc comment naming a token would register as a use of it. Tokens carrying
 *  an interpolation or an arbitrary `[…]` value are skipped: those are JIT-generated by design. */
function classTokens(text) {
  const out = [];
  for (const m of text.matchAll(/"([^"\n]*)"|'([^'\n]*)'|`([^`]*)`/g)) {
    const body = m[1] ?? m[2] ?? m[3];
    if (!body) continue;
    for (const tok of body.split(/\s+/))
      if (tok && !tok.includes("${") && !tok.includes("[")) out.push(tok);
  }
  return out;
}

describe("every colour utility names a token the theme defines", () => {
  const offenders = [];
  let inNamespace = 0;
  for (const [path, text] of SOURCES) {
    for (const tok of classTokens(text)) {
      const m = CLASS_TOKEN.exec(tok);
      if (!m) continue;
      const color = m[2];
      if (!PM_FAMILIES.has(family(color))) continue; // default palette — deliberately out of scope
      inNamespace += 1;
      if (!THEME_COLORS.has(color)) offenders.push(`${path}: ${tok}`);
    }
  }

  it("read the theme, and found utilities to check", () => {
    // A scan that matches nothing passes silently — the floor `gates-inspect-something` exists for.
    expect(THEME_COLORS.get("ink")).toBe("--ink");
    expect(THEME_COLORS.has("st-due")).toBe(true);
    expect(THEME_COLORS.has("ink1")).toBe(false);
    expect(SOURCES.length).toBeGreaterThan(100);
    expect(inNamespace).toBeGreaterThan(500);
  });

  it("classifies real utilities without false positives", () => {
    // The exact collisions whole-segment family matching exists to survive.
    for (const safe of [
      "text-start",
      "text-stone-400",
      "bg-blend-multiply",
      "border-b",
      "border-2",
    ])
      expect(PM_FAMILIES.has(family(CLASS_TOKEN.exec(safe)[2]))).toBe(false);
    for (const real of ["text-ink2", "hover:bg-surface", "border-border2", "text-st-due"])
      expect(THEME_COLORS.has(CLASS_TOKEN.exec(real)[2])).toBe(true);
    // And the two that started this, as they were written.
    for (const dead of ["text-ink1", "hover:bg-bg2"]) {
      const color = CLASS_TOKEN.exec(dead)[2];
      expect(PM_FAMILIES.has(family(color))).toBe(true);
      expect(THEME_COLORS.has(color)).toBe(false);
    }
  });

  it("finds no class naming a token that does not exist", () => {
    expect(offenders).toEqual([]);
  });
});

// ---------------------------------------------------------------------------------------------
// Rule 2 — the same bug through the other door. `text-[var(--ink1)]` fails exactly as `text-ink1`
// does, and no class-name scan can see it: an arbitrary value IS JIT-generated, so the utility
// compiles and simply resolves to nothing. Green on arrival; it keeps the escape hatch shut.
// ---------------------------------------------------------------------------------------------

describe("every var(--…) reference resolves to a property something writes", () => {
  const defined = new Set([...ROOT_PROPS, ...RUNTIME_PROPS]);
  const offenders = [];
  let seen = 0;
  for (const [path, text] of SOURCES) {
    for (const m of text.matchAll(/var\(\s*(--[\w-]+)/g)) {
      // `var(--st-${STATUS_KEY[status]})` (StatusBadge) is assembled at runtime and made safe by a
      // `Record<ProjectStatus, StatusKey>` — the type is the guard there, so skip the partial name.
      if (text.slice(m.index + m[0].length).startsWith("${")) continue;
      seen += 1;
      if (!defined.has(m[1])) offenders.push(`${path}: var(${m[1]})`);
    }
  }

  it("found references to check", () => {
    expect(seen).toBeGreaterThan(50);
    expect(defined.has("--tap-min")).toBe(true); // DENSITY_VARS
    expect(defined.has("--scrim")).toBe(true); // :root only — no runtime writer, by decision
    expect(defined.has("--ink1")).toBe(false);
  });

  it("finds no var() naming a property nothing defines", () => {
    expect(offenders).toEqual([]);
  });
});

// ---------------------------------------------------------------------------------------------
// Rule 3 — the three authorities agree.
//
// This is the one that catches the NEXT person rather than the last. Adding a token means editing
// three places: `ROLES`/`themeVars` (the runtime writer), `:root` (first paint) and `@theme inline`
// (the utility). Miss the third and `text-newrole` compiles to nothing — rule 1 above cannot see it,
// because a class naming a token that no longer exists and a token that never got a utility look
// identical from the call site. Miss the first and the utility resolves to nothing after mount.
// ---------------------------------------------------------------------------------------------

describe("the token map, the bootstrap :root and the runtime writer agree", () => {
  it("every generated colour utility reads a property applyTheme writes", () => {
    const unwritten = [...THEME_COLORS.entries()]
      .filter(([, prop]) => !RUNTIME_PROPS.has(prop))
      .map(([name, prop]) => `--color-${name} → var(${prop})`);
    expect(unwritten).toEqual([]);
  });

  it("every colour property applyTheme writes has a utility to reach it", () => {
    // The reverse direction: a token nobody can name from a className is either dead weight or a
    // forgotten `@theme inline` line. Non-colour properties are excluded — they are read through
    // `var()` in arbitrary values (`rounded-[var(--radius)]`), never as a colour utility.
    const NON_COLOR = /^--(head|ui|mono|radius|radius-sm|font-scale|tap-min|tg-.*)$/;
    const mapped = new Set(THEME_COLORS.values());
    const unreachable = [...RUNTIME_PROPS].filter((p) => !NON_COLOR.test(p) && !mapped.has(p));
    expect(unreachable).toEqual([]);
  });

  it("every generated colour utility is also declared for first paint", () => {
    // `:root` styles the frame before React's mount effect runs. A property mapped by `@theme` but
    // missing here paints as nothing until applyTheme lands.
    const unbootstrapped = [...THEME_COLORS.values()].filter((p) => !ROOT_PROPS.has(p));
    expect(unbootstrapped).toEqual([]);
  });
});

// ---------------------------------------------------------------------------------------------
// Selector validity — the same failure mode as an unknown utility, one level up.
// ---------------------------------------------------------------------------------------------
//
// A style rule whose selector list contains ONE invalid selector is discarded ENTIRELY. Not the bad
// half — the whole rule, every declaration in it, silently, with no warning from Tailwind, no build
// failure and nothing in a diff to look at. It is the token-typo bug with a bigger blast radius,
// because the selector that dies is usually not the one that was wrong.
//
// It has already happened once: `> ul:has(…):not(:has(> li:not(:has(> input))))` — a `:has()` inside
// a `:has()`, which Selectors-4 §5.1 forbids — sat beside a perfectly good class-only selector and
// took it down with it, so every note checklist quietly regained the indent the rule removes.
//
// `nwsapi` (jsdom's engine) is the arbiter rather than a regex, because "is this selector legal" is
// a parser's question. It is stricter than a regex could be and it agrees with Blink/WebKit/Gecko on
// the nesting rules that matter here. Selectors it cannot know about are excluded by construction:
// `@`-prefixed lines, Tailwind's `theme(…)`/`@apply` bodies, and anything holding a `&`.
describe("every selector in index.css is one a browser will accept", () => {
  // The selector lists of PM's own style rules. Comments are stripped first — this file is more
  // prose than CSS, and a comment's closing delimiter sitting on the line above a rule would
  // otherwise be read as part of that rule's selector. Naive brace-matching does the rest: there is
  // no string literal containing `{`, and at-rule preludes are excluded by the `@` in the class.
  const selectorLists = () => {
    const out = [];
    const bare = CSS.replace(/\/\*[\s\S]*?\*\//g, "\n");
    for (const m of bare.matchAll(/(^|[};])\s*([^{};@]+?)\s*\{/g)) {
      const prelude = m[2].trim().replace(/\s+/g, " ");
      // `&` is nesting (resolved against a parent this scan does not track), and a bare `--custom`
      // line is a property, not a rule.
      if (!prelude || prelude.includes("&") || prelude.startsWith("--")) continue;
      // A `@keyframes` stop is a percentage, not a selector — a different grammar in the same shape.
      if (/^(from|to|-?[\d.]+%)(\s*,\s*(from|to|-?[\d.]+%))*$/.test(prelude)) continue;
      if (!/[.#:[\w*]/.test(prelude[0])) continue;
      out.push(prelude);
    }
    return out;
  };

  it("parses, every one of them", async () => {
    const { JSDOM } = await import("jsdom");
    const { document } = new JSDOM("<!doctype html><div></div>").window;
    const rejected = [];
    for (const list of selectorLists()) {
      try {
        document.querySelector(list);
      } catch (e) {
        rejected.push(`${list}  →  ${e.message}`);
      }
    }
    expect(rejected).toEqual([]);
  });

  it("finds the rules it is meant to be looking at", () => {
    // Without this the scan could quietly match nothing and pass forever. The checklist rule is the
    // one the guard was written for, so it is the one named here.
    const lists = selectorLists();
    expect(lists.length).toBeGreaterThan(20);
    expect(lists.some((s) => s.includes("ul.contains-task-list"))).toBe(true);
  });
});
