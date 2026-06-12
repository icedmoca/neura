#!/usr/bin/env python3
"""Local Kcode UI server.

Serves the built React UI from ui/dist and exposes lightweight live Kcode state at
/api/state, plus a chat bridge (/api/chats, /api/chat) that drives the real kcode
agent via `kcode run --json`. Each chat is a real kcode session; its name is the
two-word handle kcode uses everywhere else: a server modifier ("harbor") paired
with the session's animal ("fox") -> "harbor fox".

This intentionally uses only the Python standard library so it can be called from
slash commands, desktop reload hooks, or manually without setup.
"""
from __future__ import annotations

import argparse
import json
import os
import queue
import random
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time
from http.server import ThreadingHTTPServer, SimpleHTTPRequestHandler
from pathlib import Path
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parents[1]
UI_DIR = ROOT / "ui"
DIST_DIR = UI_DIR / "dist"
KCODE_HOME = Path(os.environ.get("KCODE_HOME", str(Path.home() / ".kcode")))
SESSIONS_DIR = KCODE_HOME / "sessions"
SERVER_IDENTITY_FILE = KCODE_HOME / "ui-server.json"

SUBSCRIBERS: set[queue.Queue[dict]] = set()
SUBSCRIBERS_LOCK = threading.Lock()
SESSION_MTIME_LOCK = threading.Lock()
LIVE_UPDATES_LOCK = threading.Lock()
LIVE_UPDATES_STOP: threading.Event | None = None
LIVE_UPDATES_THREAD: threading.Thread | None = None
LAST_SESSION_MTIME = 0.0


def publish_event(kind: str = "state_changed", **payload) -> None:
    event = {"type": kind, **payload}
    with SUBSCRIBERS_LOCK:
        subscribers = list(SUBSCRIBERS)
    for subscriber in subscribers:
        try:
            subscriber.put_nowait(event)
        except queue.Full:
            pass


def subscribe_events() -> queue.Queue[dict]:
    subscriber: queue.Queue[dict] = queue.Queue(maxsize=128)
    with SUBSCRIBERS_LOCK:
        SUBSCRIBERS.add(subscriber)
    subscriber.put({"type": "connected"})
    return subscriber


def unsubscribe_events(subscriber: queue.Queue[dict]) -> None:
    with SUBSCRIBERS_LOCK:
        SUBSCRIBERS.discard(subscriber)


def latest_session_mtime() -> float:
    try:
        return max((p.stat().st_mtime for p in SESSIONS_DIR.rglob("*.jsonl")), default=0.0)
    except Exception:
        return 0.0


def watch_session_changes(stop: threading.Event) -> None:
    global LAST_SESSION_MTIME
    LAST_SESSION_MTIME = latest_session_mtime()
    while not stop.wait(0.5):
        mtime = latest_session_mtime()
        with SESSION_MTIME_LOCK:
            if mtime <= LAST_SESSION_MTIME:
                continue
            LAST_SESSION_MTIME = mtime
        publish_event("state_changed", reason="session_file_changed")


def initialize_live_updates() -> None:
    """Start live update infrastructure. Safe to call again from reload hooks."""
    global LIVE_UPDATES_STOP, LIVE_UPDATES_THREAD
    with LIVE_UPDATES_LOCK:
        if LIVE_UPDATES_THREAD and LIVE_UPDATES_THREAD.is_alive():
            publish_event("state_changed", reason="live_updates_reinitialized")
            return
        LIVE_UPDATES_STOP = threading.Event()
        LIVE_UPDATES_THREAD = threading.Thread(
            target=watch_session_changes,
            args=(LIVE_UPDATES_STOP,),
            name="kcode-live-updates",
            daemon=True,
        )
        LIVE_UPDATES_THREAD.start()
    publish_event("state_changed", reason="live_updates_initialized")


