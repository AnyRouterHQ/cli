#!/usr/bin/env bash
# Local installer tests: grep URLs/channels, then copy ANYR_SETUP_BIN (no network).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SETUP="${ROOT}/setup.sh"

[ -f "$SETUP" ] || {
  echo "missing ${SETUP}" >&2
  exit 1
}

grep -q 'github.com/AnyRouterHQ/cli' "$SETUP"
grep -q 'stable' "$SETUP"
grep -q 'beta' "$SETUP"
grep -q 'auth login' "$SETUP"
grep -q '${BIN_NAME} claude' "$SETUP"
grep -q 'releases/latest/download' "$SETUP"
grep -q 'releases/download' "$SETUP"

if grep -E 'github.com/duyet/|duyet/anyrouter' "$SETUP"; then
  echo "setup.sh must not use github.com/duyet/ download URLs" >&2
  exit 1
fi

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

fake="${tmpdir}/fake-anyr"
cat >"$fake" <<'EOF'
#!/bin/sh
echo "AnyRouter CLI"
echo "login"
echo "claude"
if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
  echo "Usage: anyr <command>"
  echo "  login     Sign in"
  echo "  claude    Launch Claude Code"
fi
EOF
chmod +x "$fake"

bindir="${tmpdir}/bin"
echo "ANYR_SETUP_BIN=${fake} ANYR_BIN_DIR=${bindir}"
ANYR_SETUP_BIN="$fake" ANYR_BIN_DIR="$bindir" bash "$SETUP"

[ -x "${bindir}/anyr" ] || {
  echo "anyr not installed at ${bindir}/anyr" >&2
  exit 1
}
[ -L "${bindir}/anyrouter" ] || {
  echo "missing anyrouter symlink" >&2
  exit 1
}
# `ar` must never be created: it shadows the system archiver (breaks cc/cargo).
[ ! -e "${bindir}/ar" ] && [ ! -L "${bindir}/ar" ] || {
  echo "setup.sh must not create an ar symlink" >&2
  exit 1
}

# An old `ar -> anyr` link is removed on reinstall; an unrelated ar is kept.
ln -sfn anyr "${bindir}/ar"
ANYR_SETUP_BIN="$fake" ANYR_BIN_DIR="$bindir" bash "$SETUP" | grep -q 'Removed .*/ar'
[ ! -L "${bindir}/ar" ] || {
  echo "stale ar -> anyr symlink was not removed" >&2
  exit 1
}
ln -sfn /usr/bin/ar "${bindir}/ar"
ANYR_SETUP_BIN="$fake" ANYR_BIN_DIR="$bindir" bash "$SETUP" >/dev/null
[ "$(readlink "${bindir}/ar")" = /usr/bin/ar ] || {
  echo "unrelated ar symlink must be left alone" >&2
  exit 1
}
rm -f "${bindir}/ar"

help_out="$("${bindir}/anyr" --help)"
echo "$help_out"
echo "$help_out" | grep -q 'AnyRouter CLI'
echo "$help_out" | grep -q 'login'
echo "$help_out" | grep -q 'claude'

# --- 404 / 403 paths: fake curl, no network ---
FAKE_CURL="${ROOT}/tests/fake-curl.py"
[ -f "$FAKE_CURL" ] || {
  echo "missing ${FAKE_CURL}" >&2
  exit 1
}
chmod +x "$FAKE_CURL"

grep -q 'GH_TOKEN' "$SETUP"
grep -q 'never calls api.github.com' "$SETUP"

: >"${tmpdir}/curl.log"
empty_latest_bin="${tmpdir}/bin-empty-latest"
mkdir -p "$empty_latest_bin"
echo "setup.sh stable with empty /latest (no token, no api.github.com)"
# CI injects GITHUB_TOKEN; this path must not use the REST API.
if ! env -u GH_TOKEN -u GITHUB_TOKEN \
  FAKE_CURL_LOG="${tmpdir}/curl.log" \
  ANYR_CURL="$FAKE_CURL" \
  ANYR_BIN_DIR="$empty_latest_bin" \
  bash "$SETUP"; then
  echo "setup.sh failed on empty /latest fallback" >&2
  cat "${tmpdir}/curl.log" >&2 || true
  exit 1
