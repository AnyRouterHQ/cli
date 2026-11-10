#!/usr/bin/env bash
# AnyRouter CLI installer. Downloads GitHub Release binaries (not npm).
#   curl -fsSL https://raw.githubusercontent.com/AnyRouterHQ/cli/main/setup.sh | bash
#   curl -fsSL https://anyrouter.dev/setup.sh | bash
#   ANYR_CHANNEL=beta ./setup.sh --channel beta
# Assets: https://github.com/AnyRouterHQ/cli/releases/download/<tag>/anyr-<os>-<arch>
set -euo pipefail

REPO="AnyRouterHQ/cli"
GITHUB="https://github.com/AnyRouterHQ/cli"
GITHUB_API="https://api.github.com/repos/AnyRouterHQ/cli"

BIN_NAME="anyr"
[[ "$(uname -s)" == MSYS* || "$(uname -s)" == MINGW* || "$(uname -s)" == CYGWIN* ]] && BIN_NAME="anyr.exe"
BIN_DIR="${ANYR_BIN_DIR:-${HOME}/.local/bin}"
CHANNEL="${ANYR_CHANNEL:-stable}"
VERSION="${ANYR_VERSION:-}"
CURL="${ANYR_CURL:-curl}"
# When set, anyr writes `telemetry_endpoint: <url>` to the active config after
# install, so the installed binary sends OTLP telemetry to your own collector
# instead of PostHog.
TELEMETRY_OTLP_ENDPOINT="${ANYR_TELEMETRY_OTLP_ENDPOINT:-}"

usage() {
  cat <<EOF
Install AnyRouter CLI (${BIN_NAME}) from GitHub Releases.

Usage:
  ./setup.sh [--channel stable|beta] [--version X.Y.Z]

Channels:
  stable  (default)  GitHub /releases/latest if that asset exists; otherwise
                     the newest release (including prerelease) that has binaries
  beta               newest prerelease that has binaries

Listing uses GH_TOKEN/GITHUB_TOKEN against the GitHub API when set.
Without a token, setup.sh never calls api.github.com (unauth quota 403s);
it reads github.com/releases HTML instead.

Tagged version:
  ${GITHUB}/releases/download/v\${version}/anyr-\${os}-\${arch}

Env:
  ANYR_CHANNEL     stable (default) | beta
  ANYR_VERSION     install this tag (v prefix optional)
  ANYR_BIN_DIR     install directory (default: ~/.local/bin)
  ANYR_TELEMETRY_OTLP_ENDPOINT   send telemetry to your own
                    OpenTelemetry collector over OTLP/HTTP (e.g. http://localhost:4318)
                    instead of PostHog. Written to the anyr config file.
  ANYR_SETUP_BIN   if this is a file, copy it instead of downloading (tests)
  GH_TOKEN / GITHUB_TOKEN   optional; authenticated api.github.com (avoids 403)
  ANYR_CURL        curl binary (tests)

Assets: anyr-linux-x86_64 anyr-linux-arm64 anyr-darwin-x86_64 anyr-darwin-arm64 anyr-windows-x86_64.exe
EOF
}

die() {
  echo "setup.sh: $*" >&2
  exit 1
}

while [ $# -gt 0 ]; do
  case "$1" in
    -h | --help)
      usage
      exit 0
      ;;
    --channel)
      [ $# -ge 2 ] || die "--channel requires a value"
      CHANNEL="$2"
      shift 2
      ;;
    --channel=*)
      CHANNEL="${1#--channel=}"
      shift
      ;;
    --version)
      [ $# -ge 2 ] || die "--version requires a value"
      VERSION="$2"
      shift 2
      ;;
    --version=*)
      VERSION="${1#--version=}"
      shift
      ;;
    --bin-dir)
      [ $# -ge 2 ] || die "--bin-dir requires a value"
      BIN_DIR="$2"
      shift 2
      ;;
    --bin-dir=*)
      BIN_DIR="${1#--bin-dir=}"
      shift
      ;;
    *)
      die "unknown argument: $1"
      ;;
  esac
done

CHANNEL="$(printf '%s' "$CHANNEL" | tr '[:upper:]' '[:lower:]')"
case "$CHANNEL" in
  stable | beta) ;;
  *) die "unknown channel \"${CHANNEL}\" (use stable or beta)" ;;
esac

detect_os() {
  case "$(uname -s)" in
    Linux) printf 'linux' ;;
    Darwin) printf 'darwin' ;;
    MSYS*|MINGW*|CYGWIN*) printf 'windows' ;;
    *) die "unsupported OS: $(uname -s) (need linux, darwin or windows)" ;;
  esac
}

