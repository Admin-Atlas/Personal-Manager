# PM checks — the single source of truth. Every check is defined exactly once
# here; pre-commit (the fast subset) and the CI workflows (.github/workflows/)
# invoke these same recipes, so local and CI can never drift.
#
#   just            # list recipes
#   just check-fast # the fast subset (what pre-commit runs)
#   just check      # everything (what a full PR run covers)
#   just fmt        # auto-apply every formatter
#
# Recipes run under bash on every OS (Linux CI + the maintainer's Git-Bash), so
# there are no PowerShell-isms to leak onto the Linux runners.
set shell := ["bash", "-c"]

manifest := "src-tauri/Cargo.toml"

# Default: show the recipe list.
default:
    @just --list

# --- aggregates -----------------------------------------------------------

# The fast subset (formatting, types, lint, bespoke gates) — what pre-commit runs.
# `just ci-membership` asserts BOTH directions of that claim: every member below has a
# step in pr.yml AND a hook in .pre-commit-config.yaml. The claim used to be prose only,
# and pre-commit had drifted to 9 of the 13 — missing, of all things, the drift guards.
check-fast: prettier eslint tsc cargo-fmt ruff ruff-fmt version files headers license-subset ci-membership sync-set script-deps action-pins requirements-lock node-version npm-licenses sidecar-licences

# Everything a PR is gated on (adds the compile/test/supply-chain/security checks).
check: check-fast frontend-test build-frontend clippy cargo-check rust-test sidecar-test deny pip-audit npm-audit gitleaks gitleaks-history zizmor

# Auto-apply every formatter (the writing counterpart to the --check recipes).
fmt:
    npx prettier --write .
    npx eslint . --fix || true
    cargo fmt --manifest-path {{manifest}}
    ruff check --fix sidecar
    ruff format sidecar

# --- frontend (TS / React) ------------------------------------------------

prettier:
    npx prettier --check .

eslint:
    npx eslint .

# Type-check both TS projects: the app (src) and the Node-side config (vite.config.ts,
# via the tsconfig.node.json project reference) — plain `tsc` only checks the root
# project, so vite.config.ts went unchecked (T1-9). Two `-p` passes, not `tsc -b`:
# build mode requires composite refs to emit, which --noEmit forbids (TS6310).
tsc:
    npx tsc --noEmit
    npx tsc -p tsconfig.node.json --noEmit

# Frontend unit tests (vitest). Started as T-07 over the pure src/lib/** modules the audit flagged as
# guarded by nothing (date formatting, the markdown sanitize allowlist), and now also covers component
# tests and scripts/**/*.test.mjs — so "frontend" undersells it; see vitest.config.ts for the globs.
frontend-test:
    npx vitest run

# Build the PRODUCTION webview bundle — the artifact `tauri build` actually ships.
# `tsc --noEmit`, ESLint and vitest between them never run the bundler, so a broken
# dynamic import, a missing asset, a Tailwind/plugin config error or a dependency that
# simply won't bundle was invisible to every PR and first appeared inside the release
# job. In `check` rather than `check-fast`: it is the one frontend check slow enough
# to notice on every commit.
build-frontend:
    npx vite build

# --- backend (Rust) -------------------------------------------------------

# Fetch the bundled standalone interpreter before anything that compiles the
# crate. On Windows and Linux the merged tauri.<platform>.conf.json bundles
# `src-tauri/python/` as a resource, so the Tauri build script needs it present;
# this fetches it (idempotent — the .pm-pyver stamp skips when current, so it's
# a one-time ~70 MB download per machine; CI runners re-download each run since
# nothing caches src-tauri/python). A NO-OP on macOS, which downloads its
# interpreter at runtime instead (python_fetch.rs).
fetch-python:
    node scripts/fetch-python.mjs

# Refresh the curated local-model catalog (#296) from Hugging Face into
# src-tauri/local_models.json. Dev-only + network-bound, so deliberately NOT part of
# `check` (it must never make the PR gate flaky). Uses the @huggingface/gguf
# devDependency; `--discover` prints the top GGUF repos as a curation aid. Idempotent —
# rewrites only on a real content change.
generate-local-catalog:
    node scripts/generate-local-catalog.mjs

