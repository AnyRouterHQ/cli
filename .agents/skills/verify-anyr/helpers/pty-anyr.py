#!/usr/bin/env python3
"""Drive `anyr` in a PTY. Used by control-anyr when tmux is unavailable.

Usage:
  pty-anyr.py [--timeout SEC] [--wait REGEX] [--send TEXT] [--out PATH] -- command [args...]

Writes a transcript to stdout (and --out). Exit status is the child's.
"""
from __future__ import annotations

import argparse
import os
import pty
import re
import select
import sys
import time


def main() -> int:
    parser = argparse.ArgumentParser(description="Drive anyr in a PTY")
    parser.add_argument("--timeout", type=float, default=8.0)
    parser.add_argument("--wait", default="", help="Regex that must appear before --send")
    parser.add_argument("--send", default="", help="Bytes to write after --wait (use \\n for Enter)")
    parser.add_argument("--out", default="", help="Write the full transcript here")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()

    cmd = args.command
    if cmd and cmd[0] == "--":
        cmd = cmd[1:]
    if not cmd:
        print("pty-anyr.py: missing command after --", file=sys.stderr)
        return 2

    master_fd, slave_fd = pty.openpty()
    pid = os.fork()
    if pid == 0:
        os.close(master_fd)
        os.setsid()
        os.dup2(slave_fd, 0)
        os.dup2(slave_fd, 1)
        os.dup2(slave_fd, 2)
        if slave_fd > 2:
            os.close(slave_fd)
        os.execvp(cmd[0], cmd)

    os.close(slave_fd)
    buffer = bytearray()
    deadline = time.time() + args.timeout
    sent = not args.send
    wait_re = re.compile(args.wait.encode() if args.wait else b"^")
    waited = not args.wait

    try:
        while time.time() < deadline:
            remaining = max(0.05, deadline - time.time())
            ready, _, _ = select.select([master_fd], [], [], min(0.25, remaining))
            if ready:
                try:
                    chunk = os.read(master_fd, 4096)
                except OSError:
                    break
                if not chunk:
                    break
                buffer.extend(chunk)
                if not waited and wait_re.search(bytes(buffer)):
                    waited = True
                if waited and not sent:
                    payload = args.send.encode("utf-8").decode("unicode_escape").encode("utf-8")
                    os.write(master_fd, payload)
                    sent = True
            else:
                if waited and sent:
                    # Drain a little more after send, then stop if quiet.
                    time.sleep(0.15)
                    ready, _, _ = select.select([master_fd], [], [], 0.2)
                    if not ready:
                        break
        if args.wait and not waited:
            print("pty-anyr.py: timed out waiting for pattern", file=sys.stderr)
    finally:
        try:
            os.close(master_fd)
        except OSError:
            pass
        try:
            _, status = os.waitpid(pid, os.WNOHANG)
            if status == 0:
                os.kill(pid, 15)
                time.sleep(0.2)
                os.kill(pid, 9)
                _, status = os.waitpid(pid, 0)
        except (ChildProcessError, ProcessLookupError, OSError):
            status = 0

    text = buffer.decode("utf-8", errors="replace")
    sys.stdout.write(text)
    if not text.endswith("\n"):
        sys.stdout.write("\n")
    if args.out:
        parent = os.path.dirname(args.out)
        if parent:
            os.makedirs(parent, exist_ok=True)
        with open(args.out, "w", encoding="utf-8") as fh:
            fh.write(text)
            if not text.endswith("\n"):
                fh.write("\n")

    if os.WIFEXITED(status):
        return os.WEXITSTATUS(status)
    return 1


if __name__ == "__main__":
    sys.exit(main())