detect_arch() {
  case "$(uname -m)" in
    x86_64 | amd64) printf 'x86_64' ;;
    arm64 | aarch64) printf 'arm64' ;;
    *) die "unsupported arch: $(uname -m) (need x86_64 or arm64)" ;;
  esac
}

write_otlp_telemetry_endpoint() {
  [ -n "$TELEMETRY_OTLP_ENDPOINT" ] || return 0
  local cfg_dir="${HOME}/.anyrouter" cfg="${cfg_dir}/config.yaml"
  mkdir -p "$cfg_dir"
  # Write or update telemetry_endpoint in the active config.
  if [ -f "$cfg" ]; then
    if grep -q '^telemetry_endpoint:' "$cfg"; then
      sed -i.bak 's/^telemetry_endpoint:.*/telemetry_endpoint: '"$TELEMETRY_OTLP_ENDPOINT"'/' "$cfg"
      rm -f "${cfg}.bak"
    else
      printf 'telemetry_endpoint: %s\n' "$TELEMETRY_OTLP_ENDPOINT" >> "$cfg"
    fi
  else
    printf 'active_profile: default\ntelemetry_endpoint: %s\n' "$TELEMETRY_OTLP_ENDPOINT" > "$cfg"
    chmod 600 "$cfg"
  fi
  echo "Telemetry endpoint written to ${cfg}: ${TELEMETRY_OTLP_ENDPOINT}"
}

install_bin() {
  local src="$1"
  [ -f "$src" ] || die "source is not a file: $src"
  mkdir -p "$BIN_DIR"
  cp "$src" "${BIN_DIR}/${BIN_NAME}"
  # macOS Gatekeeper can SIGKILL a freshly curl'd binary before it starts; that
  # is what "error: killed" means. Clear the quarantine xattr if present.
  if command -v xattr >/dev/null 2>&1; then
    for attr in com.apple.quarantine com.apple.security; do
      xattr -d "$attr" "${BIN_DIR}/${BIN_NAME}" 2>/dev/null || true
    done
  fi
  # Apple Silicon also SIGKILLs a binary whose code signature is missing or
  # stale (strip after link breaks it). Re-sign ad hoc when verification fails.
  if [ "$(uname -s)" = "Darwin" ] && command -v codesign >/dev/null 2>&1; then
    if ! codesign --verify "${BIN_DIR}/${BIN_NAME}" >/dev/null 2>&1; then
      codesign --force --sign - "${BIN_DIR}/${BIN_NAME}" \
        || die "codesign failed for ${BIN_DIR}/${BIN_NAME}"
    fi
  fi
  # Windows does not have Unix chmod or symlinks; Linux/Darwin do.
  if [ "$(uname -s)" != MSYS* ] && [ "$(uname -s)" != MINGW* ] && [ "$(uname -s)" != CYGWIN* ]; then
    chmod +x "${BIN_DIR}/${BIN_NAME}"
    ln -sfn "$BIN_NAME" "${BIN_DIR}/anyrouter"
    # Older installers also linked `ar`, which shadows the system archiver and
    # breaks C/Rust builds. Remove it only when it points at anyr.
    if [ -L "${BIN_DIR}/ar" ]; then
      case "$(readlink "${BIN_DIR}/ar")" in
        anyr | */anyr | anyrouter | */anyrouter)
          rm -f "${BIN_DIR}/ar"
          echo "Removed ${BIN_DIR}/ar (it shadowed the system ar archiver)"
          ;;
      esac
    fi
  fi
  echo "Installed ${BIN_DIR}/${BIN_NAME}"
  case "$(uname -s)" in
    MSYS*|MINGW*|CYGWIN*)
      echo "Add ${BIN_DIR} to your PATH in the Windows environment variables, or:"
      echo "  export PATH=\"${BIN_DIR}:\$PATH\""
      ;;
    *)
      echo "Symlink: ${BIN_DIR}/anyrouter"
      case ":${PATH}:" in
        *":${BIN_DIR}:"*) ;;
        *)
          echo "Add ${BIN_DIR} to PATH:"
          echo "  export PATH=\"${BIN_DIR}:\$PATH\""
          ;;
      esac
      ;;
  esac
  echo "Next:"
  echo "  ${BIN_NAME} auth login"
  echo "  ${BIN_NAME} claude"
}

# Local test mode: copy ANYR_SETUP_BIN, never hit the network.
if [ -n "${ANYR_SETUP_BIN:-}" ]; then
  [ -f "$ANYR_SETUP_BIN" ] || die "ANYR_SETUP_BIN is not a file: $ANYR_SETUP_BIN"
  echo "ANYR_SETUP_BIN=${ANYR_SETUP_BIN} (local, no download)"
  install_bin "$ANYR_SETUP_BIN"
  write_otlp_telemetry_endpoint
  exit 0