cargo-fmt:
    cargo fmt --check --manifest-path {{manifest}}

clippy: fetch-python
    cargo clippy --all-targets --all-features --manifest-path {{manifest}} -- -D warnings

cargo-check: fetch-python
    cargo check --all-targets --tests --manifest-path {{manifest}}

rust-test: fetch-python
    cargo test --manifest-path {{manifest}}

# Local-only test coverage for spotting blind spots (needs `cargo install cargo-llvm-cov`).
# Deliberately NOT in `check` and never gated on a percentage — visibility, not a bar (T1-11).
coverage: fetch-python
    cargo llvm-cov --manifest-path {{manifest}}

# --- sidecar (Python) -----------------------------------------------------

ruff:
    ruff check sidecar

ruff-fmt:
    ruff format --check sidecar

# Fast sidecar unit tests (standard library only; the real-tokenizer check
# self-skips when fastembed isn't installed, e.g. on CI). Locks the token-count
# padding fix — see sidecar/test_pm_sidecar.py.
sidecar-test:
    python sidecar/test_pm_sidecar.py

# --- supply chain & secrets ----------------------------------------------

# Advisories + licences + bans + sources in one (config: src-tauri/deny.toml).
deny:
    cd src-tauri && cargo deny check

# Python dependency CVE audit. Scans the LOCKS — which is every package that actually installs,
# transitive ones included, on the base venv AND both optional components (L-6). It used to scan
# requirements.txt, i.e. the six top-level pins only, so a CVE in (say) pdfminer.six or lxml — both
# of which parse untrusted input — was never looked at.
pip-audit:
    pip-audit -r sidecar/requirements.lock -r sidecar/requirements-ocr.lock -r sidecar/requirements-tsne.lock

# JS dependency CVE audit against the npm lockfile. `moderate` (not `high`) so a moderate-rated
# sanitizer / proto-pollution advisory in the render path can't pass green (L-7).
npm-audit:
    npm audit --audit-level=moderate

# Secret scan of the WHOLE working tree — including git-ignored files, so it's the
# sole net for a secret in an ignored path (config: .gitleaks.toml). CI runs this
# same recipe over its checkout — it does NOT scan a diff (T1-12).
gitleaks:
    gitleaks dir . --config .gitleaks.toml --redact --no-banner

# Secret scan of git HISTORY (every commit) — `dir` mode above misses a secret that
# was committed and later deleted, which lingers in history on a public repo. Fast
# (the whole history is small); CI needs a full-depth checkout to see every commit (T1-12).
gitleaks-history:
    gitleaks git . --config .gitleaks.toml --redact --no-banner

# Workflow-security audit (the pull_request_target / fork-secret class).
zizmor:
    zizmor .github/workflows

# --- bespoke gates (pure Node, no deps) -----------------------------------

# Version lockstep + changelog; pass --base <ref> to require a bump, --tag <vX.Y.Z> to match a tag.
version *args:
    node scripts/check-version-lockstep.mjs {{args}}

# Pre-commit's version check: try the bumped-vs-base check against origin/main so a
# forgotten bump is caught locally (not just in CI), but fall back to lockstep-only
# with a warning when the ref isn't fetched — an offline commit isn't blocked (T1-10).
version-local:
    @if git rev-parse --verify --quiet origin/main >/dev/null 2>&1; then \
        node scripts/check-version-lockstep.mjs --base origin/main; \
    else \
        echo "version-local: origin/main not fetched — lockstep only (bump-vs-base deferred to CI)"; \
        node scripts/check-version-lockstep.mjs; \
    fi

files:
    node scripts/check-files-in-place.mjs

headers:
    node scripts/check-spdx-headers.mjs

# Licence allow-lists agree: everything deny.toml permits (PR gate) is attributable
# in about.toml's accepted set (release NOTICE), so NOTICE generation can't hit an
# unexpected licence after signing (T-02).
license-subset:
    node scripts/check-license-subset.mjs

