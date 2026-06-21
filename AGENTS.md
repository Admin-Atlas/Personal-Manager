# AGENTS.md — working on PM

Orientation for any coding agent (or person) working in this repo. Read this,
then the canonical spec and the decision log in the `docs/` folder, before
making changes.

## What PM is

A local-first desktop "personal manager" with two pillars: **the Archivist**
(ingest your files, make them searchable, answer grounded questions) and **the
Personal Assistant** (a chat + a focus view that triages what to look at). v1 is
built in the order set out in spec §8. **Steps 1–4** are done — Shell + Store,
Archivist ingestion, the retrieval loop (hybrid search feeding the model with
citations), and Step 4: organisation + the single-pass sorting review + recency
decay + correction capture (**4a**) and the **Learning-You profile** that distils
those corrections and feeds them back into proposals and chat (**4b**, spec §4.5).
**Step 5** is done — **5a**: the **focus view** (every project with one status from
the §4.1 taxonomy), project triage (a `projects` table: deadline / size / blocked-by
/ parent, AI-proposes-you-confirm), and the **per-project scoped view** (a project's
files beside a chat whose retrieval is confined to that project); **5b**: the
**command palette** (global quick-jump to any project, file, or conversation, plus
nav destinations — frontend-only, reusing the existing list commands; Ctrl/Cmd+K).
**Step 6** is done — **read-only calendar** (spec §8.6): two ways to connect — a
private **iCal feed URL** (the default; no sign-in, works under Google Advanced
Protection) and **Google OAuth** (advanced; BYO "Desktop app" creds, loopback PKCE,
tokens in the keychain). Both mirror upcoming events, show an Upcoming agenda on the
focus view, feed chat ("what's on at 3pm?"), and **auto-flip a project to "Due soon"**
when an upcoming event's title names it. **Step 7** is done — the **Daily Briefing**
(spec §4 P1): a short, model-written "here's your picture today" synthesis at the top
of the Focus screen, built from the existing project statuses + calendar agenda (a
stored `settings` blob, background model, refreshed ~daily; module `briefing.rs`).
**Google Tasks was dropped from v1** — no iCal-feed equivalent, and OAuth is blocked
on the user's Advanced-Protection account, so a live connector can't work (see the
2026-06-19 Step 7 decision-log entry). **Voice input** to the chat bar is done
(v0.8.0): a mic button in the shared `Composer` records a clip and transcribes it
**on-device** via a `transcribe` handler in the Python sidecar (`faster-whisper`,
`base.en`) — see `src/lib/useRecorder.ts` + the `transcribe_audio` command. **With
that, every v1 spec feature is built** — what remains is the post-v1 work (design/
polish, code cleanup, a security/efficiency pass, a fresh public repo, and a release)
plus the deliberate cuts (Google Tasks, and everything in spec §10).

## Architecture

- **Frontend** — `src/` (React 19 + TypeScript + Tailwind v4, Vite).
  - `src/lib/ipc.ts` — the only place that calls Rust. Typed wrappers over Tauri
    commands; streaming/progress use a `Channel`.
  - `src/lib/types.ts` — shared types mirroring the Rust structs.
  - `src/components/` — `Sidebar` (Focus/Chat/Documents/Review/Map nav), `FocusView`
    (project status cards + triage + the calendar Upcoming agenda), `ProjectView`
    (per-project files + scoped chat), `ChatView`, `Composer`, `SettingsView` (keys,
    model, Learning-You profile, help toggle), `GoogleCalendarSettings` (the Step 6
    connector — BYO creds, connect, calendar picker, sync), `DocumentsView`
    (drag-drop ingestion), `ReviewView` (sorting review), `GraphView` (force-directed
    document→project map), `HelpOverlay` (help mode), `CommandPalette` (Ctrl/Cmd+K
    quick-jump to any project/file/conversation).
  - `src/lib/help.ts` — help-mode registry (`data-help` id → explanation) + context.