fi

command -v "$CURL" >/dev/null 2>&1 || die "curl is required to download releases"

os="$(detect_os)"
arch="$(detect_arch)"
asset="anyr-${os}-${arch}"
[ "$os" = windows ] && asset="${asset}.exe"

github_token() {
  if [ -n "${GH_TOKEN:-}" ]; then
    printf '%s' "$GH_TOKEN"
  elif [ -n "${GITHUB_TOKEN:-}" ]; then
    printf '%s' "$GITHUB_TOKEN"
  fi
}

curl_ua() {
  "$CURL" -A "anyr-setup" "$@"
}

# Follow redirects. Empty /releases/latest 302s then 404s; a real asset ends 200.
asset_available() {
  local url="$1"
  local code
  code="$(curl_ua -sI -L -o /dev/null -w '%{http_code}' "$url" || true)"
  case "$code" in
    200 | 206) return 0 ;;
    *) return 1 ;;
  esac
}

download_url_for_tag() {
  printf '%s/releases/download/%s/%s' "$GITHUB" "$1" "$asset"
}

# Authenticated REST only. Never call api.github.com without a token.
pick_tag_from_api() {
  local token="$1"
  local json
  json="$(curl_ua -fsSL \
    -H "Accept: application/vnd.github+json" \
    -H "Authorization: Bearer ${token}" \
    "${GITHUB_API}/releases")" || return 1
  command -v python3 >/dev/null 2>&1 || die "python3 is required to parse GitHub Releases JSON"
  printf '%s' "$json" | python3 -c '
import json, sys
asset, channel = sys.argv[1], sys.argv[2]
releases = json.load(sys.stdin)

def tag_of(rel):
    if rel.get("draft"):
        return ""
    return rel.get("tag_name") or ""

def has_asset(rel):
    names = [a.get("name") for a in (rel.get("assets") or [])]
    return asset in names

def is_pre(rel):
    return bool(rel.get("prerelease"))

ordered = [r for r in releases if tag_of(r)]
# GitHub already returns newest-first; keep that order.
if channel == "beta":
    for rel in ordered:
        if is_pre(rel) and has_asset(rel):
            print(tag_of(rel))
            sys.exit(0)
    sys.stderr.write("no beta release has %s\n" % asset)
    sys.exit(1)

for rel in ordered:
    if (not is_pre(rel)) and has_asset(rel):
        print(tag_of(rel))
        sys.exit(0)
# Stable latest is often empty (v0.1.11). Use newest release that has binaries.
for rel in ordered:
    if has_asset(rel):
        print(tag_of(rel))
        sys.exit(0)
sys.stderr.write("no GitHub release has %s\n" % asset)
sys.exit(1)
' "$asset" "$CHANNEL"
}

list_tags_from_html() {
  local html="$1"
  if command -v python3 >/dev/null 2>&1; then
    printf '%s' "$html" | python3 -c '
import re, sys
html = sys.stdin.read()
seen = set()
tags = []
for m in re.finditer(r"releases/tag/(v[0-9][^\"<>\s#?]*)", html):
    tag = m.group(1).rstrip("/")
    if tag in seen or "/" in tag:
        continue
    seen.add(tag)
    tags.append(tag)

def ver_key(tag):
    s = tag[1:] if tag.startswith("v") else tag
    core, _, pre = s.partition("-")
    nums = []
    for p in core.split("."):
        try:
            nums.append(int(p))
        except ValueError:
            nums.append(0)
    while len(nums) < 3:
        nums.append(0)
    if pre:
        pre_nums = [int(p) for p in re.split(r"[^0-9]+", pre) if p]
        return (tuple(nums), 0, tuple(pre_nums))
    return (tuple(nums), 1, ())

tags.sort(key=ver_key, reverse=True)
for t in tags:
    print(t)
'
    return
  fi
  printf '%s' "$html" | grep -oE 'releases/tag/v[^"<>[:space:]#?]+' \
    | sed 's|.*/||' | awk '!s[$0]++'
}

pick_tag_from_html() {
  local html tag url
  html="$(curl_ua -fsSL "${GITHUB}/releases")" \
    || die "could not fetch ${GITHUB}/releases (set GH_TOKEN to use the GitHub API)"
  local tags
  tags="$(list_tags_from_html "$html")"
  [ -n "$tags" ] || die "no GitHub release tags found at ${GITHUB}/releases"

  if [ "$CHANNEL" = "beta" ]; then
    while IFS= read -r tag; do
      [ -n "$tag" ] || continue
      case "$tag" in
        *-*)
          url="$(download_url_for_tag "$tag")"
          if asset_available "$url"; then
            printf '%s' "$tag"
            return 0
          fi
          ;;
      esac
    done <<EOF
