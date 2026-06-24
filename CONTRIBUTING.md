# Contributing to PM

Thanks for looking. PM is a local-first desktop app in **alpha** under active
development. This file is the short version of *how a change gets from your editor
to `main`*: the workflow, what the automated gates check for you, and — just as
important — **what they don't**, so you know what to verify by hand before asking
for a merge.

For a deep tour of the architecture, the data model, and the non-negotiable
security rules, read [`AGENTS.md`](AGENTS.md) first. This file assumes it.

---

## Before you start

- Read [`AGENTS.md`](AGENTS.md) — what PM is, how it's wired, and the rules that
  override everything else (encryption stays on, secrets only in the OS keychain,
  ingested content is untrusted data, the repo is public).
- Set up your environment per the [README](README.md#prerequisites) and confirm the
  app runs: `npm install && npm run tauri dev`.

## The change workflow

1. **Branch off `main`.** Direct pushes to `main` are rejected by branch protection
   — every change lands through a PR.
2. **Make the change**, matching the surrounding style. New source files get the
   two-line SPDX/AGPL header (see existing files).
3. **Run the gate locally** before you push (below).
4. **Bump the version + add a What's New entry** — every PR does this (below).
5. **Open a PR.** CI runs the same gate. A maintainer reviews and merges.

Releases are a separate, tag-driven process — see [`RELEASING.md`](RELEASING.md).

## One command runs everything

The [`justfile`](justfile) is the **single source of truth** for every check. Local
dev, the pre-commit hook, and CI all invoke the same recipes, so they can't drift.

```bash
just check        # everything a PR is gated on — run this before you push
just check-fast   # the fast subset the pre-commit hook runs
just fmt          # auto-apply every formatter (the fix-it counterpart)
```

If `just check` is green, the PR's automated jobs will be too.

## What the gates check for you

So you can trust them, here's the full static net (see [`.github/workflows/pr.yml`](.github/workflows/pr.yml)):

- **Format / lint / types** — Prettier, ESLint, `tsc --noEmit` (frontend); rustfmt,
  Clippy (`-D warnings`) (backend); Ruff check + format (sidecar).
- **Compile + unit tests** — `cargo check --tests` and `cargo test` (includes the
  encrypted-store test and the pure-logic tests).
- **Supply chain** — cargo-deny (advisories, licences, bans, sources), pip-audit,
  npm audit.
- **Secrets & workflows** — gitleaks over the tree, zizmor over the workflows.
- **Version & hygiene** — version lockstep + bump-vs-base + changelog presence,
  files-in-place, SPDX headers, licence integrity.

## What the gates do **not** check — verify these yourself

The gates are strong on *static* quality but do almost nothing at *runtime*. Treat
this as the checklist for every change:

- [ ] **Run the app and exercise your change.** There are no end-to-end or UI tests
      — nothing proves a feature actually works end to end. Launch it
      (`npm run tauri dev`) and try the real path. This is the most important check
      and only you can do it.
- [ ] **No placeholder or mock data** wired into any surface — build real empty and
      loading states instead (design rule; see [`AGENTS.md`](AGENTS.md)).
- [ ] **No hex colour literals in components.** Read design tokens (`var(--…)` / the
      mapped Tailwind utilities) only. This rule isn't linted yet.
- [ ] **Dependency or licence change?** A new crate licence must be added to **both**
      `src-tauri/deny.toml` (the PR gate) **and** `src-tauri/about.toml` (the release
      NOTICE) — otherwise a green PR still fails at release time.
- [ ] **Release-bound change?** The PR gate compiles but never *packages*. Do a real
      `npm run tauri build` before a release relies on it — installer/signing/bundling
      issues don't surface on the PR.
- [ ] **Version bump type is your call.** CI proves the number *moved*, not that it
      moved correctly: features bump the **minor**, fixes/chores the **patch**.

## Versioning & "What's New"

**Every PR bumps the version** — there is no docs-only or chore exemption; the
lockstep gate enforces it. Set the same version across all the lockstep files
(regenerate the lockfiles, don't hand-edit them) and add a matching entry at the top
of [`src/lib/changelog.ts`](src/lib/changelog.ts) — that entry is the in-app
"What's New" users see, and a missing one fails the gate.

```bash
# after editing package.json / tauri.conf.json / Cargo.toml to the new version:
npm install --package-lock-only
cargo update -p pm --manifest-path src-tauri/Cargo.toml
node scripts/check-version-lockstep.mjs --base main   # or: just version --base main
```

The exact file list lives in [`scripts/check-version-lockstep.mjs`](scripts/check-version-lockstep.mjs)
(its `SOURCES` set is the source of truth) and is described in [`RELEASING.md`](RELEASING.md).

## Security

The repository is **public**, and `.gitignore` is **not** a security boundary —
treat every file in the tree as a public surface. Secrets live only in their proper
stores (OS keychain at runtime, GitHub Actions secrets in CI), never in the tree.
Run `git diff --cached` before committing and scan for anything sensitive. The full
rules are in [`AGENTS.md`](AGENTS.md); to report a vulnerability privately, see
[`SECURITY.md`](SECURITY.md).

## License

By contributing you agree your contributions are licensed under the project's
**AGPL-3.0-or-later** ([`LICENCE.txt`](LICENCE.txt)). New source files carry the
two-line SPDX header; never alter the AGPL boilerplate or licence headers.
</content>
