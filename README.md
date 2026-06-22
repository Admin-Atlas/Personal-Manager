# PM — Personal Manager

![Status: Alpha](https://img.shields.io/badge/status-alpha-orange)

> [!WARNING]
> **PM is in alpha.** It's feature-complete for v1 and usable day to day, but
> still under active development — expect rough edges and changes between
> versions. Releases carry a semver pre-release tag (e.g. `1.0.0-alpha`), which
> sorts below a future stable `1.0.0`.

A local-first desktop app that **archives your knowledge so you can find and use
it** and **gives you one clean view of everything you have going on**. Your data
stays on your device, encrypted; the only traffic that leaves is the model API
call (and, if you connect one, a read-only calendar fetch).

This repo is the **application code**. Your personal data is never committed — it
lives in a git-ignored local data directory (see below). The full product spec and
the decision log live in the `docs/` folder.

## Features

Built in steps (see the spec in the `docs/` folder); the in-app
**What's New** view and [`src/lib/changelog.ts`](src/lib/changelog.ts) track each
release. What works today:

### The Archivist — your searchable second brain

- **Ingest anything.** Drag files or a folder into the **Documents** view; each is
  converted to Markdown (MarkItDown), chunked, embedded locally (ONNX
  `bge-small-en-v1.5`), and indexed. The Markdown vault is the rebuildable source of
  truth. Conversion + embeddings run in a small Python sidecar (a managed venv set
  up on first run).
- **Grounded answers.** Ask a question and get an answer drawn from your own files,
  with the source documents cited. Hybrid retrieval (vector + keyword, fused with
  RRF) with gentle recency decay.
- **Sorting review.** PM proposes a project, tags, and importance for each new
  document; you confirm or correct in one pass.
- **Learning You.** Your corrections distil into a short, readable profile that
  feeds back into PM's suggestions and chat, so it organises more like you over time.

### The Personal Assistant — one clean view

- **Focus view.** Every project on one screen, each with a single honest status —
  Due soon, Quick win, Take a look, Blocked, Part of, or On track.
- **Per-project view.** Click in and everything narrows to that project: its files
  beside a chat that answers only from them.
- **Command palette.** Ctrl/Cmd+K to jump to any project, file, or past conversation.
- **Read-only calendar.** Connect a private iCal feed (or Google sign-in) for an
  upcoming agenda, schedule answers in chat, and an automatic "Due soon" when an
  event names a project. See the calendar setup guide in the `docs/` folder.
- **Model picker.** Choose any OpenRouter model, with separate chat/background models
  and auto-switch fallback on rate limits.
- **Voice input.** A microphone button in the chat box records a short clip and
  transcribes it **on your device** (local Whisper, in the same Python sidecar) into
  the box to edit before sending — no audio leaves the machine. The voice model
  downloads once on first use.
- **Document map + help mode.** A force-directed map of documents by project, and a
  hover-to-explain help overlay.

### Cross-cutting

- Encrypted local store (SQLCipher), all secrets in the OS keychain, and ingested
  content treated as untrusted data.
- **Silent auto-update:** a tagged release is built and signed in CI; each running
  app downloads it in the background and offers a one-click restart. See the
  release guide in the `docs/` folder.

## Tech stack

- **Shell:** Tauri v2 (Rust) · **UI:** React + TypeScript + Tailwind (Vite)
- **Store:** one bundled SQLite connection — SQLCipher encryption + `sqlite-vec`
  (vectors) + FTS5 (keyword), all vendored (no system libs)
- **Secrets:** OS keychain (`keyring`) · **Model gateway:** OpenRouter (streaming)
- **Design system:** a token-driven, accent-driven OKLCH theme (`src/theme/`) with
  switchable System / Mode / Accent / Depth axes and self-hosted fonts. The full design
  reference lives in [`design-system-docs/`](design-system-docs/); the rules for working with
  it are in [`AGENTS.md`](AGENTS.md#design-system-v2).

## Prerequisites

- **Node.js** 20+ and **Rust** (stable)
- **Python** 3.10+ on your PATH — needed for development (`tauri dev`) and as the
  base for the document sidecar's managed venv (MarkItDown + local embeddings),
  built on first ingest. Point `PM_PYTHON` at a specific interpreter if it isn't
  named `python`/`python3`. (Packaged **Windows** release builds **bundle** a
  standalone interpreter — `npm run tauri build` fetches it via
  [`scripts/fetch-python.mjs`](scripts/fetch-python.mjs) — so end users need no
  Python install. macOS bundling is deferred behind signing.)
- **Tauri OS prerequisites:** see <https://tauri.app/start/prerequisites/>
  - Windows: Microsoft C++ Build Tools + WebView2 (preinstalled on Win 11)
  - macOS: Xcode Command Line Tools
  - Linux (dev): `libwebkit2gtk-4.1-dev`, `librsvg2-dev`, `libssl-dev`,
    `build-essential`, `libayatana-appindicator3-dev`

## Run it

```bash
npm install
npm run tauri dev
```

On first launch, paste your **OpenRouter API key** (from
<https://openrouter.ai/keys>) into the welcome screen. It's stored in your OS
keychain — never on disk or in this repo. Then start chatting.

## Build a release

A local build (unsigned, for testing the bundle):

```bash
npm run tauri build
```

Shipping a real auto-updatable release is tag-driven and handled by CI — bump the
version, push a `vX.Y.Z` tag (a `-alpha`/`-beta` suffix is allowed), and the
installers + update feed publish to this repo's releases. Full steps and one-time
setup are in the release guide in the `docs/` folder. Remember to add a changelog
entry in `src/lib/changelog.ts` for each release.

**Code signing.** The auto-update feed is minisign-signed (so updates are verified),
but the **installers themselves are not yet OS code-signed** on either platform — so
each warns once on first launch: Windows SmartScreen (**More info → Run anyway**) and
macOS Gatekeeper (**System Settings → Privacy & Security → Open Anyway**). The release
notes spell this out for downloaders. The macOS Apple-signing pipeline is already wired
and **dormant**: add the `APPLE_*` GitHub Actions secrets (a Developer ID certificate +
an App Store Connect API key) and the macOS build signs + notarizes automatically — no
code change.

## Useful commands

```bash
npm run build                                      # type-check + bundle the frontend
cargo test  --manifest-path src-tauri/Cargo.toml   # run backend tests
cargo build --manifest-path src-tauri/Cargo.toml   # compile the backend
```

## Where your data lives

A stable directory **outside** this repo and the app bundle, so updates never
touch it:

- the encrypted `pm.sqlite` store
- a `vault/` of Markdown (the source of truth; populated from Step 2 on)

It lives in a friendly, machine-local folder named **`Personal Manager`**:
`%LOCALAPPDATA%\Personal Manager` on Windows, `~/Library/Application Support/Personal Manager`
on macOS. The folder name is deliberately decoupled from the app's bundle identifier
(`org.itsatlas.pm`, which stays fixed because the OS keychain is keyed to it) — so it's
easy to find and back up. Override the location for development with the `PM_DATA_DIR`
environment variable. Encryption keys and API keys live in the OS keychain, never the data
directory.

## Privacy

Local-first by design: data is encrypted at rest, secrets are in the keychain,
and the only outbound traffic is the OpenRouter model call. The repo holds code
only — `.gitignore` keeps data and secrets out.

## License

Copyright (C) 2026 Bobby Yu

PM is free software: you can redistribute it and/or modify it under the terms of
the **GNU Affero General Public License** as published by the Free Software
Foundation, either version 3 of the License, or (at your option) any later
version (`AGPL-3.0-or-later`).

This program is distributed in the hope that it will be useful, but WITHOUT ANY
WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A
PARTICULAR PURPOSE. See the full license text in [`LICENCE.txt`](LICENCE.txt)
for details.
