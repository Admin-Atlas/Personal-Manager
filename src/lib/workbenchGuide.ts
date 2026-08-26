// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Install copy for local model runners, shown in the Local AI tab (#296). Platform-keyed like
// setupGuide.ts so the wording lives in one place. PM recommends and points — it never bundles or
// provisions a runner. You install one yourself, and your runner fetches the model weights (from
// the Ollama registry / Hugging Face), never PM. The three runners PM auto-detects are Ollama
// (:11434), LM Studio (:1234) and llama-server (:8080).
//
// Every command and requirement below was checked against the vendors' own documentation on
// 2026-08-26 rather than written from memory, because a wrong command in a settings pane is worse
// than no command. Two things that check corrected, worth recording so they are not "fixed" back:
//   * Ollama is NOT the only runner with a download API any more. LM Studio has
//     `POST /api/v1/models/download` (0.4.0+, no auth by default) and llama-server has
//     `POST /models` in router mode. PM still drives only Ollama's — the other two are newer and
//     conditional (llama-server's exists only when it was launched with no model at all), so
//     wiring them is its own piece of work, not a copy change.
//   * `lms get` is deliberately NOT offered a Hugging Face repo id anywhere in this file. LM
//     Studio's GUI and its REST endpoint both document accepting `user/model` and full HF URLs;
//     the `lms get` docs show only bare names and catalogue ids and say nothing either way. An
//     unverified command in a copy box is exactly what this comment exists to prevent.

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
  /** The port PM auto-detects it on. */
  port: number;
  /** Who it suits — the answer to "which of these do I actually pick?". */
  bestFor: string;
  /** How models get into it, in one line. */
  models: string;
  /** The thing most likely to rule it out for someone, or null when there isn't one. */
  caveat: string | null;
}

/** How to install Ollama on this platform — the runner PM can drive a one-click model pull for. */
export function ollamaGuide(platform: SetupPlatform = PLATFORM): RunnerGuide {
  const base = {
    name: "Ollama",
    summary:
      "A small local server that runs models on your machine. PM can download a recommended model straight into it.",
    url: "https://ollama.com/download",
    port: 11434,
    bestFor:
      "Pick this if you want the least to think about. It starts with your machine and stays running, so PM just finds it.",
    models:
      "From Ollama's own library, by a name like `qwen2.5:7b-instruct-q4_K_M`. It's the only one PM can download into for you.",
    caveat: null,
  };
  switch (platform) {
    case "mac":
      return {
        ...base,
        steps: [
          "Install Ollama — run `brew install ollama`, or download it from ollama.com/download.",
          "Start it (the menu-bar app, or `ollama serve` in a terminal). It listens on http://127.0.0.1:11434.",
          "Come back here — PM detects it automatically, and you can download a recommended model in one click.",
        ],
      };
    case "linux":
      return {
        ...base,
        steps: [
          "Install Ollama — run `curl -fsSL https://ollama.com/install.sh | sh`.",
          "It runs as a service on http://127.0.0.1:11434 (or start it with `ollama serve`).",
          "Come back here — PM detects it automatically, and you can download a recommended model in one click.",
        ],
      };
    default:
      return {
        ...base,
        steps: [
          "Download Ollama from ollama.com/download and run the installer — it doesn't need an administrator.",
          "Launch it. It keeps running in the background on http://127.0.0.1:11434.",
          "Come back here — PM detects it automatically, and you can download a recommended model in one click.",
        ],
      };
  }
}

