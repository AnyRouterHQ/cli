#!/usr/bin/env python3
"""Measure anyr binary size + startup, or fold CI JSON into a markdown report."""
from __future__ import annotations

import argparse
import json
import os
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

# Ratchet for the stripped linux x86_64 release binary: the last measured CI
# size plus 2%. Lower it after each size cut; raise it only on purpose.
# 3,536,328 bytes at v0.1.44-beta.865. Raised on purpose for phase 4
# (telemetry counters, crash capture, redaction, `anyr report`): 3,662,768 is
# the local stripped size with `anyr report`; CI built 3,626,424 the commit
# before it. Lower it again after the next size cut.
LINUX_X86_64_BUDGET_BYTES = 3_662_768
BUDGET_ASSET = "anyr-linux-x86_64"
# Median wall time each local command must stay under on the budget asset.
STARTUP_BUDGET_MS = 5.0
# Local commands timed on every native binary. The last two run against a
# throwaway ANYROUTER_HOME with a fake key and make no network call.
STARTUP_CMDS = (
    ("startup_version", ["--version"]),
    ("startup_help", ["--help"]),
    ("startup_account", ["account", "--json"]),
    ("startup_dry_run", ["claude", "--dry-run", "--json"]),
)
FAKE_CONFIG = (
    "active_profile: default\nprofiles:\n  default:\n"
    "    api_key: sk-ar-v1-benchbenchbenchbenchbench0000\n"
)


def human_bytes(n: int) -> str:
    if n < 1024:
        return f"{n} B"
    x = float(n)
    for unit in ("KiB", "MiB", "GiB"):
        x /= 1024.0
        if x < 1024:
            return f"{x:.1f} {unit}"
    return f"{x:.1f} TiB"


def time_cmd(cmd: list[str], n: int = 21, env: dict | None = None) -> dict:
    samples: list[float] = []
    for i in range(n):
        t0 = time.perf_counter()
        proc = subprocess.run(
            cmd,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
            env=env,
        )
        ms = (time.perf_counter() - t0) * 1000.0
        if proc.returncode != 0 and i == 0:
            raise SystemExit(f"command failed ({proc.returncode}): {' '.join(cmd)}")
        samples.append(ms)
    samples.sort()
    p95_idx = min(len(samples) - 1, max(0, int(round(0.95 * (len(samples) - 1)))))
    return {
        "n": n,
        "min_ms": round(samples[0], 2),
        "median_ms": round(statistics.median(samples), 2),
        "p95_ms": round(samples[p95_idx], 2),
        "mean_ms": round(statistics.mean(samples), 2),
    }


def measure(args: argparse.Namespace) -> None:
    path = Path(args.bin).expanduser()
    if not path.is_absolute():
        path = Path.cwd() / path
    path = path.resolve()
    if not path.is_file():
        raise SystemExit(f"binary not found: {path}")
    bin_cmd = str(path)
    size = path.stat().st_size
    version = (
        subprocess.check_output([bin_cmd, "--version"], text=True).strip()
        if args.kind == "native"
        else args.version or "wasm"
    )
    record = {
        "asset": args.asset,
        "kind": args.kind,
        "path": str(path),
        "bytes": size,
        "size": human_bytes(size),
        "version": version,
        "target": args.target or "",
        "os": args.os or "",
    }
    if args.kind == "native":
        with tempfile.TemporaryDirectory() as home:
            Path(home, "config.yaml").write_text(FAKE_CONFIG, encoding="utf-8")
            env = {
                k: v
                for k, v in os.environ.items()
                if not k.startswith(("ANYR", "ANYROUTER")) and k != "CI"
            }
            env.update(
                ANYROUTER_HOME=home,
                HOME=home,
                USERPROFILE=home,
                ANYR_NO_UPDATE="1",
                ANYR_NO_CATALOG="1",
            )
            for key, argv in STARTUP_CMDS:
                record[key] = time_cmd([bin_cmd, *argv], n=args.iters, env=env)
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(record, indent=2))


def load_records(paths: list[str]) -> list[dict]:
    rows = []
    for p in paths:
        path = Path(p)
        if path.is_dir():
            for child in sorted(path.rglob("*.json")):
                rows.append(json.loads(child.read_text(encoding="utf-8")))
        elif path.is_file():
            rows.append(json.loads(path.read_text(encoding="utf-8")))
    rows.sort(key=lambda r: r.get("asset") or r.get("path") or "")
    return rows