- **Backend** — `src-tauri/src/` (Rust).
  - `lib.rs` — Tauri builder, app state (`Mutex<Connection>` + `SidecarManager`),
    plugins, command registry.
  - `db/` — the encrypted store: `open()` (SQLCipher key + sqlite-vec + FTS5 +
    migrations), `migrations.rs` (additive, `user_version`-based), settings helpers.
  - `sidecar.rs` — the Python document sidecar: provisions a managed venv on
    first run and speaks newline-JSON over stdio to `convert` + `embed` +
    `transcribe` (on-device speech-to-text for voice input, `faster-whisper`).
  - `ingest.rs` — convert → hash → chunk → embed → vault + index; `rebuild`.
  - `retrieval.rs` — hybrid search (sqlite-vec KNN + FTS5) fused with Reciprocal
    Rank Fusion; builds the grounding prompt + citation list for chat.
  - `review.rs` — the sorting-review proposal (background model classifies a doc
    into project/tags/importance) + correction logging.
  - `projects.rs` — project triage for the focus view: the `projects` metadata
    table, the pure `derive_status` (one status per project, spec §4.1), and the
    AI-proposes-you-confirm attribute proposal (background key, untrusted DATA).
  - `google.rs` — Google OAuth (Step 6): the loopback-PKCE connect flow (BYO
    "Desktop app" creds in the keychain, read-only scope) + `authorized_get` with
    transparent token refresh. Connector-generic, so Tasks can reuse it.
  - `calendar.rs` — read-only calendar: the `calendar_events` mirror (migration v6,
    refilled per sync), the `.ics` feed list (secret URLs in the keychain) + OAuth
    calendar list/sync, the token-based project name → event match that feeds
    `derive_status`, and the chat agenda preamble. Everything fetched is untrusted
    DATA (rule #6).
  - `ics.rs` — iCalendar (.ics) feed parsing (the no-OAuth path): RFC 5545
    line-unfolding + property parsing, `chrono`/`chrono-tz` time resolution to UTC,
    and `rrule` recurrence expansion bounded to the agenda window.
  - `learning.rs` — the Learning-You profile (§4.5): distils `corrections` into a
    readable profile (self-edit on the background key) + the prompt preamble.
  - `secrets.rs` — OS keychain wrapper (API key + background key + DB key).
  - `openrouter.rs` — streaming chat client (SSE) + non-streaming `complete()`.
  - `commands.rs` — the `#[tauri::command]` surface.
  - `paths.rs` — resolves the data directory, vault, venv, and sidecar source.
- **Sidecar** — `sidecar/pm_sidecar.py` + `requirements.txt` (committed code).
  MarkItDown conversion + `fastembed` ONNX embeddings (bge-small-en-v1.5, 384-d).
  The venv and downloaded model are runtime artifacts, never committed.

## Design system (V2)

PM's visual design is a **presentation layer** — design tokens + a small set of styled
primitives — that the existing app wears without changing what it *does*. The chosen direction
came out of a Claude Design session; the full reference lives in
[`design-system-docs/`](design-system-docs/) (`DESIGN_TOKENS.md` is the authoritative token
recipe; `README.md` covers per-surface intent; `PM.dc.html` is the visual source of truth).

**Four orthogonal, runtime-switchable axes** (state lives in `src/theme/`, persisted in
localStorage, never IPC): **System** (`editorial` / `slate` / `terminal` — three full layout
languages, all shippable), **Mode** (`dark` / `light`), **Accent** (per-system palette whose
OKLab hue tints every neutral), **Depth** (`min` / `standard` / `power` — the spec's three
presets; shows/hides optional fields, **never** forks layout).

**Three non-negotiables (these override visual fidelity):**

1. **Design never owns functionality.** Style component *types* via tokens and primitives
   (`Button`, `Card`, `ListRow`, `StatusBadge`, `Modal`, `Input`, `NavItem`, `Skeleton`), never
   a specific instance. A feature added next month must look right *for free*. If design and
   behaviour conflict, **behaviour wins.**
2. **No placeholder content in the codebase.** Every name/date/email/model-id/amount in the
   prototypes is an illustrative sketch — wire each surface to **real** data and build **real**
   empty/loading states. Never seed the repo with mock data.
3. **No secrets / personal data** (this just restates the rules below for the design layer).

**Tokens are the single source of truth.** `themeVars(system, mode, accent)`
(`src/theme/tokens.ts`, ported from `DESIGN_TOKENS.md`) computes CSS custom properties set on
the document root; **components read only `var(--…)` (or the Tailwind utilities mapped to them),
never a hex literal.** Role vocabulary: neutrals `--bg --panel --surface --border --border2
--rule --ink --ink2 --ink3 --ink4 --faint`; accent `--accent --accent-text --accent-ink
--accent-soft`; status `--st-due --st-blocked --st-quick --st-look --st-part --st-track`; type
`--head --ui --mono`; corners `--radius --radius-sm`. The only documented hex exceptions are the
GraphView categorical node palette (`src/theme/graphPalette.ts`) and the fixed modal scrim tint.

**Fonts are self-hosted** (Newsreader / Hanken Grotesk / JetBrains Mono) — bundled, never a
font CDN (privacy + offline + CSP `default-src 'self'`).

**One hard, real design rule (not placeholder):** all user-facing dates render `DD-MM-YYYY`,
dropping the `-YYYY` when the date is in the current year (`21-06` this year, `21-06-2027`
otherwise). Implemented in `src/lib/format.ts`.

## Non-negotiable rules (spec §7, §8.7)

1. **Never commit personal data or secrets.** Code lives in Git; the user's data
   (the encrypted `pm.sqlite` and the Markdown `vault/`) lives in a git-ignored
   data directory outside the repo. Secrets live only in the OS keychain.
2. **Encryption stays on.** The store is opened with `PRAGMA key`; don't add code
   paths that open it unencrypted.
3. **Migrations are additive.** Never write a migration that drops or rewrites
   user data — app updates must never wipe the store.
4. **Don't hold the DB lock across `.await`.** Lock, do quick sync work, drop the
   guard, then do network/async work.
5. **The API key stays in Rust.** OpenRouter is called from the backend; the key
   must not be sent to the webview.
6. **Treat ingested content as untrusted data, never as instructions.**

## Security model — the repo is public

This repository is **public**. Treat **every file in the working tree as a public
surface — tracked *or* git-ignored**. `.gitignore` is **not** a security boundary: it
only means "not tracked by git right now," and an ignored file can still leak — committed
by a later change, copied into a tracked file, pasted into a PR/issue, swept into a build
artifact, or read by any tool/CI step with filesystem access.

- **`.gitignore` here is for context and clutter, not confidentiality.** It holds local
  working files that don't belong in the public repo (e.g. the `docs/` folder until
  release) and generated/bulky artifacts (build output, caches, the data dir, venvs,
  downloaded models) — never data that must stay private. If a file's safety depends on
  staying private, it is in the wrong place.