def shutdown_live_updates() -> None:
    global LIVE_UPDATES_STOP, LIVE_UPDATES_THREAD
    with LIVE_UPDATES_LOCK:
        if LIVE_UPDATES_STOP:
            LIVE_UPDATES_STOP.set()
        LIVE_UPDATES_STOP = None
        LIVE_UPDATES_THREAD = None

# Mirrors src/id.rs SERVER_MODIFIERS so the UI server's name matches kcode's scheme.
SERVER_MODIFIERS = [
    "cove", "grove", "meadow", "marsh", "lake", "river", "creek", "brook", "cliff",
    "peak", "summit", "forest", "garden", "island", "desert", "beach", "harbor",
    "camp", "forge", "citadel", "station", "observatory", "workshop", "lighthouse",
    "temple", "castle", "bridge", "fountain", "stadium", "factory", "pagoda", "hut",
]

# Single timeout for an agent turn; chat calls hit a real provider.
KCODE_RUN_TIMEOUT = int(os.environ.get("KCODE_UI_RUN_TIMEOUT", "300"))


def kcode_bin() -> str:
    """Locate the kcode binary the same way a shell would."""
    found = shutil.which("kcode")
    if found:
        return found
    local = Path.home() / ".local" / "bin" / "kcode"
    return str(local) if local.exists() else "kcode"


def server_identity() -> dict:
    """Stable per-install server name (the modifier word), persisted under KCODE_HOME."""
    try:
        if SERVER_IDENTITY_FILE.exists():
            data = json.loads(SERVER_IDENTITY_FILE.read_text())
            if data.get("serverName"):
                return data
    except Exception:
        pass
    name = random.choice(SERVER_MODIFIERS)
    data = {"serverName": name, "createdAt": time.time()}
    try:
        SERVER_IDENTITY_FILE.write_text(json.dumps(data))
    except Exception:
        pass
    return data


SERVER_NAME = server_identity()["serverName"]


def session_animal(session_id: str) -> str:
    """Extract the memorable animal from a `session_<animal>_<ts>_<rand>` id."""
    if session_id.startswith("session_"):
        rest = session_id[len("session_"):]
        if "_" in rest:
            return rest.split("_", 1)[0]
        return rest
    return session_id


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
        "serverName": SERVER_NAME,
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


# ----------------------------- chat bridge -----------------------------------

def _message_text(blocks) -> tuple[str, list[str]]:
    """Flatten a session message's content into (text, tool_names)."""
    if isinstance(blocks, str):
        return blocks, []
    texts: list[str] = []
    tools: list[str] = []
    if isinstance(blocks, list):
        for b in blocks:
            if not isinstance(b, dict):
                continue
            kind = b.get("type")
            if kind == "text" and b.get("text"):
                texts.append(b["text"])
            elif kind == "tool_use":
                tools.append(b.get("name", "tool"))
    return "\n".join(texts).strip(), tools


def session_messages(session_id: str, data: dict) -> list:
    """Reconstruct a session's full message list the way kcode does: the snapshot
    `.json` messages plus every `append_messages` batch from the `.journal.jsonl`
    (the snapshot lags; the journal holds turns since the last checkpoint)."""
    msgs = list(data.get("messages", []))
    journal = SESSIONS_DIR / f"{session_id}.journal.jsonl"
    if journal.exists():
        try:
            for line in journal.read_text(errors="ignore").splitlines():
                line = line.strip()
                if not line:
                    continue
                try:
                    entry = json.loads(line)
                except Exception:
                    continue
                msgs.extend(entry.get("append_messages") or [])
        except Exception:
            pass
    return msgs


def load_chat(session_id: str) -> dict | None:
    path = SESSIONS_DIR / f"{session_id}.json"
    if not path.exists():
        return None
    try:
        data = json.loads(path.read_text())
    except Exception:
        return None
    messages = []
    for m in session_messages(session_id, data):
        role = m.get("role", "")
        if role not in ("user", "assistant"):
            continue
        text, tools = _message_text(m.get("content"))
        if not text and not tools:
            continue
        messages.append({"role": role, "text": text, "tools": tools})
    animal = data.get("short_name") or session_animal(session_id)
    return {
        "id": session_id,
        "name": animal,
        "serverName": SERVER_NAME,
        "title": data.get("title") or f"{SERVER_NAME} {animal}",
        "model": data.get("model"),
        "updatedAt": data.get("updated_at") or data.get("last_active_at"),
        "messageCount": len(messages),
        "messages": messages,
    }


