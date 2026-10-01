// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Install copy for local model runners, shown in the Local AI tab (#296). Platform-keyed like
// setupGuide.ts so the wording lives in one place. PM recommends and points — it never bundles or
// provisions a runner. You install one yourself, and your runner fetches the model weights (from
// the Ollama registry / Hugging Face), never PM. The three runners PM auto-detects are Ollama
// (:11434), LM Studio (:1234) and llama-server (:8080).
//
// Every command and requirement below was checked against the vendors' own documentation on
// 2026-08-26 (and the lifecycle/context claims re-checked on 2026-08-30) rather than written from
// memory, because a wrong command in a settings pane is worse than no command.
//
// Re-checked 01-10-2026: Ollama FAQ K/V-cache and Flash Attention; llama.cpp server README
// -fa/-ctk/-ctv/-c. Two things came of it:
//   * PM sizes most graphics-card configs on a q8_0 K/V cache, and nothing told anyone to turn one
//     on. Ollama's `OLLAMA_KV_CACHE_TYPE` is a GLOBAL setting (every model it runs), and it only
//     compresses the cache where Flash Attention is on — which Ollama enables by itself when the
//     backend supports it. Hence the step after the context one, and `tuningFor`.
//   * llama-server's `-c/--ctx-size` defaults to 0, "loaded from model": the model's whole trained
//     context, not a small one. The old step said the opposite.
//
// What the 30-08 pass corrected, recorded so none of it is "fixed" back:
//   * The flat "4,096-token default" was wrong on every platform. Ollama picks its default from
//     VRAM — under 24 GiB → 4k, 24-48 GiB → 32k, 48 GiB and up → 256k (docs.ollama.com/context-
//     length). 4k is still what most machines get, which is why the step exists, but stating it as
//     THE default was a claim about the user's machine PM had not made.
//   * Ollama does NOT reduce the context to make a model fit. That is an open feature request
//     (ollama/ollama#12353, `--fit-vram`), not behaviour. What actually happens is the KV cache
//     spills into system memory and inference gets many times slower with no error — the opposite
//     failure from the one an earlier draft of this file was going to warn about.
//   * The Linux context step named a drop-in path with neither the file's contents nor
//     `daemon-reload`, so followed literally it changed nothing and said nothing. The FAQ documents
//     `systemctl edit ollama.service` → `Environment=` → `daemon-reload` → `restart`.
//   * `ollama serve` was offered on Linux as an alternative to the service. The install script
//     enables the service, so it is already bound to 11434 and this fails with a port clash.
//   * The Windows step omitted that Ollama must be QUIT FROM THE TASK BAR before the environment
//     variable is set — setting it under a running Ollama does nothing.
//   * `brew install ollama` is the FORMULA (server only, nothing starts it). The app that registers
//     a login item is the download from ollama.com. The old copy promised autostart for both.
//
// Two things the 26-08 check corrected, kept here for the same reason:
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

/** The three servers PM knows by name — and auto-detects, on 11434, 1234 and 8080. */
export type RunnerName = "Ollama" | "LM Studio" | "llama-server";

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
  /**
   * Whether it starts with the machine, whether it keeps running, and what stops it.
   *
   * Its own field because the fact has no platform-invariant answer and kept leaking into whichever
   * field the author happened to be writing — into Ollama's `bestFor` on a SHARED base, where one
   * autostart promise went out on three platforms whose install paths differ; into llama-server's
   * `caveat`, which everywhere else means a hardware exclusion; and into LM Studio's copy nowhere at
   * all. It is the first thing someone needs to know and the last thing they were told.
   *
   * Always set inside the platform switch, never on a shared `base` — a shared lifecycle line is the
   * defect this field exists to fix.
   */
  lifecycle: string;
}

/**
 * The context-length facts, shared because they are properties of Ollama rather than of a platform —
 * only the HOW differs, and that is spliced in per platform between these two.
 *
 * Both sentences are load-bearing and both were wrong before. The default is chosen from VRAM, not
 * fixed at 4,096; and Ollama does not shrink a context to make it fit — an over-large one spills the
 * KV cache into system memory and runs many times slower with no error, which is the failure people
 * actually hit and the opposite of a loud one.
 */
const OLLAMA_CONTEXT_WHY =
  "Give it room to work \u2014 Ollama picks its context size from your graphics card (4k under 24 GB of video memory, 32k up to 48, 256k beyond that), so most machines get 4,096 tokens and anything longer is cut without a word. ";
