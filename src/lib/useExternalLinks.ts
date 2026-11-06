// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// External links open in the system browser. The webview can't honour an `<a target="_blank">` on
// its own (PM ships no shell/opener plugin), so intercept clicks on any such link app-wide and hand
// the URL to the OS browser via the backend (`open_url`, which opens web and mail links and nothing
// else). One handler covers every link — existing and future — so individual `<a>`s need no special
// wiring.
//
// It lives here rather than inline in App because App is NOT the only root: `main.tsx` forks on
// `?window=briefing` and renders `PopoverRoot` instead, with App behind a lazy import that window
// never downloads. While this was an inline effect in App, every link in the always-on-top briefing
// — including the bare-URL autolinks remark-gfm makes out of model prose — was silently dead: the
// webview swallows a `target="_blank"` it has no handler for, with no error and no feedback. Both
// roots mount this hook; nothing else should re-implement it.
//
// `open_url` is one of PM's own `#[tauri::command]`s, which are not ACL-gated, so mounting this in
// the briefing window needs no change to `capabilities/briefing.json` and keeps that window's
// invariant (PM app commands ONLY). Keep this file free of any `@tauri-apps` plugin import: a
// `plugin:`-prefixed call would fail at runtime in that window alone, with nothing in `just check`
// catching it.
//
// Deliberately fail-closed and deliberately NOT the place to fix a hostile href: it fires only for
// `target="_blank"` + an `http(s)://` prefix on the RAW attribute, so a schemeless `//host` target
// never reaches it (`rehype-external-links` doesn't mark those external either). Neutralising those
// is `safeUrl`'s job, upstream in `markdown.tsx`. Do not widen either condition here — dropping the
// `target === "_blank"` test would newly intercept in-app anchors across the whole main window.
//
// The one other link it takes is a mail link (a RAW href starting `mailto:`, any target): rendered
// Markdown (chat, the reader, the briefing) keeps them (`safeUrl`) without a target, and PM's
// navigation guard refuses the click's default, so without this a mail link did nothing. It goes to
// the mail app through the same backend opener, which keeps only the address. A handler that has
// already taken a mail link (a calendar description's) prevents the default first, and this leaves
// it alone.
//
// A link that didn't open says so in the link notice (lib/linkFailure.ts, drawn by LinkNotice): the
// backend's words, and for a mail link the address, as an event description's mail links do. A
// machine with no mail app set up is common on Linux, and without this the click did nothing.

import { useEffect } from "react";
import { openLink } from "./linkFailure";

/** Route clicks on `target="_blank"` http(s) links to the OS browser, and on mail links to the mail
 *  app; one that didn't open shows in the link notice. Mount once per webview root. */
export function useExternalLinks(): void {
  useEffect(() => {
    function onClick(e: MouseEvent) {
      if (e.defaultPrevented || e.button !== 0) return;
      const anchor = (e.target as HTMLElement | null)?.closest?.("a");
      const href = anchor?.getAttribute("href");
      if (!href) return;
      const web = anchor?.target === "_blank" && /^https?:\/\//i.test(href);
      if (web || /^mailto:/i.test(href)) {
        e.preventDefault();
        // A new click takes down the last one's notice first, so it never stays up for a link that
        // opened.
        openLink(href);
      }
    }
    document.addEventListener("click", onClick);
    return () => document.removeEventListener("click", onClick);
  }, []);
}
