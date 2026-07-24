# Releasing PM

Operational runbook for cutting a public release of PM. Read this end to end before
doing anything release-related — even if you only mean to "bump a version."

Releases are **cryptographically signed** and **auto-update onto every installed
copy**. The steps below — and the order they run in — are load-bearing: a release
that builds fine but is assembled wrong can fail *silently*, leaving users
un-prompted on an old version with no error anywhere. Follow the sequence exactly.

> **Roles in this doc.** "The **maintainer**" is whoever holds merge + tag authority
> (currently Bobby). "An **agent**" is an automated assistant that prepares the work
> (e.g. Claude Code). The names are illustrative — the *roles* are what matter, and
> anyone stepping into a role inherits its rules.

---

## The one idea to hold

**A release is a git tag — nothing more.** Releasing does **not** modify `main`
and does **not** create a release branch. You pick a commit on `main`, make sure
its version files already read the release version, and push a tag at that commit.

The tag is the trigger. CI reacts to it by building, signing, and publishing.
Everything else is a consequence of the tag.

`main` sitting ahead of the last release tag — carrying merged features users
can't see yet — is the **correct, expected state**. The absence of a newer tag is
what holds those features back, not a branch.

---

## Who does what (read this first)

Two actions in this process are irreversible and **belong to the maintainer alone**:
**merging the version-bump PR** and **pushing the tag**.

- **An agent (or any contributor) prepares.** Opens the version-bump PR, surfaces the
  exact tag command and the commit SHA it should point at, monitors CI, and reports
  back. Preparation **stops** at each human gate and waits.
- **No agent or contributor**, on their own initiative:
  - merges a release PR,
  - pushes a `v*` tag (these are also restricted to repo admins by branch/tag
    protection),
  - creates a GitHub Release through the web UI.
- **The maintainer owns** the merge and the tag push, every time.

Pushing a release tag ships signed binaries to **everyone** who has PM installed.
Treat the tag push as requiring the maintainer's explicit go-ahead **every single
time** — it is never a routine, fire-and-forget command.

---

## Versioning scheme

PM's roadmap phases are called **Stages**. Code version numbers are **separate** from
Stages — don't conflate them.

- **Every PR moves the number.** Feature PRs bump the **minor**
  (`v2.3 → v2.4`); fixes and chores bump the **patch** (`v2.4.0 → v2.4.1`). This is
  CI-enforced (see [`CONTRIBUTING.md`](CONTRIBUTING.md) and the version gate).
- **A ship is a major bump.** Each major (`vX.0.0`, with minor/patch reset to 0) marks
  a Stage shipping to users. Between ships, feature PRs walk the minor up again, and
  the **release PR performs the major bump**.
- **The project is alpha**, so a pre-release suffix (`-alpha`) stays on every tag for
  now. A pre-release sorts *below* the matching stable version, which is what we want.

Because **every** PR already bumps the version and adds a "What's New" entry, the
in-between work is recorded as it lands. The release PR's job is to finalise the
version to the ship number and roll the accumulated entries into the release notes —
users only see them at the ship, so *late, not live* is the expected experience.

---

## The lockstep version files

PM keeps its version in **several files that must always agree** — with each other
**and** with the git tag at release time. Drift across these is a known, recurring
failure mode; never bump some and defer the rest.

The canonical list is whatever
[`scripts/check-version-lockstep.mjs`](scripts/check-version-lockstep.mjs) actually
checks — **that script is the source of truth.** As of writing its `SOURCES` set is
these (note `package-lock.json` carries the version in **two** places):

1. `package.json` — `.version`
2. `package-lock.json` — root `.version`
3. `package-lock.json` — `packages[""].version` (the self-referential entry)
4. `src-tauri/tauri.conf.json` — `.version`
5. `src-tauri/Cargo.toml` — `[package]` `version`
6. `src-tauri/Cargo.lock` — the `version` of the `[[package]] name = "pm"` entry
7. `src/lib/changelog.ts` — the **top** `CHANGELOG` entry's `version` (this is what
   makes a missing "What's New" entry fail the gate — the newest entry must name the
   new version)

> ⚠ If that script's `SOURCES` set ever changes, **this list follows it** — re-confirm
> against the script before editing, don't trust this prose alone.

The invariant is the only thing that truly matters:

```text
all lockstep files == each other == the pushed git tag (e.g. v2.0.0-alpha)
```

**Regenerate the lockfiles; don't hand-edit them.** Bump the three manifests, then let
the tooling rewrite the locks:

```bash
# after editing package.json / src-tauri/tauri.conf.json / src-tauri/Cargo.toml:
npm install --package-lock-only
cargo update -p pm --manifest-path src-tauri/Cargo.toml
just version --base main        # or: node scripts/check-version-lockstep.mjs --base main
```

---

## Cutting a release — step by step

The worked example uses `v2.0.0-alpha`; substitute the real version for later ships.

### 1. Get `main` to the exact shipping state

