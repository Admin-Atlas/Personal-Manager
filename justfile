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
check-fast: prettier eslint tsc cargo-fmt ruff ruff-fmt version files headers

# Everything a PR is gated on (adds the compile/test/supply-chain/security checks).
check: check-fast clippy cargo-check rust-test deny pip-audit npm-audit gitleaks zizmor

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

tsc:
    npx tsc --noEmit

# --- backend (Rust) -------------------------------------------------------

cargo-fmt:
    cargo fmt --check --manifest-path {{manifest}}

clippy:
    cargo clippy --all-targets --all-features --manifest-path {{manifest}} -- -D warnings

cargo-check:
    cargo check --all-targets --tests --manifest-path {{manifest}}

rust-test:
    cargo test --manifest-path {{manifest}}

# --- sidecar (Python) -----------------------------------------------------

ruff:
    ruff check sidecar

ruff-fmt:
    ruff format --check sidecar

# --- supply chain & secrets ----------------------------------------------

# Advisories + licences + bans + sources in one (config: src-tauri/deny.toml).
deny:
    cd src-tauri && cargo deny check

# Python dependency CVE audit (resolves + audits the pinned sidecar deps).
pip-audit:
    pip-audit -r sidecar/requirements.txt

# JS dependency CVE audit against the npm lockfile.
npm-audit:
    npm audit --audit-level=high

# Secret scan of the working tree (config: .gitleaks.toml). CI scans the PR diff.
gitleaks:
    gitleaks dir . --config .gitleaks.toml --redact --no-banner

# Workflow-security audit (the pull_request_target / fork-secret class).
zizmor:
    zizmor .github/workflows

# --- bespoke gates (pure Node, no deps) -----------------------------------

# Version lockstep + changelog; pass --base <ref> to require a bump, --tag <vX.Y.Z> to match a tag.
version *args:
    node scripts/check-version-lockstep.mjs {{args}}

files:
    node scripts/check-files-in-place.mjs

headers:
    node scripts/check-spdx-headers.mjs

# --- release-only ---------------------------------------------------------

# Regenerate the third-party licence NOTICE for the bundled Rust deps.
notice out="THIRD-PARTY-NOTICES.txt":
    cd src-tauri && cargo about generate about.hbs -o "../{{out}}"
