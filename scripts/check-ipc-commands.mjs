// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Every command name `src/lib/ipc.ts` invokes is a command the backend actually registers.
//
// WHY. `ipc.ts` is the one CI-enforced boundary between the webview and Rust (#431), and it names
// its ~270 commands as bare string literals: `invoke<boolean>("has_openrouter_key")`. Nothing on
// either side checks that the string matches. `tsc` sees a `string`, clippy sees a registered
// function nobody calls, and the mismatch surfaces as a runtime "command not found" the first time
// a user opens that particular screen — which, for a rarely-visited surface, can be a release or
// two later. Renaming a Rust command and missing one call site is exactly this.
//
// It also catches the reverse where it matters: a command that is registered but wrapped nowhere is
// reported, since it is either dead weight or a wrapper someone forgot to write.
//
// ZERO-DEPENDENCY (INVARIANTS.md I-18): plain text extraction, no parser, no npm package — pr.yml's
// `hygiene` job runs with no `npm ci`.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

export const IPC_FILE = "src/lib/ipc.ts";
export const LIB_RS = "src-tauri/src/lib.rs";

/** Fewest wrappers `ipc.ts` can plausibly hold; below this the extraction has stopped matching. */
const COMMAND_FLOOR = 200;

/**
 * Commands registered in Rust that deliberately have no `ipc.ts` wrapper.
 *
 * Keep this list short and reasoned — every entry is a command the frontend cannot reach, so the
 * only honest reasons are "the backend calls it" or "it is invoked from a surface that does not go
 * through ipc.ts".
 */
export const NO_WRAPPER_EXPECTED = new Set([
  // Deliberately unwrapped: `set_milestone_status` is the single writer for a milestone's progress,
  // and `set_milestone_state` is the primitive `flags` asserts through underneath it. A wrapper
  // here is how the removed second control finds its way back — see the comment above
  // `setMilestoneStatus` in ipc.ts.
  "set_milestone_state",
]);

/**
 * Every `invoke(…)` call site: the literal command names, how many calls there were, and how many
 * could not be read as a literal.
 *
 * A regex was the obvious approach and it does not survive contact with the file. The type argument
 * nests (`invoke<Record<string, string>>(…)`) and it contains semicolons
 * (`invoke<{ document_id: number; proposal: MetadataProposal }[]>(…)`), so every character class
 * narrow enough to stop at the end of the call also stops in the middle of a real one — and the
 * failure is SILENT: the call is skipped, and its command is then reported as unwrapped. Both
 * variants of that bug showed up on the first two runs of this gate.
 *
 * So the type argument is skipped by counting angle brackets, and anything left unreadable is
 * counted rather than ignored.
 */
export function scanInvokeCalls(source) {
  // `function invoke<T>(cmd: string, …)` is the wrapper's own DEFINITION, not a call site.
  const src = source.replace(/\bfunction\s+invoke\b/g, "function __invokeDefinition");
  const names = new Set();
  let calls = 0;
  let unparsed = 0;

  const skipSpace = (i) => {
    while (i < src.length && /\s/.test(src[i])) i++;
    return i;
  };

  const re = /\binvoke\b/g;
  let m;
  while ((m = re.exec(src)) !== null) {
    let i = skipSpace(m.index + "invoke".length);
    if (src[i] === "<") {
      let depth = 0;
      while (i < src.length) {
        if (src[i] === "<") depth++;
        else if (src[i] === ">") {
          depth--;
          if (depth === 0) {
            i++;
            break;
          }
        }
        i++;
      }
      i = skipSpace(i);
    }
    // `invoke` appearing as a plain identifier (the import alias) is not a call.
    if (src[i] !== "(") continue;
    calls++;
    i = skipSpace(i + 1);
    // The literal must be the WHOLE first argument — hence the required `,` or `)` after it.
    // Without that, `invoke("set_" + suffix)` reads as the command `set_`, which is worse than not
    // reading it: the gate then reports a command nobody wrote as missing from the backend.
    const lit = /^"([a-z0-9_]+)"\s*[,)]/.exec(src.slice(i, i + 120));
    if (lit) names.add(lit[1]);
    else unparsed++;
  }
  return { names, calls, unparsed };
}

/**
 * The commands the backend registers, read from `generate_handler![…]`.
 *
 * That macro list — not the `#[tauri::command]` attributes — is the authority: an attributed
 * function that is never added to the list is not reachable from the webview at all.
 */
export function registeredCommands(rustSource) {
  const start = rustSource.indexOf("tauri::generate_handler![");
  if (start < 0) throw new Error(`could not find generate_handler! in ${LIB_RS}`);
  const end = rustSource.indexOf("])", start);
  if (end < 0) throw new Error(`generate_handler! in ${LIB_RS} is not terminated`);
  const body = rustSource.slice(start, end);

  const names = new Set();
  for (const line of body.split("\n")) {
    const code = line.split("//")[0].trim();
    // `module::command_name,` — take the last path segment.
    const m = code.match(/^(?:[a-z0-9_]+::)*([a-z0-9_]+)\s*,?$/);
    if (m && m[1] !== "generate_handler") names.add(m[1]);
  }
  return names;
}

export function scan(root) {
  const read = (rel) => readFileSync(join(root, rel), "utf8");
  const ipc = read(IPC_FILE);
  const rust = read(LIB_RS);
  const problems = [];

  const { names: invoked, calls, unparsed } = scanInvokeCalls(ipc);
  const registered = registeredCommands(rust);

  // 1. Nothing is invoked that the backend does not register. This is the failure that reaches a
  //    user: the screen loads, the call rejects, and the surface shows an error nobody can action.
  for (const name of [...invoked].sort()) {
    if (!registered.has(name)) {
      problems.push(
        `${IPC_FILE} invokes \`${name}\`, which ${LIB_RS} does not register — a renamed or removed ` +
          `command, and the webview will get "command not found" the first time that screen opens`,
      );
    }
  }

  // 2. Nothing is registered that no wrapper reaches. Either it is dead, or a wrapper is missing.
  for (const name of [...registered].sort()) {
    if (!invoked.has(name) && !NO_WRAPPER_EXPECTED.has(name)) {
      problems.push(
        `${LIB_RS} registers \`${name}\`, which no wrapper in ${IPC_FILE} invokes — write the ` +
          `wrapper, drop the command, or add it to NO_WRAPPER_EXPECTED with a reason`,
      );
    }
  }

  // 3. Every `invoke(` call was read. A name assembled at runtime would slip past check 1 silently,
  //    and a gate that quietly stops seeing its subject is worse than no gate at all.
  if (unparsed > 0) {
    problems.push(
      `${IPC_FILE} has ${unparsed} of ${calls} \`invoke(\` calls whose command name is not a plain ` +
        `string literal — those cannot be checked against the backend`,
    );
  }

  if (invoked.size < COMMAND_FLOOR) {
    problems.push(
      `only ${invoked.size} commands extracted from ${IPC_FILE} (expected at least ${COMMAND_FLOOR}) ` +
        `— the extraction has stopped matching, not the commands gone away`,
    );
  }

  return { problems, invoked: invoked.size, registered: registered.size, calls };
}

function main() {
  const { problems, invoked, registered } = scan(repoRoot);
  if (problems.length > 0) {
    console.error("✗ ipc/commands:\n");
    for (const p of problems) console.error(`  • ${p}`);
    process.exit(1);
  }
  console.log(
    `✓ ipc/commands: all ${invoked} command names in ${IPC_FILE} match the ${registered} the ` +
      `backend registers`,
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