Everything intended for this release has merged; CI on the latest `main` commit is
green. There is **no separate "stabilise" phase** — `main` has been release-quality
all along behind branch protection. Preparing to release is mostly *confirming* `main`
is where you want it and not merging anything new for the moment.

> The commit you tag is precisely what users get. Nothing can slip in afterward
> without cutting a new tag — that's the point.

(If anyone else has write access, give them a heads-up to hold merges briefly.)

### 2. Open the release PR — the version bump *(an agent may prepare this, then stops)*

On a branch, set **all** lockstep files to the release version (regenerating the
locks) and roll the accumulated per-PR "What's New" entries into the release notes —
which live in **two places**, both part of this PR:

- **`src/lib/changelog.ts`** — add the release's entry at the top: an at-a-glance
  digest of the accumulated per-PR entries (which stay below it as the detail).
  The lockstep gate reads this top entry, so a missing one fails the version check.
  **Set `release: true` on this digest entry** (the per-PR entries below it stay
  unmarked) so What's New badges the release boundary — the `--tag` release gate
  refuses to tag if the newest entry lacks it.
- **`.github/RELEASE_NOTES.md`** — update the version and the "What's new" digest.
  This file **is** the GitHub Release page (the publish job passes it as
  `--notes-file`), so it must carry the install and self-update instructions a
  downloader needs, written for non-technical readers. **The routine rollup touches
  only the version header and the "What's new" digest** — the `## Install` lines and
  the bottom `## 🐧 Linux` guide (per-format table + AppImage/rpm/deb commands) are
  durable boilerplate: leave them intact unless the install or update mechanics change.
  Three Linux formats ship — AppImage (self-updating), rpm, deb — and a package install
  can't self-update (it's shown a "reinstall to update" note; see `update_delivery.rs`).

Open the PR. **Stop here — do not merge.**

> A whole PR for a version string feels heavy, but direct pushes to `main` are
> rejected by branch protection, and that's correct: the bump *defines* the release,
> so it gets reviewed and CI-checked like any other change.

### 3. The maintainer merges; confirm `main` is green *(maintainer's hands)*

After merge, pull and confirm the lockstep files on `main` actually read the release
version and the post-merge CI passed. This is the last look before the trigger.

```bash
git checkout main
git pull origin main
# confirm every lockstep file reads the release version
```

### 4. Tag the bump commit and push *(maintainer's hands — irreversible)*

From an up-to-date `main`, create an **annotated** tag on the bump commit and push it.
The tag string must be **byte-identical** to what the lockstep files say.

```bash
git checkout main
git pull origin main
git tag -a v2.0.0-alpha -m "PM v2.0.0-alpha"

# sanity-check the tag sits on the bump commit BEFORE pushing:
git log -1 v2.0.0-alpha

git push origin v2.0.0-alpha
```

> **Order is the whole game.** The tag must point at the *already-bumped* commit. Tag
> before the bump merges and the build's internal version won't match the tag, and the
> updater manifest will be wrong. (Recovery is in Gotchas below.)

### 5. Let CI build, sign, and publish *(automatic — do not touch the UI)*

On the `v*` tag, the release workflow builds the per-platform installers, signs them
with the updater key (from the GitHub Actions secret — **never** a local laptop copy),
generates the updater manifest, and creates the GitHub Release with artifacts attached,
in-repo via `GITHUB_TOKEN`.

> Do **not** use GitHub's "Draft a new release" button. CI owns release creation; a
> manual release collides with what CI does. The preparer's role here is to watch the
> workflow and report success or failure.

### 6. Verify end-to-end — the test that actually matters

Two checks:

1. Download the published artifact; confirm it installs and **reports the release
   version**.
2. The real one: take an **existing install on an older version** and confirm it
   **sees the update and applies it**.

