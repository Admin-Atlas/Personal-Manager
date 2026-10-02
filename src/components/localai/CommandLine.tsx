// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { Button } from "../ui";

/**
 * One command to run, with a way to copy it. Shown whole, wrapping rather than truncated: the end of
 * a llama-server line is the part that carries the settings PM sized the model for.
 *
 * Shared by the start card's steps and the models under Already on this device, which give the same
 * command for the same file — one component, so the two can't come to look like two different things.
 */
export function CommandLine({ command }: { command: string }) {
  return (
    <div className="mt-1.5 flex items-start gap-2">
      <code className="min-w-0 flex-1 break-all rounded-[var(--radius-sm)] bg-bg px-2 py-1 font-mono text-[0.6875rem] text-ink3">
        {command}
      </code>
      <Button
        variant="tertiary"
        size="sm"
        onClick={() => void navigator.clipboard?.writeText(command)}
      >
        Copy
      </Button>
    </div>
  );
}
