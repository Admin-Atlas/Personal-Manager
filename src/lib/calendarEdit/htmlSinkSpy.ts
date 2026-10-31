// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Test-only: imported by the jsdom suites, never by the app. Watches every way a page could parse
// HTML or write it in (INVARIANTS.md I-23, the C19 spec's §3.1), so a suite that draws a Google
// description can prove the view never used one: the ESLint fence reads the source, and this watches
// what runs, React and the libraries included. Not named `*.test.ts`, so vitest doesn't collect it as
// an empty suite; nothing in the app imports it, so the bundle never holds it.
//
// Each sink is wrapped in place and still does its work; restore() puts the originals back. Start it
// before `render` and restore it before `cleanup`: the views write while they render, and some
// suites' cleanup empties `document.body` through one of these very sinks.

export interface HtmlSinkSpy {
  /** Each sink used since the spy started, by name, in order. */
  readonly sinks: string[];
  /** Each `execCommand` command name, in order. */
  readonly commands: string[];
  restore(): void;
}

/** Start watching. A sink jsdom lacks is an error rather than a sink silently not watched. */
export function spyOnHtmlSinks(): HtmlSinkSpy {
  const sinks: string[] = [];
  const commands: string[] = [];
  const undo: (() => void)[] = [];
  const watch = (owner: object, key: string, name: string, part: "set" | "value") => {
    const was = Object.getOwnPropertyDescriptor(owner, key);
    const inner: unknown = part === "set" ? was?.set : was?.value;
    if (was === undefined || typeof inner !== "function") {
      throw new Error(`jsdom has no ${name} to watch`);
    }
    const wrapped = function (this: unknown, ...args: unknown[]): unknown {
      sinks.push(name);
      return (inner as (...a: unknown[]) => unknown).apply(this, args);
    };
    Object.defineProperty(owner, key, { ...was, [part]: wrapped });
    undo.push(() => Object.defineProperty(owner, key, was));
  };
  watch(Element.prototype, "innerHTML", "innerHTML", "set");
  watch(Element.prototype, "outerHTML", "outerHTML", "set");
  watch(ShadowRoot.prototype, "innerHTML", "ShadowRoot innerHTML", "set");
  watch(Element.prototype, "insertAdjacentHTML", "insertAdjacentHTML", "value");
  watch(Range.prototype, "createContextualFragment", "createContextualFragment", "value");
  watch(DOMParser.prototype, "parseFromString", "parseFromString", "value");
  watch(Document.prototype, "write", "document.write", "value");
  watch(Document.prototype, "writeln", "document.writeln", "value");

  // jsdom has no execCommand: a stand-in records each command and does nothing.
  const doc = document;
  const own = Object.getOwnPropertyDescriptor(doc, "execCommand");
  Object.defineProperty(doc, "execCommand", {
    configurable: true,
    writable: true,
    value: (command: string) => {
      commands.push(command);
      return false;
    },
  });
  undo.push(() => {
    if (own === undefined) Reflect.deleteProperty(doc, "execCommand");
    else Object.defineProperty(doc, "execCommand", own);
  });

  return {
    sinks,
    commands,
    restore() {
      // Last wrapped, first restored; a second call finds nothing left to do.
      for (const step of undo.splice(0).reverse()) step();
    },
  };
}
