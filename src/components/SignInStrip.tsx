// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useLayoutEffect, useRef, useState } from "react";

import { openUrl } from "../lib/ipc";
import { useSignInPages } from "../lib/signInPages";
import type { SignInPage } from "../lib/types";
import { Button, Callout, CopyLinkButton } from "./ui";

/**
 * Browser sign-ins PM is waiting on: Google's, Microsoft's or Proton's page, which PM opens in the
 * default browser. That isn't always the browser signed in to the right account, so a strip offers
 * each page's link to copy into another one (the sign-in comes back to PM from any browser on this
 * computer) and to open again if the tab was closed. When the browser couldn't be opened at all, it
 * says so, and the link is the way in.
 *
 * Every sign-in goes through `oauth_loopback`, and App follows them all from launch
 * (`signInPages`), so a link shows wherever the strip is drawn and goes as soon as its wait ends,
 * however it ends. It is drawn where it can be used: inside Settings while Settings is open (every
 * sign-in starts there, and Settings is a dialog whose scrim covers the app), and at the top of the
 * app otherwise. The link carries the sign-in's one-time state, so it is only ever copied or opened,
 * never shown whole.
 */
export function SignInStrip() {
  const pages = useSignInPages();
  const box = useRef<HTMLDivElement>(null);
  // Whether focus was on one of the strips. A strip goes on its own when its sign-in ends, and the
  // focus on it would fall to the page's body, outside the Settings dialog it sat in: it's kept
  // here instead. Engines send no focusout for an element taken away, so this still holds then.
  const held = useRef(false);
  useLayoutEffect(() => {
    const el = box.current;
    if (el && held.current && !el.contains(document.activeElement)) el.focus();
  }, [pages]);
  return (
    <div
      ref={box}
      tabIndex={-1}
      className="outline-none"
      onFocus={() => {
        held.current = true;
      }}
      onBlur={(e) => {
        // PM's window losing focus (to the browser, for the sign-in) blurs the button too, with
        // nothing to go to: focus is still there for when the window comes back.
        if (e.relatedTarget === null && !document.hasFocus()) return;
        held.current = box.current?.contains(e.relatedTarget as Node | null) ?? false;
      }}
    >
      {pages.map((page) => (
        <SignInRow key={page.id} page={page} />
      ))}
    </div>
  );
}

/** One sign-in waiting. Keyed by its id, so nothing (a "Copied ✓") carries over to another. */
function SignInRow({ page }: { page: SignInPage }) {
  // A browser that wouldn't open, then did from here, has the page open after all.
  const [reopened, setReopened] = useState(false);
  const opened = page.opened || reopened;
  return (
    <Callout
      tone="info"
      variant="strip"
      size="md"
      body="ink"
      live
      className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1.5"
    >
      <span>
        {opened
          ? `${page.label}: finish signing in to ${page.provider} in your browser.`
          : `${page.label}: PM couldn't open your browser to sign in to ${page.provider}.`}{" "}
        <span className="text-ink3">
          {opened
            ? "To use a different browser on this computer, copy the link and open it there."
            : "Copy the link and open it in a browser on this computer."}
        </span>
      </span>
      <span className="flex shrink-0 gap-2">
        <CopyLinkButton url={page.url} to={`the ${page.label} sign-in`} />
        <Button
          variant="tertiary"
          size="sm"
          aria-label={`${opened ? "Open again" : "Try opening it"}: the ${page.label} sign-in`}
          onClick={() =>
            void openUrl(page.url).then(
              () => setReopened(true),
              () => {},
            )
          }
        >
          {opened ? "Open again" : "Try opening it"}
        </Button>
      </span>
    </Callout>
  );
}