const OLLAMA_CONTEXT_HAZARD =
  " Don't overshoot, though: Ollama will not shrink the context to make a model fit. If it doesn't fit in video memory it spills into ordinary system memory and runs many times slower, with no error to tell you.";

/**
 * The compressed-cache step's two halves, around the per-platform HOW — the same shape as the context
 * step, for the same reason. The second half is what the FAQ says and a user needs before turning it
 * on: it is global, and it does nothing without Flash Attention. It must never name
 * OLLAMA_CONTEXT_LENGTH: the context step is found by that name.
 */
const OLLAMA_KV_WHEN =
  "If PM's pick says it was sized with a compressed (q8_0) cache, turn that on too: ";
const OLLAMA_KV_SCOPE =
  " Ollama applies it to every model it runs, and only compresses the cache where Flash Attention is on — which it switches on by itself when your graphics card supports it.";

/** Ollama's last step, the same on every platform: PM finds it by itself, and then it is the one
 *  server PM can download its pick into. */
const OLLAMA_LAST_STEP =
  "Come back to PM. It finds Ollama by itself within about half a minute — or press Look now under Your local model — then connect it there, and PM can download its pick into it for you. (First-run onboarding looks for it by itself too.)";

/** The two settings PM's numbers assume, worded for one server — the context it sized for, and the
 *  compressed cache. */
export interface RunnerTuning {
  context: string;
  cache: string;
}

/** Ollama's context and cache steps on this platform: the same strings its guide shows. */
function ollamaTuning(platform: SetupPlatform): RunnerTuning {
  switch (platform) {
    case "mac":
      return {
        context:
          OLLAMA_CONTEXT_WHY +
          "To change it, run `launchctl setenv OLLAMA_CONTEXT_LENGTH 32768`, then restart Ollama." +
          OLLAMA_CONTEXT_HAZARD,
        cache:
          OLLAMA_KV_WHEN +
          "run `launchctl setenv OLLAMA_KV_CACHE_TYPE q8_0`, then restart Ollama." +
          OLLAMA_KV_SCOPE,
      };
    case "linux":
      return {
        context:
          OLLAMA_CONTEXT_WHY +
          'To change it, run `sudo systemctl edit ollama.service` and add `Environment="OLLAMA_CONTEXT_LENGTH=32768"` under `[Service]`, then `sudo systemctl daemon-reload && sudo systemctl restart ollama`.' +
          OLLAMA_CONTEXT_HAZARD,
        cache:
          OLLAMA_KV_WHEN +
          'add `Environment="OLLAMA_KV_CACHE_TYPE=q8_0"` under the same `[Service]` line, then `sudo systemctl daemon-reload && sudo systemctl restart ollama`.' +
          OLLAMA_KV_SCOPE,
      };
    default:
      return {
        context:
          OLLAMA_CONTEXT_WHY +
          "To change it, quit Ollama from the task bar first — setting this while it is running does nothing — then search Settings for \u201cenvironment variables\u201d, open Edit environment variables for your account, add `OLLAMA_CONTEXT_LENGTH` as 32768, and start Ollama again from the Start menu." +
          OLLAMA_CONTEXT_HAZARD,
        cache:
          OLLAMA_KV_WHEN +
          "with Ollama quit from the task bar, add `OLLAMA_KV_CACHE_TYPE` as q8_0 to your account's environment variables the same way, then start Ollama again." +
          OLLAMA_KV_SCOPE,
      };
  }
}

