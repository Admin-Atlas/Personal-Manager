// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { PLATFORM, type SetupPlatform } from "../lib/setupGuide";
import type { AppUpdate } from "../lib/useUpdater";
import { Button } from "./ui";

/**
 * Slim banner shown once a new version has been downloaded in the background.
 * Restarting is always the user's choice — until then the staged update sits
 * quietly and the app keeps working on the current version.
 *
 * Failure surfaces two ways. On Windows, Smart App Control (when enforced) blocks our
 * unsigned installer with no override, so we detect that up front and explain how to proceed
 * rather than firing a restart that can't succeed. Otherwise, if an install threw (macOS
 * Gatekeeper, or Windows refusing to launch the installer) or a prior attempt silently didn't
 * apply, we point the user at a manual download. A throw on Windows can't be retried in the
 * same session — the plugin cleared the staged update before the launch failed — so that banner
 * says to reopen PM instead of offering "Try again".
 */
export function UpdateBanner({
  update,
  platform = PLATFORM,
}: {
  update: AppUpdate;
  /** The running OS — a prop only so tests can pick one; App passes nothing. */
  platform?: SetupPlatform;
}) {
  if (update.status !== "ready" && update.status !== "installing") return null;

  const shell =
    "flex items-center justify-between gap-3 border-b border-border bg-accent-soft px-4 py-2 text-sm text-ink2";
  const actions = "flex shrink-0 items-center gap-2";

  if (update.status === "installing") {
    return (
      <div className={shell}>
        <span>Installing update…</span>
      </div>
    );
  }

  const label = update.version ? `Version ${update.version}` : "An update";

  // A Linux package install (rpm/deb) can't be updated in place — Tauri's updater only swaps an
  // AppImage. So we never downloaded one; invite a reinstall from the releases page instead of a
  // restart that would do nothing. (Windows/macOS and the AppImage never take this branch.)
  if (update.packageManaged) {
    if (update.dismissed) {
      return (
        <div className="flex items-center justify-end gap-2 border-b border-border bg-accent-soft px-4 py-1 text-xs text-ink3">
          <span>{label} available — reinstall to update</span>
          <a
            href={update.releasesUrl}
            target="_blank"
            rel="noreferrer"
            className="font-medium text-ink underline underline-offset-2 hover:text-ink2"
          >
            Get it
          </a>
        </div>
      );
    }
    return (
      <div className={shell}>
        <span>
          {label} is available. PM was installed from a package, so it can&apos;t update itself —
          download the new package and reinstall to update.
        </span>
        <span className={actions}>
          <a
            href={update.releasesUrl}
            target="_blank"
            rel="noreferrer"
            className="font-medium text-ink underline underline-offset-2 hover:text-ink2"
          >
            Get the update
          </a>
          <Button variant="tertiary" size="sm" onClick={update.dismiss}>
            Later
          </Button>
        </span>
      </div>
    );
  }

  const sacBlocked = update.sac === "enforced";
  // A restart threw (macOS, or Windows when the installer wouldn't launch) or a prior attempt
  // silently didn't apply (a Windows block after the installer launched, once PM had exited) —
  // a manual download is the way forward in both.
  const installFailed = !sacBlocked && (update.installFailed || update.blockedByPriorAttempt);
  // Can a restart work in THIS session? Not after an install threw on Windows: before the launch
  // failed, the updater plugin cleared the staged update (and hid every window, dropped the tray),
  // so only a fresh PM can download it again. A prior attempt's silent block stays retryable —
  // this session downloaded the update afresh.
  const retryable = !(update.installFailed && platform === "windows");

  // After "Later", collapse to a slim, always-reachable chip so the staged update isn't lost
  // for the session — the user can still restart (or retry once SAC is off) whenever they like.
  if (update.dismissed) {
    return (
      <div className="flex items-center justify-end gap-2 border-b border-border bg-accent-soft px-4 py-1 text-xs text-ink3">
        <span>
          {sacBlocked
            ? `${label} paused — Smart App Control is on`
            : !retryable
              ? `${label} couldn't install — reopen PM to try again`
              : installFailed
                ? `${label} couldn't install`
                : `${label} ready`}
        </span>
        {retryable ? (
          <Button variant="tertiary" size="sm" onClick={update.restart}>
            {sacBlocked ? "Try again" : "Restart to update"}
          </Button>
        ) : (
          <a
            href={update.releasesUrl}
            target="_blank"
            rel="noreferrer"
            className="font-medium text-ink underline underline-offset-2 hover:text-ink2"
          >
            Get it
          </a>
        )}
      </div>
    );
  }

  // Smart App Control is enforcing: a restart would be silently blocked. Explain the one path
  // that works — turning SAC off — instead of a manual download (which SAC blocks just the same).
  if (sacBlocked) {
    return (
      <div className={shell}>
        <span>
          {label} is ready, but Windows Smart App Control is blocking the installer. To update, turn
          Smart App Control off in Windows Security (App and browser control), then click Restart. A
          manual download will not run while it is on.
        </span>
        <span className={actions}>
          <Button variant="primary" size="sm" onClick={update.restart}>
            Restart
          </Button>
          <Button variant="tertiary" size="sm" onClick={update.dismiss}>
            Later
          </Button>
        </span>
      </div>
    );
  }

  // The in-place update couldn't apply — point the user at a manual download from the releases
  // page instead of silently looping a failing restart. When this session can't retry, say how to
  // get a fresh attempt instead of offering a "Try again" that would only fail again.
  if (installFailed) {
    return (
      <div className={shell}>
        <span>
          Couldn&apos;t install the update automatically
          {update.version ? ` (version ${update.version})` : ""}.
          {!retryable &&
            " Close PM and open it again to retry, or download it from the releases page."}
        </span>
        <span className={actions}>
          <a
            href={update.releasesUrl}
            target="_blank"
            rel="noreferrer"
            className="font-medium text-ink underline underline-offset-2 hover:text-ink2"
          >
            Download it manually
          </a>
          {retryable ? (
            <Button variant="tertiary" size="sm" onClick={update.restart}>
              Try again
            </Button>
          ) : (
            <Button variant="tertiary" size="sm" onClick={update.dismiss}>
              Later
            </Button>
          )}
        </span>
      </div>
    );
  }

  return (
    <div className={shell}>
      <span>{label} is ready to install.</span>
      <span className={actions}>
        <Button variant="primary" size="sm" onClick={update.restart}>
          Restart now
        </Button>
        <Button variant="tertiary" size="sm" onClick={update.dismiss}>
          Later
        </Button>
      </span>
    </div>
  );
}