fi
[ -x "${empty_latest_bin}/anyr" ] || {
  echo "stable fallback did not install anyr" >&2
  exit 1
}
if grep -q 'api.github.com' "${tmpdir}/curl.log"; then
  echo "setup.sh must not call api.github.com without a token:" >&2
  cat "${tmpdir}/curl.log" >&2
  exit 1
fi
if grep -q 'releases/download/v0.1.12-beta.98/' "${tmpdir}/curl.log"; then
  :
else
  echo "expected download of v0.1.12-beta.98 (release with binaries):" >&2
  cat "${tmpdir}/curl.log" >&2
  exit 1
fi
python3 -c 'import sys; sys.exit(0 if open(sys.argv[1],"rb").read(4)==b"\x7fELF" else 1)' \
  "${empty_latest_bin}/anyr" || {
  echo "installed file is not the fake ELF asset" >&2
  exit 1
}

: >"${tmpdir}/curl.log"
token_bin="${tmpdir}/bin-token"
mkdir -p "$token_bin"
echo "setup.sh stable with GH_TOKEN uses authenticated API"
env -u GITHUB_TOKEN \
  GH_TOKEN="ghs_test_token" \
  FAKE_CURL_LOG="${tmpdir}/curl.log" \
  ANYR_CURL="$FAKE_CURL" \
  ANYR_BIN_DIR="$token_bin" \
  bash "$SETUP"
[ -x "${token_bin}/anyr" ]
if grep -q 'Authorization: Bearer ghs_test_token' "${tmpdir}/curl.log"; then
  :
else
  echo "expected authenticated GitHub API request:" >&2
  cat "${tmpdir}/curl.log" >&2
  exit 1
fi
if grep -q 'api.github.com' "${tmpdir}/curl.log"; then
  :
else
  echo "token path should call api.github.com:" >&2
  cat "${tmpdir}/curl.log" >&2
  exit 1
fi

: >"${tmpdir}/curl.log"
beta_bin="${tmpdir}/bin-beta"
mkdir -p "$beta_bin"
echo "setup.sh --channel beta (no token, no api.github.com)"
env -u GH_TOKEN -u GITHUB_TOKEN \
  ANYR_CHANNEL=beta \
  FAKE_CURL_LOG="${tmpdir}/curl.log" \
  ANYR_CURL="$FAKE_CURL" \
  ANYR_BIN_DIR="$beta_bin" \
  bash "$SETUP"
[ -x "${beta_bin}/anyr" ]
if grep -q 'api.github.com' "${tmpdir}/curl.log"; then
  echo "beta without token must not call api.github.com:" >&2
  cat "${tmpdir}/curl.log" >&2
  exit 1
fi
if grep -q 'releases/download/v0.1.12-beta.98/' "${tmpdir}/curl.log"; then
  :
else
  echo "expected beta download of v0.1.12-beta.98:" >&2
  cat "${tmpdir}/curl.log" >&2
  exit 1
fi

# --- checksum verification (tags >= 0.1.14 fail closed) ---
os_name="$(uname -s | tr '[:upper:]' '[:lower:]')"
case "$(uname -m)" in x86_64 | amd64) arch_name=x86_64 ;; *) arch_name=arm64 ;; esac
for mode in good bad missing noentry; do
  d="${tmpdir}/bin-sums-${mode}"
  mkdir -p "$d"
  set +e
  env -u GH_TOKEN -u GITHUB_TOKEN \
    FAKE_CURL_SUMS="$mode" FAKE_CURL_ASSET="${os_name}-${arch_name}" \
    ANYR_CURL="$FAKE_CURL" ANYR_VERSION=0.1.20 ANYR_BIN_DIR="$d" \
    bash "$SETUP" >"${tmpdir}/sums-${mode}.log" 2>&1
  rc=$?
  set -e
  if [ "$mode" = good ]; then
    [ "$rc" -eq 0 ] && [ -x "${d}/anyr" ] || {
      echo "matching checksum must install:" >&2
      cat "${tmpdir}/sums-${mode}.log" >&2
      exit 1
    }
  else
    # A tampered or unverifiable binary must never land on PATH.
    [ "$rc" -ne 0 ] && [ ! -e "${d}/anyr" ] || {
      echo "checksums mode=${mode} must abort without installing (rc=${rc}):" >&2
      cat "${tmpdir}/sums-${mode}.log" >&2
      exit 1
    }
  fi
done

echo "setup-sh.test.sh ok"
