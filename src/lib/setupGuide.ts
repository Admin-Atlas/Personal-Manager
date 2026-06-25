// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Copy for the document-engine troubleshooting guide (DocumentEngineGuide.tsx).
// Kept out of the component so the wording lives in one place, like help.ts /
// changelog.ts. The backend tags each failure with a `SidecarErrorKind`
// (src-tauri/src/sidecar.rs); `guideFor` turns that — plus the OS — into a short,
// actionable guide. Because the backend now finds modern Pythons automatically
// and rebuilds a stale venv, the macOS steps are just "install Python, then
// Retry" — no Terminal, PM_PYTHON, or manual venv deletion.

import type { SidecarErrorKind } from "./types";

/** True on macOS — same detection the title bar uses (no extra dependency). */
export const IS_MAC =
  typeof navigator !== "undefined" && /Mac|iPhone|iPad|iPod/.test(navigator.userAgent);

/** "install" is the proactive, not-yet-installed case; the rest are failures. */
export type SetupGuideMode = SidecarErrorKind | "install";

export interface SetupGuide {
  title: string;
  summary: string;
  steps: string[];
}

export function guideFor(mode: SetupGuideMode, isMac: boolean): SetupGuide {
  switch (mode) {
    case "install":
      return isMac
        ? {
            title: "Set up the document engine",
            summary:
              "PM converts and indexes your documents on your device using a small Python engine. Setting it up is a one-time step that needs Python 3.10 or newer.",
            steps: [
              "Make sure Python 3.10 or newer is installed — run `brew install python@3.12`, or download it from python.org and run the installer.",
              "Click “Set it up now”. PM finds your Python automatically and builds the engine — this can take a minute on the first run.",
            ],
          }
        : {
            title: "Set up the document engine",
            summary:
              "PM converts and indexes your documents on your device using a small Python engine. Setting it up is a one-time step.",
            steps: [
              "Click “Set it up now” — PM ships with everything it needs and builds the engine. This can take a minute on the first run.",
            ],
          };

    case "python_too_old":
    case "python_missing": {
      const summary =
        mode === "python_too_old"
          ? "PM's document engine needs Python 3.10 or newer, and the Python on this computer is older. (macOS ships an old Python, so this is common on a fresh Mac.)"
          : "PM's document engine needs Python 3.10 or newer, and PM couldn't find a suitable one on this computer.";
      return isMac
        ? {
            title: "PM needs a newer Python",
            summary,
            steps: [
              "Install Python 3.10 or newer — run `brew install python@3.12`, or download it from python.org and run the installer.",
              "Click Retry below. PM checks the usual install locations automatically — you don't need the Terminal or any environment variables.",
              "Still stuck? Open Technical details below to see exactly what PM found.",
            ],
          }
        : {
            title: "PM needs a newer Python",
            summary,
            steps: [
              "Install Python 3.10 or newer from python.org — tick “Add python.exe to PATH” in the installer.",
              "Click Retry below.",
              "Unusual setup? You can point PM at a specific interpreter with the PM_PYTHON environment variable.",
            ],
          };
    }

    case "pip_failed":
      return {
        title: "Couldn't download the engine's components",
        summary:
          "The first time it sets up, PM downloads a few Python packages. That download didn't finish — usually a network problem.",
        steps: [
          "Check your internet connection.",
          "A VPN, proxy, or strict firewall can block PyPI (pypi.org). Allow access or pause it, then Retry.",
          "Click Retry below once you're back online.",
        ],
      };

    case "requirements_missing":
      return {
        title: "Some app files are missing",
        summary:
          "Part of PM's document engine is missing from this install, so it can't be set up.",
        steps: [
          "Reinstall PM from the latest release.",
          "If it keeps happening, antivirus software may be quarantining files — allow PM, then reinstall.",
        ],
      };

    case "packaging_bug":
      return {
        title: "This is a problem with PM, not your computer",
        summary:
          "PM's document engine couldn't start because the Python that ships inside PM is incomplete on this install. That's a bug in PM's packaging — not your computer or your setup — so there's nothing for you to fix. Reporting it helps us fix it for everyone.",
        steps: [
          "Click “Report on GitHub” below — it opens a pre-filled report with the version and the technical details already attached. (Nothing from your documents is included.)",
          "As a workaround, reinstalling the latest version of PM from the releases page usually lays the files down correctly.",
          "Open Technical details below if you'd like to see exactly what the engine reported.",
        ],
      };

    case "unknown":
    default:
      return {
        title: "Document engine setup didn't finish",
        summary: "Setup hit a problem PM couldn't categorize.",
        steps: [
          "Click Retry below.",
          "If it keeps failing, open Technical details below and include that text when reporting the issue.",
        ],
      };
  }
}