def list_chats() -> list[dict]:
    out = []
    if not SESSIONS_DIR.exists():
        return out
    for path in SESSIONS_DIR.glob("session_*.json"):
        sid = path.stem
        try:
            data = json.loads(path.read_text())
        except Exception:
            continue
        msgs = session_messages(sid, data)
        if not msgs:
            continue
        animal = data.get("short_name") or session_animal(sid)
        out.append({
            "id": sid,
            "name": animal,
            "serverName": SERVER_NAME,
            "title": data.get("title") or f"{SERVER_NAME} {animal}",
            "model": data.get("model"),
            "updatedAt": data.get("updated_at") or data.get("last_active_at") or path.stat().st_mtime,
            "messageCount": sum(1 for m in msgs if m.get("role") in ("user", "assistant")),
        })
    out.sort(key=lambda c: str(c.get("updatedAt") or ""), reverse=True)
    return out


def run_chat_turn(session_id: str | None, message: str) -> dict:
    """Drive one agent turn via `kcode run --json`, returning a normalized result."""
    cmd = [kcode_bin(), "run", "--json", "--no-update", "--no-selfdev"]
    if session_id:
        cmd.append(f"--resume={session_id}")
    cmd += ["--", message]
    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=KCODE_RUN_TIMEOUT,
            cwd=str(Path.home()),
        )
    except subprocess.TimeoutExpired:
        return {"error": f"kcode run timed out after {KCODE_RUN_TIMEOUT}s"}
    except Exception as exc:
        return {"error": f"failed to launch kcode: {exc}"}

    if proc.returncode != 0:
        detail = (proc.stderr or proc.stdout or "").strip()[-800:]
        return {"error": f"kcode run exited {proc.returncode}: {detail}"}

    # `--json` prints a single pretty JSON object; tolerate leading log noise.
    raw = proc.stdout.strip()
    report = None
    for start in (raw.find("{"), 0):
        if start < 0:
            continue
        try:
            report = json.loads(raw[start:])
            break
        except Exception:
            continue
    if report is None:
        return {"error": f"could not parse kcode output: {raw[:500]}"}

    sid = report.get("session_id", session_id or "")
    animal = session_animal(sid)
    return {
        "session_id": sid,
        "name": animal,
        "serverName": SERVER_NAME,
        "title": f"{SERVER_NAME} {animal}",
        "text": report.get("text", ""),
        "model": report.get("model"),
        "usage": report.get("usage"),
    }


# ------------------------------- shutdown ------------------------------------

def _proc_exe(pid: int) -> str | None:
    try:
        return os.path.realpath(f"/proc/{pid}/exe")
    except Exception:
        return None


def _proc_cmdline(pid: int) -> str:
    try:
        with open(f"/proc/{pid}/cmdline", "rb") as fh:
            return fh.read().replace(b"\0", b" ").decode(errors="ignore")
    except Exception:
        return ""


def _all_pids() -> list[int]:
    return [int(e) for e in os.listdir("/proc") if e.isdigit()]


def kcode_processes() -> list[int]:
    """Every live kcode *binary* process, found by its real executable path so it
    works regardless of how proctitle renames argv/comm. Resolves the install
    path too, and falls back to a basename match for dev/self-dev binaries."""
    target = os.path.realpath(kcode_bin())
    pids = []
    for pid in _all_pids():
        exe = _proc_exe(pid)
        if not exe:
            continue
        if exe == target or os.path.basename(exe) == "kcode":
            pids.append(pid)
    return pids


