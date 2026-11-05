// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// ESLint flat config. A lean baseline FLOOR: JS + TypeScript recommended
// (syntactic, not type-checked — type errors are already caught by `tsc --noEmit`)
// plus the classic React Hooks rules that catch real bugs. Formatting is owned by
// Prettier, so eslint-config-prettier turns off any rules that would fight it.
//
// Note: eslint-plugin-react-hooks v7's `recommended` preset also turns on the new
// React-Compiler rules (refs / immutability / set-state-in-effect). Those flag
// idiomatic patterns throughout the existing, shipped code, so they are NOT part
// of this floor — enable them as a deliberate, code-changing pass later.

import js from "@eslint/js";
import globals from "globals";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import prettier from "eslint-config-prettier";

// The backend boundary: see the frontend block below. The package root re-exports `core` too, so
// `import { core } from "@tauri-apps/api"` (or from its `/index` subpath, the same file) is the same
// door and is shut with it. no-restricted-imports sees static imports only; IPC_DYNAMIC shuts a
// dynamic `import()` of the same modules, which would hand over `invoke` just the same.
const IPC_MESSAGE = "Call backend commands only through src/lib/ipc.ts (the typed IPC boundary).";
const IPC_BOUNDARY = [
  { name: "@tauri-apps/api/core", message: IPC_MESSAGE },
  { name: "@tauri-apps/api", importNames: ["core"], message: IPC_MESSAGE },
  { name: "@tauri-apps/api/index", importNames: ["core"], message: IPC_MESSAGE },
];
const IPC_DYNAMIC = [
  {
    selector: "ImportExpression[source.value=/^@tauri-apps\\/api(\\/(core|index)(\\.js)?)?$/]",
    message: IPC_MESSAGE,
  },
];

// Calendar editing (#884) changes the user's Google Calendar with their own sign-in, so its
// wrappers have exactly one importer: the editor's hook. Any screen that changes an event goes
// through that hook, and nothing else can hold a write. ESLint also refuses `import * as … from
// ipc` outside it (a namespace could reach them), and `scripts/check-calendar-write-fence.mjs`
// backs all this with a text check that dynamic imports can't slip past.
const CALENDAR_WRITES = {
  regex: "(^|/)ipc$",
  importNames: [
    "getCalendarEventForEdit",
    "updateCalendarEvent",
    "rescheduleCalendarEvent",
    "undoCalendarChange",
    "respondToCalendarEvent",
    "createCalendarEvent",
    "deleteCalendarEvent",
    "cancelCalendarDelete",
    "listHeldDeletes",
    "seriesChoices",
    "getSeriesSummary",
    "settleSeriesSplit",
  ],
  message: "Calendar edits go only through src/components/calendar/edit/useEventWrites.ts (#884).",
};

// Nothing in the page parses HTML or writes an HTML string into it (C19, #884; INVARIANTS.md I-23).
// CodeQL reads each of these as an XSS sink, and its highs block merges. Google's formatted
// descriptions are read by src/lib/calendarEdit/descriptionTokens.ts and drawn as React elements or
// DOM nodes made one by one. The selectors catch the computed forms (`el["innerHTML"]`) and an
// assignment through an object (`Object.assign(el, { innerHTML })`) too.
const HTML_SINK = "Never parse HTML or write it into the page: see descriptionTokens.ts (C19).";
const HTML_SINKS = [
  { selector: "NewExpression[callee.name='DOMParser']", message: HTML_SINK },
  { selector: "NewExpression[callee.property.name='DOMParser']", message: HTML_SINK },
  {
    selector: "MemberExpression[property.name=/^(innerHTML|outerHTML|srcdoc)$/]",
    message: HTML_SINK,
  },
  {
    selector: "MemberExpression[property.value=/^(innerHTML|outerHTML|srcdoc)$/]",
    message: HTML_SINK,
  },
  {
    selector: "Property[key.name=/^(innerHTML|outerHTML|srcdoc|srcDoc|dangerouslySetInnerHTML)$/]",
    message: HTML_SINK,
  },
  {
    selector: "Property[key.value=/^(innerHTML|outerHTML|srcdoc|srcDoc|dangerouslySetInnerHTML)$/]",
    message: HTML_SINK,
  },
  {
    selector:
      "CallExpression[callee.property.name=/^(insertAdjacentHTML|createContextualFragment|parseFromString|setHTML|setHTMLUnsafe|parseHTMLUnsafe)$/]",
    message: HTML_SINK,
  },
  {
    selector:
      "CallExpression[callee.property.value=/^(insertAdjacentHTML|createContextualFragment|parseFromString|setHTML|setHTMLUnsafe|parseHTMLUnsafe)$/]",
    message: HTML_SINK,
  },
  // On any object: `box.ownerDocument.write(s)`, an alias of the document, `window.document`.
  { selector: "CallExpression[callee.property.name=/^(write|writeln)$/]", message: HTML_SINK },
  { selector: "CallExpression[callee.property.value=/^(write|writeln)$/]", message: HTML_SINK },
  {
    selector: "CallExpression[callee.property.name='setAttribute'][arguments.0.value=/^srcdoc$/i]",
    message: HTML_SINK,
  },
  { selector: "JSXAttribute[name.name=/^(dangerouslySetInnerHTML|srcDoc)$/]", message: HTML_SINK },
  {
    selector: "CallExpression[callee.property.name='createElement'][arguments.0.type!='Literal']",
    message: "createElement takes a literal tag name (CodeQL reads its argument as an XSS sink).",
  },
  {
    selector: "CallExpression[callee.property.name='execCommand'][arguments.0.type!='Literal']",
    message: "execCommand takes a literal command name.",
  },
  {
    selector:
      "CallExpression[callee.property.name='execCommand'][arguments.0.value=/^inserthtml$/i]",
    message: HTML_SINK,
  },
];

