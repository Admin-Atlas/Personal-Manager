// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The calendar write path stays where it was put, and nothing that talks to a model can reach it.
//
// WHY. Calendar editing (#884) changes the user's Google Calendar under their own sign-in. Its
// commands refuse any window but the main one, and no model call in PM has tools, but neither of
// those says anything about the NEXT change: a helper that imports the write wrapper into the chat
// view, a briefing tweak that "just fixes the time" through the Rust command, a model path that
// builds a Calendar URL and sends it with the stored token. Each would compile, pass review on a busy
// day, and quietly let model output edit someone's calendar. Stage 4's AI editing is a deliberate
// design of its own (with the user approving each change), not something to arrive by accident.
//
// So the write path has a fixed shape, checked here as text:
//   1. The Rust command names appear only where they are defined and registered, and in ipc.ts,
//      where each is invoked exactly once, by its own wrapper.
//   2. The frontend write wrappers are named only in ipc.ts, the editor's hook and tests (ESLint
//      enforces the imports; this also sees namespace access and dynamic imports).
//   3. `google_io`, the only code that sends a calendar write, is named only in its own module.
//   4. The write core (`calendar_write::`) is used only by the write commands, plus its data types
//      and sync reconcile by the calendar sync.
//   5. No module on a model path names any of it, sends an authorised Google request, or holds a
//      Calendar API URL.
//   6. The write path calls no model.
//   7. No other module both holds a Calendar API URL and can send something other than a read.
// And the gate checks its own lists against the code, so a new or renamed write command can't
// quietly fall outside it: the commands calendar_edit.rs defines, the wrappers ipc.ts declares, and
// the names eslint.config.js restricts must all be the ones listed here.
//
// ZERO-DEPENDENCY (INVARIANTS.md I-18): plain text matching, no parser — pr.yml's `hygiene` job runs
// with no `npm ci`.

import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, relative, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

export const RUST_ROOT = "src-tauri/src";
export const WEB_ROOT = "src";

/** The write commands, as Rust names each one and as its ipc.ts wrapper does. */
export const WRITE_COMMANDS = [
  ["get_calendar_event_for_edit", "getCalendarEventForEdit"],
  ["update_calendar_event", "updateCalendarEvent"],
  ["delete_calendar_event", "deleteCalendarEvent"],
  ["cancel_calendar_delete", "cancelCalendarDelete"],
  ["list_held_deletes", "listHeldDeletes"],
];
export const COMMANDS = WRITE_COMMANDS.map(([command]) => command);
export const WRAPPERS = WRITE_COMMANDS.map(([, wrapper]) => wrapper);

const EDIT_COMMANDS_RS = "src-tauri/src/commands/calendar_edit.rs";
const LIB_RS = "src-tauri/src/lib.rs";
const IPC_TS = "src/lib/ipc.ts";
const ESLINT_CONFIG = "eslint.config.js";
const WRITES_HOOK = "src/components/calendar/edit/useEventWrites.ts";
const WRITE_CORE_DIR = "src-tauri/src/calendar_write/";
const CALENDAR_SYNC_RS = "src-tauri/src/commands/calendars.rs";

/** What the calendar sync may use from the write core: the list's reasons and the reconcile. */
const SYNC_MAY_USE = /\bcalendar_write::(dto|reconcile)::/g;

/** Mentions that put a file on a model path. */
const MODEL_PATH = /\b(llm_gateway|openrouter|openai_compat|model_gateway)::/;
/** The model gateways themselves, which are on the path by definition. */
const MODEL_GATEWAYS = new Set([
  "src-tauri/src/llm_gateway.rs",
  "src-tauri/src/openrouter.rs",
  "src-tauri/src/openai_compat.rs",
  "src-tauri/src/model_gateway.rs",
]);
/** lib.rs names every module and registers every command; it carries no model output anywhere. */
const NOT_A_MODEL_PATH = new Set([LIB_RS]);
/** Fewest model-path modules there can plausibly be; below this the matching has broken. */
const MODEL_PATH_FLOOR = 10;

/** What a model-path module must never name. */
const FORBIDDEN_ON_MODEL_PATH = [
  [/\bcalendar_edit\b/, "the calendar write commands' module"],
  [/\bCalendarEditState\b/, "the calendar editing state"],
  [/\bcalendar_write::/, "the calendar write core"],
  [/\bgoogle_io\b/, "the calendar write sender"],
  [/\bauthorized_send(_with)?\b/, "an authorised Google request"],
  [/googleapis\.com\/calendar|\bCALENDAR_API\b/, "the Google Calendar API"],
  ...COMMANDS.map((c) => [new RegExp(`\\b${c}\\b`), `the \`${c}\` command`]),
];

