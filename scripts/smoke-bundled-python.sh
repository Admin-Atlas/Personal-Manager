# SPDX-FileCopyrightText: 2026 Bobby Yu
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Clean-profile guard for the BUNDLED Linux interpreter, shared verbatim by
# release.yml and linux-bundle-dryrun.yml so the two can't drift. Exercises the
# tree exactly as the bundler staged it into the AppDir: a packaging regression
# that flattens the Python tree (losing lib/python3.x/encodings), drops the
# relative symlinks, or loses the .pm-pyver stamp (which keys the AppImage
# stable-copy machinery in sidecar.rs) makes the document engine dead on first
# ingest while build + package still succeed — this fails the run instead.
#
# Usage: bash scripts/smoke-bundled-python.sh <bundle-dir>
#   <bundle-dir> = the tauri bundle output dir, e.g. src-tauri/target/release/bundle.
#   Anchoring the search THERE matters: tauri-build also stages the same resource
#   map under target/release/python/ on every compile, and matching that copy
#   would validate the wrong tree (and a `find | head` pipeline would die of
#   SIGPIPE on the second match under pipefail — hence -print -quit).

set -euo pipefail

bundle_dir="${1:?usage: smoke-bundled-python.sh <bundle-dir>}"

exe=$(find "$bundle_dir" -path '*/python/bin/python3' \( -type f -o -type l \) -print -quit)
if [ -z "$exe" ]; then
  echo "bundled bin/python3 not found under $bundle_dir — the interpreter wasn't bundled." >&2
  exit 1
fi
echo "bundled interpreter: $exe"

root=$(dirname "$(dirname "$exe")")
if ! ls "$root"/lib/python3.*/encodings/__init__.py >/dev/null 2>&1; then
  ls "$root" | head -30 >&2
  echo "bundled stdlib is flattened/incomplete: lib/python3.x/encodings/__init__.py missing under $root" >&2
  exit 1
fi
if [ ! -f "$root/.pm-pyver" ]; then
  echo "bundled tree has no .pm-pyver stamp under $root — the AppImage stable-copy machinery needs it (fetch-python.mjs writes it)." >&2
  exit 1
fi

"$exe" -c "import encodings, venv, ssl; print('bundled interpreter import OK')"
"$exe" -m venv "${RUNNER_TEMP:-/tmp}/pm-smoke-venv"
"${RUNNER_TEMP:-/tmp}/pm-smoke-venv/bin/python" -c "import encodings; print('venv python import OK')"
echo "bundled-interpreter clean-profile smoke test passed."
