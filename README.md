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
device or protect with a passphrase and carry between machines. Little leaves your device:
the model API calls that power chat, a launch-time update check, a one-time first-run download
of PM's local models, and — only if you set them up — a read-only calendar fetch and encrypted
backups to your own cloud.

This repo is the **application code**. Your personal data is never committed — it lives
in a separate, machine-local data directory (see [Where your data lives](#where-your-data-lives)).

> **The always-current feature list is in the app itself.** PM ships an in-app
> **What's New** view, sourced from [`src/lib/changelog.ts`](src/lib/changelog.ts),
> which records every release. This README describes what PM *is* and how to run it;
> the changelog is the canonical, up-to-date record of what it *does* right now.

## What PM does

PM is built around two pillars. This is a curated tour of what makes PM *distinctive* —
not an exhaustive list; the in-app **What's New** is the complete, current record.

### The Archivist — your searchable second brain

- **Ingest almost anything, on-device.** Drag in files, whole folders, photos (read with
  on-device OCR) or spreadsheets (indexed row-by-row, not mangled into one blob) — each is
  converted to Markdown, chunked, embedded and indexed locally in a managed Python sidecar,
  set up automatically on first use. The Markdown vault is the rebuildable source of truth.
- **Index your cloud and your computer without copying a thing.** Connect Google Drive,
  OneDrive, or a folder on your own machine and PM indexes what's inside so it turns up in
  search — reading each file only to index it, never importing the bytes. Local folders stay
  live: edit a file and it re-indexes within seconds.
- **Grounded answers, with citations.** Ask a question and get an answer drawn from your own
  material, each source cited. Retrieval blends meaning and keywords — and works in
  non-space-separated scripts like Chinese and Japanese — so the right passage surfaces even
  when your wording doesn't match the page.
- **Your conversations are a source too.** Chats with PM fold into the same searchable
  memory: a decision or preference you mentioned weeks ago can resurface later, cited back to
  the exact turn.
- **Sorting review that learns you.** PM proposes a project, tags and importance for each new
  document; you confirm or correct in one pass. Those corrections — and the names and aliases
  you teach it — distil into a short, readable profile that shapes future suggestions.

### The Personal Assistant — one clean view

- **Focus view.** Every project on one screen, each with a single honest status, plus a
  short daily briefing that synthesises your real projects and agenda into "here's your
  picture today."
- **A "what needs your attention" layer.** Approaching deadlines, today's events and
  prep-ahead nudges become stable, tracked items you can mark done — not a paragraph the
  model rewrites each morning. Resolve one and it stays resolved.
- **Milestones & deadlines** on each project feed straight into the briefing, Focus and the
  calendar — sorted by deadline, by name, or by hand — and the same conversation, chat or
  briefing all read from one honest picture of what matters.
- **Per-project view.** Click in and everything narrows to that project: its files beside a
  chat that answers only from them.
- **A pinboard to think on.** A board for notes, checklists and timelines you can drag,
  resize and tidy into folders, with undo when you change your mind. Nothing on it is
  trapped there: a note can become a real document in your vault when it's ready, and a
  timeline can become a project's milestones.
- **A map of your knowledge.** See your documents laid out by meaning — related material
  clusters together — as a navigable map you can explore.
- **One calendar, never written to.** Gather Google, Outlook and iCal calendars into one
  agenda — Month, Week, Day, Year and Agenda views, extra timezones down the side, and your
  own working hours — with schedule-aware chat answers and an automatic "Due soon" when an
  event names a project. Your milestones and pinboard timelines show alongside as markers you
  can toggle off. PM only ever reads your calendars; it never writes to them.
- **Bring your own model.** Choose your model through OpenRouter — separate chat and
  background models, spend at a glance, and zero-data-retention requested on every call. The
  list offers only models a provider will actually serve on those terms, so a model can't
  quietly become the less private option; you can still enter any model id by hand. PM
  starts you on a cheap default; add a second model and turn on auto-switch to get automatic
  fallback when one is rate-limited.
- **Voice input.** Speak into the chat box and PM transcribes it **on your device** into
  editable text — no audio leaves the machine.
- **Command palette.** A single keystroke (Ctrl/Cmd+K) to jump to any project, file, or past
  conversation.

### Cross-cutting

- **Private by design.** The local store is always encrypted, secrets live in the OS
  keychain, and ingested content is treated as untrusted data — never as instructions.
- **Encrypted, restore-anywhere backups.** Pack your whole vault into a single
  passphrase-protected file — on demand or on a schedule — straight to your own Proton Drive
  or Google Drive, and restore it on any machine.
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
- **Python** 3.10+ — needed for development (`tauri dev`) and as the base for the
  document sidecar's managed virtual environment, built on first use. PM finds a suitable
  interpreter automatically: `PM_PYTHON`, then versioned names (`python3.12` … `python3.10`),
  `python3`/`python`, and common macOS locations (Homebrew, python.org) — so even a
  Finder-launched app finds one off its stripped PATH. It also rebuilds the venv if that
  interpreter is older than 3.10. Set `PM_PYTHON` only for an unusual setup. (Packaged
  **Windows and Linux** release builds **bundle** a standalone interpreter at build time —
  `npm run tauri build` fetches it via [`scripts/fetch-python.mjs`](scripts/fetch-python.mjs)
  — so end users need no Python install. On **macOS**, packaged apps instead **download** a
  pinned standalone interpreter into PM's data directory on first run **only if** no suitable
  Python is found — so macOS end users need no manual install either. The two use different
  mechanisms for different reasons: `python-build-standalone` has no universal2 build and
  build-time bundling into the signed `.app` is gated on the Apple-signing pipeline, whereas a
  runtime download picks the right per-arch build and isn't gated on signing — see
  [`docs/MACOS-SIGNING.md`](docs/MACOS-SIGNING.md). Development from source still needs Python
  3.10+ on your machine on every platform.)
- **Tauri OS prerequisites:** see <https://tauri.app/start/prerequisites/>
  - Windows: Microsoft C++ Build Tools + WebView2 (preinstalled on Win 11)
  - macOS: Xcode Command Line Tools
  - Linux (dev, Debian/Ubuntu names): `libwebkit2gtk-4.1-dev`, `librsvg2-dev`, `libssl-dev`,
    `build-essential`, `libayatana-appindicator3-dev`, `libdbus-1-dev`
    (Fedora: `webkit2gtk4.1-devel gtk3-devel librsvg2-devel openssl-devel dbus-devel`).
    On a fresh Windows or Linux clone, run `just fetch-python` (or any `just` compile
    recipe) once before `npm run tauri dev` or a raw `cargo check` — the Tauri build
    script requires the bundled-interpreter resource (`src-tauri/python/`) to exist.

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

## Installing on Linux

Two formats ship per release, both x86_64:

- **AppImage** (recommended — it auto-updates like the Windows build): download,
  `chmod +x PM_*.AppImage`, run. If your distro lacks FUSE (recent Fedora minimal
  installs), either `sudo dnf install fuse` or run with `--appimage-extract-and-run`.
- **rpm** (Fedora/RHEL): `sudo dnf install ./PM-*.rpm`. Package-manager installs update
  via the next release's rpm — the in-app auto-updater only covers the AppImage.

Two Linux-specific notes:

- **Secrets need a running keychain.** PM stores its encryption keys in the
  freedesktop Secret Service — KWallet (KDE) or GNOME Keyring — over D-Bus. Every
  mainstream desktop provides one; expect a one-time wallet/keyring prompt on first
  launch. On a minimal window-manager setup you must run one yourself, or PM cannot
  store the key that protects its database.
- **Uninstalling** (rpm removal or deleting the AppImage) leaves PM's data —
  including the multi-hundred-MB regenerable `runtime/` (Python venv + models) —
  under `~/.local/share/Personal Manager`. Free it from **Settings → Storage**, use
  **Settings → Remove PM data** before uninstalling, or delete the folder by hand.
  (An rpm `%postun` script is deliberately not used: it runs as root and can't safely
  enumerate per-user data.)

## Moving between computers (Windows ↔ Linux ↔ macOS)

Your vault travels with an encrypted **backup** (Settings → Backup): one
passphrase-protected `.pmbackup` file containing the database, the Markdown vault, and
your settings. The archive is fully cross-platform — its only lock is the passphrase
you set (the app-lock plays no role in backups).

On the new machine: install PM, then **Settings → Backup → Restore** (from a copied
file, or straight from Proton Drive / Google Drive), enter the passphrase, and
**Switch to restored vault**. The store's encryption key travels inside the encrypted
archive and is re-seeded into the new machine's keychain automatically.

What doesn't travel (by design — re-add on the new machine):

- **API keys and cloud sign-ins** (OpenRouter, Google/Microsoft, iCal URLs) stay in
  the old machine's keychain — re-enter/reconnect them.
- **Watched local folders** point at paths on the old machine, so they show as
  unreachable — remove and re-add them by their new paths. Until you do, their
  already-indexed content **remains searchable and citable** (embeddings and
  summaries live in the database); only opening the full original file needs the
  files present.
- **App lock** re-arms only where the OS can verify (it reads as "not available on
  Linux yet" — you're never locked out by a restored setting).
- The Python runtime and on-device models re-download on first use.

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
- Linux: `~/.local/share/Personal Manager`

The folder name is deliberately decoupled from the app's bundle identifier
(`org.itsatlas.pm`, which stays fixed because the OS keychain is keyed to it). Override
the location for development with the `PM_DATA_DIR` environment variable. Encryption and
API keys live in the OS keychain, never in the data directory.

## Privacy & security

Local-first by design. The database is always encrypted at rest (SQLCipher) and secrets
live in the OS keychain. The Markdown vault's at-rest protection depends on how you set
it up:

- A **device vault** (the default) stores Markdown as plaintext on disk and relies on
  your OS full-disk encryption (BitLocker / FileVault / LUKS) — so enable that.
- A **passphrase-protected vault** additionally encrypts each Markdown file at rest,
  so it stays protected even when shared or carried to another machine.

Either way, you can export everything to plain Markdown whenever you like. Outbound traffic
is limited and enumerable: the model API calls that power chat; a check for updates on launch
(and the download if you accept one); a one-time first-run download of PM's on-device models
and Python dependencies; and — only if you turn them on — a read-only calendar fetch and
encrypted backups to your chosen cloud (Proton Drive or Google Drive). Nothing else leaves the
machine: there is no telemetry, analytics, or crash reporting. The repo holds code only — see
[`SECURITY.md`](SECURITY.md) for the security policy and how to report an issue privately.

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
