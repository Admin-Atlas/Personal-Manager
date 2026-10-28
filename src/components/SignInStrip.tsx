// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from "react";

import { onSignInClosed, onSignInOpened, openUrl } from "../lib/ipc";
import { subscribeUntilCleanup } from "../lib/subscribe";
import type { SignInPage } from "../lib/types";
import { Button, Callout, CopyLinkButton } from "./ui";

/**
 * A browser sign-in PM is waiting on, as an app-wide strip: Google's, Microsoft's or Proton's page,
 * which PM opens in the default browser. That isn't always the browser signed in to the right
 * account, so the strip offers the link to copy into another one (the sign-in comes back to PM from
 * any browser on this computer) and to open again if the tab was closed. When the browser couldn't
 * be opened at all, it says so, and the link is the way in.
 *
 * Every sign-in goes through `oauth_loopback::open_sign_in`, so one strip covers them all, wherever
 * the button that started it is. It is subscribed for as long as the app is open, before any
 * sign-in can start, and the link goes as soon as the wait ends, however it ends. The link carries
 * the sign-in's one-time state, so it is only ever copied or opened, never shown whole.
 */
export function SignInStrip() {
  const [pages, setPages] = useState<SignInPage[]>([]);
  useEffect(
    () =>
      subscribeUntilCleanup(() =>
        onSignInOpened((page) => setPages((all) => [...all.filter((p) => p.id !== page.id), page])),
      ),
    [],
  );
  useEffect(
    () =>
      subscribeUntilCleanup(() =>
        onSignInClosed((id) => setPages((all) => all.filter((p) => p.id !== id))),
      ),
    [],
  );
  // The newest: two at once means one was started from another section while the first waited.
  const page = pages[pages.length - 1];
  if (!page) return null;
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
        {page.opened
          ? `Finish signing in to ${page.provider} in your browser.`
          : `PM couldn't open your browser to sign in to ${page.provider}.`}{" "}
        <span className="text-ink3">
          {page.opened
            ? "To use a different browser on this computer, copy the link and open it there."
            : "Copy the link and open it in a browser on this computer."}
        </span>
      </span>
      <span className="flex shrink-0 gap-2">
        <CopyLinkButton url={page.url} />
        <Button variant="tertiary" size="sm" onClick={() => void openUrl(page.url).catch(() => {})}>
          {page.opened ? "Open again" : "Try opening it"}
        </Button>
      </span>
    </Callout>
  );
}
