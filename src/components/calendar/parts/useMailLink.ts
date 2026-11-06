// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Mail links in an event's description (#884, C19b): the address goes to the mail app as a new
// message, through the backend's guarded opener (reader.rs's `link_to_open`, which drops a link's
// subject, body and other fields). When nothing opens, the description says so on a line under it,
// with the address, so it can still be copied. The one way PM opens a description's mail links: the
// formatted view's and the Markdown view's alike. The words are lib/linkFailure.ts's, which the
// app-wide link handler shows too; a click here takes down that handler's notice first, so a notice
// left by an earlier link never shows beside this line.

import { useCallback, useState, type MouseEvent as ReactMouseEvent } from "react";
import { openUrl } from "../../../lib/ipc";
import { dismissLinkFailure, mailFailureText, openLink } from "../../../lib/linkFailure";

/** Opens a description's mail links, and the line to show when one didn't open. `on` names what
 *  the description belongs to (the event's id): a line from one event never shows on the next one a
 *  re-pointed pop-up shows. */
export function useMailLink(on: string): {
  open: (target: string) => void;
  failure: string | null;
} {
  const [said, setSaid] = useState<{ on: string; text: string } | null>(null);
  const open = useCallback(
    (target: string) => {
      setSaid(null);
      dismissLinkFailure();
      void openUrl(target).catch((e: unknown) => setSaid({ on, text: mailFailureText(e, target) }));
    },
    [on],
  );
  return { open, failure: said?.on === on ? said.text : null };
}

/** A middle-click (any button but the first) on a description's link. The engine sends it as
 *  `auxclick`, never `click`, and its default loads the address in PM's own window: before PM's windows
 *  had a navigation guard (mods/mod.rs), WebKitGTK replaced the app with an error page for a mail link
 *  (C19 critique C2-H3, the details' links too; review 1). The guard now refuses that navigation, and
 *  this stays the first wall, so a link that isn't a web link does nothing. A web link
 *  opening in the browser opens there on the middle button, as Google Calendar's page opens a new tab:
 *  no webview does so by itself, and `useExternalLinks` hears only the first button (review 2). Its
 *  failure shows in the link notice, as that click's does. Two of these can hear one click, the
 *  formatted view's and the details' around it: the first to act stops the second. */
export function stopLinkAuxClick(e: ReactMouseEvent): void {
  if (e.defaultPrevented) return;
  const at = e.target instanceof Element ? e.target : null;
  const link = at?.closest("a") ?? null;
  if (link === null) return;
  const href = link.getAttribute("href") ?? "";
  if (link.getAttribute("target") === "_blank" && /^https?:\/\//i.test(href)) {
    if (e.button !== 1) return;
    e.preventDefault();
    openLink(href);
    return;
  }
  e.preventDefault();
}