/** A Calendar API URL, and what could send something other than a read with it. */
const CALENDAR_API = /googleapis\.com\/calendar|\bCALENDAR_API\b/;
const CAN_WRITE =
  /\b(authorized_send(_with)?|valid_access_token|refresh_now)\b|\.(patch|delete|post|put)\(/;

/** Every file under `dir` (repo-relative, `/`-separated) whose name passes `keep`. */
function walk(root, dir, keep) {
  const out = [];
  const visit = (abs) => {
    for (const entry of readdirSync(abs, { withFileTypes: true })) {
      const path = join(abs, entry.name);
      if (entry.isDirectory()) {
        if (entry.name !== "node_modules" && entry.name !== "target") visit(path);
      } else if (keep(entry.name)) {
        out.push(relative(root, path).split(sep).join("/"));
      }
    }
  };
  visit(join(root, dir));
  return out.sort();
}

const isTest = (path) => /\.test\.(ts|tsx|mjs)$/.test(path);
const lineOf = (text, index) => text.slice(0, index).split("\n").length;

/** The first match of `re` in `text`, as `path:line`, or null. */
function where(path, text, re) {
  const m = new RegExp(re.source, re.flags.replace("g", "")).exec(text);
  return m ? `${path}:${lineOf(text, m.index)}` : null;
}

export function scan(root) {
  const read = (rel) => readFileSync(join(root, rel), "utf8");
  const rust = walk(root, RUST_ROOT, (n) => n.endsWith(".rs")).map((p) => [p, read(p)]);
  const web = walk(root, WEB_ROOT, (n) => /\.(ts|tsx)$/.test(n)).map((p) => [p, read(p)]);
  const problems = [];

  // 1. Command names: defined and registered in Rust, invoked from ipc.ts (and checked by its
  //    tests), and nowhere else.
  for (const command of COMMANDS) {
    const re = new RegExp(`\\b${command}\\b`);
    for (const [path, text] of [...rust, ...web]) {
      if (path === EDIT_COMMANDS_RS || path === LIB_RS || path === IPC_TS || isTest(path)) continue;
      const at = where(path, text, re);
      if (at) {
        problems.push(
          `${at} names the calendar write command \`${command}\`, which only ${EDIT_COMMANDS_RS}, ` +
            `${LIB_RS} and ${IPC_TS} may`,
        );
      }
    }
  }

  // 2. Wrapper names: ipc.ts defines them, the editor's hook calls them, tests drive them.
  for (const wrapper of WRAPPERS) {
    const re = new RegExp(`\\b${wrapper}\\b`);
    for (const [path, text] of web) {
      if (path === IPC_TS || path === WRITES_HOOK || isTest(path)) continue;
      const at = where(path, text, re);
      if (at) {
        problems.push(
          `${at} names the calendar write wrapper \`${wrapper}\`; calendar edits go only through ` +
            `${WRITES_HOOK}`,
        );
      }
    }
  }

  // 3. The sender is private to its module.
  for (const [path, text] of rust) {
    if (path === EDIT_COMMANDS_RS) continue;
    const at = where(path, text, /\bgoogle_io\b/);
    if (at) problems.push(`${at} names \`google_io\`, the calendar write sender`);
  }

  // 4. The write core belongs to the write commands; the sync takes only its types and reconcile.
  for (const [path, text] of rust) {
    if (path.startsWith(WRITE_CORE_DIR) || path === EDIT_COMMANDS_RS) continue;
    const scrubbed = path === CALENDAR_SYNC_RS ? text.replace(SYNC_MAY_USE, "") : text;
    const at = where(path, scrubbed, /\bcalendar_write::/);
    if (at) {
      problems.push(
        `${at} uses the calendar write core, which only ${EDIT_COMMANDS_RS} may` +
          (path === CALENDAR_SYNC_RS ? " (the sync may take its `dto` and `reconcile`)" : ""),
      );
    }
  }

  // 5. Nothing on a model path reaches any of it.
  const modelPath = rust.filter(
    ([path, text]) =>
      !NOT_A_MODEL_PATH.has(path) && (MODEL_GATEWAYS.has(path) || MODEL_PATH.test(text)),
  );
  for (const [path, text] of modelPath) {
    for (const [re, what] of FORBIDDEN_ON_MODEL_PATH) {
      const at = where(path, text, re);
      if (at) {
        problems.push(
          `${at} is on a model path (it calls a model, or is a gateway) and names ${what}; nothing ` +
            `a model's output passes through may reach a calendar write`,
        );
      }
    }
  }
  if (modelPath.length < MODEL_PATH_FLOOR) {
    problems.push(
      `only ${modelPath.length} model-path modules found (expected at least ${MODEL_PATH_FLOOR}) — ` +
        `the matching has stopped working, not the model calls gone away`,
    );
  }

  // 6. The write path calls no model.
  for (const [path, text] of rust) {
    if (!(path.startsWith(WRITE_CORE_DIR) || path === EDIT_COMMANDS_RS)) continue;
    const at = where(path, text, MODEL_PATH);
    if (at) problems.push(`${at} is on the calendar write path and calls a model`);
  }

  // 7. No second writer: a module holding a Calendar API URL can't also send anything but a read.
  for (const [path, text] of rust) {
    if (path === EDIT_COMMANDS_RS || !CALENDAR_API.test(text)) continue;
    const at = where(path, text, CAN_WRITE);
    if (at) {
      problems.push(
        `${at} holds a Google Calendar API URL and can send more than a read; calendar writes ` +
          `go only through ${EDIT_COMMANDS_RS}`,
      );
    }
  }

  // The gate's own lists are the code's: what calendar_edit.rs defines, ipc.ts declares and ESLint
  // restricts. Otherwise a new or renamed command would simply fall outside every rule above.
  const sources = new Map([...rust, ...web]);
  const editModule = sources.get(EDIT_COMMANDS_RS);
  if (editModule === undefined) {
    problems.push(`${EDIT_COMMANDS_RS} is missing`);
  } else {
    if (!/\bmod google_io\b/.test(editModule)) {
      problems.push(`${EDIT_COMMANDS_RS} no longer declares \`mod google_io\``);
    }
    const defined = [
      ...editModule.matchAll(/#\[tauri::command\]\s*pub\s+(?:async\s+)?fn\s+(\w+)/g),
    ].map((m) => m[1]);
    if (!sameSet(defined, COMMANDS)) {
      problems.push(
        `${EDIT_COMMANDS_RS} defines the commands [${defined.join(", ")}] but this fence lists ` +
          `[${COMMANDS.join(", ")}]; add a new write command to WRITE_COMMANDS (and its wrapper)`,
      );
    }
    // `commands/mod.rs` re-exports the module with a glob, so any other `pub fn` is reachable as
    // `crate::commands::<name>` without naming the module at all.
    for (const m of editModule.matchAll(/^pub\s+(?:async\s+)?fn\s+(\w+)/gm)) {
      if (!COMMANDS.includes(m[1])) {
        problems.push(
          `${EDIT_COMMANDS_RS}:${lineOf(editModule, m.index)} has a plain \`pub fn ${m[1]}\`, ` +
            `which the commands glob re-exports; make it \`pub(super)\` or narrower`,
        );
      }
    }
  }
  const ipc = sources.get(IPC_TS) ?? "";
  for (const [command, wrapper] of WRITE_COMMANDS) {
    const literal = `"${command}"`;
    const uses = ipc.split(literal).length - 1;
    const declared = ipc.indexOf(`export const ${wrapper} =`);
    const next = declared < 0 ? -1 : ipc.indexOf("\nexport ", declared + 1);
    const own = declared < 0 ? "" : ipc.slice(declared, next < 0 ? undefined : next);
    if (declared < 0 || !own.includes(literal)) {
      problems.push(`${IPC_TS} has no \`export const ${wrapper}\` invoking ${literal}`);
    } else if (uses !== 1) {
      problems.push(
        `${IPC_TS} invokes ${literal} ${uses} times; only its own wrapper \`${wrapper}\` may, so ` +
          `every way to the command stays under the import rule`,
      );
    }
  }
  const eslint = read(ESLINT_CONFIG);
  const restricted = /CALENDAR_WRITES\s*=\s*\{[\s\S]*?importNames:\s*\[([^\]]*)\]/.exec(eslint);
  const names = restricted ? [...restricted[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]) : [];
  if (!sameSet(names, WRAPPERS)) {
    problems.push(
      `${ESLINT_CONFIG} restricts the imports [${names.join(", ")}] but the write wrappers are ` +
        `[${WRAPPERS.join(", ")}]`,
    );
  }

  return { problems, rust: rust.length, web: web.length, modelPath: modelPath.length };
}

const sameSet = (a, b) => a.length === b.length && a.every((x) => b.includes(x));

function main() {
  const { problems, rust, web, modelPath } = scan(repoRoot);
  if (problems.length > 0) {
    console.error("✗ calendar write fence:\n");
    for (const p of problems) console.error(`  • ${p}`);
    process.exit(1);
  }
  console.log(
    `✓ calendar write fence: ${rust} Rust and ${web} TypeScript files checked; none of the ` +
      `${modelPath} model-path modules reaches a calendar write`,
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
