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


def reap(pid: int, until: float) -> int:
    """Wait for the child until `until`, then kill it.

    The pty reports EOF as soon as the child closes it, which can be before
    the child is reapable. A bare WNOHANG there sees a live child, kills a
    process that was exiting cleanly and reports failure.
    """
    while True:
        wpid, status = os.waitpid(pid, os.WNOHANG)
        if wpid == pid:
            return status
        if time.time() >= until:
            break
        time.sleep(0.02)
    for sig in (15, 9):
        try:
            os.kill(pid, sig)
        except ProcessLookupError:
            break
        time.sleep(0.2)
    return os.waitpid(pid, 0)[1]


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
    eof = False

    try:
        while time.time() < deadline:
            remaining = max(0.05, deadline - time.time())
            ready, _, _ = select.select([master_fd], [], [], min(0.25, remaining))
            if ready:
                try:
                    chunk = os.read(master_fd, 4096)
                except OSError:
                    chunk = b""
                if not chunk:
                    eof = True
                    break
                buffer.extend(chunk)
                if not waited and wait_re.search(bytes(buffer)):
                    waited = True
                if waited and not sent:
                    payload = args.send.encode("utf-8").decode("unicode_escape").encode("utf-8")
                    os.write(master_fd, payload)
                    sent = True
            else:
                # Stop on a quiet pty only after a --send; otherwise read to
                # EOF, which comes when the child closes the terminal.
                if args.send and waited and sent:
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
        # After EOF the child is exiting: give it time. Otherwise it is still
        # running past the deadline or the quiet drain, so stop it now.
        status = reap(pid, time.time() + (5 if eof else 0))

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
