// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Install copy for local model runners, shown in the Local AI tab (#296). Platform-keyed like
// setupGuide.ts so the wording lives in one place. PM recommends and points — it never bundles or
// provisions a runner. You install Ollama or LM Studio yourself, and your runner fetches the model
// weights (from the Ollama registry / Hugging Face), never PM. The three runners PM auto-detects are
// Ollama (:11434), LM Studio (:1234), and llama-server (:8080); only Ollama has a one-click pull API,
// so the others get a copy-paste command instead.

import { PLATFORM, type SetupPlatform } from "./setupGuide";

export interface RunnerGuide {
  /** Runner name, e.g. "Ollama". */
  name: string;
  /** One-line description of what it is. */
  summary: string;
  /** Ordered install steps for this platform. */
  steps: string[];
  /** The download / docs page. */
  url: string;
}

/** How to install Ollama on this platform — the runner PM can drive a one-click model pull for. */
export function ollamaGuide(platform: SetupPlatform = PLATFORM): RunnerGuide {
  const base = {
    name: "Ollama",
    summary:
      "A small local server that runs models on your machine. PM can download a recommended model straight into it.",
    url: "https://ollama.com/download",
  };
  switch (platform) {
    case "mac":
      return {
        ...base,
        steps: [
          "Install Ollama — run `brew install ollama`, or download it from ollama.com/download.",
          "Start it (the menu-bar app, or `ollama serve` in a terminal). It listens on http://localhost:11434.",
          "Come back here — PM detects it automatically, and you can download a recommended model in one click.",
        ],
      };
    case "linux":
      return {
        ...base,
        steps: [
          "Install Ollama — run `curl -fsSL https://ollama.com/install.sh | sh`.",
          "It runs as a service on http://localhost:11434 (or start it with `ollama serve`).",
          "Come back here — PM detects it automatically, and you can download a recommended model in one click.",
        ],
      };
    default:
      return {
        ...base,
        steps: [
          "Download Ollama from ollama.com/download and run the installer.",
          "Launch it — it listens on http://localhost:11434.",
          "Come back here — PM detects it automatically, and you can download a recommended model in one click.",
        ],
      };
  }
}
