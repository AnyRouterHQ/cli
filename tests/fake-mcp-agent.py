#!/usr/bin/env python3
"""Stand-in coding agent for tests/mcp_auto.rs.

`anyr <agent>` execs this in place of the real binary (ANYROUTER_<AGENT>_PATH).
It finds the MCP server config the way that agent does (Claude Code's
--mcp-config with ${VAR} header expansion, Codex's -c mcp_servers overrides,
OpenCode's inline config with {env:VAR}, Grok's config.toml with
bearer_token_file), then calls initialize and tools/list on it, so the mock
sees exactly the URL and credential the real agent would send. It prints the
tool names, or NO_MCP when anyr wired nothing.
"""
from __future__ import annotations

import json
import os
import re
import sys
import urllib.request

NAME = "anyrouter"


def claude():
    for a in sys.argv[1:]:
        if a.startswith("--mcp-config="):
            s = json.loads(a.split("=", 1)[1])["mcpServers"][NAME]
            # Claude Code expands ${VAR} in headers from the environment, but
            # expands its own ANTHROPIC_* credentials to "" (seen on 2.1.310),
            # which is why anyr exports ANYROUTER_API_KEY for it.
            def expand(m):
                name = m.group(1)
                return "" if name.startswith("ANTHROPIC_") else os.environ.get(name, "")
            auth = re.sub(r"\$\{(\w+)\}", expand, s["headers"]["Authorization"])
            return s["url"], auth
    return None


def codex():
    conf = {}
    args = sys.argv[1:]
    for i, a in enumerate(args):
        if a == "-c" and i + 1 < len(args):
            k, _, v = args[i + 1].partition("=")
            conf[k] = v.strip('"')
    url = conf.get(f"mcp_servers.{NAME}.url")
    if not url:
        return None
    return url, "Bearer " + os.environ[conf[f"mcp_servers.{NAME}.bearer_token_env_var"]]


def opencode(var="OPENCODE_CONFIG_CONTENT"):
    conf = json.loads(os.environ.get(var, "{}"))
    s = conf.get("mcp", {}).get(NAME)
    if not s:
        return None
    auth = re.sub(r"\{env:(\w+)\}", lambda m: os.environ.get(m.group(1), ""),
                  s["headers"]["Authorization"])
    return s["url"], auth


def crush():
    path = os.path.join(os.environ["CRUSH_GLOBAL_CONFIG"], "crush.json")
    s = json.load(open(path, encoding="utf-8")).get("mcp", {}).get(NAME)
    if not s:
        return None
    # Crush expands $VAR in MCP headers from the environment.
    auth = re.sub(r"\$(\w+)", lambda m: os.environ.get(m.group(1), ""),
                  s["headers"]["Authorization"])
    return s["url"], auth


def grok():
    path = os.path.join(os.environ["GROK_HOME"], "config.toml")
    if not os.path.exists(path):
        return None
    text = open(path, encoding="utf-8").read()
    m = re.search(r"^\[mcp_servers\.anyrouter\]\n(.*?)(?=^\[|\Z)", text, re.S | re.M)
    if not m:
        return None
    body = m.group(1)
    url = re.search(r'^url = "(.*)"$', body, re.M).group(1)
    env_var = re.search(r'^bearer_token_env_var = "(.*)"$', body, re.M)
    if env_var:
        return url, "Bearer " + os.environ[env_var.group(1)]
    token_file = re.search(r'^bearer_token_file = "(.*)"$', body, re.M).group(1)
    return url, "Bearer " + open(token_file, encoding="utf-8").read().strip()


def rpc(url, auth, method, rid):
    req = urllib.request.Request(
        url,
        data=json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": {}}).encode(),
        headers={"Authorization": auth, "Content-Type": "application/json",
                 "Accept": "application/json, text/event-stream"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=10) as resp:
        return json.loads(resp.read().decode())["result"]


def main() -> int:
    agent = os.environ["FAKE_AGENT"]
    server = {
        "claude": claude,
        "codex": codex,
        "opencode": opencode,
        "kilo": lambda: opencode("KILO_CONFIG_CONTENT"),
        "crush": crush,
        "grok": grok,
    }.get(agent, lambda: None)()
    if server is None:
        print("NO_MCP")
        return 0
    url, auth = server
    rpc(url, auth, "initialize", 1)
    tools = rpc(url, auth, "tools/list", 2)["tools"]
    print("TOOLS " + ",".join(t["name"] for t in tools))
    return 0


if __name__ == "__main__":
    sys.exit(main())
