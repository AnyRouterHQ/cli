#!/usr/bin/env bash
# Point this repo at .githooks (prepare-commit-msg appends co-author trailers,
# pre-push runs cargo fmt --check and clippy).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
git config core.hooksPath .githooks
chmod +x .githooks/prepare-commit-msg .githooks/pre-push
echo "git core.hooksPath set to .githooks"
echo "Skip one commit with ANYR_SKIP_COAUTHORS=1"
echo "Skip one push with ANYR_SKIP_PREPUSH=1"
