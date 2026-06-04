#!/usr/bin/env python3
"""Local Kcode UI server.

Serves the built React UI from ui/dist and exposes lightweight live Kcode state at
/api/state. This intentionally uses only the Python standard library so it can be
called from slash commands, desktop reload hooks, or manually without setup.
"""
from __future__ import annotations

import argparse
import json
import os
import socket
import subprocess
import sys
import time
from http.server import ThreadingHTTPServer, SimpleHTTPRequestHandler
from pathlib import Path
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parents[1]
UI_DIR = ROOT / "ui"
DIST_DIR = UI_DIR / "dist"
KCODE_HOME = Path(os.environ.get("KCODE_HOME", str(Path.home() / ".kcode")))


def git(cmd: list[str]) -> str:
    try:
        return subprocess.check_output(["git", "-C", str(ROOT), *cmd], text=True, stderr=subprocess.DEVNULL).strip()
    except Exception:
        return ""


def count_files(path: Path, suffix: str) -> int:
    try:
        return sum(1 for p in path.rglob(f"*{suffix}") if ".git" not in p.parts and "target" not in p.parts and "node_modules" not in p.parts)
    except Exception:
        return 0


def build_state() -> dict:
    logs = sorted(KCODE_HOME.glob("*.log"), key=lambda p: p.stat().st_mtime if p.exists() else 0, reverse=True)[:5]
    events = KCODE_HOME / "events.jsonl"
    event_tail: list[dict | str] = []
    if events.exists():
        try:
            lines = events.read_text(errors="ignore").splitlines()[-30:]
            for line in lines:
                try:
                    event_tail.append(json.loads(line))
                except Exception:
                    event_tail.append(line[:500])
        except Exception:
            pass

    status = git(["status", "--short", "--branch"])
    branch = git(["branch", "--show-current"]) or "unknown"
    commits = git(["log", "--oneline", "-8"])
    remotes = git(["remote", "-v"])

    return {
        "generatedAt": time.time(),
        "root": str(ROOT),
        "kcodeHome": str(KCODE_HOME),
        "git": {"branch": branch, "status": status.splitlines(), "commits": commits.splitlines(), "remotes": remotes.splitlines()},
        "repo": {
            "rustFiles": count_files(ROOT / "crates", ".rs"),
            "pythonFiles": count_files(ROOT, ".py"),
            "tsFiles": count_files(UI_DIR / "src", ".ts") + count_files(UI_DIR / "src", ".tsx"),
        },
        "runtime": {
            "pid": os.getpid(),
            "cwd": os.getcwd(),
            "activeMarkers": [p.name for p in KCODE_HOME.glob("*.active")][:20],
            "logs": [{"name": p.name, "size": p.stat().st_size, "mtime": p.stat().st_mtime} for p in logs],
            "eventTail": event_tail,
        },
        "memory": {
            "ctxBands": [
                {"name": "instruction stack", "used": 19, "source": "system/developer/user"},
                {"name": "repo evidence", "used": 27, "source": "git + source scan"},
                {"name": "runtime events", "used": min(35, len(event_tail)), "source": "events.jsonl tail"},
                {"name": "working artifacts", "used": 25, "source": "ui/src + scripts"},
            ],
            "layers": ["working", "episodic", "semantic", "procedural", "artifact", "ctx"],
        },
    }


class Handler(SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(DIST_DIR), **kwargs)

    def log_message(self, fmt: str, *args) -> None:
        sys.stderr.write(f"[kcode-ui] {self.client_address[0]} {fmt % args}\n")

    def do_GET(self) -> None:  # noqa: N802
        path = urlparse(self.path).path
        if path == "/api/state":
            payload = json.dumps(build_state(), indent=2).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Access-Control-Allow-Origin", "http://127.0.0.1:8768")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return
        if path.startswith("/api/"):
            self.send_error(404, "Unknown Kcode API endpoint")
            return
        if path == "/" or (DIST_DIR / path.lstrip("/")).exists():
            return super().do_GET()
        # SPA fallback.
        self.path = "/index.html"
        return super().do_GET()


def ensure_built() -> None:
    if (DIST_DIR / "index.html").exists():
        return
    subprocess.check_call(["npm", "install"], cwd=UI_DIR)
    subprocess.check_call(["npm", "run", "build"], cwd=UI_DIR)


def find_port(preferred: int) -> int:
    for port in range(preferred, preferred + 20):
        with socket.socket() as sock:
            try:
                sock.bind(("127.0.0.1", port))
                return port
            except OSError:
                continue
    raise RuntimeError("No free localhost port found")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=int(os.environ.get("KCODE_UI_PORT", "8768")))
    parser.add_argument("--no-build", action="store_true")
    args = parser.parse_args()

    if not args.no_build:
        ensure_built()
    port = find_port(args.port)
    url = f"http://{args.host}:{port}"
    print(f"KCODE_UI_URL={url}", flush=True)
    httpd = ThreadingHTTPServer((args.host, port), Handler)
    httpd.serve_forever()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