/** How to install Ollama on this platform — the runner PM can drive a one-click model pull for. */
export function ollamaGuide(platform: SetupPlatform = PLATFORM): RunnerGuide {
  // The context step, then the cache step straight after it — the two settings PM's numbers assume,
  // shared word for word with `tuningFor`.
  const tuning = ollamaTuning(platform);
  const base = {
    name: "Ollama",
    summary:
      "A small local server that runs models on your machine. PM can download a recommended model straight into it.",
    url: "https://ollama.com/download",
    port: 11434,
    bestFor:
      "Pick this if you want the least to think about: install it, and PM can do the rest from here.",
    models:
      "Its own library, by a name like `qwen2.5:7b-instruct-q4_K_M` — or any Hugging Face GGUF repo, as `hf.co/<user>/<repo>:<QUANT>`. It's the only runner PM can download into for you, and it does so through Hugging Face, so the file you get is the one the model list measured.",
    caveat: null,
  };
  switch (platform) {
    case "mac":
      return {
        ...base,
        lifecycle:
          "The app from ollama.com adds itself to your login items, so it starts with your Mac and keeps running in the menu bar — you can turn that off in Settings \u203a General \u203a Login Items. `brew install ollama` is the command-line server only: nothing starts it for you, so run `brew services start ollama` if you want it up on login.",
        steps: [
          "Install Ollama — download it from ollama.com/download for the menu-bar app, or run `brew install ollama` for just the server.",
          "Start it. It listens on http://127.0.0.1:11434.",
          tuning.context,
          tuning.cache,
          OLLAMA_LAST_STEP,
        ],
      };
    case "linux":
      return {
        ...base,
        lifecycle:
          "The install script sets Ollama up as a systemd service and enables it, so it starts with the machine and keeps running. That also means `ollama serve` will fail with a port clash — the service already holds 11434. Use `sudo systemctl stop ollama` if you want it out of the way.",
        steps: [
          "Install Ollama — run `curl -fsSL https://ollama.com/install.sh | sh`. It runs as a service on http://127.0.0.1:11434.",
          tuning.context,
          tuning.cache,
          OLLAMA_LAST_STEP,
        ],
      };
    default:
      return {
        ...base,
        lifecycle:
          "The installer adds Ollama to your startup items, so it starts when you log in and keeps running in the task bar. You can turn that off in Task Manager \u203a Startup apps.",
        steps: [
          "Download Ollama from ollama.com/download and run the installer — it doesn't need an administrator.",
          "Launch it. It keeps running in the background on http://127.0.0.1:11434.",
          tuning.context,
          tuning.cache,
          OLLAMA_LAST_STEP,
        ],
      };
  }
}

/**
 * LM Studio's lifecycle is genuinely the same on all three platforms — it is an app setting, not an
 * OS integration — so it is named once here and referenced from each arm rather than being set on the
 * shared `base`. The distinction is not pedantry: putting it on `base` is how Ollama ended up
 * promising autostart on a platform where its own install path does not provide it, and a named
 * constant used three times makes the sameness a reviewable claim instead of an accident.
 *
 * Checked 2026-08-30 against lmstudio.ai/docs — the "run the LLM server on login" setting, and that
 * with it on, exiting minimises to the tray and the server keeps serving.
 */
/** LM Studio's context step, the same on every platform — and the context half of its tuning. */
const LM_STUDIO_CONTEXT =
  "Check the model's context-length slider before you rely on it — a short context is silently truncated, and PM's background work sends more than a few thousand tokens at a time.";

/** LM Studio's last step, the same on every platform. PM can't download into it, so the model is
 *  got in its own app first. */
const LM_STUDIO_LAST_STEP =
  "Come back to PM and connect to that address under Your local model or Model server. PM can't download into LM Studio, so get your model in its app first — Your local model says which one PM would pick.";

