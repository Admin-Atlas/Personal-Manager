// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Mail links in an event's description (#884, C19b): the address goes to the mail app as a new
// message, through the backend's guarded opener (reader.rs's `link_to_open`, which drops a link's
// subject, body and other fields). When nothing opens, the description says so on a line under it,
// with the address, so it can still be copied. The one way PM opens a description's mail links: the
// formatted view's and the Markdown view's alike.

import { useCallback, useState, type MouseEvent as ReactMouseEvent } from "react";
import { openUrl } from "../../../lib/ipc";

/** What the backend refuses a mail link the mail app didn't open with (reader.rs's
 *  `MAIL_APP_FAILED`): matched exactly, to add the address. */
export const MAIL_APP_FAILED = "PM couldn't open your mail app.";

/** The line under a description when a mail link didn't open: the backend's own words for a link it
 *  refused, or that the mail app didn't open, with the address. */
export function mailFailureText(error: unknown, target: string): string {
  if (typeof error === "string" && error !== MAIL_APP_FAILED) return error;
  // The address alone, as the backend sends it: the part before any "?" or "#".
  const address = target.slice("mailto:".length).split(/[?#]/)[0];
  return `${MAIL_APP_FAILED} The address is ${address}.`;
}

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
      void openUrl(target).catch((e: unknown) => setSaid({ on, text: mailFailureText(e, target) }));
    },
    [on],
  );
  return { open, failure: said?.on === on ? said.text : null };
}

/** A middle-click (any button but the first) on a description's link that isn't a web link opening
 *  in the browser: nothing. The engine sends it as `auxclick`, never `click`, and its default loads
 *  the address in PM's own window, which has no navigation guard: WebKitGTK replaced the app with an
 *  error page for a mail link (C19 critique C2-H3, the details' links too; review 1). */
export function stopLinkAuxClick(e: ReactMouseEvent): void {
  const at = e.target instanceof Element ? e.target : null;
  const link = at?.closest("a") ?? null;
  if (link === null) return;
  const href = link.getAttribute("href") ?? "";
  if (link.getAttribute("target") === "_blank" && /^https?:\/\//i.test(href)) return;
  e.preventDefault();
}
