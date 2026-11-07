#!/usr/bin/env python3
"""Build GitHub release notes from the release-please CHANGELOG.

Release-please writes CHANGELOG.md (grouped conventional commits, compare
links, SHAs). GitHub Releases should show only the tagged version's section
— not the full history — then the optional bench report. CHANGELOG.md itself
keeps the full history; a release page that repeats it is unreadable (v0.1.37
was 44k chars / 38 versions).
"""
from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

REPO = "https://github.com/AnyRouterHQ/cli"
ROOT = Path(__file__).resolve().parents[1]
CHANGELOG = ROOT / "CHANGELOG.md"
HEADER = """# Changelog

This file is maintained automatically by [release-please](https://github.com/googleapis/release-please).

GitHub Releases use only the tagged version's section as the release notes.
"""

# Same grouping release-please uses with the default changelog notes builder.
SECTION_ORDER = [
    ("feat", "Features"),
    ("fix", "Bug Fixes"),
    ("perf", "Performance Improvements"),
    ("revert", "Reverts"),
]
VISIBLE = {key for key, _ in SECTION_ORDER}
COMMIT_RE = re.compile(
    r"^(?P<type>[a-z]+)(?:\((?P<scope>[^)]+)\))?(?P<breaking>!)?: (?P<subject>.+)$"
)
HEADING_RE = re.compile(r"^## \[(\d+\.\d+\.\d+)\]")
TAG_RE = re.compile(r"^v(\d+\.\d+\.\d+)$")
BENCH_MARKER = "<!-- anyr-bench-report -->"


def git(*args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def version_of(tag: str) -> str:
    match = TAG_RE.match(tag)
    if not match:
        raise SystemExit(f"not a vX.Y.Z tag: {tag}")
    return match.group(1)


def list_tags() -> list[str]:
    tags = [
        line
        for line in git("tag", "-l", "v*", "--sort=v:refname").splitlines()
        if TAG_RE.match(line)
    ]
    if not tags:
        raise SystemExit("no vX.Y.Z tags")
    return tags


def commit_date(rev: str) -> str:
    return git("log", "-1", "--format=%cs", rev)


def list_commits(rev_range: str) -> list[tuple[str, str]]:
    raw = git("log", "--format=%H\t%s", rev_range)
    rows = []
    for line in raw.splitlines():
        sha, _, subject = line.partition("\t")
        if sha and subject:
            rows.append((sha, subject))
    return rows


def format_item(sha: str, subject: str) -> tuple[str, str] | None:
    parsed = COMMIT_RE.match(subject)
    if not parsed:
        return None
    kind = parsed.group("type")
    if kind not in VISIBLE:
        return None
    scope = parsed.group("scope")
    text = parsed.group("subject").rstrip(".")
    prefix = f"**{scope}:** " if scope else ""
    short = sha[:7]
    line = f"* {prefix}{text} ([{short}]({REPO}/commit/{sha}))"
    return kind, line


def section_markdown(commits: list[tuple[str, str]]) -> str:
    buckets: dict[str, list[str]] = {key: [] for key, _ in SECTION_ORDER}
    for sha, subject in commits:
        item = format_item(sha, subject)
        if item is None:
            continue
        kind, line = item
        buckets[kind].append(line)
    parts: list[str] = []
    for kind, title in SECTION_ORDER:
        lines = buckets[kind]
        if not lines:
            continue
        parts.append(f"### {title}")
        parts.append("")
        parts.extend(lines)
        parts.append("")
    return "\n".join(parts)


def version_heading(version: str, previous: str | None, date: str) -> str:
    if previous:
        url = f"{REPO}/compare/{previous}...v{version}"
    else:
        url = f"{REPO}/releases/tag/v{version}"
    return f"## [{version}]({url}) ({date})"


def build_versions() -> list[tuple[str, str]]:
    tags = list_tags()
    versions: list[tuple[str, str]] = []
    previous = None
    for tag in tags:
        version = version_of(tag)
        rev_range = tag if previous is None else f"{previous}..{tag}"
        body = section_markdown(list_commits(rev_range))
        heading = version_heading(version, previous, commit_date(tag))
        block = heading + "\n\n" + (body if body else "")
        versions.append((version, block.rstrip() + "\n"))
        previous = tag
    versions.reverse()
    return versions


def render_changelog(versions: list[tuple[str, str]]) -> str:
    return HEADER + "\n" + "\n".join(block for _, block in versions)


def parse_changelog(text: str) -> list[tuple[str, str]]:
    lines = text.splitlines()
    versions: list[tuple[str, str]] = []
    current: str | None = None
    buf: list[str] = []
    for line in lines:
        match = HEADING_RE.match(line)
        if match:
            if current is not None:
                versions.append((current, "\n".join(buf).rstrip() + "\n"))
            current = match.group(1)
            buf = [line]
            continue
        if current is not None:
            buf.append(line)
    if current is not None:
        versions.append((current, "\n".join(buf).rstrip() + "\n"))
    return versions


def notes_from(versions: list[tuple[str, str]], tag: str, full: bool) -> str:
    version = version_of(tag)
    index = next((i for i, (ver, _) in enumerate(versions) if ver == version), None)
    if index is None:
        raise SystemExit(f"{version} not in changelog")
    chosen = versions[index:] if full else versions[index : index + 1]
    return "\n".join(block for _, block in chosen).rstrip() + "\n"


def cmd_changelog(_args: argparse.Namespace) -> None:
    CHANGELOG.write_text(render_changelog(build_versions()), encoding="utf-8")
    print(f"wrote {CHANGELOG}")


def cmd_github(args: argparse.Namespace) -> None:
    versions = parse_changelog(CHANGELOG.read_text(encoding="utf-8"))
    notes = notes_from(versions, args.tag, full=False)
    if args.bench:
        bench = Path(args.bench).read_text(encoding="utf-8").strip()
        if bench:
            notes = notes.rstrip() + f"\n\n{BENCH_MARKER}\n\n{bench}\n"
    Path(args.out).write_text(notes, encoding="utf-8")
    print(f"wrote {args.out}")


def main() -> None:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="cmd", required=True)

    c = sub.add_parser("changelog", help="rebuild CHANGELOG.md from git tags")
    c.set_defaults(func=cmd_changelog)

    g = sub.add_parser("github", help="write GitHub release notes for a tag")
    g.add_argument("--tag", required=True)
    g.add_argument("--out", required=True)
    g.add_argument("--bench", default="")
    g.set_defaults(func=cmd_github)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as exc:
        sys.stderr.write(exc.output if isinstance(exc.output, str) else "")
        raise SystemExit(exc.returncode)
