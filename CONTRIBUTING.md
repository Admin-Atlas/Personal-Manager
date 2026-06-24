# Contributing to PM

Welcome. This guide takes you from **zero context** to confidently landing a change
on `main`. It's deliberately detailed: PM has accumulated a set of rules the hard way
— through real mistakes, security reviews, and broken releases — and the fastest way
to be productive here is to absorb the *why* behind them up front rather than trip
over them in review.

Read this once end to end. After that, the [Quick checklist](#9-quick-checklist-every-change)
at the bottom is your per-change reminder.

> **A note on roles.** This doc mentions "the **maintainer**" (whoever holds merge +
> release authority — currently Bobby) and "an **agent**" (an automated assistant such
> as Claude Code that may prepare changes). The names are illustrative; the *roles* are
> what carry the rules, and anyone in a role inherits them.

---

## 1. What PM is (the 60-second model)

PM is a **local-first desktop app** — a Tauri (Rust) shell around a React + TypeScript
UI — with two pillars: **the Archivist** (ingest your files, make them searchable,
answer grounded questions) and **the Personal Assistant** (a focus view + chat that
triage your day). Everything runs and stays **on the user's machine**; the only thing
that leaves is the model API call (and an optional read-only calendar fetch).

Three documents orient you, in order:

1. **[`README.md`](README.md)** — what PM does and how to run it.
2. **[`AGENTS.md`](AGENTS.md)** — the deep tour: architecture, the module map, the data
   model, the design system, and the non-negotiable rules. **This is the most important
   file to read before writing code.** This guide assumes it.
3. **This file** — the *process*: how a change is made, checked, versioned, and merged.

The canonical product spec and the decision log live in a `docs/` folder that is
currently kept local (not yet public). If you don't have them, `AGENTS.md` plus this
guide are enough to contribute well; ask the maintainer when a decision's *intent* is
unclear.

---

## 2. Ground rules that override everything else

These come straight from PM's threat model and spec. If a task seems to require
breaking one, **stop and raise it** rather than working around it — they are not
negotiable, and "just to make it work" is exactly how each one originally got broken.

- **The repository is public. `.gitignore` is *not* a security boundary.** Treat every
  file in the working tree — tracked **or** ignored — as a public surface. An ignored
  file can still leak (committed later, copied into a tracked file, pasted into a PR,
  swept into a build artifact). Write every file as if it will be public.
- **Secrets live only in their proper stores, never in the tree.** Runtime secrets (the
  user's API key, the DB encryption key, OAuth tokens, calendar feed URLs) → the OS
  keychain. CI secrets (the updater signing key) → GitHub Actions secrets. User data →
  the OS app-data directory outside the repo. Never a committed file, an ignored file,
  or a workflow `env:` value.
- **Encryption stays on.** The local store is opened with an encryption key; never add a
  code path that opens it unencrypted.
- **Migrations are additive.** Never write a migration that drops or rewrites user data
  — an app update must never wipe someone's store. The version number only goes up; the
  schema only grows.
- **Don't hold the database lock across an `.await`.** Lock, do quick synchronous work,
  drop the guard, *then* do the network/async work. Holding it across `await` is how you
  deadlock the app.
- **The model API key stays in Rust.** OpenRouter is called from the backend; the key
  must never be handed to the webview.
- **Ingested content is untrusted *data*, never instructions.** Files, calendar feeds,
  and anything fetched are hostile input — size-bound them and never let them steer the
  model as if they were the user.

If you ever find a secret already in the tree (tracked or ignored), **stop and flag it**
— it needs removal and, if it was ever pushed, rotation. Deleting the file does not undo
prior exposure.

---

## 3. Set up your environment

Full prerequisites are in the [README](README.md#prerequisites). In short: **Node 20+**,
**Rust (stable)**, **Python 3.10+** on your PATH, plus your platform's Tauri build
prerequisites.

```bash
npm install
npm run tauri dev      # run the app (needs a desktop; it won't render headless)
```

Platform gotchas worth knowing before you lose an afternoon:

- **Windows builds need a native Perl on PATH** (e.g. Strawberry Perl) — SQLCipher's
  vendored OpenSSL compiles from source via cargo, and that build script needs Perl.
- **In dev, React runs under StrictMode**, which intentionally double-invokes effects.
  If something fires twice in development only, that's why — design effects to tolerate
  it rather than "fixing" the symptom.

---

## 4. The development loop

### a. Branch off `main`

Direct pushes to `main` are **rejected by branch protection** — there are no
exceptions, not even a one-line version bump. Every change lands through a PR. Branch
off an up-to-date `main` with a short, descriptive name (`feat/…`, `fix/…`, `docs/…`,
`chore/…`).

### b. Make the change, in the project's grain

- **Match the surrounding code** — its naming, its idioms, its comment density. Comments
  explain the *why*, not the *what*.
- **Lean by default.** Prefer reusing existing machinery over adding a dependency; keep
  core logic pure and unit-testable; a feature that can live entirely in the frontend
  shouldn't grow the backend.
- **Every new source file gets the two-line SPDX/AGPL header** (copy it from any
  existing file). Never alter existing licence headers or the AGPL boilerplate.
- **Design-system rules are real rules, not suggestions** (full detail in `AGENTS.md`):
  components read design tokens (`var(--…)` or the mapped Tailwind utilities) and carry
  **no hex colour literals**; **no placeholder/mock data** ever enters the tree — build
  real empty and loading states; when design and behaviour conflict, **behaviour wins**;
  all user-facing dates render `DD-MM-YYYY`.

### c. Run the gate locally before you push

The [`justfile`](justfile) is the **single source of truth** for every check. You,
the pre-commit hook, and CI all invoke the *same* recipes, so local and CI cannot
drift — if it's green locally, the PR's automated jobs will be green too.

```bash
just check        # everything a PR is gated on — run this before you push
just check-fast   # the faster subset the pre-commit hook runs
just fmt          # auto-apply every formatter (the fix-it counterpart to the --check recipes)
```

### d. Commit

Use **Conventional Commits** — the prefix sets intent and lines up with the version
bump (below):

```text
feat: …      a user-facing capability        → minor bump
fix: …       a bug fix                        → patch bump
docs: …      documentation only               → patch bump
chore: …     tooling / housekeeping           → patch bump
```

Commit under **your own identity** with a no-reply email (never expose a private email
on the public repo). If an agent co-authored the work, add a `Co-Authored-By:` trailer.

### e. Open a PR — then let the maintainer merge

Open a PR against `main` with a body that explains **what changed and why**. CI runs the
same gate. **An agent or contributor may open detailed PRs but does not merge them** —
the maintainer reviews and performs the merge. Releases are a separate, tag-driven
process owned by the maintainer; see [`RELEASING.md`](RELEASING.md).

---

## 5. What the automated gates check *for* you

So you can trust the green checkmark, here's the full static net (defined in the
[`justfile`](justfile), run by [`.github/workflows/pr.yml`](.github/workflows/pr.yml)):

| Area | Checks |
| --- | --- |
| **Format / lint / types** | Prettier, ESLint, `tsc --noEmit`; rustfmt, Clippy (`-D warnings`); Ruff check + format |
| **Compile + unit tests** | `cargo check --tests`, `cargo test` (incl. the encrypted-store test and pure-logic tests) |
| **Supply chain** | cargo-deny (advisories, licences, bans, sources), pip-audit, npm audit |
| **Secrets & workflows** | gitleaks over the tree, zizmor over the workflows |
| **Version & hygiene** | version lockstep + bump-vs-base + changelog presence, files-in-place, SPDX headers, licence integrity |

Two supply-chain rules contributors hit most often:

- **GitHub Actions are SHA-pinned.** Every `uses:` must reference a full 40-character
  commit SHA (the repo enforces it), never a tag or branch — a moving ref is a supply-
  chain hole.
- **A new Rust dependency's licence must be allow-listed in *two* places** —
  `src-tauri/deny.toml` (the PR gate) **and** `src-tauri/about.toml` (the release
  NOTICE). Miss the second and a green PR still fails at release time.

---

## 6. What the gates do **not** check — verify these yourself

This is the part people miss, so it's spelled out. The gates are strong on *static*
quality but do almost nothing at *runtime* — there are **no end-to-end or UI tests**.
A green PR proves the code compiles, is well-formed, and passes unit tests; it does
**not** prove the feature works. That last mile is on you:

- **Run the app and exercise the real path** (`npm run tauri dev`). This is the single
  most important check and only a human can do it. "The types compile" is not "it works."
- **No placeholder or mock data** wired into any surface — real empty/loading states
  instead.
- **No hex colour literals in components** — tokens only. (Not currently linted.)
- **Dependency/licence change?** Update **both** `deny.toml` and `about.toml`
  (see above).
- **Release-bound change?** The PR gate *compiles* but never *packages*. Do a real
  `npm run tauri build` before a release relies on it — installer/signing/bundling
  issues don't surface on the PR, only at release time.
- **Did you pick the right bump *type*?** CI proves the number *moved*, not that it
  moved correctly. That judgement is yours (next section).

---

## 7. Versioning & "What's New"

**Every PR bumps the version** — there is no docs-only or chore exemption, and the
lockstep gate enforces it. The rule:

- **feat → minor**, **fix / docs / chore → patch**. CI only checks the number went
  *up*; choosing the right *kind* is your call.

Set the same new version across **all** the lockstep files and add a matching entry at
the **top** of [`src/lib/changelog.ts`](src/lib/changelog.ts). That entry is the in-app
**What's New** users see, written in plain, user-facing language — and a missing one
fails the gate (the changelog is one of the lockstep files).

**Regenerate the lockfiles; don't hand-edit them:**

```bash
# after editing package.json / src-tauri/tauri.conf.json / src-tauri/Cargo.toml:
npm install --package-lock-only
cargo update -p pm --manifest-path src-tauri/Cargo.toml
just version --base main      # verify: all files agree AND the number moved vs main
```

The exact file list is defined by `scripts/check-version-lockstep.mjs`'s `SOURCES`
set (the source of truth) and is documented in [`RELEASING.md`](RELEASING.md). If you
push a feature branch over time, bump incrementally as you go; a large feature meant to
land in one shot can be kept local until done (the lockstep gate expects one coherent
version per merged change).

---

## 8. Reporting security issues

Don't open a public issue for a vulnerability. The private reporting process, the
supported versions, and what's in and out of scope are in [`SECURITY.md`](SECURITY.md).

---

## 9. Quick checklist (every change)

- [ ] Branched off an up-to-date `main`.
- [ ] Change matches the surrounding style; new files have the SPDX header.
- [ ] No secrets, personal data, or hex literals; no placeholder/mock data.
- [ ] `just check` passes locally.
- [ ] **Ran the app and exercised the change by hand** (no e2e tests cover this).
- [ ] Dependency/licence change reflected in **both** `deny.toml` and `about.toml`.
- [ ] Version bumped (feat → minor, fix/docs/chore → patch) across the lockstep files,
      lockfiles regenerated, and a top "What's New" entry added.
- [ ] Conventional-commit message; PR body explains *what and why*.
- [ ] Opened the PR for review — **left the merge to the maintainer.**

---

## License

By contributing you agree your contributions are licensed under the project's
**AGPL-3.0-or-later** (see [`LICENCE.txt`](LICENCE.txt)). New source files carry the
two-line SPDX header; never alter the AGPL boilerplate or existing licence headers.
</content>