// Mod frames (Mods spec §1.4; INVARIANTS.md I-24). PM's commands have no per-window ACL and Tauri counts
// the `pmmod` scheme as local, so the frame's sandbox is the wall: on WebView2 a mod frame that lost it
// sends a local Origin and reaches every command. So an <iframe> exists only in ModFrame.tsx, written as
// `sandbox="allow-scripts" allow=""` with literal values, no spread and only the attributes listed below;
// nothing anywhere makes a frame another way, names "iframe" as a string (`const Tag = "iframe"; <Tag />`
// is an iframe too), or touches a frame's sandbox or permissions once it is drawn.
const FRAME = "Mod frames live only in src/components/mods/ModFrame.tsx (INVARIANTS.md I-24).";
const FRAME_TAGS = "/^(iframe|frame|frameset|object|embed|portal|fencedframe)$/i";
const FRAME_PROPS = "/^(sandbox|allow|allowfullscreen|credentialless)$/i";
const FRAME_SINKS = [
  {
    selector: `CallExpression[callee.property.name='createElement'][arguments.0.value=${FRAME_TAGS}]`,
    message: FRAME,
  },
  {
    selector: `CallExpression[callee.name=/^(createElement|jsx|jsxs|jsxDEV)$/][arguments.0.value=${FRAME_TAGS}]`,
    message: FRAME,
  },
  {
    selector: "JSXOpeningElement[name.name=/^(frame|frameset|object|embed|portal|fencedframe)$/]",
    message: FRAME,
  },
  { selector: `MemberExpression[property.name=${FRAME_PROPS}]`, message: FRAME },
  { selector: `MemberExpression[property.value=${FRAME_PROPS}]`, message: FRAME },
  // `Object.assign(frame, { sandbox })`, and `const { sandbox } = frame`.
  { selector: `Property[key.name=${FRAME_PROPS}]`, message: FRAME },
  { selector: `Property[key.value=${FRAME_PROPS}]`, message: FRAME },
  {
    selector: `CallExpression[callee.property.name=/^(setAttribute|removeAttribute|toggleAttribute|getAttributeNode)$/][arguments.0.value=${FRAME_PROPS}]`,
    message: FRAME,
  },
  {
    selector: `CallExpression[callee.property.name=/^(setAttributeNS|removeAttributeNS)$/][arguments.1.value=${FRAME_PROPS}]`,
    message: FRAME,
  },
];
const NO_IFRAME_ELEMENT = [{ selector: "JSXOpeningElement[name.name='iframe']", message: FRAME }];
const NO_IFRAME_NAME = [
  { selector: "Literal[value=/^iframe$/i]", message: FRAME },
  { selector: "TemplateElement[value.raw=/^iframe$/i]", message: FRAME },
];
// ModFrame.tsx's one frame, attribute by attribute.
const IFRAME = "JSXOpeningElement[name.name='iframe']";
const MOD_FRAME_SANDBOX = 'A mod frame is sandbox="allow-scripts", written literally (I-24).';
const MOD_FRAME_ALLOW = 'A mod frame is allow="", written literally (I-24).';
const MOD_FRAME_SHAPE = [
  {
    selector: `${IFRAME}:not(:has(> JSXAttribute[name.name='sandbox'][value.type='Literal'][value.value='allow-scripts']))`,
    message: MOD_FRAME_SANDBOX,
  },
  {
    selector: `${IFRAME} > JSXAttribute[name.name='sandbox']:not([value.type='Literal'][value.value='allow-scripts'])`,
    message: MOD_FRAME_SANDBOX,
  },
  {
    selector: `${IFRAME}:not(:has(> JSXAttribute[name.name='allow'][value.type='Literal'][value.value='']))`,
    message: MOD_FRAME_ALLOW,
  },
  {
    selector: `${IFRAME} > JSXAttribute[name.name='allow']:not([value.type='Literal'][value.value=''])`,
    message: MOD_FRAME_ALLOW,
  },
  {
    selector: `${IFRAME} > JSXSpreadAttribute`,
    message: "A mod frame takes no spread props (I-24).",
  },
  {
    selector: `${IFRAME} > JSXAttribute:not([name.name=/^(key|ref|src|title|sandbox|allow|className|style|onLoad)$/])`,
    message:
      "A mod frame takes only key, ref, src, title, sandbox, allow, className, style and onLoad (I-24).",
  },
];

