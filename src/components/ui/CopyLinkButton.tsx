// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Copy link", beside whatever sends a page to the browser: a sign-in, a call, an event in Google
// Calendar. PM opens pages in the default browser, which isn't always the one signed in to the
// right account, so the link can be taken to another browser instead. Text links need none: the
// webview's own right-click menu already offers to copy them.

import { useEffect, useState } from "react";
import { Button, type ButtonProps } from "./Button";

/** How long "Copied ✓" shows before the button reads "Copy link" again, in ms. */
const COPIED_MS = 1500;

export interface CopyLinkButtonProps extends Omit<ButtonProps, "children" | "onClick"> {
  /** The link put on the clipboard. */
  url: string;
  /** What the link leads to, for its accessible name ("Copy link to the call"), so two of these on
   *  one screen can be told apart without seeing what each sits beside. */
  to?: string;
}

export function CopyLinkButton({
  url,
  to,
  variant = "tertiary",
  size = "sm",
  ...rest
}: CopyLinkButtonProps) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), COPIED_MS);
    return () => clearTimeout(t);
  }, [copied]);
  return (
    <Button
      type="button"
      variant={variant}
      size={size}
      // The visible words lead the name, so "Copy link" said aloud still finds it.
      aria-label={to ? `${copied ? "Copied" : "Copy"} link to ${to}` : undefined}
      {...rest}
      // `?.`: a webview without a clipboard leaves the button inert rather than throwing, and a
      // refused write leaves it reading "Copy link".
      onClick={() =>
        void navigator.clipboard?.writeText(url).then(
          () => setCopied(true),
          () => {},
        )
      }
    >
      {copied ? "Copied ✓" : "Copy link"}
    </Button>
  );
}
