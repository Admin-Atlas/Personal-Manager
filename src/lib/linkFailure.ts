// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What PM says when a link it handed to the OS didn't open, and the one notice that says it.
//
// The words: the backend's own (reader.rs's `open_url`), and for a mail link the address too, so it
// can still be copied.
//
// Where they show. Every click PM sends to `open_url` is one of two kinds:
// - With no line of its own, it opens through `openLink` and a failure shows in the link notice:
//   the app-wide handler (useExternalLinks: chat, the reader, the briefing), a middle-click on a
//   description's web link and a Ctrl+click on one in the description editor, the right-click
//   menu's Open link, a note's Ctrl+click (NoteEditor, RichNoteEditor) and Proton Drive's install
//   link.
// - With a line of its own beside what was clicked, it says the failure there: an event
//   description's mail links (useMailLink, under the description), the call's Join and "Open in
//   Google Calendar" (MeetField, the event pop-up), and the sign-in strip's Open again (the strip
//   then says the browser didn't open, in its own words: the link carries one-time state). Each
//   takes the notice down first, so a notice left by an earlier link never shows beside its line.
// None is silent. linkFailure.test.tsx reads src/'s non-test files with the TypeScript parser, each
// as the bundler does (a .js file as JavaScript), and accounts for every identifier named `openUrl`
// in them and every literal that spells its command. Comments aren't code, so they may name either.
// - Calls (`openUrl(u)`, and `openUrl?.(u)` or `(openUrl)(u)`): only `openLink` here and the four
//   files of the second kind may make one, as many as it pins for each.
// - Imports: by name from lib/ipc (a relative path to it, with or without `.ts` or `.js`), never
//   under another name. No file reaches lib/ipc any other way: a namespace import, a re-export of
//   all of it, a require or a dynamic import of it (`ipc/.` and `ipc/x/..` are it too) or of
//   anything but a plain string, or Vite's `import.meta.glob` (refused anywhere). So no file
//   holds lib/ipc as an object for `ipc.openUrl(u)` or `ipc["openUrl"](u)`, and no string,
//   template, regex or JSX text may spell the name.
// - Any other `openUrl` is pinned by file, role and line: ipc.ts's `export const`, and the
//   right-click menu's `openUrl` action (contextMenu.ts's type member and `actions.openUrl(link)`),
//   which ContextMenuHost's `openUrl: openLink` fills.
// - The command: the one string, template, regex or JSX text that spells `open_url` is on ipc.ts's
//   `export const openUrl` line, so no second wrapper names it, and scripts/check-ipc-commands.mjs
//   refuses an `invoke(` call in ipc.ts whose command isn't a plain string.
// Every call is a statement of its own (under `void`, `await` or parentheses at most) with
// `dismissLinkFailure();` straight before it in the same block; only declarations of names,
// property reads and literals, like `const on = event.id;` or `const { id: on } = event;`, may
// come between (a read is by dot or bracket, `ids[i]` too, and may have `?.`, `!`, `as T`,
// `satisfies T` or parentheses on it, as in `const on = (event as Ev)?.id!;`). That name is this
// file's: declared here, imported by name from lib/linkFailure once in any other caller, and
// otherwise only called.
// It handles its failure on the call itself, `.catch(h)` or `.then(ok, h)` (a `.finally(f)` may
// sit on the chain too, but handles nothing), where `h` is an inline arrow or function, not a
// generator, whose body makes a call or a `new` of its own: outside any function, class or type it
// defines and any `declare` (the build erases it, as it does a type), so neither a default in a
// function it only defines nor a type's `{ m(): void }` counts.
// So `() => {}`, `() => undefined`, `(e) => void e`, `(e) => e`, `() => () => say()` and
// `() => { const f = () => say(); }` are refused, and so are a name and a try/catch around an
// awaited call, which the test can't read. One thing it can't tell is whether that call says the
// failure: `setFailed(true)` passes and so does `console.log(e)`. That is for review.
// The test catches a mistake, such as a new caller, a second wrapper of the command, or a handler
// that swallows the failure. It does not stop code written to get past it, such as a command name
// built at runtime or a lookup on Tauri's global. Everyone who can change src/ can also change this
// test, so review catches that.
//
// The notice is one at a time and app-wide, so it is a tiny store, like the right-click menu
// (contextMenu.ts): a click reports here, and the one `LinkNotice` (mounted in main.tsx, so every
// window and every one of App's screens has it) draws it.

import { useSyncExternalStore } from "react";
import { openUrl } from "./ipc";

/** What the backend refuses a mail link the mail app didn't open with (reader.rs's
 *  `MAIL_APP_FAILED`): matched exactly, to add the address. */
export const MAIL_APP_FAILED = "PM couldn't open your mail app.";

/** The words for a mail link that didn't open: the backend's own for a link it refused, or that
 *  the mail app didn't open, with the address. */
export function mailFailureText(error: unknown, target: string): string {
  if (typeof error === "string" && error !== MAIL_APP_FAILED) return error;
  // The address alone, as the backend sends it: the part before any "?" or "#".
  const address = target.slice("mailto:".length).split(/[?#]/)[0];
  return `${MAIL_APP_FAILED} The address is ${address}.`;
}

/** The words for any link `open_url` refused or couldn't open: a mail link's as above, a web
 *  link's the backend's own, and a plain line when the error isn't PM's. */
export function linkFailureText(error: unknown, href: string): string {
  if (/^mailto:/i.test(href)) return mailFailureText(error, href);
  return typeof error === "string" ? error : "PM couldn't open the link.";
}

// ---- The notice ---------------------------------------------------------------------------------

let current: string | null = null;
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((l) => l());

/** Show `text` in the link notice, in place of any notice already up. */
export function showLinkFailure(text: string): void {
  current = text;
  emit();
}

/** Take the link notice down (its Dismiss, or a new click on a link). */
export function dismissLinkFailure(): void {
  if (current === null) return;
  current = null;
  emit();
}

/** Open `href` (a web or mail link) through the backend, for a click with no line of its own to
 *  say a failure on: the last link's notice goes at once, and this one's failure shows in it. */
export function openLink(href: string): void {
  dismissLinkFailure();
  void openUrl(href).catch((err: unknown) => showLinkFailure(linkFailureText(err, href)));
}

/** What the link notice says now; null when it is down. */
export function useLinkFailure(): string | null {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => current,
  );
}
