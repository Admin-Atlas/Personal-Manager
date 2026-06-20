// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useState } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

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
 */
export type UpdateStatus =
  | "idle" // no update, still checking, or check failed (silent)
  | "downloading" // a newer version is being fetched in the background
  | "ready" // downloaded and staged; waiting for the user to restart
  | "installing"; // applying the update and relaunching

export interface AppUpdate {
  status: UpdateStatus;
  version: string | null;
  /** 0–1 download progress, or null until content length is known. */
  progress: number | null;
  /** True once the user clicked "Later" — the banner collapses to a slim chip. */
  dismissed: boolean;
  restart: () => void;
  dismiss: () => void;
}

export function useUpdater(): AppUpdate {
  const [status, setStatus] = useState<UpdateStatus>("idle");
  const [version, setVersion] = useState<string | null>(null);
  const [progress, setProgress] = useState<number | null>(null);
  const [update, setUpdate] = useState<Update | null>(null);
  const [dismissed, setDismissed] = useState(false);

  useEffect(() => {
    let cancelled = false;

    (async () => {
      let found: Update | null = null;
      try {
        found = await check();
      } catch {
        // Dev build, offline, or unreachable feed — stay quiet, retry next launch.
        return;
      }
      if (!found || cancelled) return;

      setUpdate(found);
      setVersion(found.version);
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
    (async () => {
      setStatus("installing");
      try {
        await update.install();
        await relaunch();
      } catch {
        // If install fails, fall back to "ready" so the user can retry.
        setStatus("ready");
      }
    })();
  }, [update]);

  const dismiss = useCallback(() => {
    // Keep the staged update fully reachable; collapse the banner to a slim "restart"
    // chip rather than hiding it (status "idle" would lose it until the next launch).
    setDismissed(true);
  }, []);

  return { status, version, progress, dismissed, restart, dismiss };
}
