// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The IPC-command gate's own rules.
//
// The extraction is the whole gate, and it got this wrong twice before it ran clean — both times by
// skipping a real call site and then reporting its command as unwrapped. Those two shapes are
// pinned first, because a gate that silently stops seeing part of its subject is worse than no gate
// at all: it reports green over the thing it can no longer read.
//
// Importing the module does not run the gate — entry-point guard at the bottom of it.

import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import {
  registeredCommands,
  scanInvokeCalls,
  scan,
  IPC_FILE,
  LIB_RS,
} from "./check-ipc-commands.mjs";

describe("scanInvokeCalls", () => {
  it("reads a plain call and one with a simple type argument", () => {
    const { names, calls, unparsed } = scanInvokeCalls(
      `export const a = () => invoke<boolean>("has_openrouter_key");
       export const b = () => invoke("set_onboarding_done");`,
    );
    expect([...names].sort()).toEqual(["has_openrouter_key", "set_onboarding_done"]);
    expect(calls).toBe(2);
    expect(unparsed).toBe(0);
  });

  it("reads a NESTED type argument", () => {
    // Regression: `[^>]*` stopped at the first `>`, so both Drive owner wrappers were skipped and
    // then reported as commands the frontend never invokes.
    const { names, unparsed } = scanInvokeCalls(
      `export const owners = (email) => invoke<Record<string, string>>("drive_shared_owners", { email });`,
    );
    expect([...names]).toEqual(["drive_shared_owners"]);
    expect(unparsed).toBe(0);
  });

  it("reads a type argument containing a semicolon", () => {
    // Regression: narrowing the class to `[^(;]*` fixed nesting and broke this instead — an inline
    // object type separates its members with `;`.
    const { names, unparsed } = scanInvokeCalls(
      `export const cached = () =>
         invoke<{ document_id: number; proposal: MetadataProposal }[]>("cached_proposals");`,
    );
    expect([...names]).toEqual(["cached_proposals"]);
    expect(unparsed).toBe(0);
  });

  it("does not count the wrapper's own definition as a call", () => {
    const { names, calls } = scanInvokeCalls(
      `async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
         return tauriInvoke<T>(cmd, args);
       }
       export const a = () => invoke<void>("set_reranking", { enabled: true });`,
    );
    expect(calls).toBe(1);
    expect([...names]).toEqual(["set_reranking"]);
  });

  it("does not treat the import alias as a call", () => {
    const { calls } = scanInvokeCalls(`import { invoke as tauriInvoke, Channel } from "x";`);
    expect(calls).toBe(0);
  });

  it("counts a non-literal command name as unparsed rather than ignoring it", () => {
    // A name the gate cannot read is a name it cannot check — that has to be loud.
    const { names, calls, unparsed } = scanInvokeCalls(
      `const go = (suffix) => invoke<void>("set_" + suffix);`,
    );
    expect(names.size).toBe(0);
    expect(calls).toBe(1);
    expect(unparsed).toBe(1);
  });
});

describe("registeredCommands", () => {
  const RUST = `
    fn main() {
      builder
        .invoke_handler(tauri::generate_handler![
            commands::has_openrouter_key,
            // A comment naming set_something_else must not register anything.
            local_ai::probe_local_llm_ports,
            wipe::wipe_pm_data,
        ])
        .run();
    }
    #[tauri::command]
    pub fn never_registered() {}
  `;

  it("reads the handler list and strips module paths", () => {
    expect([...registeredCommands(RUST)].sort()).toEqual([
      "has_openrouter_key",
      "probe_local_llm_ports",
      "wipe_pm_data",
    ]);
  });

  it("ignores an attributed command that was never added to the list", () => {
    // The macro list is the authority: `#[tauri::command]` alone does not make a command reachable
    // from the webview, so counting attributes would report commands that cannot be called.
    expect(registeredCommands(RUST).has("never_registered")).toBe(false);
  });

  it("throws rather than reporting an empty set when the macro has moved", () => {
    expect(() => registeredCommands("fn main() {}")).toThrow(/generate_handler/);
  });
});

/** A throwaway tree holding just the two files the gate reads. */
function fixture(ipcSource, rustSource) {
  const root = mkdtempSync(join(tmpdir(), "pm-ipc-commands-"));
  mkdirSync(join(root, "src", "lib"), { recursive: true });
  mkdirSync(join(root, "src-tauri", "src"), { recursive: true });
  writeFileSync(join(root, IPC_FILE), ipcSource);
  writeFileSync(join(root, LIB_RS), rustSource);
  return root;
}

/** `count` well-formed wrapper/registration pairs, so a test's own case is what fails. */
function filler(count) {
  let ipc = "";
  let rust = ".invoke_handler(tauri::generate_handler![\n";
  for (let i = 0; i < count; i++) {
    ipc += `export const f${i} = () => invoke<void>("filler_${i}");\n`;
    rust += `    commands::filler_${i},\n`;
  }
  return { ipc, rust: rust + "])" };
}

describe("scan", () => {
  it("passes on the real tree, having read a realistic number of commands", () => {
    const root = new URL("..", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1");
    const { problems, invoked, registered } = scan(root);
    expect(problems).toEqual([]);
    expect(invoked).toBeGreaterThan(200);
    expect(registered).toBeGreaterThanOrEqual(invoked);
  });

  it("catches a command the frontend invokes that the backend does not register", () => {
    // The renamed-command case: the app compiles, ships, and rejects the first time that screen
    // opens.
    const f = filler(210);
    const root = fixture(
      f.ipc + `export const gone = () => invoke<void>("removed_command");\n`,
      f.rust,
    );
    expect(scan(root).problems.join(" ")).toMatch(
      /invokes `removed_command`, which .* not register/,
    );
  });

  it("catches a registered command that no wrapper reaches", () => {
    const f = filler(210);
    const root = fixture(f.ipc, f.rust.replace("])", "    commands::orphan_command,\n])"));
    expect(scan(root).problems.join(" ")).toMatch(/registers `orphan_command`, which no wrapper/);
  });

  it("accepts an unwrapped command that is explicitly excused", () => {
    const f = filler(210);
    const root = fixture(f.ipc, f.rust.replace("])", "    commands::set_milestone_state,\n])"));
    expect(scan(root).problems).toEqual([]);
  });

  it("refuses a truncated extraction rather than reporting a clean scan of nothing", () => {
    const f = filler(3);
    const root = fixture(f.ipc, f.rust);
    expect(scan(root).problems.join(" ")).toMatch(/extraction has stopped matching/);
  });

  it("reports a command name it could not read", () => {
    const f = filler(210);
    const root = fixture(f.ipc + `export const dyn = (s) => invoke<void>("set_" + s);\n`, f.rust);
    expect(scan(root).problems.join(" ")).toMatch(/not a plain string literal/);
  });
});
