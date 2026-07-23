// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useState } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { getVersion } from "@tauri-apps/api/app";
import { smartAppControlState, packageManagedLinux } from "./ipc";
import type { SmartAppControlState } from "./types";
import { evaluateAttemptMarker } from "./updateGate";

/**
 * Background auto-update. On launch we silently ask the release feed whether a
 * newer version exists; if so we download it quietly and surface a dismissible
 * banner inviting the user to restart whenever they like. The actual install +
 * relaunch only happens on that explicit click, so an update never interrupts
 * work in progress.
 *
 * Every step is best-effort: offline, dev builds (no signature), or a malformed
 * feed all just leave the app in the "idle" state with no visible error — the
 * check simply runs again next launch.
 *
 * Linux caveat this hook defends against: Tauri's updater applies an update by replacing the
 * running AppImage in place (found via the `APPIMAGE` env var). A package install (rpm/deb) sets
 * no such variable, but `check()` compares versions only — so a package install is still offered
 * the AppImage update and would silently download the whole thing before `install()` fails. We ask
 * the backend up front whether this is a package install and, if so, skip the download entirely and
 * surface a "reinstall to update" banner pointing at the releases page instead.
 *
 * Windows caveat this hook defends against: our installer is an unsigned NSIS setup, and
 * the updater plugin applies an update by launching it and exiting the process WITHOUT
 * observing whether it launched. So an OS block — Smart App Control (SAC) enforced, or a
 * SmartScreen "Don't run" — silently closes the app and it reopens on the old version, and
 * `install()` never throws (the `catch` below is unreachable on that path). Two guards:
 * (1) we read SAC state up front and, when it's enforcing, warn instead of firing a restart
 * that would no-op; (2) we record the version we're about to install and, if the app reopens
 * still on the old one, flag it next launch instead of silently re-offering the same update.
 */
export type UpdateStatus =
  | "idle" // no update, still checking, or check failed (silent)
  | "downloading" // a newer version is being fetched in the background
  | "ready" // downloaded and staged; waiting for the user to restart
  | "installing"; // applying the update and relaunching

/** localStorage key: the version we last began an install for (see the loop-marker logic). */
const ATTEMPT_KEY = "pm.update.attemptedVersion";

export interface AppUpdate {
  status: UpdateStatus;
  version: string | null;
  /** 0–1 download progress, or null until content length is known. */
  progress: number | null;
  /** True once the user clicked "Later" — the banner collapses to a slim chip. */
  dismissed: boolean;
  /** True after an in-place install threw — the banner offers a manual download instead.
   *  Reachable on macOS (Gatekeeper can refuse the swapped app); on Windows the plugin exits
   *  the process before it can throw, so the loop marker below covers that case instead. */
  installFailed: boolean;
  /** Windows Smart App Control state. When "enforced", a restart would be silently blocked,
   *  so the banner warns and explains how to proceed rather than offering it. */
  sac: SmartAppControlState;
  /** True when a prior install attempt silently didn't apply (the app reopened on the old
   *  version and the feed is re-offering the same update) — the banner warns instead of
   *  looping a download-and-fail. */
  blockedByPriorAttempt: boolean;
  /** True on a Linux package install (rpm/deb), which the in-app updater can't apply in place.
   *  The banner then invites a reinstall from the releases page rather than a restart, and no
   *  AppImage is downloaded. False on Windows, macOS, and the Linux AppImage. */
  packageManaged: boolean;
  /** The releases page for the manual-download fallback. */
  releasesUrl: string;
  restart: () => void;
  dismiss: () => void;
}

/** The "latest release" page — the manual-download fallback when an auto-update can't apply. */
const RELEASES_URL = "https://github.com/Admin-Atlas/Personal-Manager/releases/latest";

