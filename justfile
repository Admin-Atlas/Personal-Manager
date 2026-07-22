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
check-fast: prettier eslint tsc cargo-fmt ruff ruff-fmt version files headers license-subset ci-membership

# Everything a PR is gated on (adds the compile/test/supply-chain/security checks).
check: check-fast frontend-test clippy cargo-check rust-test sidecar-test deny pip-audit npm-audit gitleaks gitleaks-history zizmor

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

# Frontend unit tests (vitest, scoped to src/lib/** — the pure invariant-bearing modules the audit
# flagged as guarded by nothing: date formatting, the markdown sanitize allowlist) (T-07).
frontend-test:
    npx vitest run

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

# Python dependency CVE audit (resolves + audits the pinned sidecar deps). Scans the OPTIONAL
# OCR/t-SNE pins too (requirements-optional.txt) so an on-demand component's CVE can't ship unnoticed
# (L-6). The optional file is audit-only — the base venv still installs from requirements.txt alone.
pip-audit:
    pip-audit -r sidecar/requirements.txt -r sidecar/requirements-optional.txt

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

# --- release-only ---------------------------------------------------------

# Regenerate the third-party licence NOTICE for the bundled Rust deps.
notice out="THIRD-PARTY-NOTICES.txt":
    cd src-tauri && cargo about generate about.hbs -o "../{{out}}"