def check_budget(rows: list[dict], budget: int = LINUX_X86_64_BUDGET_BYTES) -> list[str]:
    """Return human-readable violations (empty = ok)."""
    violations = []
    for row in rows:
        asset = row.get("asset") or ""
        if asset != BUDGET_ASSET and not asset.endswith("linux-x86_64"):
            continue
        if row.get("kind") != "native":
            continue
        nbytes = int(row.get("bytes") or 0)
        if nbytes > budget:
            violations.append(
                f"{asset} is {human_bytes(nbytes)} ({nbytes} bytes), "
                f"over budget {human_bytes(budget)} ({budget} bytes)"
            )
        for key, argv in STARTUP_CMDS:
            median = (row.get(key) or {}).get("median_ms")
            if median is None:
                violations.append(f"{asset} has no `anyr {' '.join(argv)}` timing")
            elif median > STARTUP_BUDGET_MS:
                violations.append(
                    f"{asset} `anyr {' '.join(argv)}` median {median} ms, "
                    f"over {STARTUP_BUDGET_MS} ms"
                )
    return violations


def to_markdown(rows: list[dict], title: str) -> str:
    lines = [
        f"## {title}",
        "",
        "Startup is wall time for a cold local command (median of 21 runs; "
        "`account --json` and `claude --dry-run --json` use a throwaway home "
        "and a fake key). Size is the stripped release binary, or the "
        "`.wasm` for the browser demo.",
        "",
        f"**Size budget:** `{BUDGET_ASSET}` must stay ≤ "
        f"{human_bytes(LINUX_X86_64_BUDGET_BYTES)} "
        f"({LINUX_X86_64_BUDGET_BYTES} bytes) stripped, and each command "
        f"≤ {STARTUP_BUDGET_MS} ms median.",
        "",
        "| Asset | Kind | Size | `--version` | `--help` | `account --json` "
        "| `claude --dry-run` |",
        "| --- | --- | ---: | ---: | ---: | ---: | ---: |",
    ]
    for row in rows:
        cells = []
        for key, _ in STARTUP_CMDS:
            t = row.get(key) or {}
            cells.append(f"{t['median_ms']} ms" if "median_ms" in t else "—")
        lines.append(
            f"| `{row.get('asset', '?')}` | {row.get('kind', '?')} | "
            f"{row.get('size', '?')} | " + " | ".join(cells) + " |"
        )
    violations = check_budget(rows)
    if violations:
        lines.append("")
        lines.append("**Budget check:** FAILED")
        for v in violations:
            lines.append(f"- {v}")
    else:
        lines.append("")
        lines.append("**Budget check:** ok")
    lines.append("")
    lines.append("<details><summary>raw timings</summary>")
    lines.append("")
    lines.append("```json")
    lines.append(json.dumps(rows, indent=2))
    lines.append("```")
    lines.append("")
    lines.append("</details>")
    lines.append("")
    return "\n".join(lines)


def report(args: argparse.Namespace) -> None:
    rows = load_records(args.inputs)
    if not rows:
        raise SystemExit("no bench JSON found")
    md = to_markdown(rows, args.title)
    Path(args.out).write_text(md, encoding="utf-8")
    json_out = Path(args.json_out) if args.json_out else Path(args.out).with_suffix(".json")
    json_out.write_text(json.dumps(rows, indent=2) + "\n", encoding="utf-8")
    sys.stdout.write(md)
    if args.check_budget:
        violations = check_budget(rows)
        if violations:
            for v in violations:
                print(v, file=sys.stderr)
            raise SystemExit(1)


def main() -> None:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="cmd", required=True)

    m = sub.add_parser("measure")
    m.add_argument("--bin", required=True)
    m.add_argument("--asset", required=True)
    m.add_argument("--out", required=True)
    m.add_argument("--kind", default="native", choices=("native", "wasm"))
    m.add_argument("--target", default="")
    m.add_argument("--os", default="")
    m.add_argument("--version", default="")
    m.add_argument("--iters", type=int, default=21)
    m.set_defaults(func=measure)

    r = sub.add_parser("report")
    r.add_argument("inputs", nargs="+")
    r.add_argument("--out", required=True)
    r.add_argument("--json-out", default="")
    r.add_argument("--title", default="anyr size and startup")
    r.add_argument(
        "--check-budget",
        action="store_true",
        help=f"fail if {BUDGET_ASSET} exceeds {LINUX_X86_64_BUDGET_BYTES} bytes",
    )
    r.set_defaults(func=report)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
