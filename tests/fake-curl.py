#!/usr/bin/env python3
"""Stand-in curl for setup.sh tests. Never talks to the network."""
from __future__ import annotations

import hashlib
import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
LISTING = ROOT / "fixtures" / "releases-listing.html"
API_JSON = ROOT / "fixtures" / "releases-empty-latest.json"
ELF = b"\x7fELF" + b"\x00" * 32


def parse_args(argv: list[str]) -> tuple[str, dict]:
    url = ""
    opts: dict = {
        "head": False,
        "fail": False,
        "output": None,
        "write_out": None,
        "headers": [],
    }
    i = 0
    while i < len(argv):
        a = argv[i]
        if a.startswith("http://") or a.startswith("https://"):
            url = a
        elif a in ("-o", "--output"):
            i += 1
            opts["output"] = argv[i]
        elif a in ("-w", "--write-out"):
            i += 1
            opts["write_out"] = argv[i]
        elif a == "-H":
            i += 1
            opts["headers"].append(argv[i])
        elif a == "-A":
            i += 1
        elif a.startswith("-") and not a.startswith("--"):
            flags = a[1:]
            if "I" in flags:
                opts["head"] = True
            if "f" in flags:
                opts["fail"] = True
        i += 1
    if not url:
        sys.stderr.write("fake-curl: no URL\n")
        sys.exit(2)
    return url, opts


def classify(url: str) -> str:
    if "api.github.com" in url:
        return "api"
    if "/releases/latest/download/" in url:
        return "latest"
    if "/releases/download/v0.1.11/" in url:
        return "empty-tag"
    if url.endswith("/v0.1.12-beta.98/checksums.txt"):
        return "latest"  # legacy tag: no checksums.txt, plain 404
    if "/releases/download/v0.1.12-beta.98/" in url:
        return "beta-asset"
    if "/releases/download/v0.1.20/" in url:
        return "checksummed-sums" if url.endswith("/checksums.txt") else "checksummed-asset"
    if url.rstrip("/").endswith("/releases"):
        return "listing"
    if "/expanded_assets/" in url:
        return "expanded"
    return "other"


def body_and_code(url: str, headers: list[str]) -> tuple[int, bytes]:
    kind = classify(url)
    if kind == "api":
        authed = any("Authorization:" in h for h in headers)
        if not authed:
            return 403, b'{"message":"API rate limit exceeded"}'
        return 200, API_JSON.read_bytes()
    if kind in ("latest", "empty-tag"):
        return 404, b"Not Found"
    if kind in ("beta-asset", "checksummed-asset"):
        return 200, ELF
    if kind == "checksummed-sums":
        mode = os.environ.get("FAKE_CURL_SUMS", "good")
        if mode == "missing":
            return 404, b"Not Found"
        digest = hashlib.sha256(ELF).hexdigest()
        if mode == "bad":
            digest = "0" * 64
        if mode == "noentry":
            return 200, f"{digest}  anyr-other\n".encode()
        return 200, f"{digest}  anyr-{os.environ.get('FAKE_CURL_ASSET', 'linux-x86_64')}\n".encode()
    if kind == "listing":
        return 200, LISTING.read_bytes()
    if kind == "expanded":
        return 200, (
            b'<a href="/AnyRouterHQ/cli/releases/download/'
            b'v0.1.12-beta.98/anyr-linux-x86_64">anyr-linux-x86_64</a>'
        )
    return 404, b"unexpected URL: " + url.encode()


def main() -> None:
    log_path = os.environ.get("FAKE_CURL_LOG")
    if log_path:
        with open(log_path, "a", encoding="utf-8") as fh:
            fh.write(" ".join(sys.argv[1:]) + "\n")
    url, opts = parse_args(sys.argv[1:])
    code, body = body_and_code(url, opts["headers"])
    if opts["write_out"]:
        sys.stdout.write(opts["write_out"].replace("%{http_code}", str(code)))
    if opts["output"]:
        Path(opts["output"]).write_bytes(b"" if opts["head"] else body)
    elif not opts["head"] and not opts["write_out"]:
        sys.stdout.buffer.write(body)
    if opts["fail"] and not (200 <= code < 400):
        sys.exit(22)
    sys.exit(0)


if __name__ == "__main__":
    main()
