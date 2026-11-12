#!/usr/bin/env python3
"""A tiny AnyRouter API stand-in for scripts/smoke.sh --mock.

Serves fixed JSON on a random loopback port, writes that port to the file
named by argv[1], and logs each request path to stderr. `/api/v1/status/<n>`
answers with HTTP <n>, so the smoke can check how anyr maps 401, 402 and 429.
Standard library only.
"""
from __future__ import annotations

import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

KEY_ROW = {
    "hash": "0123456789abcdef0123456789abcdef",
    "id": "key_smoke",
    "name": "smoke",
    "masked": "sk-ar-v1-smo…oke0",
    "disabled": False,
    "created_at": "2026-01-01T00:00:00Z",
}

ROUTES = {
    "/api/v1/me": {"data": {"email": "smoke@example.com", "username": "smoke", "balance": 12.5}},
    "/api/v1/key": {"data": {"label": "smoke", "limit": None, "usage": 0}},
    "/api/v1/keys": {"data": [KEY_ROW]},
    "/api/v1/credits": {"data": {"balance": 12.5, "total_credits": 20, "total_usage": 7.5}},
    "/api/v1/logs": {"data": [], "has_more": False},
    "/api/v1/models": {"data": [{"id": "smoke/model", "name": "Smoke", "context_length": 8192}]},
}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt: str, *args: object) -> None:
        sys.stderr.write(f"mock: {self.command} {self.path}\n")

    def reply(self, status: int, body: object) -> None:
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("X-Request-Id", f"req_smoke_{status}")
        self.end_headers()
        self.wfile.write(data)

    def handle_any(self) -> None:
        length = int(self.headers.get("Content-Length") or 0)
        if length:
            self.rfile.read(length)
        path = self.path.split("?", 1)[0].rstrip("/")
        if path.startswith("/api/v1/status/"):
            code = int(path.rsplit("/", 1)[1])
            self.reply(code, {"error": {"message": f"smoke status {code}"}})
        elif path in ROUTES:
            self.reply(200, ROUTES[path])
        else:
            self.reply(404, {"error": {"message": f"no smoke fixture for {path}"}})

    do_GET = do_POST = do_PATCH = do_DELETE = do_PUT = handle_any


def main() -> None:
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    with open(sys.argv[1], "w", encoding="utf-8") as f:
        f.write(str(server.server_address[1]))
    server.serve_forever()


if __name__ == "__main__":
    main()