Per-platform artifact set to expect on the release page: Windows `-setup.exe`,
macOS `.app.tar.gz` + `.dmg`, Linux `.AppImage` + `.rpm` + `.deb`, plus `latest.json`
(updater feed: `windows-x86_64`, `darwin-x86_64`/`aarch64`, `linux-x86_64` — the
AppImage is the only Linux format the updater handles; rpm/deb users update by
reinstalling the next release's package), `SHA256SUMS`, and `THIRD-PARTY-NOTICES.txt`.

> **Linux lane dry-run:** `.github/workflows/linux-bundle-dryrun.yml` builds the real
> AppImage + rpm + deb and smoke-tests the bundled interpreter **without a tag and without
> secrets** (it signs with a throwaway key). It runs automatically on PRs touching the
> Linux packaging surface and via manual dispatch — use it before a release whenever
> the Linux lane changed, and to answer version-format questions (rpm rejects a raw
> `-alpha`; the bundler owns the label conversion, and this proves it).

What a healthy update looks like: the app checks the feed **once, at launch** (there
is no mid-session poll — a copy that was already running won't notice until it's
reopened), downloads in the background, then shows a banner; clicking **Restart now**
applies the update and relaunches on the new version. On an **unsigned macOS** build
the in-place apply can be refused by Gatekeeper — the banner then degrades to a
manual-download link. That fallback appearing is the app working as designed, not a
broken release; the Windows path must apply in-place.

> "The release page looks right" is **not** the test. The entire value of signed
> auto-update is that installed copies trust and pull it. A release whose manifest the
> updater can't resolve fails *silently* — users simply never get prompted.

---

## Fixing something already shipped — roll forward, never back

There is **one release channel**, and auto-update keeps every install on the latest
tag. So you never go back to patch an old version — you **roll forward**. Fixing
something already shipped is not a special case; it is just normal work on `main` plus
another tag.

1. Land the fix on `main` as an ordinary PR. It bumps the **patch** like any fix
   (`2.0.0-alpha → 2.0.1-alpha`) and adds its own "What's New" entry.
2. Keep going if more fixes are needed — `2.0.1-alpha → 2.0.2-alpha → …`, each its own
   PR, the version walking forward on the single line.
3. When the line is good to ship again, tag a later `main` commit (e.g.
   `v2.0.4-alpha`) — same tag-and-push as any release. Everyone auto-updates to it.

```bash
# (each fix is a normal PR to main that bumps the patch + adds a changelog entry)
# when ready to ship the fixed line, from an up-to-date main:
git tag -a v2.0.4-alpha -m "PM v2.0.4-alpha"
git push origin v2.0.4-alpha                 # CI ships it (maintainer's go-ahead)
```

> **Never branch off a shipped tag to patch it.** There is no second line to support,
> so a back-branch only creates drift. The number always moves forward, and the newest
> tag is always what users get.

---

## Gotchas / never-do

- **Lockstep files always agree.** Never bump some and defer others; the tag must equal
  all of them. `scripts/check-version-lockstep.mjs` is the source of truth for the list
  — confirm against it before editing, don't guess paths.
- **The tag points at the bumped commit.** If you tagged the wrong commit, delete and
  re-push:
  ```bash
  git tag -d v2.0.0-alpha
  git push origin :refs/tags/v2.0.0-alpha
  # fix, then re-tag the correct commit and push again
  ```
- **CI creates the release, not the UI.** Never hand-create a GitHub Release.
- **The signing key never touches a laptop.** It lives only in the GitHub Actions
  secret, with a secure password manager as the offline copy of record. Local
  development (`tauri dev`) never needs it. Never move it into the repo tree — not a
  committed file, a git-ignored file, or a workflow `env:` value.
- **Never flag a PM release as "pre-release" on GitHub.** The updater feed is
  `releases/latest/download/latest.json`, and GitHub resolves `latest` to the newest
  release that is **not** flagged pre-release (or draft). Flag one and every installed
  copy silently stops seeing new versions until a newer *unflagged* release exists.
  The `-alpha` suffix belongs in the version string and tag only; CI's
  `gh release create` deliberately publishes **without** the pre-release flag — don't
  "correct" that in the web UI afterwards, however natural it looks for an alpha.
  Step 6's update test is what proves the feed still resolves.
- **Re-running a failed release.** If the workflow fails after the tag exists (flaky
  runner, transient network), don't invent a new version — re-dispatch the same tag:
  `gh workflow run release.yml -f tag=vX.Y.Z`. If a partial GitHub Release was already
  created, delete it first so the publish step can recreate it cleanly. Only a release
  that never published is safe to re-run this way — once installs have *seen* a
  published release, fixes roll forward instead (see above).
- **Don't invent versions or dependency strings.** Use the **real** values from the
  repo. Made-up versions and made-up dependency versions are a recurring failure mode —
  if you're unsure, read the file, don't guess.
- **A new dependency licence must be allow-listed in two places.** Add it to **both**
  `src-tauri/deny.toml` (the PR gate) **and** `src-tauri/about.toml` (the release
  NOTICE), or a green PR will still fail at release time when `cargo about generate`
  runs. Regenerate the NOTICE with `just notice`.
- **Cross-platform commands.** PM builds on Windows and also runs/builds on macOS. Keep
  any release scripting shell-agnostic — don't leak PowerShell-only syntax into
  something that also runs on macOS CI, and vice versa. (The git commands above are
  identical on both.)
- **Leave AGPL boilerplate untouched.** Never edit licence headers as part of a bump.
- **SHA-pin every GitHub Action.** Release and PR workflows pin each `uses:` to a full
  commit SHA (enforced by the repo) — never a moving tag or branch.

---

## Definition of done

- [ ] All lockstep files read the release version, and they agree.
- [ ] Release PR merged to `main`; post-merge CI green.
- [ ] Annotated tag pushed, sitting on the bump commit; tag string == version.
- [ ] CI workflow completed: installers built, **signed**, manifest generated, GitHub
      Release created in-repo.
- [ ] Fresh install reports the correct version.
- [ ] An older install successfully **auto-updates** to this release. *(N/A for the very
      first ship — there is no prior install to update from; the fresh-install check
      above is the one that applies. Required from the second release on.)*
</content>