# Every `just check` recipe is wired into pr.yml, so a gate added here can't silently
# skip CI until pr.yml is separately edited (T-04).
ci-membership:
    node scripts/check-ci-membership.mjs

# Every table the schema creates is classified in SYNC-SET.md (truth / derived / device /
# mixed), so a new table can't land without a declared owner of truth for a future sync.
sync-set:
    node scripts/check-sync-set.mjs

# scripts/ stays zero-dependency (INVARIANTS.md I-18): node: builtins and repo-relative paths only,
# plus a small allowlist of justified exceptions inside the check itself. Not taste — pr.yml's
# hygiene job runs no `npm ci`, so an unlisted import in a gate script passes locally and dies only
# in CI. Also asserts each exception is still imported, dev-only, and exactly pinned.
script-deps:
    node scripts/check-script-deps.mjs

# Every `uses:` in .github/workflows is pinned to a 40-char commit SHA with a readable
# version comment. Both workflow headers already CLAIM "the repo enforces it" — until
# now nothing did, so a `uses: foo/bar@v3` would have sailed through while the repo
# kept asserting the opposite.
action-pins:
    node scripts/check-action-pins.mjs

# The sidecar's dependency locks are current, fully pinned and fully hashed. Offline and
# zero-dependency: each lock stamps the SHA-256 of every input it was generated from, and this
# recomputes them. Also in release.yml's `guards` job — a stale lock blocks a release, not just a
# merge, because the lock is what installs on a user's machine.
requirements-lock:
    node scripts/check-requirements-lock.mjs

# Every CI job runs the Node major package.json declares. The version used to live only in eleven
# hand-maintained workflow pins, so local and CI drifted apart unnoticed — PM was developed on 24
# while CI built and bundled on 20, months after 20 went end-of-life.
node-version:
    node scripts/check-node-version.mjs

# Every npm package that SHIPS carries an accepted licence. The npm half of what cargo-deny does
# for crates: pr.yml's `dependencies` job was named for all three ecosystems and gated only Rust, so
# a copyleft package arriving transitively — through a patch bump four levels down — would first
# have been noticed as a complaint about a released binary. Offline: package-lock.json records a
# licence for every package and marks the dev-only ones.
npm-licenses:
    node scripts/check-npm-licenses.mjs

# Every Python package the sidecar installs is under a licence someone read and accepted — the
# third ecosystem in the job named for all three. Compares sidecar/licences.json against the locks:
# every locked package covered, at every locked version, by a value written BY HAND. PyPI's own
# metadata is too unreliable to normalise automatically (pillow-heif declared BSD-3-Clause while
# classifying itself GPLv2), so the network and the review live in `just lock-regen`; this is
# offline and reaches nothing.
sidecar-licences:
    node scripts/check-sidecar-licences.mjs

# --- generators (not part of `check`) -------------------------------------

# Regenerate the sidecar dependency locks and refresh the licence evidence behind them. Needs `uv`
# on PATH; both halves reach the network. Run after any change to sidecar/requirements.txt or the
# optional pins in sidecar.rs, review anything the second half flags, then commit the result.
lock-regen:
    node scripts/regen-sidecar-locks.mjs
    node scripts/regen-sidecar-licences.mjs

# --- release-only ---------------------------------------------------------

# Regenerate the third-party licence NOTICE: the bundled Rust crates, then the npm packages
# compiled into the shipped webview bundle. The second half used to be missing entirely, so 122
# production packages — the four self-hosted @fontsource families among them — were conveyed with no
# attribution, though MIT, ISC and OFL-1.1 all require their notices to travel with the software.
# Needs `cargo about` AND node_modules (`npm ci`); the npm generator fails rather than emitting a
# short NOTICE, because a partial attribution file reads as compliance.
notice out="THIRD-PARTY-NOTICES.txt":
    cd src-tauri && cargo about generate about.hbs -o "../{{out}}"
    node scripts/generate-npm-notice.mjs >> "{{out}}"
