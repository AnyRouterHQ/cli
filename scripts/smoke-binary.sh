#!/usr/bin/env bash
# Run a built anyr binary the way a user's first launch does, and fail loudly.
#   scripts/smoke-binary.sh <path-to-binary>
# Catches: macOS code signature broken by strip (SIGKILL, no message), a binary
# that does not execute on its platform, and a --version / --help that crashes.
set -euo pipefail

bin="${1:?usage: smoke-binary.sh <binary>}"
[ -f "$bin" ] || { echo "smoke: not a file: $bin" >&2; exit 1; }
chmod +x "$bin"
case "$bin" in */*) ;; *) bin="./$bin" ;; esac

if [ "$(uname -s)" = "Darwin" ]; then
  codesign --verify --strict --verbose=2 "$bin" \
    || { echo "smoke: $bin has an invalid code signature; Apple Silicon will SIGKILL it" >&2; exit 1; }
fi

status=0
out="$("$bin" --version 2>&1)" || status=$?
if [ "$status" -ne 0 ]; then
  echo "smoke: '$bin --version' exited $status (137 = killed by the OS)" >&2
  printf '%s\n' "$out" >&2
  exit 1
fi
[ -n "$out" ] || { echo "smoke: '$bin --version' printed nothing" >&2; exit 1; }
echo "smoke: $bin --version -> $out"

"$bin" --help >/dev/null 2>&1 || { echo "smoke: '$bin --help' failed" >&2; exit 1; }
echo "smoke: ok"