export function useUpdater(): AppUpdate {
  const [status, setStatus] = useState<UpdateStatus>("idle");
  const [version, setVersion] = useState<string | null>(null);
  const [progress, setProgress] = useState<number | null>(null);
  const [update, setUpdate] = useState<Update | null>(null);
  const [dismissed, setDismissed] = useState(false);
  const [installFailed, setInstallFailed] = useState(false);
  const [sac, setSac] = useState<SmartAppControlState>("unknown");
  const [blockedByPriorAttempt, setBlockedByPriorAttempt] = useState(false);
  const [packageManaged, setPackageManaged] = useState(false);

  useEffect(() => {
    let cancelled = false;

    void (async () => {
      // Smart App Control state is best-effort and independent of whether an update exists.
      try {
        const s = await smartAppControlState();
        if (!cancelled) setSac(s);
      } catch {
        // Leave "unknown" — the UI treats that as "proceed normally".
      }

      // Is this a Linux package install (rpm/deb)? Independent of whether an update exists, and
      // best-effort — default false (behave as a self-updating build) if the query fails.
      let pkg = false;
      try {
        pkg = await packageManagedLinux();
        if (!cancelled) setPackageManaged(pkg);
      } catch {
        // Leave false — treat as self-updating.
      }

      let running = "";
      try {
        running = await getVersion();
      } catch {
        // Non-Tauri context or API error — the marker logic tolerates an empty running version.
      }

      let found: Update | null;
      try {
        found = await check();
      } catch {
        // Dev build, offline, or unreachable feed — stay quiet, retry next launch.
        return;
      }

      // Reconcile the "previous attempt" marker: did the last restart actually apply?
      try {
        const attempted = localStorage.getItem(ATTEMPT_KEY);
        const decision = evaluateAttemptMarker({
          attempted,
          running,
          offered: found?.version ?? null,
        });
        if (decision.clearMarker) localStorage.removeItem(ATTEMPT_KEY);
        if (decision.blocked && !cancelled) setBlockedByPriorAttempt(true);
      } catch {
        // localStorage unavailable — skip the loop guard, everything else still works.
      }

      if (!found || cancelled) return;

      setVersion(found.version);

      // A Linux package install can't be updated in place by the plugin — don't download the
      // AppImage (it would only fail to apply). Surface a "reinstall to update" banner instead.
      if (pkg) {
        setStatus("ready");
        return;
      }

      setUpdate(found);
      setStatus("downloading");

      try {
        let downloaded = 0;
        let total = 0;
        await found.download((event) => {
          if (event.event === "Started") {
            total = event.data.contentLength ?? 0;
          } else if (event.event === "Progress") {
            downloaded += event.data.chunkLength;
            setProgress(total > 0 ? downloaded / total : null);
          }
        });
        if (!cancelled) {
          setProgress(1);
          setStatus("ready");
        }
      } catch {
        // Download failed mid-flight — drop back to idle, try again next launch.
        if (!cancelled) {
          setStatus("idle");
          setUpdate(null);
        }
      }
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  const restart = useCallback(() => {
    if (!update) return;
    void (async () => {
      // The user may have toggled Smart App Control since launch — re-check at click time.
      let current = sac;
      try {
        current = await smartAppControlState();
        setSac(current);
      } catch {
        // Keep the last-known state.
      }
      if (current === "enforced") {
        // Do NOT call install(): under SAC-enforced the plugin would launch the unsigned
        // installer, get silently blocked, and exit(0) — closing PM with no signal and no
        // update. Leave the app running; the banner explains that SAC must be turned off.
        return;
      }

      setStatus("installing");
      setInstallFailed(false);
      try {
        // Record the version we're about to install so that, if the app reopens still on the
        // old version (a silent OS block we can't catch inside install()), the next launch
        // warns instead of silently re-offering the same update.
        if (update.version) {
          try {
            localStorage.setItem(ATTEMPT_KEY, update.version);
          } catch {
            // Non-fatal — the SAC pre-check is the primary guard.
          }
        }
        await update.install();
        await relaunch();
      } catch {
        // The in-place update threw (reachable on macOS — Gatekeeper can refuse the swapped
        // bundle). We got a real signal, so clear the marker and offer a manual download.
        try {
          localStorage.removeItem(ATTEMPT_KEY);
        } catch {
          // ignore
        }
        setStatus("ready");
        setInstallFailed(true);
      }
    })();
  }, [update, sac]);

  const dismiss = useCallback(() => {
    // Keep the staged update fully reachable; collapse the banner to a slim "restart"
    // chip rather than hiding it (status "idle" would lose it until the next launch).
    setDismissed(true);
  }, []);

  return {
    status,
    version,
    progress,
    dismissed,
    installFailed,
    sac,
    blockedByPriorAttempt,
    packageManaged,
    releasesUrl: RELEASES_URL,
    restart,
    dismiss,
  };
}