/** How to install LM Studio — the runner with a real app around it, and the narrowest hardware. */
export function lmStudioGuide(platform: SetupPlatform = PLATFORM): RunnerGuide {
  const base = {
    name: "LM Studio",
    summary:
      "A full desktop app for finding, downloading and chatting to local models, with a server you switch on when you want PM to use it.",
    url: "https://lmstudio.ai/download",
    port: 1234,
    bestFor:
      "Pick this if you want to browse and try models yourself, with a proper interface rather than a terminal.",
    models:
      "In its Discover tab — search by name, or paste a Hugging Face `user/model` string straight into the search box.",
    caveat: null as string | null,
  };
  switch (platform) {
    case "mac":
      return {
        ...base,
        // The vendor's stated requirement is stricter than the Homebrew cask's metadata; PM quotes
        // the vendor. This is the one hard exclusion among the three runners.
        caveat: "Apple Silicon only — Intel Macs aren't supported. Needs macOS 14 or newer.",
        steps: [
          "Install LM Studio — run `brew install --cask lm-studio`, or download it from lmstudio.ai/download.",
          "Open it once and download a model from the Discover tab (⌘2).",
          "Turn the server on: the toggle at the top of the Developer tab, or `lms server start` in a terminal. It listens on http://127.0.0.1:1234.",
          "Come back here and connect to that address. PM can't download into LM Studio, so pick your model in its app first.",
        ],
      };
    case "linux":
      return {
        ...base,
        caveat: "Ships as an AppImage. Needs Ubuntu 20.04 or newer (or an equivalent).",
        steps: [
          "Install LM Studio — run `curl -fsSL https://lmstudio.ai/install.sh | bash`, or download the AppImage from lmstudio.ai/download.",
          "Open it once and download a model from the Discover tab (Ctrl+2).",
          "Turn the server on: the toggle at the top of the Developer tab, or `lms server start` in a terminal. It listens on http://127.0.0.1:1234.",
          "Come back here and connect to that address. PM can't download into LM Studio, so pick your model in its app first.",
        ],
      };
    default:
      return {
        ...base,
        caveat:
          "On an Intel or AMD PC the processor needs AVX2 support. ARM PCs are supported too.",
        steps: [
          "Install LM Studio — run `irm https://lmstudio.ai/install.ps1 | iex` in PowerShell, or download it from lmstudio.ai/download.",
          "Open it once and download a model from the Discover tab (Ctrl+2).",
          "Turn the server on: the toggle at the top of the Developer tab, or `lms server start` in a terminal. It listens on http://127.0.0.1:1234.",
          "Come back here and connect to that address. PM can't download into LM Studio, so pick your model in its app first.",
        ],
      };
  }
}

/** How to install llama.cpp's `llama-server` — the engine the other two are built on. */
export function llamaServerGuide(platform: SetupPlatform = PLATFORM): RunnerGuide {
  const base = {
    name: "llama-server",
    summary:
      "The engine underneath most local AI, run directly. One command downloads a model and serves it — no app, no registry, no account.",
    url: "https://github.com/ggml-org/llama.cpp",
    port: 8080,
    bestFor:
      "Pick this if you're comfortable in a terminal and want the leanest option, or the newest models the moment they appear on Hugging Face.",
    models:
      "Straight from Hugging Face, in the same command that starts it — PM gives you the exact line for each model below.",
    caveat:
      "It runs only while its terminal window is open, and doesn't start with your machine. You launch it each session.",
  };
  const serve = [
    "Start it with a model — for example `llama-server -hf ggml-org/gemma-3-4b-it-GGUF:Q4_K_M`. It downloads the model the first time, then serves it on http://127.0.0.1:8080.",
    "Come back here and connect to that address. Each model below shows the exact command to run.",
  ];
  switch (platform) {
    case "mac":
      return { ...base, steps: ["Install it — run `brew install llama.cpp`.", ...serve] };
    case "linux":
      return {
        ...base,
        steps: [
          "Install it — run `brew install llama.cpp`, or `conda install -c conda-forge llama.cpp`, or download a build from github.com/ggml-org/llama.cpp/releases.",
          ...serve,
        ],
      };
    default:
      return {
        ...base,
        steps: [
          "Install it — run `winget install llama.cpp`, or download a build from github.com/ggml-org/llama.cpp/releases.",
          ...serve,
        ],
      };
  }
}

/**
 * All three runners, in the order PM suggests trying them: easiest first, leanest last.
 *
 * A list rather than three named calls, so the tab renders whatever is here and a fourth runner
 * never touches the component.
 */
export function runnerGuides(platform: SetupPlatform = PLATFORM): RunnerGuide[] {
  return [ollamaGuide(platform), lmStudioGuide(platform), llamaServerGuide(platform)];
}

/**
 * The command that gets `repo` into `runner`, or null when PM can't honestly give one.
 *
 * `repo` is a Hugging Face GGUF repo id (`bartowski/Qwen2.5-7B-Instruct-GGUF`) — what the catalogue
 * actually stores — and `quant` is the quantization PM's fit picked for this machine.
 *
 * Only llama-server gets a command, and that is not an oversight:
 *   * llama-server takes a Hugging Face repo id directly. `-hf <user>/<model>[:quant]` is its
 *     documented form; the quant is optional and case-insensitive.
 *   * LM Studio names models its own way and its `lms get` documentation never says a raw Hugging
 *     Face repo id is accepted — so PM points at the Discover tab, which documents accepting one,
 *     rather than printing a command that may not work.
 *   * Ollama's registry uses names of its own (`qwen2.5:7b-instruct-q4_K_M`) that cannot be derived
 *     from a Hugging Face repo id. PM will offer a one-click download once the catalogue carries
 *     those names; guessing one would produce a command that fails.
 */
export function installCommand(
  runner: "ollama" | "lmstudio" | "llama-server",
  repo: string,
  quant: string | null,
): string | null {
  if (runner !== "llama-server") return null;
  return `llama-server -hf ${repo}${quant ? `:${quant}` : ""}`;
}
