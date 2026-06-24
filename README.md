# PM — Personal Manager

![Status: Alpha](https://img.shields.io/badge/status-alpha-orange)

> [!WARNING]
> **PM is in alpha.** It's usable day to day, but still under active development —
> expect rough edges and changes between versions. Releases carry a semver
> pre-release tag (e.g. an `-alpha` / `-beta` suffix), which sorts below the matching
> stable version.

A local-first desktop app that **archives your knowledge so you can find and use it**
and **gives you one clean view of everything you have going on**. Your data stays on
your device: the local database (settings, search index, metadata) is always
encrypted, and your documents live in a Markdown vault you can keep private to this
device or protect with a passphrase and carry between machines. The only traffic that
leaves is the model API call — and, if you connect one, a read-only calendar fetch.

This repo is the **application code**. Your personal data is never committed — it lives
in a separate, machine-local data directory (see [Where your data lives](#where-your-data-lives)).

> **The always-current feature list is in the app itself.** PM ships an in-app
> **What's New** view, sourced from [`src/lib/changelog.ts`](src/lib/changelog.ts),
> which records every release. This README describes what PM *is* and how to run it;
> the changelog is the canonical, up-to-date record of what it *does* right now.

## What PM does

PM is built around two pillars. The capabilities below are representative, not
exhaustive — features are added and refined release to release.

### The Archivist — your searchable second brain

- **Ingest anything.** Drag files or a folder into the **Documents** view; each is
  converted to Markdown, chunked, embedded locally, and indexed. The Markdown vault is
  the rebuildable source of truth. Conversion and embeddings run on-device in a small
  managed Python sidecar, set up automatically on first use.
- **Grounded answers.** Ask a question and get an answer drawn from your own files,
  with the source documents cited. Retrieval blends meaning and keywords so relevant
  material surfaces even when your wording doesn't match the page.
- **Sorting review.** PM proposes a project, tags, and importance for each new
  document; you confirm or correct in one pass.
- **Learns how you organise.** Your corrections distil into a short, readable profile
  that feeds back into PM's suggestions and chat, so it organises more like you over
  time.

### The Personal Assistant — one clean view

- **Focus view.** Every project on one screen, each with a single honest status, plus
  a short daily briefing that synthesises your real projects and agenda into "here's
  your picture today."
- **Per-project view.** Click in and everything narrows to that project: its files
  beside a chat that answers only from them.
- **Command palette.** A single keystroke (Ctrl/Cmd+K) to jump to any project, file,
  or past conversation.
- **Read-only calendar.** Optionally connect a calendar for an upcoming agenda,
  schedule-aware chat answers, and an automatic "Due soon" when an event names a
  project.
- **Bring your own model.** Choose any model through OpenRouter, with separate
  chat and background models, automatic fallback on rate limits, and an at-a-glance
  view of what you're spending.
- **Voice input.** Speak into the chat box and PM transcribes it **on your device**
  into editable text — no audio leaves the machine.
- **Make it yours.** A themeable interface (multiple visual styles, light/dark, accent
  colour, and density), a document map, and a hover-to-explain help mode.

### Cross-cutting

- **Private by design.** The local store is always encrypted, secrets live in the OS
  keychain, and ingested content is treated as untrusted data — never as instructions.
- **Portable when you want it.** Keep a vault private to one device, or protect it with
  a passphrase to open it from another profile or machine; either way you can export
  everything to plain Markdown at any time. Encryption protects your notes; it never
  locks you in.
- **Optional app lock.** Ask PM to confirm it's you (e.g. Windows Hello) before it opens
  — a convenience gate for the window, not a second password on your already-encrypted
  data.
- **Keeps itself current.** Tagged releases are built and signed in CI; each running app
  downloads updates in the background and offers a one-click restart.

## Tech stack

- **Shell:** Tauri v2 (Rust) · **UI:** React + TypeScript + Tailwind (Vite)
- **Store:** one bundled SQLite connection — SQLCipher encryption + `sqlite-vec`
  (vectors) + FTS5 (keyword), all vendored (no system libs)
- **Secrets:** OS keychain (`keyring`) · **Model gateway:** OpenRouter (streaming)
- **On-device ML:** a managed Python sidecar for document conversion, local embeddings,
  and speech-to-text — provisioned on first use
- **Design system:** a token-driven, accent-driven OKLCH theme (`src/theme/`) with
  switchable System / Mode / Accent / Depth axes and self-hosted fonts. The full design
  reference lives in [`design-system-docs/`](design-system-docs/); the rules for working
  with it are in [`AGENTS.md`](AGENTS.md#design-system-v2).

For a full tour of the architecture and the rules for working in this repo, see
[`AGENTS.md`](AGENTS.md).

## Prerequisites

- **Node.js** 20+ and **Rust** (stable)
- **Python** 3.10+ on your PATH — needed for development (`tauri dev`) and as the base
  for the document sidecar's managed virtual environment, built on first use. Point
  `PM_PYTHON` at a specific interpreter if it isn't named `python`/`python3`. (Packaged
  **Windows** release builds **bundle** a standalone interpreter — `npm run tauri build`
  fetches it via [`scripts/fetch-python.mjs`](scripts/fetch-python.mjs) — so end users
  need no Python install. macOS bundling is deferred behind signing.)
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

On first launch, paste your **OpenRouter API key** (from <https://openrouter.ai/keys>)
into the welcome screen. It's stored in your OS keychain — never on disk or in this
repo. Then start chatting.

## Build a release

A local build (unsigned, for testing the bundle):

```bash
npm run tauri build
```

Shipping a real auto-updatable release is tag-driven and handled by CI: bump the
version, push a `vX.Y.Z` tag (a pre-release suffix like `-alpha`/`-beta` is allowed),
and the installers + update feed publish to this repo's releases. Add a matching
[`src/lib/changelog.ts`](src/lib/changelog.ts) entry for every release. Full steps,
the lockstep version files, and the one-time setup live in
[`RELEASING.md`](RELEASING.md).

**Code signing.** The auto-update feed is cryptographically signed, so updates are
always verified. The **installers themselves are not yet OS code-signed**, so each may
warn once on first launch — Windows SmartScreen (**More info → Run anyway**) and macOS
Gatekeeper (**Open Anyway**); the release notes spell this out for downloaders. The
macOS Apple-signing pipeline is already wired and dormant: add the `APPLE_*` GitHub
Actions secrets and macOS builds sign + notarize automatically, with no code change.

## Useful commands

```bash
npm run build                                      # type-check + bundle the frontend
cargo test  --manifest-path src-tauri/Cargo.toml   # run backend tests
cargo build --manifest-path src-tauri/Cargo.toml   # compile the backend
```

## Where your data lives

A stable directory **outside** this repo and the app bundle, so updates never touch it.
It holds the encrypted `pm.sqlite` store and a `vault/` of Markdown (the rebuildable
source of truth). The folder is named **`Personal Manager`** for easy backup:

- Windows: `%LOCALAPPDATA%\Personal Manager`
- macOS: `~/Library/Application Support/Personal Manager`

The folder name is deliberately decoupled from the app's bundle identifier
(`org.itsatlas.pm`, which stays fixed because the OS keychain is keyed to it). Override
the location for development with the `PM_DATA_DIR` environment variable. Encryption and
API keys live in the OS keychain, never in the data directory.

## Privacy & security

Local-first by design. The database is always encrypted at rest (SQLCipher) and secrets
live in the OS keychain. The Markdown vault's at-rest protection depends on how you set
it up:

- A **device vault** (the default) stores Markdown as plaintext on disk and relies on
  your OS full-disk encryption (BitLocker / FileVault) — so enable that.
- A **passphrase-protected vault** additionally encrypts each Markdown file at rest,
  so it stays protected even when shared or carried to another machine.

Either way, you can export everything to plain Markdown whenever you like. The only
outbound traffic is the model call (and an optional read-only calendar fetch). The repo
holds code only — see [`SECURITY.md`](SECURITY.md) for the security policy and how to
report an issue privately.

## Contributing & releasing

[`CONTRIBUTING.md`](CONTRIBUTING.md) covers the change workflow, the one-command check
gate, and what to verify by hand; [`RELEASING.md`](RELEASING.md) is the release
runbook; and [`AGENTS.md`](AGENTS.md) is the deep orientation for the codebase and its
non-negotiable rules.

## License

Copyright (C) 2026 Bobby Yu

PM is free software: you can redistribute it and/or modify it under the terms of the
**GNU Affero General Public License** as published by the Free Software Foundation,
either version 3 of the License, or (at your option) any later version
(`AGPL-3.0-or-later`).

This program is distributed in the hope that it will be useful, but WITHOUT ANY
WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A
PARTICULAR PURPOSE. See the full license text in [`LICENCE.txt`](LICENCE.txt) for
details.
</content>
</invoke>
