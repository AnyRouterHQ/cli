#!/usr/bin/env bash
# Smoke the PACKAGED anyr binary the way users and agents run it.
#   scripts/smoke.sh <binary>          offline checks (every build leg)
#   scripts/smoke.sh <binary> --mock   offline + a local mock API (Linux, macOS)
# One line per check; exits non-zero on the first failure. Runs with a fresh
# ANYROUTER_HOME, no update checks and no telemetry. Needs python3 (or python).
set -uo pipefail

bin="${1:?usage: smoke.sh <binary> [--mock]}"
mode="${2:-}"
case "$bin" in */*) ;; *) bin="./$bin" ;; esac
here="$(cd "$(dirname "$0")" && pwd)"
PY="$(command -v python3 || command -v python)" || { echo "smoke: python3 not found" >&2; exit 1; }

# Platform checks (code signature, --version, --help) live in smoke-binary.sh.
bash "$here/smoke-binary.sh" "$bin" >/dev/null || { echo "FAIL smoke-binary.sh" >&2; exit 1; }
echo "ok   smoke-binary.sh"

work="$(mktemp -d)"
# Git Bash on Windows: hand anyr.exe a Windows path, not /tmp/….
command -v cygpath >/dev/null 2>&1 && work="$(cygpath -m "$work")"
mock_pid=""
cleanup() {
  [ -n "$mock_pid" ] && kill "$mock_pid" 2>/dev/null
  rm -rf "$work"
}
trap cleanup EXIT

export ANYROUTER_HOME="$work/home" HOME="$work/home"
export ANYR_NO_UPDATE=1 ANYR_NO_TELEMETRY=1 DO_NOT_TRACK=1 ANYR_NO_CATALOG=1
unset ANYROUTER_API_KEY ANYROUTER_MANAGEMENT_KEY ANYROUTER_PROFILE ANYR_OUTPUT ANYR_QUIET
mkdir -p "$ANYROUTER_HOME"
# An obviously fake key; the output must never show it in full.
FAKE_KEY="sk-ar-v1-smokesmokesmokesmokesmoke0000"

write_config() { # <base_url or empty>
  {
    printf 'active_profile: default\nprofiles:\n  default:\n    api_key: %s\n' "$FAKE_KEY"
    [ -n "$1" ] && printf '    base_url: %s\n' "$1"
  } >"$ANYROUTER_HOME/config.yaml"
}

fail() { echo "FAIL $1" >&2; [ -s "$work/out" ] && sed 's/^/  out: /' "$work/out" >&2; [ -s "$work/err" ] && sed 's/^/  err: /' "$work/err" >&2; exit 1; }
pass() { echo "ok   $1"; }

# run <stdin-file> <args…>: sets $code; output in $work/out, $work/err.
run() {
  local input="$1"; shift
  "$bin" "$@" <"$input" >"$work/out" 2>"$work/err"
  code=$?
}
is_json() { "$PY" -c 'import json,sys; json.load(open(sys.argv[1]))' "$1" 2>/dev/null; }
# The single JSON error object on stderr carries this exit code.
err_envelope() { # <exit>
  "$PY" - "$work/err" "$1" <<'PY' 2>/dev/null
import json, sys
lines = [l for l in open(sys.argv[1]) if l.strip()]
assert len(lines) == 1
e = json.loads(lines[0])["error"]
assert e["exit"] == int(sys.argv[2]) and e["message"]
PY
}
expect() { # <name> <want-exit>
  [ "$code" -eq "$2" ] || fail "$1: exit $code, want $2"
}

write_config ""
null=/dev/null

run "$null" --version; expect "--version" 0
[ -s "$work/out" ] || fail "--version printed nothing"; pass "--version: $(head -n1 "$work/out")"

run "$null" --help; expect "--help" 0; pass "--help"

run "$null" schema; expect "schema" 0
is_json "$work/out" || fail "schema: stdout is not JSON"; pass "schema is JSON"

run "$null" claude --dry-run --json; expect "claude --dry-run --json" 0
grep -q "$FAKE_KEY" "$work/out" "$work/err" && fail "claude --dry-run --json printed the full key"
pass "claude --dry-run --json masks the key"

run "$null"; expect "bare anyr with no stdin" 0
grep -qiE '\[y/n\]|press enter|paste' "$work/out" "$work/err" && fail "bare anyr prompted"
pass "bare anyr, stdin /dev/null: no prompt"

run "$null" no-such-command --json; expect "unknown command --json" 2
err_envelope 2 || fail "unknown command: stderr is not one JSON error with exit 2"
pass "unknown command: exit 2 with a JSON error"

run "$null" artifact rm smoke123 --json; expect "artifact rm without --yes" 6
err_envelope 6 || fail "artifact rm: stderr is not one JSON error with exit 6"
pass "mutating command without --yes: exit 6"

[ "$mode" = "--mock" ] || { echo "smoke: offline ok"; exit 0; }

"$PY" "$here/smoke_mock.py" "$work/port" 2>"$work/mock.log" &
mock_pid=$!
for _ in $(seq 1 50); do [ -s "$work/port" ] && break; sleep 0.1; done
[ -s "$work/port" ] || fail "mock server did not start"
write_config "http://127.0.0.1:$(cat "$work/port")/api"

for cmd in "whoami" "keys list" "usage" "logs"; do
  # shellcheck disable=SC2086
  run "$null" $cmd --json; expect "$cmd --json" 0
  is_json "$work/out" || fail "$cmd --json: stdout is not JSON"
  grep -q "$FAKE_KEY" "$work/out" "$work/err" && fail "$cmd --json printed the full key"
  pass "$cmd --json (mock)"
done

for pair in "401:4" "402:8" "429:5"; do
  status="${pair%%:*}" want="${pair##*:}"
  run "$null" api GET "/status/$status" --json; expect "HTTP $status" "$want"
  pass "HTTP $status -> exit $want (mock)"
done

echo "smoke: offline and mock ok"
