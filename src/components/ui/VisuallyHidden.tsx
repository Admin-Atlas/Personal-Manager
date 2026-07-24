// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Content present for assistive tech but removed from the visual layout — wraps Tailwind's
// `sr-only`. The cheapest way to add an accessible name or a status announcement without any
// visual change: author labels on chat bubbles ("You" / "Assistant"), a live-region node for a
// streaming reply, or a name for an icon-only control. Render it as a `<span>` by default; pass
// `role`/`aria-live` through for a live region (e.g. `<VisuallyHidden role="status" aria-live="polite">`).

import type { ComponentPropsWithoutRef } from "react";
import { cn } from "./cn";

export function VisuallyHidden({ className, ...rest }: ComponentPropsWithoutRef<"span">) {
  return <span className={cn("sr-only", className)} {...rest} />;
}