def ui_server_processes() -> list[int]:
    """Other processes belonging to this web UI (python servers + the bash
    launcher), excluding our own pid which we kill last."""
    me = os.getpid()
    pids = []
    for pid in _all_pids():
        if pid == me:
            continue
        cmd = _proc_cmdline(pid)
        if "kcode-ui-server.py" in cmd or "scripts/kcodeui" in cmd:
            pids.append(pid)
    return pids


def _kill(pids: list[int], sig: int) -> None:
    for pid in pids:
        try:
            os.kill(pid, sig)
        except ProcessLookupError:
            pass
        except Exception:
            pass


def shutdown_everything() -> None:
    """Kill all kcode processes, sibling UI-server processes, then this server.
    Only sends signals — no files are touched — so `kcode` restarts cleanly."""
    others = kcode_processes() + ui_server_processes()
    _kill(others, signal.SIGTERM)
    time.sleep(0.4)
    # Anything still alive gets SIGKILL.
    survivors = [p for p in others if _proc_exe(p) is not None or _proc_cmdline(p)]
    _kill(survivors, signal.SIGKILL)
    # Finally take ourselves down.
    sys.stdout.flush()
    sys.stderr.flush()
    os._exit(0)


# ------------------------------ http layer -----------------------------------

class Handler(SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(DIST_DIR), **kwargs)

    def log_message(self, fmt: str, *args) -> None:
        sys.stderr.write(f"[kcode-ui] {self.client_address[0]} {fmt % args}\n")

    def _send_json(self, payload, status: int = 200) -> None:
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def stream_events(self):
        subscriber = subscribe_events()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "keep-alive")
        self.send_header("Access-Control-Allow-Origin", "*")
        self.end_headers()
        try:
            while True:
                try:
                    event = subscriber.get(timeout=20)
                    payload = json.dumps(event).encode()
                    self.wfile.write(b"event: message\n")
                    self.wfile.write(b"data: " + payload + b"\n\n")
                except queue.Empty:
                    self.wfile.write(b": keepalive\n\n")
                self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError, TimeoutError):
            pass
        finally:
            unsubscribe_events(subscriber)

    def do_GET(self) -> None:  # noqa: N802
        path = urlparse(self.path).path
        if path == "/api/events":
            self.stream_events()
            return
        if path == "/api/state":
            return self._send_json(build_state())
        if path == "/api/chats":
            return self._send_json({"serverName": SERVER_NAME, "chats": list_chats()})
        if path.startswith("/api/chats/"):
            sid = path[len("/api/chats/"):]
            chat = load_chat(sid)
            if chat is None:
                return self.send_error(404, "Unknown chat session")
            return self._send_json(chat)
        if path.startswith("/api/"):
            return self.send_error(404, "Unknown Kcode API endpoint")
        if path == "/" or (DIST_DIR / path.lstrip("/")).exists():
            return super().do_GET()
        # SPA fallback.
        self.path = "/index.html"
        return super().do_GET()

    def do_POST(self) -> None:  # noqa: N802
        path = urlparse(self.path).path
        if path == "/api/shutdown":
            self._send_json({"ok": True, "message": "kcode shutting down"})
            try:
                self.wfile.flush()
            except Exception:
                pass
            # Defer slightly so this response reaches the browser before we die.
            threading.Timer(0.35, shutdown_everything).start()
            return
        if path != "/api/chat":
            return self.send_error(404, "Unknown Kcode API endpoint")
        try:
            length = int(self.headers.get("Content-Length", "0"))
            body = json.loads(self.rfile.read(length) or b"{}")
        except Exception as exc:
            return self._send_json({"error": f"bad request: {exc}"}, status=400)

        message = (body.get("message") or "").strip()
        if not message:
            return self._send_json({"error": "message is required"}, status=400)
        session_id = body.get("session_id") or None

        result = run_chat_turn(session_id, message)
        status = 200 if "error" not in result else 502
        return self._send_json(result, status=status)


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