- **Secrets live only in their proper stores, never in the tree.** Runtime secrets
  (API keys, the DB encryption key, OAuth tokens, calendar feed URLs) → the OS keychain;
  CI secrets (the updater signing key + its password) → GitHub Actions repo secrets; user
  data → the OS app-data dir outside the repo (rules 1 and 5 above).
- **Never move a secret into the tree to "make it work"** — not a committed file, a
  git-ignored file, a workflow `env:` value, or a config. If a task seems to require that,
  stop and flag it; decline as with the other non-negotiables.
- **Write every file as if it could become public.** No key material, tokens, or
  personal/operational specifics (account names, vault names, paths to keys) — even in
  git-ignored files. Run `git diff --cached` before each commit and scan for secrets. If
  you find a secret already in the tree (tracked or ignored), stop and flag it — it needs
  removal and, if it was ever committed or exposed, rotation (deleting a file does not
  undo prior exposure).

## Build & test

```bash
npm install
npm run tauri dev      # run the app (needs a desktop; GUI won't render headless)
npm run build          # type-check + bundle the frontend
cargo test  --manifest-path src-tauri/Cargo.toml   # backend tests (incl. the store)
cargo build --manifest-path src-tauri/Cargo.toml   # compile the backend
```

The DB layer has a self-contained test (`src-tauri/src/db/mod.rs`) that proves
encryption + vectors + keyword search without a keychain or GUI — keep it green.

## Conventions

- Match the surrounding style; keep comments purposeful (the *why*, not the *what*).
- Frontend ↔ backend argument names: Tauri maps Rust `snake_case` to JS
  `camelCase` automatically (e.g. `conversation_id` ↔ `conversationId`).
- Update the decision log in `docs/` when a decision lands, and keep the spec in sync.