$tags
EOF
    die "no beta release has ${asset}"
  fi

  # stable: prefer a non-prerelease tag that actually has the asset
  while IFS= read -r tag; do
    [ -n "$tag" ] || continue
    case "$tag" in
      *-*) continue ;;
    esac
    url="$(download_url_for_tag "$tag")"
    if asset_available "$url"; then
      printf '%s' "$tag"
      return 0
    fi
  done <<EOF
$tags
EOF

  echo "setup.sh: GitHub /releases/latest has no ${asset}; using newest release that has binaries" >&2
  while IFS= read -r tag; do
    [ -n "$tag" ] || continue
    url="$(download_url_for_tag "$tag")"
    if asset_available "$url"; then
      printf '%s' "$tag"
      return 0
    fi
  done <<EOF
$tags
EOF
  die "no GitHub release has ${asset}"
}

resolve_download_url() {
  local tag url token
  if [ -n "$VERSION" ]; then
    tag="v${VERSION#v}"
    printf '%s' "$(download_url_for_tag "$tag")"
    return 0
  fi

  token="$(github_token || true)"
  if [ -n "$token" ]; then
    if tag="$(pick_tag_from_api "$token")"; then
      [ -n "$tag" ] || die "empty tag from GitHub API"
      printf '%s' "$(download_url_for_tag "$tag")"
      return 0
    fi
    echo "setup.sh: authenticated GitHub API failed; listing github.com/releases instead" >&2
  fi

  if [ "$CHANNEL" = "stable" ]; then
    url="${GITHUB}/releases/latest/download/${asset}"
    if asset_available "$url"; then
      printf '%s' "$url"
      return 0
    fi
  fi

  tag="$(pick_tag_from_html)"
  [ -n "$tag" ] || die "could not resolve a GitHub release for ${asset}"
  printf '%s' "$(download_url_for_tag "$tag")"
}

echo "channel=${CHANNEL} os=${os} arch=${arch}"
url="$(resolve_download_url)"
echo "Downloading ${url}"

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
if ! curl_ua -fsSL "$url" -o "$tmp"; then
  die "download failed: $url"
fi
if [ ! -s "$tmp" ]; then
  die "download was empty: $url"
fi
# GitHub 404 pages are HTML; release assets are ELF / Mach-O.
if head -c 16 "$tmp" | grep -q '<'; then
  die "download did not look like a binary (got HTML?): $url"
fi

# Verify against the release's checksums.txt. Mirrors `anyr upgrade`: tags
# older than CHECKSUMS_REQUIRED_FROM never shipped one, so a 404 there only
# warns; anywhere else a missing or mismatched file aborts the install.
CHECKSUMS_REQUIRED_FROM_PATCH=14 # 0.1.14

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d' ' -f1
  else
    return 1
  fi
}

# Legacy only when the URL names a 0.1.<patch> tag below the cutoff. The
# /releases/latest/ form has no tag, so it fails closed.
checksums_optional() {
  local tag patch
  tag="$(printf '%s' "$1" | sed -n 's#.*/releases/download/v\([^/]*\)/.*#\1#p')"
  case "$tag" in
    0.1.*) ;;
    *) return 1 ;;
  esac
  patch="${tag#0.1.}"
  patch="${patch%%[!0-9]*}"
  [ -n "$patch" ] && [ "$patch" -lt "$CHECKSUMS_REQUIRED_FROM_PATCH" ]
}

verify_checksum() {
  local url="$1" file="$2" sums code expected actual
  sums="$(mktemp)"
  code="$(curl_ua -sL -o "$sums" -w '%{http_code}' "${url%/*}/checksums.txt" || true)"
  if [ "$code" != "200" ]; then
    rm -f "$sums"
    if [ "$code" = "404" ] && checksums_optional "$url"; then
      echo "setup.sh: warning: legacy release has no checksums.txt; installing unverified" >&2
      return 0
    fi
    die "could not fetch checksums.txt (HTTP ${code:-none}); refusing to install unverified binary"
  fi
  expected="$(awk -v a="$asset" '$2 == a || $2 == "*" a { print tolower($1); exit }' "$sums")"
  rm -f "$sums"
  [ -n "$expected" ] || die "checksums.txt has no entry for ${asset}; refusing to install"
  actual="$(sha256_of "$file")" || die "need sha256sum or shasum to verify the download"
  [ "$actual" = "$expected" ] || die "checksum mismatch for ${asset} (expected ${expected}, got ${actual}); refusing to install"
  echo "Verified sha256 ${actual}"
}

verify_checksum "$url" "$tmp"
install_bin "$tmp"
write_otlp_telemetry_endpoint