const LM_STUDIO_LIFECYCLE =
  "The server runs only while LM Studio is running, and it is off until you switch it on. In app settings (Ctrl/Cmd + ,) you can turn on running the LLM server on login \u2014 with that on, closing the window minimises it to the tray and the server keeps answering. Whether the server was on is remembered between launches.";

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
        lifecycle: LM_STUDIO_LIFECYCLE,
        steps: [
          "Install LM Studio — run `brew install --cask lm-studio`, or download it from lmstudio.ai/download.",
          "Open it once and download a model from the Discover tab (⌘2).",
          "Turn the server on: the toggle at the top of the Developer tab, or `lms server start` in a terminal. It listens on http://127.0.0.1:1234.",
          LM_STUDIO_CONTEXT,
          LM_STUDIO_LAST_STEP,
        ],
      };
    case "linux":
      return {
        ...base,
        caveat:
          "Ships as an AppImage. Needs Ubuntu 20.04 or newer (or an equivalent); on an Intel or AMD PC the processor needs AVX2 support.",
        lifecycle: LM_STUDIO_LIFECYCLE,
        steps: [
          "Install LM Studio — run `curl -fsSL https://lmstudio.ai/install.sh | bash`, or download the AppImage from lmstudio.ai/download.",
          "Open it once and download a model from the Discover tab (Ctrl+2).",
          "Turn the server on: the toggle at the top of the Developer tab, or `lms server start` in a terminal. It listens on http://127.0.0.1:1234.",
          LM_STUDIO_CONTEXT,
          LM_STUDIO_LAST_STEP,
        ],
      };
    default:
      return {
        ...base,
        caveat:
          "On an Intel or AMD PC the processor needs AVX2 support. ARM PCs are supported too.",
        lifecycle: LM_STUDIO_LIFECYCLE,
        steps: [
          "Install LM Studio — run `irm https://lmstudio.ai/install.ps1 | iex` in PowerShell, or download it from lmstudio.ai/download.",
          "Open it once and download a model from the Discover tab (Ctrl+2).",
          "Turn the server on: the toggle at the top of the Developer tab, or `lms server start` in a terminal. It listens on http://127.0.0.1:1234.",
          LM_STUDIO_CONTEXT,
          LM_STUDIO_LAST_STEP,
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
      "Straight from Hugging Face, in the same command that starts it — PM shows the exact line for its pick under Your local model, and for every model under All models.",
    // `caveat` means a hardware exclusion everywhere else in this file. llama-server has none — what
    // it had here was a LIFECYCLE fact wearing the wrong label, which is the drift `lifecycle` exists
    // to stop. Moved verbatim; nothing replaces it, because there is nothing to exclude on.
    caveat: null as string | null,
  };
  // Same on all three platforms, and for the same reason as LM Studio's: it is a property of running
  // a foreground process, not of the OS.
  const lifecycle =
    "It runs only while its terminal window is open, and doesn't start with your machine. You launch it each session.";
  const serve = [
    "Start it with a model — for example `llama-server -hf ggml-org/gemma-3-4b-it-GGUF:Q4_K_M`. It downloads the model the first time, then serves it on http://127.0.0.1:8080.",
    // `-c/--ctx-size` defaults to 0, "loaded from model" (the server README, 01-10-2026): without
    // it a current build allocates the model's whole trained context. The old line said the
    // default was small, which was the opposite.
    "Add `--ctx-size 32768` — without it, current builds use the model's whole trained context, which for many models is far more memory than PM sized for; and llama-server discards the oldest part of an over-long prompt rather than refusing it.",
    "Come back here and connect to that address. Your local model and All models show the exact command for each model.",
  ];
  switch (platform) {
    case "mac":
      return {
        ...base,
        lifecycle,
        steps: ["Install it — run `brew install llama.cpp`.", ...serve],
      };
    case "linux":
      return {
        ...base,
        lifecycle,
        steps: [
          "Install it — run `brew install llama.cpp`, or `conda install -c conda-forge llama.cpp`, or download a build from github.com/ggml-org/llama.cpp/releases.",
          ...serve,
        ],
      };
    default:
      return {
        ...base,
        lifecycle,
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
 * The two settings PM's numbers assume — the context it sized for, and the compressed (q8_0) cache —
 * worded for one server, or null for a server PM doesn't know by name.
 *
 * Ollama's are its own guide's steps for this platform, word for word, so the two can't disagree.
 * LM Studio's cache line names the load setting by what its API calls it (the K and V cache
 * quantization); the in-app label wording is unverified.
 */
export function tuningFor(
  runner: RunnerName | null,
  platform: SetupPlatform = PLATFORM,
): RunnerTuning | null {
  switch (runner) {
    case "Ollama":
      return ollamaTuning(platform);
    case "LM Studio":
      return {
        context: LM_STUDIO_CONTEXT,
        cache:
          "In the same load settings, set the K and V cache quantization to Q8_0. The V cache needs Flash Attention switched on there too.",
      };
    case "llama-server":
      return {
        context:
          "Add `--ctx-size <n>` with the context PM sized for — current builds otherwise use the model's whole trained context.",
        cache: "For the compressed cache, also add `-fa on -ctk q8_0 -ctv q8_0`.",
      };
    default:
      return null;
  }
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
 *   * Ollama gets no command HERE because it does not need one: it has a Download button. Ollama
 *     routes by the host in a model name and Hugging Face serves Ollama manifests, so
 *     `hf.co/<repo>:<QUANT>` pulls the exact file the catalogue measured — but that tag is written
 *     by the generator only after it has checked the registry, and it reaches the UI from the
 *     catalogue through Rust. This display helper must not re-derive it: a composed-here tag would
 *     be a guess wearing a verified tag's clothes.
 */
export function installCommand(
  runner: "ollama" | "lmstudio" | "llama-server",
  repo: string,
  quant: string | null,
): string | null {
  if (runner !== "llama-server") return null;
  return `llama-server -hf ${repo}${quant ? `:${quant}` : ""}`;
}