export default tseslint.config(
  // src-tauri/python is the fetched standalone interpreter (a build artifact, like
  // src-tauri/target) — present only on Windows checkouts and full of vendored JS.
  {
    ignores: [
      "dist",
      "dist-ssr",
      "src-tauri/target",
      "src-tauri/python",
      // The sealed-frame regression app's build output (its own crate, tests/sealed-frame).
      "tests/sealed-frame/src-tauri/target",
      "tests/sealed-frame/src-tauri/gen",
      "design-system-docs",
      // The spec + decision log + scratch notes (git-ignored, local-only — not first-party
      // source). Lint shouldn't reach into it (e.g. a stray docs/calendar-view/*.js prototype).
      "docs",
      // Agent worktrees are whole checkouts of THIS repo on other branches (git-ignored,
      // local-only). Linting them audits code that isn't on the branch under test: the gate
      // double-counts every finding, and a half-finished experiment in a worktree could fail
      // `just check` on a clean main, pointing at a path that isn't part of the build.
      ".claude/worktrees",
      "node_modules",
      // design-sync (claude.ai/design) build output & staged converter scripts —
      // regenerated, vendored, git-ignored. The hand-authored .design-sync/previews/
      // and preview-provider.tsx are NOT ignored (they're first-party and linted).
      "ds-bundle",
      ".ds-sync",
      ".design-sync/.cache",
    ],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  // Frontend (browser). Type-aware for the one bug class tsc can't catch: dropped
  // promises. Every backend call is an async `invoke` through src/lib/ipc.ts, so a
  // forgotten await/.catch silently swallows an IPC error (and under StrictMode
  // doubles a fire-and-forget effect). `projectService` turns on type info just for
  // src/**; the rest of the type-aware set stays off — this is the promise floor,
  // not full recommendedTypeChecked (T-08).
  {
    files: ["src/**/*.{ts,tsx}"],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.browser,
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
    },
    plugins: {
      "react-hooks": reactHooks,
      "react-refresh": reactRefresh,
    },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      // ERROR, not warn: `just check` runs `npx eslint .` with no `--max-warnings`, so a warning
      // can never fail the gate — a stale-closure regression would ride into a green PR unnoticed,
      // which is exactly what happened once. Promoting the rule (rather than adding
      // `--max-warnings N`) keeps the gate honest without a ratchet number to maintain, and costs
      // nothing: intentional mount-once effects already opt out one at a time with an
      // `eslint-disable-next-line` carrying a reason, an idiom this codebase established long
      // before this rule changed severity. If you reach for that comment, write the WHY beside it.
      "react-hooks/exhaustive-deps": "error",
      // Deliberately left at warn. Every hit is a context/provider file exporting a provider
      // alongside its hooks — clearing them means splitting ~12 files to satisfy a dev-only Fast
      // Refresh nicety with no production or correctness effect, and the churn would bury the next
      // real finding. Kept visible, not enforced.
      "react-refresh/only-export-components": ["warn", { allowConstantExport: true }],
      "@typescript-eslint/no-floating-promises": "error",
      "@typescript-eslint/no-misused-promises": [
        "error",
        { checksVoidReturn: { attributes: false } },
      ],
      // The backend boundary (AGENTS.md "src/lib/ipc.ts — the only place that calls Rust").
      // Ban `invoke`/`Channel` (the `@tauri-apps/api/core` entrypoint) everywhere so every
      // backend command goes through the typed wrappers in ipc.ts, which normalise errors and
      // keep the surface auditable. Scoped to the exact path, NOT `@tauri-apps/api/*` — the
      // builtin plugin APIs (api/event, api/app, api/window, api/webview, plugin-*) legitimately
      // cross from ~15 components and must not be caught. ipc.ts itself is exempted below.
      "no-restricted-imports": ["error", { paths: IPC_BOUNDARY, patterns: [CALENDAR_WRITES] }],
      // The HTML sinks (HTML_SINKS above): nothing in the page parses HTML or writes it in. And no
      // frame outside ModFrame.tsx (FRAME_SINKS and the two NO_IFRAME lists above).
      "no-restricted-syntax": [
        "error",
        ...HTML_SINKS,
        ...FRAME_SINKS,
        ...NO_IFRAME_ELEMENT,
        ...NO_IFRAME_NAME,
        ...IPC_DYNAMIC,
      ],
    },
  },
  // The one home of a mod frame: an <iframe> here, in exactly the shape MOD_FRAME_SHAPE allows. A
  // rule's options replace rather than merge, so the other fences are restated.
  {
    files: ["src/components/mods/ModFrame.tsx"],
    rules: {
      "no-restricted-syntax": [
        "error",
        ...HTML_SINKS,
        ...FRAME_SINKS,
        ...NO_IFRAME_NAME,
        ...MOD_FRAME_SHAPE,
        ...IPC_DYNAMIC,
      ],
    },
  },
  // The description tokenizer names "iframe" as data (an element whose content it skips); it makes no
  // element, and every other fence still holds there.
  {
    files: ["src/lib/calendarEdit/descriptionTokens.ts"],
    rules: {
      "no-restricted-syntax": [
        "error",
        ...HTML_SINKS,
        ...FRAME_SINKS,
        ...NO_IFRAME_ELEMENT,
        ...IPC_DYNAMIC,
      ],
    },
  },
  // A plain JavaScript file in src/ (none exists) would ship just the same, so the page's fences hold
  // there too rather than stopping at the TypeScript block's glob.
  {
    files: ["src/**/*.{js,jsx,mjs,cjs}"],
    languageOptions: {
      globals: globals.browser,
    },
    rules: {
      "no-restricted-imports": ["error", { paths: IPC_BOUNDARY, patterns: [CALENDAR_WRITES] }],
      "no-restricted-syntax": [
        "error",
        ...HTML_SINKS,
        ...FRAME_SINKS,
        ...NO_IFRAME_ELEMENT,
        ...NO_IFRAME_NAME,
        ...IPC_DYNAMIC,
      ],
    },
  },
  // Tests may drive the sinks (a spy on an innerHTML setter, a fixture's markup): they ship nowhere.
  // A block of its own, so no shipped file (the write hook below included) is ever exempt.
  {
    files: ["src/**/*.test.{ts,tsx}"],
    rules: {
      "no-restricted-syntax": "off",
    },
  },
  // The calendar editor's hook is the one importer of the calendar write wrappers. A rule's options
  // replace rather than merge, so the IPC boundary is restated here, not lost. Tests are exempt:
  // they mock and drive the wrappers, and ship nowhere.
  {
    files: ["src/components/calendar/edit/useEventWrites.ts", "src/**/*.test.{ts,tsx}"],
    rules: {
      "no-restricted-imports": ["error", { paths: IPC_BOUNDARY }],
    },
  },
  // The one sanctioned caller of @tauri-apps/api/core: ipc.ts wraps invoke/Channel for the
  // whole app, so the boundary rule above is turned back off for this single file.
  {
    files: ["src/lib/ipc.ts"],
    rules: {
      "no-restricted-imports": "off",
    },
  },
  // Node tooling (build/check scripts, config files).
  {
    files: ["scripts/**/*.mjs", "*.{js,mjs,ts}"],
    languageOptions: {
      globals: globals.node,
    },
  },
  // The webview engine's init scripts (src-tauri/src/mods/*.js): plain browser scripts Rust injects
  // into PM's windows ahead of every page, not modules.
  {
    files: ["src-tauri/src/**/*.js"],
    languageOptions: {
      sourceType: "script",
      globals: globals.browser,
    },
  },
  // The sealed-frame regression app's pages (tests/sealed-frame): plain browser scripts. The hostile
  // probe does on purpose what the page's fences forbid, and none of it ships.
  {
    files: ["tests/sealed-frame/**/*.js"],
    languageOptions: {
      sourceType: "script",
      globals: globals.browser,
    },
  },
  prettier,
);
