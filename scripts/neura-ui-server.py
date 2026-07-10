#!/usr/bin/env python3
"""Local Neura UI server.

Serves the built React UI from ui/dist and exposes lightweight live Neura state at
/api/state, plus a chat bridge (/api/chats, /api/chat) that drives the real neura
agent via `neura run --json`. Each chat is a real neura session; its name is the
two-word handle neura uses everywhere else: a server modifier ("harbor") paired
with the session's animal ("fox") -> "harbor fox".

This intentionally uses only the Python standard library so it can be called from
slash commands, desktop reload hooks, or manually without setup.
"""
from __future__ import annotations

import argparse
import difflib
import hashlib
import json
import os
import queue
import random
import re
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time
import webbrowser
from http.server import ThreadingHTTPServer, SimpleHTTPRequestHandler
from pathlib import Path
from urllib.parse import parse_qs, urlparse
import urllib.request
import urllib.error

ROOT = Path(__file__).resolve().parents[1]
SCRIPTS_DIR = Path(__file__).resolve().parent
if str(SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPTS_DIR))
import dapp_engine as de  # noqa: E402
import dapp_components as dc  # noqa: E402
import dapp_context as dctx  # noqa: E402
UI_DIR = ROOT / "ui"
DIST_DIR = UI_DIR / "dist"
NEURA_HOME = Path(os.environ.get("NEURA_HOME", str(Path.home() / ".neura")))
DEFAULT_WORKSPACE_DIR = NEURA_HOME / "workspace"
PROJECTS_DIR = NEURA_HOME / "projects"
SESSIONS_DIR = NEURA_HOME / "sessions"
UI_META_DIR = NEURA_HOME / "ui"
UI_CHAT_META_FILE = UI_META_DIR / "chat-meta.json"
UI_PROJECTS_FILE = UI_META_DIR / "projects.json"
DAPP_LIBRARY_DIR = UI_META_DIR / "dapp-library"
DAPP_LIBRARY_INDEX = DAPP_LIBRARY_DIR / "index.json"
DAPP_LIBRARY_TEMPLATES = DAPP_LIBRARY_DIR / "templates"
DAPP_LIBRARY_PINS_FILE = DAPP_LIBRARY_DIR / "pins.json"
DAPP_THEMES_DIR = UI_META_DIR / "dapp-themes"
DAPP_THEMES_INDEX = DAPP_THEMES_DIR / "index.json"
SERVER_IDENTITY_FILE = NEURA_HOME / "ui-server.json"
UI_META_LOCK = threading.Lock()
TITLE_REFRESH_LOCK = threading.Lock()
TITLE_REFRESH_IN_FLIGHT: set[str] = set()
DAPP_GEN_LOCK = threading.Lock()
DAPP_GEN_IN_FLIGHT: set[str] = set()
DAPP_GEN_DEBOUNCE: dict[str, threading.Timer] = {}
DAPP_GEN_PENDING: dict[str, tuple[str, str, float]] = {}

SUBSCRIBERS: set[queue.Queue[dict]] = set()
SUBSCRIBERS_LOCK = threading.Lock()
SESSION_MTIME_LOCK = threading.Lock()
LIVE_UPDATES_LOCK = threading.Lock()
LIVE_UPDATES_STOP: threading.Event | None = None
LIVE_UPDATES_THREAD: threading.Thread | None = None
LAST_SESSION_MTIME = 0.0
LAST_DAPP_MTIMES: dict[str, float] = {}


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


def latest_dapp_mtimes() -> dict[str, float]:
    mtimes: dict[str, float] = {}
    for project in list_projects():
        path = project.get("path")
        if not isinstance(path, str) or not path:
            continue
        root = Path(path) / ".neura" / "dapp"
        if not root.is_dir():
            continue
        try:
            mtimes[path] = max(
                (item.stat().st_mtime for item in root.rglob("*") if item.is_file()),
                default=0.0,
            )
        except Exception:
            continue
    return mtimes


def watch_live_changes(stop: threading.Event) -> None:
    global LAST_SESSION_MTIME, LAST_DAPP_MTIMES
    LAST_SESSION_MTIME = latest_session_mtime()
    LAST_DAPP_MTIMES = latest_dapp_mtimes()
    while not stop.wait(0.4):
        mtime = latest_session_mtime()
        with SESSION_MTIME_LOCK:
            if mtime > LAST_SESSION_MTIME:
                LAST_SESSION_MTIME = mtime
                publish_event("state_changed", reason="session_file_changed")

        current_dapp = latest_dapp_mtimes()
        for path, dapp_mtime in current_dapp.items():
            prev = LAST_DAPP_MTIMES.get(path, 0.0)
            if dapp_mtime > prev:
                LAST_DAPP_MTIMES[path] = dapp_mtime
                publish_event("dapp_changed", project_path=path)
        for path, dapp_mtime in current_dapp.items():
            if path not in LAST_DAPP_MTIMES:
                LAST_DAPP_MTIMES[path] = dapp_mtime


def watch_session_changes(stop: threading.Event) -> None:
    watch_live_changes(stop)


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
            name="neura-live-updates",
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

# Mirrors src/id.rs SERVER_MODIFIERS so the UI server's name matches neura's scheme.
SERVER_MODIFIERS = [
    "cove", "grove", "meadow", "marsh", "lake", "river", "creek", "brook", "cliff",
    "peak", "summit", "forest", "garden", "island", "desert", "beach", "harbor",
    "camp", "forge", "citadel", "station", "observatory", "workshop", "lighthouse",
    "temple", "castle", "bridge", "fountain", "stadium", "factory", "pagoda", "hut",
]

# Single timeout for an agent turn; chat calls hit a real provider.
NEURA_RUN_TIMEOUT = int(os.environ.get("NEURA_UI_RUN_TIMEOUT", "300"))
NEURA_TITLE_TIMEOUT = int(os.environ.get("NEURA_UI_TITLE_TIMEOUT", "60"))
NEURA_TITLE_MODEL = os.environ.get("NEURA_UI_TITLE_MODEL", "").strip()
NEURA_DAPP_TIMEOUT = int(os.environ.get("NEURA_UI_DAPP_TIMEOUT", "120"))
NEURA_DAPP_MODEL = os.environ.get("NEURA_UI_DAPP_MODEL", "").strip()
NEURA_DAPP_INCREMENTAL_MODEL = os.environ.get("NEURA_UI_DAPP_INCREMENTAL_MODEL", "").strip()
NEURA_DAPP_DEBOUNCE = float(os.environ.get("NEURA_UI_DAPP_DEBOUNCE", "2"))
NEURA_DAPP_DEBOUNCE_FAST = float(os.environ.get("NEURA_UI_DAPP_DEBOUNCE_FAST", "0.45"))
NEURA_DAPP_INCREMENTAL_TIMEOUT = int(os.environ.get("NEURA_UI_DAPP_INCREMENTAL_TIMEOUT", "75"))
NEURA_DAPP_MATCH_THRESHOLD = float(os.environ.get("NEURA_UI_DAPP_MATCH_THRESHOLD", "0.28"))
NEURA_DAPP_MATCH_MIN_OVERLAP = int(os.environ.get("NEURA_UI_DAPP_MATCH_MIN_OVERLAP", "2"))
TITLE_MAX_CHARS = 72
TITLE_GEN_MARKER = "You label coding-agent chat threads in a sidebar"
DAPP_GEN_MARKER = "You generate contextual Neura project dapp files from chat transcripts"

DAPP_EDIT_HINTS = (
    "change the", "change it", "make the", "make it", "update the", "update it",
    "switch the", "switch to", "use a different", "darker", "lighter", "bigger",
    "smaller", "add a button", "remove the", "fix the dapp", "modify the",
    "edit the dapp", "in the dapp", "on the dapp", "background to", "font ",
    "make this", "can you change", "turn it", "change my dapp", "update my dapp",
)

DAPP_VISUAL_HINTS = (
    "weather", "forecast", "temperature", "rain", "snow", "sunny", "humidity",
    "video", "youtube", "spotify", "song", "music", "lyrics", "album", "artist",
    "map", "restaurant", "recipe", "flight", "hotel", "score", "stats", "chart",
    "graph", "dashboard", "calendar", "todo", "timer", "clock", "price", "stock",
    "news", "image", "photo", "gallery", "gif", "gifs", "meme", "memes", "embed", "link", "button", "card", "ui",
    "interface", "preview", "show me", "display", "visual", "watch", "listen",
    "calculator", "convert", "currency", "crypto", "game", "fox", "movie", "trailer",
)

DAPP_SKIP_ACKS = {
    "thanks", "thank you", "ok", "okay", "cool", "got it", "nice", "perfect",
    "yes", "no", "yep", "nope", "sure", "great", "lol", "haha", "k", "ty",
}

# Category/topic vocabulary lives in dapp_context (conversation-driven, no geo defaults).
DAPP_CATEGORIES = dctx.CATEGORY_HINTS
DAPP_TOPIC_ALIASES = dctx.TOPIC_ALIASES
DAPP_KEYWORD_STOPWORDS = dctx.KEYWORD_STOPWORDS


def _valid_neura_executable(path: Path) -> str | None:
    try:
        resolved = path.expanduser().resolve(strict=False)
    except Exception:
        return None
    if not resolved.is_file():
        return None
    if not os.access(resolved, os.X_OK):
        return None
    return str(resolved)


def neura_bin() -> str:
    """Locate the neura binary even when the UI server has a minimal PATH."""
    override = os.environ.get("NEURA_BIN", "").strip()
    if override:
        found = _valid_neura_executable(Path(override))
        if found:
            return found

    candidates: list[Path] = [
        ROOT / "target" / "release" / "neura",
        ROOT / "target" / "debug" / "neura",
        NEURA_HOME / "builds" / "current" / "neura",
        NEURA_HOME / "builds" / "stable" / "neura",
        Path.home() / ".local" / "bin" / "neura",
    ]
    found = shutil.which("neura")
    if found:
        candidates.append(Path(found))

    seen: set[str] = set()
    for candidate in candidates:
        resolved = _valid_neura_executable(candidate)
        if not resolved or resolved in seen:
            continue
        seen.add(resolved)
        return resolved
    return str((ROOT / "target" / "debug" / "neura").resolve())


def subtext_sidecar_target() -> tuple[str, str]:
    """Resolve the local Neura OSS model endpoint + model name for the thought
    observer. Mirrors the Rust sidecar env resolution (config.toml defaults)."""
    base = (
        os.environ.get("NEURA_SIDECAR_URL")
        or os.environ.get("NEURA_LOCAL_MODEL_BASE_URL")
        or "http://127.0.0.1:11434/v1"
    ).rstrip("/")
    model = (
        os.environ.get("NEURA_SIDECAR_MODEL")
        or os.environ.get("NEURA_LOCAL_MODEL")
        or "neura-sidecar-20b"
    )
    return base, model


def prewarm_subtext_sidecar() -> None:
    """Best-effort: load the local observer model at startup so the first turn's
    thought streams immediately instead of eating a ~30s cold model load (which
    otherwise finishes after the answer and shows nothing live)."""
    cfg = build_subtext_config()
    if not cfg.get("enabled"):
        return

    def _warm() -> None:
        base, model = subtext_sidecar_target()
        # Use Ollama's NATIVE endpoint — the OpenAI-compat /v1 route ignores
        # keep_alive, so only /api/chat can pin the model resident for 30m.
        native = base[:-3] + "/api/chat" if base.endswith("/v1") else base + "/api/chat"
        payload = {
            "model": model,
            "messages": [{"role": "user", "content": "ok"}],
            "stream": False,
            "keep_alive": "30m",
            "options": {"num_predict": 1},
        }
        try:
            req = urllib.request.Request(
                native,
                data=json.dumps(payload).encode(),
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urllib.request.urlopen(req, timeout=180) as resp:
                resp.read()
            sys.stderr.write("SUBTEXT_SIDECAR_WARM=1\n")
            sys.stderr.flush()
        except Exception as exc:  # noqa: BLE001 — warmup is best-effort
            sys.stderr.write(f"SUBTEXT_SIDECAR_WARM=0 ({exc})\n")
            sys.stderr.flush()

    threading.Thread(target=_warm, name="subtext-prewarm", daemon=True).start()


def build_subtext_config() -> dict:
    """Advertise the live thought-observer to the web UI.

    The default observer streams terse "what is being thought about" narration
    from the local Neura OSS model (Ollama) via /api/subtext-stream (SSE). The
    browser no longer talks to any Qwen/Jacobian websocket.
    """
    base, model = subtext_sidecar_target()
    enabled = os.environ.get("NEURA_SUBTEXT_ENABLED", "1").strip().lower() not in (
        "0",
        "false",
        "off",
        "no",
    )
    return {"mode": "stream", "endpoint": "/api/subtext-stream", "model": model, "base_url": base, "enabled": enabled}


def server_identity() -> dict:
    """Stable per-install server name (the modifier word), persisted under NEURA_HOME."""
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


def default_chat_title(animal: str) -> str:
    return f"{SERVER_NAME} {animal}"


def truncate_title_text(text: str, max_chars: int = TITLE_MAX_CHARS) -> str:
    trimmed = " ".join((text or "").strip().split())
    if not trimmed:
        return "New chat"
    if len(trimmed) <= max_chars:
        return trimmed
    return trimmed[: max_chars - 1].rstrip() + "…"


def normalize_project_path(path: str) -> str:
    return os.path.realpath(os.path.expanduser((path or "").strip()))


def browse_filesystem(path: str | None = None) -> dict:
    if path and path.strip():
        target = Path(normalize_project_path(path))
        if not target.exists():
            raise ValueError(f"path does not exist: {path}")
        if not target.is_dir():
            target = target.parent
    else:
        target = Path.home()

    target = target.resolve()
    parent_path = str(target.parent) if target != target.parent else None

    entries: list[dict] = []
    try:
        for item in sorted(target.iterdir(), key=lambda p: p.name.lower()):
            if item.name.startswith("."):
                continue
            try:
                if item.is_dir():
                    entries.append(
                        {
                            "name": item.name,
                            "path": str(item.resolve()),
                            "kind": "dir",
                        }
                    )
            except (PermissionError, OSError):
                continue
    except PermissionError as exc:
        raise ValueError(f"permission denied: {target}") from exc

    return {"path": str(target), "parent": parent_path, "entries": entries}


def project_id_for_path(path: str) -> str:
    normalized = normalize_project_path(path)
    digest = hashlib.sha256(normalized.encode()).hexdigest()[:16]
    return f"proj_{digest}"


def project_display_name(path: str, custom_name: str | None = None) -> str:
    if custom_name and custom_name.strip():
        return custom_name.strip()
    base = os.path.basename(normalize_project_path(path))
    return base or normalize_project_path(path)


def load_projects_meta() -> dict:
    with UI_META_LOCK:
        try:
            if UI_PROJECTS_FILE.exists():
                return json.loads(UI_PROJECTS_FILE.read_text())
        except Exception:
            pass
        return {"projects": []}


def save_projects_meta(data: dict) -> None:
    UI_META_DIR.mkdir(parents=True, exist_ok=True)
    with UI_META_LOCK:
        UI_PROJECTS_FILE.write_text(json.dumps(data, indent=2) + "\n")


def session_working_dir(data: dict | None) -> str | None:
    if not data:
        return None
    wd = data.get("working_dir")
    if not isinstance(wd, str) or not wd.strip():
        return None
    try:
        return normalize_project_path(wd)
    except Exception:
        return None


def list_projects() -> list[dict]:
    pinned_by_path: dict[str, dict] = {}
    for entry in load_projects_meta().get("projects", []):
        if not isinstance(entry, dict):
            continue
        path = entry.get("path")
        if not isinstance(path, str) or not path.strip():
            continue
        try:
            normalized = normalize_project_path(path)
        except Exception:
            continue
        pinned_by_path[normalized] = {
            "id": str(entry.get("id") or project_id_for_path(normalized)),
            "path": normalized,
            "name": project_display_name(normalized, entry.get("name")),
            "createdAt": entry.get("createdAt"),
        }

    paths = set(pinned_by_path.keys())
    chat_counts: dict[str, int] = {path: 0 for path in paths}
    if SESSIONS_DIR.exists():
        for path in SESSIONS_DIR.glob("session_*.json"):
            sid = path.stem
            try:
                data = json.loads(path.read_text())
            except Exception:
                continue
            msgs = session_messages(sid, data)
            if not msgs or is_title_generation_session_messages(msgs):
                continue
            wd = session_working_dir(data)
            if not wd:
                continue
            paths.add(wd)
            chat_counts[wd] = chat_counts.get(wd, 0) + 1

    out: list[dict] = []
    for path in sorted(paths, key=lambda p: (project_display_name(p).lower(), p)):
        pinned = pinned_by_path.get(path)
        out.append(
            {
                "id": pinned["id"] if pinned else project_id_for_path(path),
                "path": path,
                "name": pinned["name"] if pinned else project_display_name(path),
                "chatCount": chat_counts.get(path, 0),
                "pinned": pinned is not None,
            }
        )
    return out


def default_workspace_path() -> str:
    DEFAULT_WORKSPACE_DIR.mkdir(parents=True, exist_ok=True)
    return normalize_project_path(str(DEFAULT_WORKSPACE_DIR))


def slugify_project_name(name: str) -> str:
    slug = re.sub(r"[^a-zA-Z0-9]+", "-", name.strip().lower()).strip("-")
    return slug or "project"


def ensure_projects_dir() -> Path:
    PROJECTS_DIR.mkdir(parents=True, exist_ok=True)
    return PROJECTS_DIR


def is_managed_project_path(path: str) -> bool:
    try:
        normalized = normalize_project_path(path)
        root = normalize_project_path(str(ensure_projects_dir()))
        return normalized == root or normalized.startswith(root + os.sep)
    except Exception:
        return False


def suggest_project_path(name: str | None = None) -> dict:
    root = ensure_projects_dir()
    base = slugify_project_name(name or "") if name and name.strip() else "project"
    candidate = root / base
    suffix = 1
    while candidate.exists():
        candidate = root / f"{base}-{suffix}"
        suffix += 1
    return {
        "path": normalize_project_path(str(candidate)),
        "projectsDir": normalize_project_path(str(root)),
    }


def ensure_default_project() -> dict:
    return add_project(default_workspace_path(), "Workspace")


def delete_project(project_path: str) -> dict:
    normalized = normalize_project_path(project_path)

    meta = load_projects_meta()
    projects = meta.get("projects", [])
    kept: list[dict] = []
    removed_pinned = False
    for entry in projects:
        if not isinstance(entry, dict):
            continue
        try:
            if normalize_project_path(str(entry.get("path") or "")) == normalized:
                removed_pinned = True
                continue
        except Exception:
            pass
        kept.append(entry)
    if removed_pinned:
        meta["projects"] = kept
        save_projects_meta(meta)

    deleted_sessions: list[str] = []
    if SESSIONS_DIR.exists():
        for path in SESSIONS_DIR.glob("session_*.json"):
            sid = path.stem
            try:
                data = json.loads(path.read_text())
            except Exception:
                continue
            if session_working_dir(data) != normalized:
                continue
            delete_session_files(sid)
            deleted_sessions.append(sid)

    if deleted_sessions:
        chat_meta = load_ui_chat_meta()
        chats = chat_meta.get("chats", {})
        for sid in deleted_sessions:
            chats.pop(sid, None)
        chat_meta["chats"] = chats
        save_ui_chat_meta(chat_meta)

    neura_dot = Path(normalized) / ".neura"
    if neura_dot.is_dir():
        try:
            shutil.rmtree(neura_dot)
        except Exception:
            pass

    publish_event(
        "projects_changed",
        reason="project_deleted",
        path=normalized,
        chats=len(deleted_sessions),
    )
    return {
        "ok": True,
        "path": normalized,
        "deletedChats": len(deleted_sessions),
        "removedFromRegistry": removed_pinned,
    }


def add_project(path: str, name: str | None = None) -> dict:
    normalized = normalize_project_path(path)
    if not os.path.isdir(normalized):
        if is_managed_project_path(normalized):
            Path(normalized).mkdir(parents=True, exist_ok=True)
        if not os.path.isdir(normalized):
            raise ValueError(f"not a directory: {normalized}")

    meta = load_projects_meta()
    projects = meta.setdefault("projects", [])
    for entry in projects:
        if not isinstance(entry, dict):
            continue
        try:
            if normalize_project_path(str(entry.get("path") or "")) == normalized:
                if name and name.strip():
                    entry["name"] = name.strip()
                    save_projects_meta(meta)
                return {
                    "id": str(entry.get("id") or project_id_for_path(normalized)),
                    "path": normalized,
                    "name": project_display_name(normalized, entry.get("name")),
                    "chatCount": 0,
                    "pinned": True,
                }
        except Exception:
            continue

    created = {
        "id": project_id_for_path(normalized),
        "path": normalized,
        "name": project_display_name(normalized, name),
        "createdAt": time.time(),
    }
    projects.append(created)
    save_projects_meta(meta)
    publish_event("projects_changed")
    return {
        "id": created["id"],
        "path": normalized,
        "name": created["name"],
        "chatCount": 0,
        "pinned": True,
    }


def resolve_project_path(project_id: str | None, project_path: str | None) -> str | None:
    if project_path:
        try:
            return normalize_project_path(project_path)
        except Exception:
            return None
    if not project_id:
        return None
    for project in list_projects():
        if project.get("id") == project_id:
            return str(project.get("path"))
    return None


def load_ui_chat_meta() -> dict:
    with UI_META_LOCK:
        try:
            if UI_CHAT_META_FILE.exists():
                return json.loads(UI_CHAT_META_FILE.read_text())
        except Exception:
            pass
        return {"chats": {}}


def save_ui_chat_meta(data: dict) -> None:
    UI_META_DIR.mkdir(parents=True, exist_ok=True)
    with UI_META_LOCK:
        UI_CHAT_META_FILE.write_text(json.dumps(data, indent=2) + "\n")


def read_session_snapshot(session_id: str) -> dict | None:
    path = SESSIONS_DIR / f"{session_id}.json"
    if not path.exists():
        return None
    try:
        return json.loads(path.read_text())
    except Exception:
        return None


def resolve_chat_title(session_id: str, data: dict | None = None) -> tuple[str, bool, str]:
    """Return (title, locked, source). source is user|auto|heuristic|session|default."""
    meta = load_ui_chat_meta().get("chats", {}).get(session_id, {})
    if isinstance(meta, dict) and meta.get("title"):
        return (
            str(meta["title"]),
            bool(meta.get("locked")),
            str(meta.get("source") or ("user" if meta.get("locked") else "auto")),
        )
    if data is None:
        data = read_session_snapshot(session_id)
    if data and data.get("title"):
        return str(data["title"]), False, "session"
    animal = (data or {}).get("short_name") or session_animal(session_id)
    return default_chat_title(animal), False, "default"


def patch_session_snapshot_title(session_id: str, title: str) -> None:
    path = SESSIONS_DIR / f"{session_id}.json"
    if not path.exists():
        return
    try:
        data = json.loads(path.read_text())
        data["title"] = title
        path.write_text(json.dumps(data) + "\n")
    except Exception:
        pass


def set_chat_title(
    session_id: str,
    title: str,
    *,
    locked: bool,
    source: str,
) -> str:
    clean = truncate_title_text(title)
    meta = load_ui_chat_meta()
    chats = meta.setdefault("chats", {})
    chats[session_id] = {
        "title": clean,
        "locked": locked,
        "source": source,
        "updatedAt": time.time(),
    }
    save_ui_chat_meta(meta)
    patch_session_snapshot_title(session_id, clean)
    publish_event("chat_title_updated", session_id=session_id, title=clean, locked=locked)
    return clean


def conversation_transcript(messages: list[dict], *, max_chars: int = 4500) -> str:
    lines: list[str] = []
    used = 0
    for msg in messages:
        role = msg.get("role", "")
        if role not in ("user", "assistant"):
            continue
        text = (msg.get("text") or "").strip()
        if not text:
            continue
        label = "User" if role == "user" else "Assistant"
        chunk = f"{label}: {text}"
        if used + len(chunk) + 1 > max_chars:
            remaining = max_chars - used - 20
            if remaining > 40:
                lines.append(chunk[:remaining] + "…")
            break
        lines.append(chunk)
        used += len(chunk) + 1
    return "\n\n".join(lines)


def heuristic_chat_title(messages: list[dict]) -> str:
    user_texts = [
        (m.get("text") or "").strip()
        for m in messages
        if m.get("role") == "user" and (m.get("text") or "").strip()
    ]
    if not user_texts:
        return "New chat"
    if len(user_texts) == 1:
        first = user_texts[0].splitlines()[0].strip()
        return truncate_title_text(first)
    # Blend the first few user turns into a short topic label.
    joined = " · ".join(t.splitlines()[0].strip() for t in user_texts[:3])
    return truncate_title_text(joined)


def clean_generated_title(text: str) -> str:
    title = (text or "").strip()
    if not title:
        return ""
    title = title.splitlines()[0].strip()
    for prefix in ("Title:", "title:", "Chat title:", "Topic:"):
        if title.startswith(prefix):
            title = title[len(prefix) :].strip()
    if (title.startswith('"') and title.endswith('"')) or (
        title.startswith("'") and title.endswith("'")
    ):
        title = title[1:-1].strip()
    title = title.strip(" .")
    return truncate_title_text(title)


def generate_chat_title_with_neura(messages: list[dict]) -> str | None:
    transcript = conversation_transcript(messages)
    if not transcript:
        return None
    prompt = (
        f"{TITLE_GEN_MARKER}\n"
        "Read the full transcript below and reply with ONLY a short title (3-8 words) "
        "that captures what the whole conversation is about.\n"
        "No quotes, no punctuation at the end, no explanation.\n\n"
        f"{transcript}"
    )
    cmd = [neura_bin(), "run", "--json", "--no-update", "--no-selfdev", "--quiet"]
    if NEURA_TITLE_MODEL:
        cmd.extend(["-m", NEURA_TITLE_MODEL])
    cmd.extend(["--", prompt])
    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=NEURA_TITLE_TIMEOUT,
            cwd=str(Path.home()),
            # Background utility turn: never fire the cognition-state probe here
            # (it would pollute the signal + multiply concurrent /fuse load).
            env={**os.environ, "NEURA_COGNITION_TRIGGERS": "0"},
        )
    except subprocess.TimeoutExpired:
        return None
    except Exception:
        return None
    if proc.returncode != 0:
        return None
    raw = (proc.stdout or "").strip()
    report = None
    for start in (raw.find("{"), 0):
        if start < 0:
            continue
        try:
            report = json.loads(raw[start:])
            break
        except Exception:
            continue
    if not report:
        return None
    throwaway_session_id = report.get("session_id")
    if isinstance(throwaway_session_id, str) and throwaway_session_id:
        delete_session_files(throwaway_session_id)
    title = clean_generated_title(str(report.get("text") or ""))
    return title or None


def refresh_chat_title(session_id: str) -> str | None:
    with TITLE_REFRESH_LOCK:
        if session_id in TITLE_REFRESH_IN_FLIGHT:
            return None
        TITLE_REFRESH_IN_FLIGHT.add(session_id)
    try:
        data = read_session_snapshot(session_id)
        if data is None:
            return None
        _, locked, _ = resolve_chat_title(session_id, data)
        if locked:
            return None
        chat = load_chat(session_id)
        if chat is None:
            return None
        messages = chat.get("messages") or []
        if not messages:
            return None
        title = generate_chat_title_with_neura(messages) or heuristic_chat_title(messages)
        if not title:
            return None
        current, _, _ = resolve_chat_title(session_id, data)
        if title == current:
            return title
        return set_chat_title(session_id, title, locked=False, source="auto")
    finally:
        with TITLE_REFRESH_LOCK:
            TITLE_REFRESH_IN_FLIGHT.discard(session_id)


def schedule_chat_title_refresh(session_id: str | None) -> None:
    if not session_id:
        return

    def worker() -> None:
        try:
            refresh_chat_title(session_id)
        except Exception:
            pass

    threading.Thread(target=worker, name=f"neura-title-{session_id[:18]}", daemon=True).start()


def read_current_dapp_files(project_path: str) -> dict[str, str]:
    root = ensure_dapp(project_path)
    files: dict[str, str] = {}
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        if path.suffix.lower() not in (".html", ".css", ".js"):
            continue
        rel = path.relative_to(root).as_posix()
        files[rel] = path.read_text(errors="replace")[:6000]
    return files


def dapp_forks_root(project_path: str) -> Path:
    return Path(normalize_project_path(project_path)) / ".neura" / "dapp-forks"


def session_fork_dir(project_path: str, session_id: str) -> Path:
    safe_sid = session_id.replace("/", "_").replace("\\", "_")
    return dapp_forks_root(project_path) / safe_sid


def active_session_file(project_path: str) -> Path:
    return Path(normalize_project_path(project_path)) / ".neura" / "dapp-active-session.json"


def read_active_session_id(project_path: str) -> str | None:
    path = active_session_file(project_path)
    if not path.is_file():
        return None
    try:
        data = json.loads(path.read_text())
    except Exception:
        return None
    sid = data.get("sessionId")
    return sid if isinstance(sid, str) and sid else None


def set_active_session_id(project_path: str, session_id: str) -> None:
    path = active_session_file(project_path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"sessionId": session_id, "updatedAt": time.time()}))


def clone_dapp_directory(src: Path, dest: Path) -> None:
    dest.mkdir(parents=True, exist_ok=True)
    for item in src.iterdir():
        if not item.is_file() or item.name == "meta.json":
            continue
        shutil.copy2(item, dest / item.name)


def sync_fork_to_active(project_path: str, session_id: str) -> None:
    fork = session_fork_dir(project_path, session_id)
    active = ensure_dapp(project_path)
    if not fork.is_dir():
        return
    for item in fork.iterdir():
        if item.is_file() and item.name != "meta.json":
            shutil.copy2(item, active / item.name)


def reset_active_dapp_to_shell(project_path: str, *, topic_label: str | None = None) -> None:
    """Ensure the live project dapp preview is the default AI shell (not another chat's fork)."""
    root = ensure_dapp(project_path)
    files = de.apply_ai_shell_wrapper(dict(de.NEURA_AI_SHELL_FILES), topic_label=topic_label)
    for name, content in files.items():
        (root / name).write_text(content)


def read_fork_files(project_path: str, session_id: str) -> dict[str, str]:
    fork = session_fork_dir(project_path, session_id)
    files: dict[str, str] = {}
    if not fork.is_dir():
        return files
    for path in sorted(fork.rglob("*")):
        if not path.is_file() or path.name == "meta.json":
            continue
        if path.suffix.lower() not in (".html", ".css", ".js"):
            continue
        rel = path.relative_to(fork).as_posix()
        files[rel] = path.read_text(errors="replace")[:6000]
    return files


def fork_has_custom_content(project_path: str, session_id: str) -> bool:
    fork = session_fork_dir(project_path, session_id)
    meta_path = fork / "meta.json"
    if meta_path.is_file():
        try:
            meta = json.loads(meta_path.read_text())
            if meta.get("custom"):
                return True
        except Exception:
            pass
    index = fork / "index.html"
    if not index.is_file():
        return False
    default = DAPP_DEFAULT_FILES.get("index.html", "")
    return index.read_text(errors="replace").strip() != default.strip()


def write_fork_meta(
    project_path: str,
    session_id: str,
    *,
    custom: bool = True,
    template_id: str | None = None,
    source: str = "generated",
    theme_id: str | None = None,
) -> None:
    fork = session_fork_dir(project_path, session_id)
    fork.mkdir(parents=True, exist_ok=True)
    meta_path = fork / "meta.json"
    meta: dict = {}
    if meta_path.is_file():
        try:
            meta = json.loads(meta_path.read_text())
        except Exception:
            meta = {}
    meta.update({
        "custom": custom,
        "sessionId": session_id,
        "source": source,
        "templateId": template_id,
        "updatedAt": time.time(),
    })
    if theme_id:
        meta["themeId"] = theme_id
    (fork / "meta.json").write_text(json.dumps(meta, indent=2))


def read_fork_theme_id(project_path: str, session_id: str) -> str:
    fork = session_fork_dir(project_path, session_id)
    meta_path = fork / "meta.json"
    if meta_path.is_file():
        try:
            theme_id = json.loads(meta_path.read_text()).get("themeId")
            if theme_id:
                return str(theme_id)
        except Exception:
            pass
    return "brutalist-bw"


def ensure_dapp_library_dirs() -> None:
    DAPP_LIBRARY_DIR.mkdir(parents=True, exist_ok=True)
    DAPP_LIBRARY_TEMPLATES.mkdir(parents=True, exist_ok=True)
    if not DAPP_LIBRARY_INDEX.is_file():
        DAPP_LIBRARY_INDEX.write_text(json.dumps({"templates": []}, indent=2))


def ensure_dapp_library() -> None:
    ensure_dapp_library_dirs()
    ensure_ai_library_template()


def ensure_ai_library_template() -> None:
    """Register the default NEURA AI composable shell in the global library (pinned)."""
    tpl_id = de.NEURA_AI_SHELL_ID
    tpl_dir = DAPP_LIBRARY_TEMPLATES / tpl_id
    tpl_dir.mkdir(parents=True, exist_ok=True)
    for name, content in de.NEURA_AI_SHELL_FILES.items():
        path = tpl_dir / name
        if not path.is_file():
            path.write_text(content)
    widget_path = tpl_dir / "neura-widget.js"
    if not widget_path.is_file():
        widget_path.write_text(de.NEURA_WIDGET_JS)
    index = load_dapp_library_index()
    templates: list[dict] = index.setdefault("templates", [])
    existing = next((t for t in templates if t.get("id") == tpl_id), None)
    keywords = ["ai", "chat", "shell", "default", "neura", "workspace"]
    now = time.time()
    entry = {
        "id": tpl_id,
        "slug": "neura-ai",
        "title": "NEURA AI Shell",
        "keywords": keywords,
        "category": "shell",
        "pinned": True,
        "useCount": int(existing.get("useCount") or 0) if existing else 0,
        "createdAt": float(existing.get("createdAt") or now) if existing else now,
        "updatedAt": now,
        "sourceSessionId": None,
        "sourceProjectPath": None,
    }
    entry = de.enrich_library_entry(entry, keywords)
    if existing:
        use_count = int(existing.get("useCount") or 0)
        existing.clear()
        existing.update(entry)
        existing["useCount"] = use_count
    else:
        templates.insert(0, entry)
    save_dapp_library_index(index)
    pin_library_template(tpl_id)


def ensure_dapp_themes() -> None:
    DAPP_THEMES_DIR.mkdir(parents=True, exist_ok=True)
    if not DAPP_THEMES_INDEX.is_file():
        DAPP_THEMES_INDEX.write_text(json.dumps({"themes": de.DEFAULT_DAPP_THEMES}, indent=2))
        return
    try:
        data = json.loads(DAPP_THEMES_INDEX.read_text())
    except Exception:
        data = {"themes": []}
    themes = data.get("themes") or []
    known = {str(item.get("id")) for item in themes if item.get("id")}
    changed = False
    for theme in de.DEFAULT_DAPP_THEMES:
        if theme["id"] not in known:
            themes.append(theme)
            changed = True
    if changed:
        data["themes"] = themes
        DAPP_THEMES_INDEX.write_text(json.dumps(data, indent=2))


def load_dapp_themes_index() -> dict:
    ensure_dapp_themes()
    try:
        return json.loads(DAPP_THEMES_INDEX.read_text())
    except Exception:
        return {"themes": de.DEFAULT_DAPP_THEMES}


def list_dapp_theme_entries() -> list[dict]:
    index = load_dapp_themes_index()
    entries = []
    for theme in index.get("themes", []):
        entries.append({
            **theme,
            "hasVars": bool(theme.get("vars")),
        })
    entries.sort(key=lambda item: (-int(item.get("pinned") or 0), str(item.get("title") or "")))
    return entries


def get_dapp_theme_entry(theme_id: str) -> dict | None:
    for theme in list_dapp_theme_entries():
        if theme.get("id") == theme_id:
            return theme
    return None


def apply_dapp_theme(project_path: str, session_id: str, theme_id: str) -> bool:
    theme = get_dapp_theme_entry(theme_id) or de.theme_by_id(theme_id)
    if not theme:
        return False
    normalized = normalize_project_path(project_path)
    root = ensure_dapp(normalized)
    index_path = root / "index.html"
    if index_path.is_file():
        html = index_path.read_text(errors="replace")
        if 'data-neura-theme="' in html:
            html = re.sub(r'data-neura-theme="[^"]*"', f'data-neura-theme="{theme_id}"', html, count=1)
        elif "<body" in html.lower():
            html = de._replace_body_theme(html, theme_id)
        index_path.write_text(html)
    fork = session_fork_dir(normalized, session_id)
    fork.mkdir(parents=True, exist_ok=True)
    fork_index = fork / "index.html"
    if fork_index.is_file():
        html = fork_index.read_text(errors="replace")
        if 'data-neura-theme="' in html:
            html = re.sub(r'data-neura-theme="[^"]*"', f'data-neura-theme="{theme_id}"', html, count=1)
        fork_index.write_text(html)
    elif index_path.is_file():
        shutil.copy2(index_path, fork_index)
    write_fork_meta(normalized, session_id, custom=True, source="theme", theme_id=theme_id)
    publish_event("dapp_changed", project_path=normalized, themeId=theme_id)
    return True


def load_dapp_library_index() -> dict:
    ensure_dapp_library_dirs()
    try:
        data = json.loads(DAPP_LIBRARY_INDEX.read_text())
    except Exception:
        data = {"templates": []}
    if not isinstance(data.get("templates"), list):
        data["templates"] = []
    return data


def save_dapp_library_index(index: dict) -> None:
    ensure_dapp_library_dirs()
    DAPP_LIBRARY_INDEX.write_text(json.dumps(index, indent=2))


def unpin_library_template(template_id: str) -> bool:
    if not template_id:
        return False
    pinned = load_dapp_pins()
    if template_id not in pinned:
        return False
    pinned.discard(template_id)
    save_dapp_pins(pinned)
    index = load_dapp_library_index()
    for entry in index.get("templates", []):
        if entry.get("id") == template_id:
            entry["pinned"] = False
            break
    save_dapp_library_index(index)
    return True


def list_dapp_library_entries() -> list[dict]:
    index = load_dapp_library_index()
    pinned = load_dapp_pins()
    entries = []
    for template in index.get("templates", []):
        tpl_id = str(template.get("id") or "")
        tpl_dir = DAPP_LIBRARY_TEMPLATES / tpl_id
        entries.append(
            {
                **template,
                "pinned": tpl_id in pinned or bool(template.get("pinned")),
                "hasFiles": tpl_dir.is_dir(),
            }
        )
    entries.sort(key=lambda item: (-int(item.get("useCount") or 0), str(item.get("title") or "")))
    return entries


def get_dapp_library_entry(template_id: str) -> dict | None:
    index = load_dapp_library_index()
    for template in index.get("templates", []):
        if template.get("id") == template_id:
            tpl_dir = DAPP_LIBRARY_TEMPLATES / template_id
            files: list[dict] = []
            if tpl_dir.is_dir():
                for path in sorted(tpl_dir.rglob("*")):
                    if path.is_file() and path.name != "meta.json":
                        rel = path.relative_to(tpl_dir).as_posix()
                        files.append({"path": rel, "size": path.stat().st_size})
            return {
                **template,
                "pinned": template_id in load_dapp_pins() or bool(template.get("pinned")),
                "files": files,
            }
    return None


def delete_dapp_library_entry(template_id: str) -> bool:
    index = load_dapp_library_index()
    templates = index.get("templates", [])
    kept = [t for t in templates if t.get("id") != template_id]
    if len(kept) == len(templates):
        return False
    index["templates"] = kept
    save_dapp_library_index(index)
    unpin_library_template(template_id)
    tpl_dir = DAPP_LIBRARY_TEMPLATES / template_id
    if tpl_dir.is_dir():
        shutil.rmtree(tpl_dir, ignore_errors=True)
    return True


def sync_dapp_prompt_overlay(project_path: str, files: dict[str, str], chat_title: str | None = None) -> None:
    normalized = normalize_project_path(project_path)
    keywords = extract_dapp_keywords(
        [{"role": "user", "text": chat_title or "project dapp"}],
        chat_title,
    )
    category = detect_dapp_category(keywords)
    summary = de.summarize_dapp_files(files, title=chat_title, category=category)
    overlay = de.build_prompt_overlay(summary, category)
    overlay_path = Path(normalized) / ".neura" / "prompt-overlay.md"
    overlay_path.parent.mkdir(parents=True, exist_ok=True)
    overlay_path.write_text(overlay)


def list_dapp_history(project_path: str, session_id: str) -> list[dict]:
    fork = session_fork_dir(project_path, session_id)
    return de.list_fork_snapshots(fork)


def undo_dapp_snapshot(project_path: str, session_id: str, turn: int) -> bool:
    normalized = normalize_project_path(project_path)
    fork = session_fork_dir(normalized, session_id)
    if not de.restore_fork_snapshot(fork, turn):
        return False
    sync_fork_to_active(normalized, session_id)
    files = read_fork_files(normalized, session_id)
    chat = load_chat(session_id)
    sync_dapp_prompt_overlay(normalized, files, chat.get("title") if chat else None)
    publish_event("dapp_changed", project_path=normalized, session_id=session_id, undone=turn)
    return True


def get_dapp_turn_diff(project_path: str, session_id: str, turn: int) -> dict:
    normalized = normalize_project_path(project_path)
    fork = session_fork_dir(normalized, session_id)
    snap_dir = fork / "snapshots" / f"turn_{turn:04d}"
    before: dict[str, str] = {}
    if snap_dir.is_dir():
        for path in snap_dir.iterdir():
            if path.is_file() and path.name not in ("snapshot.json",):
                before[path.name] = path.read_text(errors="replace")
    after = read_fork_files(normalized, session_id)
    return de.compute_dapp_diff(before, after)


def build_chat_dapp_context(project_path: str, session_id: str | None) -> str:
    normalized = normalize_project_path(project_path)
    files: dict[str, str] = {}
    template_title = None
    if session_id and fork_has_custom_content(normalized, session_id):
        files = read_fork_files(normalized, session_id)
        meta_path = session_fork_dir(normalized, session_id) / "meta.json"
        if meta_path.is_file():
            try:
                meta = json.loads(meta_path.read_text())
                tpl_id = meta.get("templateId")
                if tpl_id:
                    entry = get_dapp_library_entry(str(tpl_id))
                    if entry:
                        template_title = str(entry.get("title") or "")
            except Exception:
                pass
    if not files:
        files = read_current_dapp_files(normalized)
    if not files or not any(v.strip() for v in files.values()):
        return ""
    keywords = extract_dapp_keywords([], template_title)
    category = detect_dapp_category(keywords) if keywords else None
    return de.build_dapp_chat_context(files, template_title=template_title, category=category)


def user_wants_rich_visual_dapp(text: str) -> bool:
    lowered = (text or "").lower()
    return any(
        hint in lowered
        for hint in (
            "gif", "gifs", "meme", "embed", "gallery", "show me", "show ",
            "build ", "make ", "create ", "dashboard", "interface", "preview",
        )
    )


def classify_dapp_route(
    messages: list[dict],
    *,
    fork_exists: bool,
    wants_edit: bool,
    chat_title: str | None,
) -> str:
    """Stage A: components | library | incremental | full | skip (no extra LLM)."""
    if not conversation_wants_dapp_refresh(messages, fork_exists=fork_exists, wants_edit=wants_edit):
        return "skip"
    last_user = last_user_message_text(messages)
    if not wants_edit and message_has_visual_intent(last_user) and user_wants_rich_visual_dapp(last_user):
        return "incremental" if fork_exists else "full"
    if not wants_edit:
        matched = dc.match_components(messages, chat_title=chat_title)
        if matched and not (fork_exists and is_simple_arithmetic_message(last_user_message_text(messages))):
            return "components"
    template = find_library_template(messages, chat_title)
    if template and not fork_exists and not wants_edit:
        return "library"
    if wants_edit or (fork_exists and should_run_dapp_llm(messages, fork_exists=True, wants_edit=wants_edit, reused_library=False)):
        return "incremental" if fork_exists else "full"
    if not fork_exists:
        return "full"
    return "skip"


def load_dapp_pins() -> set[str]:
    ensure_dapp_library_dirs()
    if not DAPP_LIBRARY_PINS_FILE.is_file():
        return set()
    try:
        data = json.loads(DAPP_LIBRARY_PINS_FILE.read_text())
        ids = data.get("pinned") or []
        return {str(item) for item in ids if item}
    except Exception:
        return set()


def save_dapp_pins(pinned: set[str]) -> None:
    ensure_dapp_library_dirs()
    DAPP_LIBRARY_PINS_FILE.write_text(json.dumps({"pinned": sorted(pinned)}, indent=2))


def pin_library_template(template_id: str) -> bool:
    if not template_id:
        return False
    pinned = load_dapp_pins()
    pinned.add(template_id)
    save_dapp_pins(pinned)
    index = load_dapp_library_index()
    for entry in index.get("templates", []):
        if entry.get("id") == template_id:
            entry["pinned"] = True
            break
    save_dapp_library_index(index)
    return True


def extract_entity_tokens(text: str) -> list[str]:
    return dctx.extract_entity_tokens(text)


def detect_dapp_category(keywords: list[str]) -> str | None:
    return dctx.detect_category(keywords)


def dapp_live_data_hints(
    category: str | None,
    keywords: list[str],
    messages: list[dict] | None = None,
    *,
    chat_title: str | None = None,
) -> str:
    if messages:
        return dctx.live_data_hints(category, messages, chat_title=chat_title, keywords=keywords)
    return dctx.live_data_hints(category, [], chat_title=chat_title, keywords=keywords)


def expand_dapp_keywords(keywords: list[str]) -> list[str]:
    return dctx.expand_keywords(keywords)


def extract_dapp_keywords(
    messages: list[dict],
    chat_title: str | None = None,
    *,
    include_assistant: bool = False,
) -> list[str]:
    return dctx.keywords_from_messages(messages, chat_title, include_assistant=include_assistant)


def last_user_message_text(messages: list[dict]) -> str:
    for msg in reversed(messages):
        if msg.get("role") == "user":
            return (msg.get("text") or "").strip()
    return ""


def message_has_visual_intent(text: str) -> bool:
    lowered = (text or "").lower()
    if not lowered:
        return False
    if any(hint in lowered for hint in DAPP_EDIT_HINTS):
        return True
    return any(hint in lowered for hint in DAPP_VISUAL_HINTS)


def is_simple_arithmetic_message(text: str) -> bool:
    return de.parse_simple_arithmetic(text) is not None


def is_dapp_skip_message(text: str) -> bool:
    trimmed = (text or "").strip()
    if not trimmed:
        return True
    if is_simple_arithmetic_message(trimmed):
        return True
    lowered = trimmed.lower()
    normalized = re.sub(r"[^\w\s]", "", lowered).strip()
    if normalized in DAPP_SKIP_ACKS:
        return True
    if len(normalized) < 14 and not message_has_visual_intent(trimmed):
        return True
    if lowered.startswith(("thanks", "thank you", "ok ", "okay ", "cool ", "got it")):
        if len(lowered) < 48 and not message_has_visual_intent(trimmed):
            return True
    return False


def recent_conversation_messages(messages: list[dict], *, max_turns: int = 3) -> list[dict]:
    picked: list[dict] = []
    turns = 0
    for msg in reversed(messages):
        if msg.get("role") not in ("user", "assistant"):
            continue
        picked.append(msg)
        if msg.get("role") == "user":
            turns += 1
            if turns >= max_turns:
                break
    picked.reverse()
    return picked


def score_library_template(keywords: list[str], template: dict, *, pinned: set[str] | None = None) -> float:
    query_keys = set(expand_dapp_keywords(keywords))
    template_keys = set(expand_dapp_keywords(list(template.get("keywords") or [])))
    overlap = len(query_keys & template_keys)
    blob = " ".join(keywords).lower()
    slug = str(template.get("slug") or "").replace("-", " ").lower()
    title = str(template.get("title") or "").lower()
    if overlap == 0:
        if blob and (blob in title or any(word in slug for word in keywords)):
            overlap = 1
        else:
            fuzzy = max(
                difflib.SequenceMatcher(None, blob, title).ratio(),
                difflib.SequenceMatcher(None, blob, slug).ratio(),
            )
            if fuzzy < 0.55:
                return 0.0
            overlap = 1
    base = overlap / max(len(query_keys), len(template_keys), 1)
    use_count = int(template.get("useCount") or 0)
    boost = min(0.18, use_count * 0.025)
    tpl_id = str(template.get("id") or "")
    if pinned and tpl_id in pinned:
        boost += 0.12
    elif template.get("pinned"):
        boost += 0.12
    query_category = detect_dapp_category(keywords)
    template_category = template.get("category") or detect_dapp_category(list(template.get("keywords") or []))
    if query_category and query_category == template_category:
        boost += 0.1
    if use_count >= 5:
        boost += 0.05
    tfidf = de.score_template_tfidf(keywords, template)
    return min(1.0, base + boost + tfidf * 0.4)


def find_library_template(messages: list[dict], chat_title: str | None = None) -> dict | None:
    last_user = last_user_message_text(messages)
    if is_dapp_skip_message(last_user) or is_simple_arithmetic_message(last_user):
        return None
    keywords = extract_dapp_keywords(messages, chat_title, include_assistant=False)
    if not keywords:
        return None
    index = load_dapp_library_index()
    pinned = load_dapp_pins()
    best: dict | None = None
    best_score = 0.0
    query_category = detect_dapp_category(keywords)
    for template in index.get("templates", []):
        tpl_id = str(template.get("id") or "")
        if tpl_id == de.NEURA_AI_SHELL_ID:
            continue
        score = score_library_template(keywords, template, pinned=pinned)
        overlap = len(set(keywords) & set(template.get("keywords") or []))
        if overlap < NEURA_DAPP_MATCH_MIN_OVERLAP and score < 0.55:
            continue
        template_category = template.get("category") or detect_dapp_category(list(template.get("keywords") or []))
        if query_category and template_category and query_category != template_category and overlap < 3:
            continue
        if score > best_score:
            best_score = score
            best = template
    if best and best_score >= max(NEURA_DAPP_MATCH_THRESHOLD, 0.42):
        if int(best.get("useCount") or 0) >= 5 and str(best.get("id") or "") not in pinned:
            pin_library_template(str(best["id"]))
        return best
    return None


def user_text_likely_matches_library(user_text: str, chat_title: str | None = None) -> bool:
    messages = [{"role": "user", "text": user_text}]
    return find_library_template(messages, chat_title) is not None


def prefetch_dapp_from_user_text(
    project_path: str,
    session_id: str,
    user_text: str,
    *,
    chat_title: str | None = None,
) -> dict:
    normalized = normalize_project_path(project_path)
    if is_dapp_skip_message(user_text):
        return {"ok": False, "reason": "skip_message"}
    if fork_has_custom_content(normalized, session_id):
        return {"ok": False, "reason": "fork_exists"}
    template = find_library_template([{"role": "user", "text": user_text}], chat_title)
    if not template:
        return {"ok": False, "reason": "no_match"}
    if apply_library_template(normalized, session_id, template):
        publish_event("dapp_generation_done", project_path=normalized, session_id=session_id, reused=True)
        return {"ok": True, "reused": True, "templateId": template.get("id"), "templateTitle": template.get("title")}
    return {"ok": False, "reason": "apply_failed"}


def apply_library_template(project_path: str, session_id: str, template: dict) -> bool:
    tpl_id = str(template.get("id") or "")
    tpl_dir = DAPP_LIBRARY_TEMPLATES / tpl_id
    if not tpl_id or not tpl_dir.is_dir():
        return False
    normalized = normalize_project_path(project_path)
    fork = session_fork_dir(normalized, session_id)
    clone_dapp_directory(tpl_dir, fork)
    write_fork_meta(
        normalized,
        session_id,
        custom=True,
        template_id=tpl_id,
        source="library",
    )
    sync_fork_to_active(normalized, session_id)
    set_active_session_id(normalized, session_id)
    index = load_dapp_library_index()
    for entry in index.get("templates", []):
        if entry.get("id") == tpl_id:
            entry["useCount"] = int(entry.get("useCount") or 0) + 1
            entry["updatedAt"] = time.time()
            break
    save_dapp_library_index(index)
    files_after = read_fork_files(normalized, session_id)
    sync_dapp_prompt_overlay(normalized, files_after, str(template.get("title") or ""))
    publish_event(
        "dapp_changed",
        project_path=normalized,
        reused=True,
        templateId=tpl_id,
        templateTitle=template.get("title"),
    )
    return True


def upsert_library_template(
    files: dict[str, str],
    messages: list[dict],
    chat_title: str | None,
    *,
    source_session: str,
    source_project: str,
) -> str:
    keywords = extract_dapp_keywords(messages, chat_title)
    title = (chat_title or " ".join(keywords[:4]).title() or "Dapp").strip()
    category = detect_dapp_category(keywords)
    index = load_dapp_library_index()
    templates = index.setdefault("templates", [])

    best_existing: dict | None = None
    best_score = 0.0
    for template in templates:
        score = score_library_template(keywords, template)
        if score > best_score:
            best_score = score
            best_existing = template

    if best_existing and best_score >= 0.5:
        tpl_id = str(best_existing["id"])
        merged = sorted(set(best_existing.get("keywords") or []) | set(keywords))[:32]
        best_existing["keywords"] = merged
        best_existing["title"] = title
        best_existing["category"] = category or best_existing.get("category")
        best_existing["updatedAt"] = time.time()
        best_existing["sourceSessionId"] = source_session
        best_existing["sourceProjectPath"] = source_project
        best_existing = de.enrich_library_entry(best_existing, merged)
    else:
        slug_base = "-".join(keywords[:4]) or "dapp"
        slug = re.sub(r"[^a-z0-9-]+", "-", slug_base.lower()).strip("-")[:48] or "dapp"
        tpl_id = f"{slug}-{hashlib.sha256(title.encode()).hexdigest()[:8]}"
        entry = {
                "id": tpl_id,
                "slug": slug,
                "title": title,
                "keywords": keywords,
                "category": category,
                "createdAt": time.time(),
                "updatedAt": time.time(),
                "useCount": 0,
                "sourceSessionId": source_session,
                "sourceProjectPath": source_project,
            }
        templates.append(de.enrich_library_entry(entry, keywords))

    tpl_dir = DAPP_LIBRARY_TEMPLATES / tpl_id
    tpl_dir.mkdir(parents=True, exist_ok=True)
    for rel_path, content in files.items():
        if rel_path == "meta.json":
            continue
        target = tpl_dir / rel_path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content)
    (tpl_dir / "meta.json").write_text(
        json.dumps(
            {
                "id": tpl_id,
                "title": title,
                "keywords": keywords,
                "category": category,
                "updatedAt": time.time(),
            },
            indent=2,
        )
    )
    save_dapp_library_index(index)
    return tpl_id


def user_wants_dapp_edit(messages: list[dict]) -> bool:
    last_user = ""
    for msg in reversed(messages):
        if msg.get("role") == "user":
            last_user = (msg.get("text") or "").lower()
            break
    if not last_user:
        return False
    return any(hint in last_user for hint in DAPP_EDIT_HINTS)


def should_run_dapp_llm(
    messages: list[dict],
    *,
    fork_exists: bool,
    wants_edit: bool,
    reused_library: bool,
) -> bool:
    if reused_library:
        return False
    if wants_edit:
        return True
    last_user = last_user_message_text(messages)
    if is_dapp_skip_message(last_user) and fork_exists:
        return False
    if not fork_exists:
        if is_dapp_skip_message(last_user):
            return any(message_has_visual_intent(m.get("text") or "") for m in messages if m.get("role") == "user")
        return True
    if not message_has_visual_intent(last_user) and not wants_edit:
        return False
    user_turns = sum(1 for msg in messages if msg.get("role") == "user")
    return user_turns > 1


def conversation_wants_dapp_refresh(messages: list[dict], *, fork_exists: bool, wants_edit: bool) -> bool:
    if wants_edit:
        return True
    last_user = last_user_message_text(messages)
    if is_simple_arithmetic_message(last_user):
        return True
    if not is_dapp_skip_message(last_user):
        return True
    if not fork_exists:
        return any(message_has_visual_intent(m.get("text") or "") for m in messages if m.get("role") == "user")
    return False


def activate_session_dapp(project_path: str, session_id: str) -> dict:
    normalized = normalize_project_path(project_path)
    fork = session_fork_dir(normalized, session_id)
    chat = load_chat(session_id)
    chat_title = str(chat.get("title") or "") if chat else None
    if fork.is_dir() and fork_has_custom_content(normalized, session_id):
        sync_fork_to_active(normalized, session_id)
    else:
        template = find_library_template(chat.get("messages") or [], chat_title) if chat else None
        if template:
            apply_library_template(normalized, session_id, template)
        else:
            reset_active_dapp_to_shell(normalized, topic_label=chat_title)
    set_active_session_id(normalized, session_id)
    publish_event("dapp_changed", project_path=normalized)
    return {"ok": True, "sessionId": session_id, "projectPath": normalized}


def build_dapp_generation_prompt(
    messages: list[dict],
    current_files: dict[str, str],
    *,
    mode: str = "full",
) -> str:
    keywords = extract_dapp_keywords(messages, include_assistant=True)
    category = detect_dapp_category(keywords)
    live_hints = dapp_live_data_hints(category, keywords, messages)
    if mode == "incremental":
        scoped_messages = recent_conversation_messages(messages, max_turns=3)
        transcript = conversation_transcript(scoped_messages, max_chars=3500)
    else:
        transcript = conversation_transcript(messages, max_chars=8000)
    current_summary = ""
    if current_files:
        current_summary = (
            "Current chat dapp fork files (edit ONLY this fork; keep unrelated parts intact):\n"
        )
        for path, content in current_files.items():
            limit = 2500 if mode == "incremental" else 3500
            current_summary += f"\n--- {path} ---\n{content[:limit]}\n"

    if mode == "incremental":
        task = (
            "Update this chat's dapp fork based on the latest user request. "
            "Apply only relevant changes—especially explicit UI requests. "
            "Do not reset unrelated sections. Prefer minimal diffs."
        )
    else:
        task = (
            "Build the live HTML/CSS/JS dapp for this Neura chat project. "
            "Given the conversation, produce a rich contextual mini-web-app that shows "
            "the MOST RELEVANT visual interface for what the user discussed—not generic placeholders."
        )

    return (
        f"{DAPP_GEN_MARKER}\n"
        f"{task}\n\n"
        "Examples:\n"
        '- "what does the fox say" → song lyrics, YouTube embed/link, Spotify button, playful fox UI.\n'
        "- weather question → forecast cards for the location mentioned.\n"
        "- coding a todo app → interactive todo UI reflecting their requirements.\n\n"
        "Reply with ONLY valid JSON (no markdown fences):\n"
        '{"files": {"index.html": "...", "style.css": "...", "app.js": "..."}}\n\n'
        "Rules:\n"
        "- Use the NEURA AI composable shell: keep `<body class=\"neura-ai-shell\">`, "
        "`#neura-widget-grid`, `#neura-topic-panel`, `#neura-chat-widget`, `neura-bridge.js`, and `neura-widget.js`.\n"
        "- Put topic visuals inside `#neura-topic-panel` AND add quick dynamic widgets as siblings in `#neura-widget-grid`.\n"
        "- Each widget: `<article class=\"neura-widget\" data-neura-widget=\"card|panel|embed|action\" data-neura-id=\"unique-id\">`.\n"
        "- Use CSS variables (--neura-bg, --neura-fg, --neura-border, --neura-card-bg) for theme-aware styling.\n"
        "- Do NOT build chat UI — inline chat widget is host-driven via `neura-widget.js`.\n"
        "- Interactive buttons: `data-neura-action=\"ask|run|open\"` or `NEURA_BRIDGE.sendChat(text)` / `sendWidgetAction(id, action, payload)`.\n"
        "- Prefer premade component patterns: math hero, weather live, music card, finance ticker, action chips, assistant insight.\n"
        "- Vanilla HTML, CSS, and JS only (no build step). Public CDNs allowed (YouTube embeds, fonts).\n"
        "- Black & white brutalist Neura aesthetic: #000 background, #fff text, monospace, sharp edges, no border-radius.\n"
        "- index.html must run in an iframe sandbox with relative style.css and app.js links.\n"
        '- External links use target="_blank" rel="noopener noreferrer".\n'
        "- Visually \"overkill\" but useful: cards, embeds, buttons, subtle motion where relevant.\n"
        "- Keep files reasonably sized; quality over quantity.\n"
        "- For complex dashboards you may use React 18 via CDN (unpkg) + Babel standalone in index.html.\n\n"
        f"{live_hints}\n"
        f"{current_summary}\n"
        f"Conversation:\n{transcript}"
    )


def parse_dapp_generation_response(text: str) -> dict[str, str] | None:
    raw = (text or "").strip()
    if not raw:
        return None
    if "```" in raw:
        fence_start = raw.find("```")
        fence_end = raw.rfind("```")
        if fence_start >= 0 and fence_end > fence_start:
            inner = raw[fence_start + 3 : fence_end].strip()
            if inner.lower().startswith("json"):
                inner = inner[4:].strip()
            raw = inner or raw
    candidates = [raw]
    brace = raw.find("{")
    if brace > 0:
        candidates.append(raw[brace:])
    for candidate in candidates:
        try:
            data = json.loads(candidate)
        except Exception:
            continue
        if not isinstance(data, dict):
            continue
        files = data.get("files")
        if not isinstance(files, dict) or not files:
            continue
        out: dict[str, str] = {}
        for rel_path, content in files.items():
            if not isinstance(rel_path, str) or not isinstance(content, str):
                continue
            clean_path = rel_path.strip().replace("\\", "/").lstrip("/")
            if not clean_path or ".." in clean_path.split("/"):
                continue
            out[clean_path] = content
        if out:
            return out
    return None


def write_dapp_files(
    project_path: str,
    files: dict[str, str],
    *,
    session_id: str | None = None,
    template_id: str | None = None,
    source: str = "generated",
    chat_title: str | None = None,
    snapshot_turn: int | None = None,
    messages: list[dict] | None = None,
) -> int:
    normalized = normalize_project_path(project_path)
    keywords = extract_dapp_keywords([{"role": "user", "text": chat_title or ""}]) if chat_title else []
    category = detect_dapp_category(keywords) if keywords else None
    files = de.apply_ai_shell_wrapper(files, topic_label=chat_title)
    files = de.inject_live_bootstrap(
        files,
        category,
        keywords,
        messages=messages,
        chat_title=chat_title,
    )
    ok, err = de.validate_dapp_files(files)
    if not ok:
        return 0

    sid = session_id or read_active_session_id(normalized)
    before: dict[str, str] = {}
    if sid:
        before = read_fork_files(normalized, sid) or read_current_dapp_files(normalized)
        if before:
            fork = session_fork_dir(normalized, sid)
            fork.mkdir(parents=True, exist_ok=True)
            turn = snapshot_turn
            if turn is None:
                chat = load_chat(sid)
                turn = len(chat.get("messages") or []) if chat else 0
            de.snapshot_fork(
                fork,
                int(turn or 0),
                meta={"source": source, "templateId": template_id},
            )

    root = ensure_dapp(normalized)
    written = 0
    for rel_path, content in files.items():
        try:
            target = dapp_resolve_file(root, rel_path)
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(content)
            written += 1
        except Exception:
            continue

    if sid and written:
        fork = session_fork_dir(normalized, sid)
        fork.mkdir(parents=True, exist_ok=True)
        for rel_path, content in files.items():
            try:
                fork_target = dapp_resolve_file(fork, rel_path)
                fork_target.parent.mkdir(parents=True, exist_ok=True)
                fork_target.write_text(content)
            except Exception:
                continue
        write_fork_meta(
            normalized,
            sid,
            custom=True,
            template_id=template_id,
            source=source,
        )
        set_active_session_id(normalized, sid)
        after = read_fork_files(normalized, sid)
        diff = de.compute_dapp_diff(before, after)
        if diff.get("changedFiles"):
            publish_event(
                "dapp_diff",
                project_path=normalized,
                session_id=sid,
                summary=diff.get("summary"),
                changedFiles=diff.get("changedFiles"),
            )
        sync_dapp_prompt_overlay(normalized, after, chat_title)

    if written:
        publish_event("dapp_changed", project_path=normalized)
    return written


def generate_dapp_with_neura(
    project_path: str,
    messages: list[dict],
    *,
    session_id: str,
    current_files: dict[str, str],
    mode: str = "full",
    chat_title: str | None = None,
) -> bool:
    transcript = conversation_transcript(messages, max_chars=200)
    if not transcript:
        return False

    def attempt(extra_suffix: str = "") -> dict[str, str] | None:
        prompt = build_dapp_generation_prompt(messages, current_files, mode=mode) + extra_suffix
        cmd = [neura_bin(), "run", "--json", "--no-update", "--no-selfdev", "--quiet"]
        if mode == "incremental":
            model = NEURA_DAPP_INCREMENTAL_MODEL or NEURA_DAPP_MODEL or NEURA_TITLE_MODEL
        else:
            model = NEURA_DAPP_MODEL or NEURA_TITLE_MODEL
        if model:
            cmd.extend(["-m", model])
        cmd.extend(["--", prompt])
        timeout = NEURA_DAPP_INCREMENTAL_TIMEOUT if mode == "incremental" else NEURA_DAPP_TIMEOUT
        try:
            proc = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=timeout,
                cwd=normalize_project_path(project_path),
                # Background dapp-generation turn: no cognition probe (avoids
                # polluting the signal + piling concurrent /fuse captures).
                env={**os.environ, "NEURA_COGNITION_TRIGGERS": "0"},
            )
        except subprocess.TimeoutExpired:
            return None
        except Exception:
            return None
        if proc.returncode != 0:
            return None
        raw = (proc.stdout or "").strip()
        report = None
        for start in (raw.find("{"), 0):
            if start < 0:
                continue
            try:
                report = json.loads(raw[start:])
                break
            except Exception:
                continue
        if not report:
            return None
        throwaway_session_id = report.get("session_id")
        if isinstance(throwaway_session_id, str) and throwaway_session_id:
            delete_session_files(throwaway_session_id)
        files = parse_dapp_generation_response(str(report.get("text") or ""))
        if not files:
            return None
        ok, _err = de.validate_dapp_files(files)
        if not ok:
            return None
        return files

    files = attempt()
    if not files:
        files = attempt(
            "\n\nYour previous output was invalid. Reply with ONLY valid JSON: "
            '{"files": {"index.html": "...", "style.css": "...", "app.js": "..."}}'
        )
    if not files:
        return False
    if not write_dapp_files(
        project_path,
        files,
        session_id=session_id,
        source=mode,
        chat_title=chat_title,
        messages=messages,
    ):
        return False
    upsert_library_template(
        files,
        messages,
        chat_title,
        source_session=session_id,
        source_project=project_path,
    )
    return True


def refresh_project_dapp(session_id: str, project_path: str) -> bool:
    chat = load_chat(session_id)
    if chat is None:
        return False
    messages = chat.get("messages") or []
    if not any(m.get("role") == "user" for m in messages):
        return False
    if not any(m.get("role") == "assistant" for m in messages):
        return False

    normalized = normalize_project_path(project_path)
    chat_title = str(chat.get("title") or "")
    fork_exists = fork_has_custom_content(normalized, session_id)
    wants_edit = user_wants_dapp_edit(messages)
    if not conversation_wants_dapp_refresh(messages, fork_exists=fork_exists, wants_edit=wants_edit):
        return fork_exists
    template = find_library_template(messages, chat_title)
    reused_library = False
    route = classify_dapp_route(
        messages,
        fork_exists=fork_exists,
        wants_edit=wants_edit,
        chat_title=chat_title,
    )

    with DAPP_GEN_LOCK:
        if session_id in DAPP_GEN_IN_FLIGHT:
            return False
        DAPP_GEN_IN_FLIGHT.add(session_id)
    try:
        if route == "skip":
            return fork_exists

        publish_event("dapp_generating", project_path=normalized, session_id=session_id, route=route)

        last_user = last_user_message_text(messages)
        if is_simple_arithmetic_message(last_user):
            current_files = read_fork_files(normalized, session_id) or read_current_dapp_files(normalized)
            patched = de.try_local_math_widget_patch(messages, current_files, topic_label=chat_title)
            if patched and write_dapp_files(
                normalized,
                patched,
                session_id=session_id,
                source="local-math",
                chat_title=chat_title,
                messages=messages,
            ):
                publish_event(
                    "dapp_components_active",
                    project_path=normalized,
                    session_id=session_id,
                    components=["comp-math-hero", "comp-action-chips", "comp-assistant-insight"],
                )
                return True
            return fork_exists

        if route == "components" and not wants_edit:
            matched = dc.match_components(messages, chat_title=chat_title)
            current_files = read_fork_files(normalized, session_id) if fork_exists else {}
            composed = dc.compose_dapp_from_components(
                matched,
                messages,
                topic_label=chat_title,
                existing_files=current_files or None,
            )
            if composed and write_dapp_files(
                normalized,
                composed,
                session_id=session_id,
                source="components",
                chat_title=chat_title,
                messages=messages,
            ):
                comp_ids = [str(c.get("id")) for c in matched if c.get("id")]
                fork = session_fork_dir(normalized, session_id)
                meta_path = fork / "meta.json"
                meta = {}
                if meta_path.is_file():
                    try:
                        meta = json.loads(meta_path.read_text())
                    except Exception:
                        meta = {}
                meta["activeComponents"] = comp_ids
                meta_path.write_text(json.dumps(meta, indent=2))
                publish_event(
                    "dapp_components_active",
                    project_path=normalized,
                    session_id=session_id,
                    components=comp_ids,
                )
                return True

        if wants_edit and not fork_exists and template:
            reused_library = apply_library_template(normalized, session_id, template)
            fork_exists = True

        if route == "library" and template and not fork_exists and not wants_edit:
            if apply_library_template(normalized, session_id, template):
                sync_dapp_prompt_overlay(normalized, read_fork_files(normalized, session_id), chat_title)
                return True

        if not should_run_dapp_llm(
            messages,
            fork_exists=fork_exists,
            wants_edit=wants_edit,
            reused_library=reused_library,
        ):
            return fork_exists or reused_library

        if fork_exists or route == "incremental":
            current_files = read_fork_files(normalized, session_id) or read_current_dapp_files(normalized)
            mode = "incremental"
        else:
            current_files = read_current_dapp_files(normalized)
            mode = "full"

        return generate_dapp_with_neura(
            normalized,
            messages,
            session_id=session_id,
            current_files=current_files,
            mode=mode,
            chat_title=chat_title,
        )
    finally:
        with DAPP_GEN_LOCK:
            DAPP_GEN_IN_FLIGHT.discard(session_id)
        publish_event("dapp_generation_done", project_path=normalized, session_id=session_id)


def schedule_dapp_generation(
    session_id: str | None,
    project_path: str | None,
    user_text: str | None = None,
) -> None:
    if not session_id or not project_path:
        return
    try:
        normalized = normalize_project_path(project_path)
    except Exception:
        return

    chat = load_chat(session_id)
    if chat:
        messages = chat.get("messages") or []
        fork_exists = fork_has_custom_content(normalized, session_id)
        wants_edit = user_wants_dapp_edit(messages)
        if not conversation_wants_dapp_refresh(messages, fork_exists=fork_exists, wants_edit=wants_edit):
            return

    def worker() -> None:
        ctx = None
        with DAPP_GEN_LOCK:
            ctx = DAPP_GEN_PENDING.pop(session_id, None)
            DAPP_GEN_DEBOUNCE.pop(session_id, None)
        if not ctx:
            return
        sid, project, _delay = ctx
        try:
            refresh_project_dapp(sid, project)
        except Exception:
            pass

    fast_path = False
    if user_text and is_simple_arithmetic_message(user_text):
        fast_path = True
    elif user_text and dc.match_components([{"role": "user", "text": user_text}], chat_title=user_text):
        fast_path = True
    elif user_text and user_text_likely_matches_library(user_text):
        fast_path = True
    elif chat and find_library_template(chat.get("messages") or [], chat.get("title")):
        fast_path = True
    delay = NEURA_DAPP_DEBOUNCE_FAST if fast_path else NEURA_DAPP_DEBOUNCE

    with DAPP_GEN_LOCK:
        DAPP_GEN_PENDING[session_id] = (session_id, normalized, delay)
        existing = DAPP_GEN_DEBOUNCE.get(session_id)
        if existing:
            existing.cancel()
        timer = threading.Timer(delay, worker)
        timer.daemon = True
        DAPP_GEN_DEBOUNCE[session_id] = timer
        timer.start()


def first_user_text_from_messages(msgs) -> str:
    for m in msgs:
        if m.get("role") != "user":
            continue
        text, _ = _message_text(m.get("content"))
        if text:
            return text
    return ""


def is_title_generation_session_messages(msgs) -> bool:
    first = first_user_text_from_messages(msgs)
    return TITLE_GEN_MARKER in first or DAPP_GEN_MARKER in first


def delete_session_files(session_id: str) -> None:
    if not session_id or not session_id.startswith("session_"):
        return
    for name in (f"{session_id}.json", f"{session_id}.journal.jsonl"):
        path = SESSIONS_DIR / name
        if path.exists():
            try:
                path.unlink()
            except Exception:
                pass
    active = NEURA_HOME / f"{session_id}.active"
    if active.exists():
        try:
            active.unlink()
        except Exception:
            pass


def purge_title_generation_sessions() -> int:
    """Remove throwaway neura-run sessions created only for sidebar title labeling."""
    removed = 0
    if not SESSIONS_DIR.exists():
        return removed
    for path in SESSIONS_DIR.glob("session_*.json"):
        sid = path.stem
        try:
            data = json.loads(path.read_text())
        except Exception:
            continue
        msgs = session_messages(sid, data)
        if not is_title_generation_session_messages(msgs):
            continue
        delete_session_files(sid)
        meta = load_ui_chat_meta()
        chats = meta.get("chats", {})
        if sid in chats:
            del chats[sid]
            save_ui_chat_meta(meta)
        removed += 1
    if removed:
        publish_event("state_changed", reason="title_generation_sessions_purged", count=removed)
    return removed


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


SIDECAR_URL = os.environ.get("NEURA_SIDECAR_URL", "http://127.0.0.1:8080/v1")
SIDECAR_ACTIVITY_FILE = NEURA_HOME / "sidecar_activity.jsonl"
SIDECAR_PID_FILE = NEURA_HOME / "local-model-server.pid"
TURN_TRACE_FILE = NEURA_HOME / "turn-trace.jsonl"
NEURA_LOGS_DIR = NEURA_HOME / "logs"
SIDECAR_RECENT_WINDOW_MS = 15 * 60 * 1000


def _tail_jsonl(path: Path, max_lines: int = 200) -> list[dict]:
    """Parse up to max_lines JSON objects from the end of a JSONL file."""
    try:
        with open(path, "rb") as fh:
            fh.seek(0, os.SEEK_END)
            size = fh.tell()
            fh.seek(max(0, size - 256 * 1024))
            chunk = fh.read().decode("utf-8", "replace")
    except OSError:
        return []
    out: list[dict] = []
    for line in chunk.splitlines()[-max_lines:]:
        line = line.strip()
        if not line:
            continue
        try:
            parsed = json.loads(line)
        except ValueError:
            continue
        if isinstance(parsed, dict):
            out.append(parsed)
    return out


def _iso_to_ms(value: str | None) -> int | None:
    if not value:
        return None
    try:
        from datetime import datetime

        return int(datetime.fromisoformat(value).timestamp() * 1000)
    except (ValueError, OSError, OverflowError):
        return None


def _probe_sidecar() -> dict:
    import urllib.request

    started = time.monotonic()
    url = SIDECAR_URL.rstrip("/") + "/models"
    try:
        with urllib.request.urlopen(url, timeout=2.0) as resp:
            body = json.loads(resp.read().decode("utf-8", "replace"))
        models = [
            m.get("id") for m in body.get("data", []) if isinstance(m, dict) and m.get("id")
        ]
        return {
            "healthy": True,
            "url": SIDECAR_URL,
            "models": models,
            "probe_ms": int((time.monotonic() - started) * 1000),
        }
    except Exception as exc:
        return {
            "healthy": False,
            "url": SIDECAR_URL,
            "models": [],
            "probe_ms": int((time.monotonic() - started) * 1000),
            "error": str(exc),
        }


def build_sidecar_status() -> dict:
    """Live view of whether the local sidecar model is doing memory work:
    server health, recent sidecar generations, memory pipeline events
    (extraction, compression, linking), and per-turn prompt reinjection."""
    now_ms = int(time.time() * 1000)

    def recent(ts_ms: int | None) -> bool:
        return ts_ms is not None and now_ms - ts_ms <= SIDECAR_RECENT_WINDOW_MS

    probe = _probe_sidecar()
    try:
        pid = int(SIDECAR_PID_FILE.read_text().strip())
        pid_alive = Path(f"/proc/{pid}").exists()
    except (OSError, ValueError):
        pid, pid_alive = None, False

    activity = _tail_jsonl(SIDECAR_ACTIVITY_FILE, 100)
    recent_calls = [e for e in activity if recent(_iso_to_ms(e.get("timestamp")))]
    recent_failures = [e for e in recent_calls if not e.get("ok")]

    mem_files = sorted(NEURA_LOGS_DIR.glob("memory-events-*.jsonl"))
    mem_events: list[dict] = []
    for path in mem_files[-2:]:
        mem_events.extend(_tail_jsonl(path, 300))

    def last_event(kinds: set[str]) -> dict | None:
        return next((e for e in reversed(mem_events) if e.get("event") in kinds), None)

    last_injection_prepared = last_event({"pending_prepared"})
    last_injection_consumed = last_event({"pending_consumed"})
    last_embedding = last_event({"embedding_complete"})
    last_extraction = last_event(
        {"extraction_complete", "extraction_started", "final_extraction_started"}
    )
    last_compression = last_event({"ingest_compressed"})
    last_graph_link = last_event({"ingest_linked", "maintenance_linked"})

    turn_events = [
        e for e in _tail_jsonl(TURN_TRACE_FILE, 300) if e.get("event") == "turn_summary"
    ]
    recent_turns = [
        {
            "turn_id": e.get("turn_id"),
            "timestamp_ms": e.get("timestamp_ms"),
            "session_id": e.get("session_id"),
            "provider": e.get("provider"),
            "model": e.get("model"),
            "memory_inject_count": e.get("memory_inject_count"),
            "memory_inject_chars": e.get("memory_inject_chars"),
        }
        for e in turn_events[-20:]
    ]
    last_turn_with_injection = next(
        (t for t in reversed(recent_turns) if (t.get("memory_inject_count") or 0) > 0),
        None,
    )

    sidecar_active = bool(recent_calls)
    memory_pipeline_active = any(
        recent(_iso_to_ms(e.get("timestamp"))) for e in mem_events
    )
    reinjection_active = bool(
        (
            last_injection_consumed
            and recent(_iso_to_ms(last_injection_consumed.get("timestamp")))
        )
        or (last_turn_with_injection and recent(last_turn_with_injection.get("timestamp_ms")))
    )

    reasons: list[str] = []
    if not probe["healthy"]:
        verdict = "down"
        reasons.append("sidecar server unreachable; neura auto-starts it on next launch")
    elif recent_failures:
        verdict = "degraded"
        reasons.append(f"{len(recent_failures)} failed sidecar call(s) in the last 15m")
    elif sidecar_active or memory_pipeline_active or reinjection_active:
        verdict = "ok"
        if sidecar_active:
            reasons.append(f"{len(recent_calls)} sidecar generation(s) in the last 15m")
        if reinjection_active:
            reasons.append("memories reinjected into prompts within the last 15m")
        if memory_pipeline_active:
            reasons.append("memory pipeline events within the last 15m")
    else:
        verdict = "idle"
        reasons.append(
            "sidecar healthy but no memory activity in the last 15m (expected when no sessions are active)"
        )

    return {
        "generated_at_ms": now_ms,
        "verdict": verdict,
        "reasons": reasons,
        "sidecar": {
            **probe,
            "pid": pid,
            "pid_alive": pid_alive,
        },
        "sidecar_activity": {
            "recent_calls_15m": len(recent_calls),
            "recent_failures_15m": len(recent_failures),
            "last_call": activity[-1] if activity else None,
            "recent": activity[-20:],
        },
        "memory": {
            "last_injection_prepared": last_injection_prepared,
            "last_injection_consumed": last_injection_consumed,
            "last_embedding": last_embedding,
            "last_extraction": last_extraction,
            "last_compression": last_compression,
            "last_graph_link": last_graph_link,
        },
        "turns": {
            "recent": recent_turns,
            "last_with_injection": last_turn_with_injection,
        },
    }


# ----------------------- cognition / knowledge state -------------------------
#
# Read-only views over the semantic memory graph, the evidence ledger, and the
# knowledge event stream — the web UI's window into v0.12–v0.14 cognition.
# Everything is derived from files the Rust side already writes; this server
# never mutates cognitive state.

MEMORY_PROJECTS_DIR = NEURA_HOME / "memory" / "projects"
EVIDENCE_LEDGER_FILE = NEURA_HOME / "evidence_ledger_chain.json"
_GRAPH_JSON_CACHE: dict[str, tuple[float, dict]] = {}
MAX_GRAPH_BYTES = 64 * 1024 * 1024


def _load_graph_json(path: Path) -> dict | None:
    """mtime-cached parse of one project graph JSON."""
    try:
        stat = path.stat()
    except OSError:
        return None
    if stat.st_size > MAX_GRAPH_BYTES:
        return None
    key = str(path)
    cached = _GRAPH_JSON_CACHE.get(key)
    if cached and cached[0] == stat.st_mtime:
        return cached[1]
    try:
        data = json.loads(path.read_text(errors="replace"))
    except (OSError, ValueError):
        return None
    if not isinstance(data, dict):
        return None
    _GRAPH_JSON_CACHE[key] = (stat.st_mtime, data)
    return data


def _norm_path(p: str | None) -> str:
    if not p:
        return ""
    try:
        return str(Path(p).resolve())
    except OSError:
        return str(p)


def discover_project_graph(project_path: str | None) -> tuple[Path, dict] | None:
    """Find the memory graph for a project. The graph filename hash is not
    reproducible here, so we match `knowledge_sources.locator` against the
    project path; fallback: the most recently modified graph that has
    knowledge sources (then any most recent graph)."""
    target = _norm_path(project_path)
    candidates = sorted(
        MEMORY_PROJECTS_DIR.glob("*.json"),
        key=lambda p: p.stat().st_mtime if p.exists() else 0,
        reverse=True,
    )
    with_sources: tuple[Path, dict] | None = None
    newest: tuple[Path, dict] | None = None
    for path in candidates:
        graph = _load_graph_json(path)
        if graph is None:
            continue
        if newest is None:
            newest = (path, graph)
        sources = (graph.get("metadata") or {}).get("knowledge_sources") or {}
        if sources and with_sources is None:
            with_sources = (path, graph)
        if target:
            for state in sources.values():
                if _norm_path(state.get("locator")) == target:
                    return path, graph
    return with_sources or newest


def _first_line(text: str, limit: int = 96) -> str:
    line = (text or "").splitlines()[0] if text else ""
    return line[:limit]


def _semantic_degree(edges_out: list[dict], incoming_count: int) -> int:
    structural = {"has_tag", "in_cluster"}
    out = sum(1 for e in edges_out if e.get("kind") not in structural)
    return out + incoming_count


def _graph_indices(graph: dict) -> tuple[dict, dict, dict]:
    """(memories, edges_by_source, incoming_semantic_counts)."""
    memories = graph.get("memories") or {}
    edges = graph.get("edges") or {}
    incoming: dict[str, int] = {}
    structural = {"has_tag", "in_cluster"}
    for source_id, edge_list in edges.items():
        if source_id not in memories:
            continue
        for e in edge_list or []:
            target = e.get("target")
            if e.get("kind") in structural or target not in memories:
                continue
            incoming[target] = incoming.get(target, 0) + 1
    return memories, edges, incoming


def _intent_list(memories: dict, tag: str, limit: int = 12) -> list[dict]:
    items = [
        {
            "id": mid,
            "label": _first_line(m.get("content", ""), 120),
            "confidence": m.get("confidence", 0.0),
            "active": m.get("active", True),
            "updated_at": m.get("updated_at"),
        }
        for mid, m in memories.items()
        if m.get("active", True) and tag in (m.get("tags") or [])
    ]
    items.sort(key=lambda i: str(i.get("updated_at") or ""), reverse=True)
    return items[:limit]


def _ledger_blocks(limit: int = 60) -> list[dict]:
    try:
        data = json.loads(EVIDENCE_LEDGER_FILE.read_text(errors="replace"))
        blocks = data.get("blocks") or []
    except (OSError, ValueError):
        return []
    out = []
    for b in blocks[-limit:]:
        if isinstance(b, dict):
            out.append(
                {
                    "index": b.get("index"),
                    "timestamp_ms": b.get("timestamp_ms"),
                    "kind": b.get("kind"),
                    "subject": b.get("subject"),
                    "summary": b.get("summary"),
                    "score": b.get("score"),
                    "passed": b.get("passed"),
                }
            )
    return out


def _knowledge_events(limit: int = 60) -> list[dict]:
    mem_files = sorted(NEURA_LOGS_DIR.glob("memory-events-*.jsonl"))
    events: list[dict] = []
    for path in mem_files[-3:]:
        events.extend(_tail_jsonl(path, 400))
    picked = [
        {
            "timestamp": e.get("timestamp"),
            "session_id": e.get("session_id"),
            "event": e.get("event"),
            "detail": e.get("detail"),
        }
        for e in events
        if str(e.get("event", "")).startswith("knowledge_")
    ]
    return picked[-limit:]


def build_knowledge_state(project_path: str | None) -> dict:
    found = discover_project_graph(project_path)
    if not found:
        return {"available": False, "reason": "no project memory graph found"}
    graph_path, graph = found
    memories, edges, incoming = _graph_indices(graph)
    meta = graph.get("metadata") or {}
    clusters = graph.get("clusters") or {}

    # ---- edge kind counts + confidence buckets ----
    edge_kinds: dict[str, int] = {}
    for source_id, edge_list in edges.items():
        if source_id not in memories:
            continue
        for e in edge_list or []:
            kind = e.get("kind", "?")
            edge_kinds[kind] = edge_kinds.get(kind, 0) + 1
    conf = {"low": 0, "mid": 0, "high": 0}
    active_count = 0
    for m in memories.values():
        if not m.get("active", True):
            continue
        active_count += 1
        c = m.get("confidence", 0.0)
        conf["low" if c < 0.4 else "mid" if c < 0.7 else "high"] += 1

    # ---- explorable node list (bounded, degree-ranked, no embeddings) ----
    ranked = sorted(
        memories.items(),
        key=lambda kv: (
            -_semantic_degree(edges.get(kv[0]) or [], incoming.get(kv[0], 0)),
            kv[0],
        ),
    )
    nodes = [
        {
            "id": mid,
            "label": _first_line(m.get("content", "")),
            "tags": (m.get("tags") or [])[:6],
            "confidence": m.get("confidence", 0.0),
            "active": m.get("active", True),
            "degree": _semantic_degree(edges.get(mid) or [], incoming.get(mid, 0)),
            "evidence_count": len(m.get("evidence") or []),
        }
        for mid, m in ranked[:500]
    ]

    # ---- knowledge sources + evolution history ----
    sources = []
    for source_id, state in (meta.get("knowledge_sources") or {}).items():
        sources.append(
            {
                "id": source_id,
                "kind": state.get("kind"),
                "locator": state.get("locator"),
                "items": len(state.get("fingerprints") or {}),
                "concepts": len(state.get("unit_ids") or {}),
                "pending_abstraction": len(state.get("pending_abstraction") or []),
                "last_ingest": state.get("last_ingest"),
                "last_report": state.get("last_report"),
                "history": (state.get("history") or [])[-24:],
            }
        )

    reflections = [b for b in _ledger_blocks(200) if b.get("kind") == "Reflection"][-12:]

    return {
        "available": True,
        "graph_file": graph_path.name,
        "generated_at_ms": int(time.time() * 1000),
        "totals": {
            "concepts": len(memories),
            "active": active_count,
            "tags": len(graph.get("tags") or {}),
            "communities": len(clusters),
            "edge_kinds": edge_kinds,
            "confidence": conf,
        },
        "sources": sources,
        "prediction_stats": meta.get("prediction_stats"),
        "last_sleep": meta.get("last_sleep"),
        "consolidations": (meta.get("consolidations") or [])[-8:],
        "goals": _intent_list(memories, "goal"),
        "decisions": _intent_list(memories, "decision"),
        "plans": _intent_list(memories, "plan"),
        "nodes": nodes,
        "ledger": _ledger_blocks(40),
        "reflections": reflections,
        "events": _knowledge_events(60),
    }


def build_concept_detail(project_path: str | None, concept_id: str) -> dict:
    found = discover_project_graph(project_path)
    if not found:
        return {"available": False}
    _, graph = found
    memories, edges, _incoming = _graph_indices(graph)
    m = memories.get(concept_id)
    if not isinstance(m, dict):
        return {"available": False, "reason": "concept not found"}

    structural = {"has_tag", "in_cluster"}
    out_edges = []
    communities = []
    for e in edges.get(concept_id) or []:
        kind = e.get("kind")
        target = e.get("target", "")
        if kind == "in_cluster":
            communities.append(target.replace("cluster:", ""))
            continue
        if kind in structural or target not in memories:
            continue
        out_edges.append(
            {
                "kind": kind,
                "target": target,
                "label": _first_line(memories[target].get("content", "")),
                "weight": e.get("weight", 1.0),
                "confidence": e.get("confidence", 0.0),
                "evidence_count": e.get("evidence_count", 0),
            }
        )
    in_edges = []
    for source_id, edge_list in edges.items():
        if source_id not in memories or source_id == concept_id:
            continue
        for e in edge_list or []:
            if e.get("target") == concept_id and e.get("kind") not in structural:
                in_edges.append(
                    {
                        "kind": e.get("kind"),
                        "source": source_id,
                        "label": _first_line(memories[source_id].get("content", "")),
                        "weight": e.get("weight", 1.0),
                        "confidence": e.get("confidence", 0.0),
                    }
                )
    out_edges.sort(key=lambda e: (-(e["weight"] * max(e["confidence"], 0.05)), e["target"]))
    in_edges.sort(key=lambda e: (-(e["weight"] * max(e["confidence"], 0.05)), e["source"]))

    evidence = [
        {
            "kind": ev.get("kind"),
            "id": ev.get("id"),
            "note": ev.get("note"),
            "at": ev.get("at"),
        }
        for ev in (m.get("evidence") or [])
    ]

    return {
        "available": True,
        "id": concept_id,
        "content": m.get("content", ""),
        "tags": m.get("tags") or [],
        "confidence": m.get("confidence", 0.0),
        "strength": m.get("strength", 0),
        "access_count": m.get("access_count", 0),
        "active": m.get("active", True),
        "superseded_by": m.get("superseded_by"),
        "source": m.get("source"),
        "created_at": m.get("created_at"),
        "updated_at": m.get("updated_at"),
        "communities": communities,
        "evidence": evidence,
        "edges_out": out_edges[:24],
        "edges_in": in_edges[:24],
        "has_embedding": bool(m.get("embedding")),
        "has_concept_embedding": bool(m.get("concept_embedding")),
    }


def build_state() -> dict:
    logs = sorted(NEURA_HOME.glob("*.log"), key=lambda p: p.stat().st_mtime if p.exists() else 0, reverse=True)[:5]
    events = NEURA_HOME / "events.jsonl"
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
        "neuraHome": str(NEURA_HOME),
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
            "activeMarkers": [p.name for p in NEURA_HOME.glob("*.active")][:20],
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
    """Reconstruct a session's full message list the way neura does: the snapshot
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
    raw_msgs = session_messages(session_id, data)
    if is_title_generation_session_messages(raw_msgs):
        delete_session_files(session_id)
        return None
    messages = []
    for m in raw_msgs:
        role = m.get("role", "")
        if role not in ("user", "assistant"):
            continue
        text, tools = _message_text(m.get("content"))
        text = de.sanitize_chat_display_text(text)
        if not text and not tools:
            continue
        if text and TITLE_GEN_MARKER in text and "Reply with ONLY" in text:
            continue
        if messages and messages[-1]["role"] == role and messages[-1].get("text") == text:
            continue
        messages.append({"role": role, "text": text, "tools": tools})
    animal = data.get("short_name") or session_animal(session_id)
    title, locked, source = resolve_chat_title(session_id, data)
    return {
        "id": session_id,
        "name": animal,
        "serverName": SERVER_NAME,
        "title": title,
        "titleLocked": locked,
        "titleSource": source,
        "model": data.get("model"),
        "workingDir": session_working_dir(data),
        "updatedAt": data.get("updated_at") or data.get("last_active_at"),
        "messageCount": len(messages),
        "messages": messages,
    }


def list_chats(project_path: str | None = None) -> list[dict]:
    purge_title_generation_sessions()
    out = []
    normalized_filter = None
    if project_path:
        try:
            normalized_filter = normalize_project_path(project_path)
        except Exception:
            normalized_filter = None
    if not SESSIONS_DIR.exists():
        return out
    for path in SESSIONS_DIR.glob("session_*.json"):
        sid = path.stem
        try:
            data = json.loads(path.read_text())
        except Exception:
            continue
        msgs = session_messages(sid, data)
        if not msgs or is_title_generation_session_messages(msgs):
            continue
        wd = session_working_dir(data)
        if normalized_filter and wd != normalized_filter:
            continue
        animal = data.get("short_name") or session_animal(sid)
        title, locked, source = resolve_chat_title(sid, data)
        out.append({
            "id": sid,
            "name": animal,
            "serverName": SERVER_NAME,
            "title": title,
            "titleLocked": locked,
            "titleSource": source,
            "model": data.get("model"),
            "workingDir": wd,
            "updatedAt": data.get("updated_at") or data.get("last_active_at") or path.stat().st_mtime,
            "messageCount": sum(1 for m in msgs if m.get("role") in ("user", "assistant")),
        })
    out.sort(key=lambda c: str(c.get("updatedAt") or ""), reverse=True)
    return out


def chat_turn_cwd(session_id: str | None, working_dir: str | None) -> str:
    if session_id:
        data = read_session_snapshot(session_id)
        wd = session_working_dir(data)
        if wd:
            return wd
    if working_dir:
        try:
            return normalize_project_path(working_dir)
        except Exception:
            pass
    return str(Path.home())


def run_chat_turn(session_id: str | None, message: str, working_dir: str | None = None) -> dict:
    """Drive one agent turn via `neura run --json`, returning a normalized result."""
    cwd = chat_turn_cwd(session_id, working_dir)
    agent_message = message
    try:
        if not is_dapp_skip_message(message):
            ctx = build_chat_dapp_context(cwd, session_id)
            if ctx:
                agent_message = de.inject_dapp_context(message, ctx)
    except Exception:
        agent_message = message
    bin_path = neura_bin()
    if not Path(bin_path).is_file():
        return {
            "error": (
                f"neura binary not found at {bin_path}. "
                f"Build it with: cd {ROOT} && cargo build --release "
                "or set NEURA_BIN to the binary path."
            ),
        }
    cmd = [bin_path, "run", "--json", "--no-update", "--no-selfdev"]
    if session_id:
        cmd.append(f"--resume={session_id}")
    cmd += ["--", agent_message]
    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=NEURA_RUN_TIMEOUT,
            cwd=cwd,
        )
    except subprocess.TimeoutExpired:
        return {"error": f"neura run timed out after {NEURA_RUN_TIMEOUT}s"}
    except Exception as exc:
        return {"error": f"failed to launch neura: {exc}"}

    if proc.returncode != 0:
        detail = (proc.stderr or proc.stdout or "").strip()[-800:]
        return {"error": f"neura run exited {proc.returncode}: {detail}"}

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
        return {"error": f"could not parse neura output: {raw[:500]}"}

    sid = report.get("session_id", session_id or "")
    animal = session_animal(sid)
    data = read_session_snapshot(sid) if sid else None
    title, locked, source = resolve_chat_title(sid, data) if sid else (default_chat_title(animal), False, "default")
    result = {
        "session_id": sid,
        "name": animal,
        "serverName": SERVER_NAME,
        "title": title,
        "titleLocked": locked,
        "titleSource": source,
        "text": report.get("text", ""),
        "model": report.get("model"),
        "usage": report.get("usage"),
    }
    schedule_chat_title_refresh(sid or None)
    schedule_dapp_generation(sid or None, cwd, message)
    return result


# ------------------------------- dapp workspace ----------------------------

DAPP_DEFAULT_FILES: dict[str, str] = de.NEURA_AI_SHELL_FILES

DAPP_MIME_TYPES = {
    ".html": "text/html; charset=utf-8",
    ".htm": "text/html; charset=utf-8",
    ".css": "text/css; charset=utf-8",
    ".js": "text/javascript; charset=utf-8",
    ".mjs": "text/javascript; charset=utf-8",
    ".json": "application/json; charset=utf-8",
    ".svg": "image/svg+xml",
    ".png": "image/png",
    ".jpg": "image/jpeg",
    ".jpeg": "image/jpeg",
    ".gif": "image/gif",
    ".webp": "image/webp",
    ".md": "text/markdown; charset=utf-8",
    ".txt": "text/plain; charset=utf-8",
}


def dapp_root(project_path: str) -> Path:
    return Path(normalize_project_path(project_path)) / ".neura" / "dapp"


def project_path_from_id(project_id: str) -> str | None:
    for project in list_projects():
        if project.get("id") == project_id:
            return str(project.get("path"))
    return None


def dapp_resolve_file(root: Path, rel_path: str) -> Path:
    rel = (rel_path or "").strip().replace("\\", "/").lstrip("/")
    if not rel or rel.endswith("/"):
        raise ValueError("invalid dapp path")
    parts = [part for part in rel.split("/") if part not in ("", ".")]
    if ".." in parts:
        raise ValueError("invalid dapp path")
    target = (root / Path(*parts)).resolve()
    root_resolved = root.resolve()
    if target != root_resolved and root_resolved not in target.parents:
        raise ValueError("invalid dapp path")
    return target


def ensure_dapp(project_path: str) -> Path:
    root = dapp_root(project_path)
    root.mkdir(parents=True, exist_ok=True)
    for name, content in DAPP_DEFAULT_FILES.items():
        path = root / name
        if not path.exists():
            path.write_text(content)
    return root


def list_dapp_files(project_path: str) -> list[dict]:
    root = ensure_dapp(project_path)
    files: list[dict] = []
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        rel = path.relative_to(root).as_posix()
        files.append({"path": rel, "size": path.stat().st_size})
    return files


def dapp_revision(project_path: str) -> str:
    root = ensure_dapp(project_path)
    hasher = hashlib.sha256()
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        stat = path.stat()
        hasher.update(path.relative_to(root).as_posix().encode())
        hasher.update(str(stat.st_mtime_ns).encode())
        hasher.update(str(stat.st_size).encode())
    return hasher.hexdigest()[:16]


def read_dapp_file(project_path: str, rel_path: str) -> dict:
    root = ensure_dapp(project_path)
    target = dapp_resolve_file(root, rel_path)
    if not target.is_file():
        raise ValueError("dapp file not found")
    return {
        "path": target.relative_to(root).as_posix(),
        "content": target.read_text(errors="replace"),
    }


def write_dapp_file(
    project_path: str,
    rel_path: str,
    content: str,
    *,
    session_id: str | None = None,
) -> dict:
    normalized = normalize_project_path(project_path)
    root = ensure_dapp(normalized)
    target = dapp_resolve_file(root, rel_path)
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content)

    sid = session_id or read_active_session_id(normalized)
    if sid:
        fork = session_fork_dir(normalized, sid)
        fork.mkdir(parents=True, exist_ok=True)
        try:
            fork_target = dapp_resolve_file(fork, rel_path)
            fork_target.parent.mkdir(parents=True, exist_ok=True)
            fork_target.write_text(content)
            write_fork_meta(normalized, sid, custom=True, source="manual")
        except Exception:
            pass

    publish_event("dapp_changed", project_path=normalized)
    return {
        "path": target.relative_to(root).as_posix(),
        "size": target.stat().st_size,
    }


def dapp_guess_mime(path: Path) -> str:
    return DAPP_MIME_TYPES.get(path.suffix.lower(), "application/octet-stream")


def dapp_preview_html(project_path: str, project_id: str) -> bytes:
    root = ensure_dapp(project_path)
    index = root / "index.html"
    if not index.is_file():
        index.write_text(DAPP_DEFAULT_FILES["index.html"])
    bridge = root / "neura-bridge.js"
    if not bridge.is_file():
        bridge.write_text(de.NEURA_BRIDGE_JS)
    widget_js = root / "neura-widget.js"
    if not widget_js.is_file():
        widget_js.write_text(de.NEURA_WIDGET_JS)
    html = index.read_text(errors="replace")
    base_href = f"/api/dapp/static/{project_id}/"
    base_tag = f'<base href="{base_href}">'
    lower = html.lower()
    if "<base" not in lower:
        if "<head>" in lower:
            idx = lower.index("<head>")
            insert_at = idx + len("<head>")
            html = html[:insert_at] + base_tag + html[insert_at:]
        else:
            html = f"<!DOCTYPE html><html><head>{base_tag}</head><body>{html}</body></html>"
            lower = html.lower()
    if "neura-bridge.js" not in lower and "neura_bridge" not in lower:
        bridge_tag = '<script src="neura-bridge.js"></script>'
        if "</body>" in lower:
            body_idx = lower.rindex("</body>")
            html = html[:body_idx] + bridge_tag + html[body_idx:]
        else:
            html += bridge_tag
        lower = html.lower()
    if "neura-widget.js" not in lower:
        widget_tag = '<script src="neura-widget.js"></script>'
        if "</body>" in lower:
            body_idx = lower.rindex("</body>")
            html = html[:body_idx] + widget_tag + html[body_idx:]
        else:
            html += widget_tag
    csp = (
        '<meta http-equiv="Content-Security-Policy" '
        'content="default-src \'none\'; img-src data: blob: *; style-src \'unsafe-inline\' *; '
        'script-src \'unsafe-inline\' *; connect-src *; font-src *; media-src *;">'
    )
    lower = html.lower()
    if "content-security-policy" not in lower and "<head>" in lower:
        idx = lower.index("<head>") + len("<head>")
        html = html[:idx] + csp + html[idx:]
    return html.encode("utf-8")


def read_dapp_static(project_id: str, rel_path: str) -> tuple[bytes, str]:
    project_path = project_path_from_id(project_id)
    if not project_path:
        raise ValueError("unknown project")
    root = ensure_dapp(project_path)
    target = dapp_resolve_file(root, rel_path or "index.html")
    if not target.is_file():
        raise ValueError("dapp file not found")
    return target.read_bytes(), dapp_guess_mime(target)


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


def neura_processes() -> list[int]:
    """Every live neura *binary* process, found by its real executable path so it
    works regardless of how proctitle renames argv/comm. Resolves the install
    path too, and falls back to a basename match for dev/self-dev binaries."""
    target = os.path.realpath(neura_bin())
    pids = []
    for pid in _all_pids():
        exe = _proc_exe(pid)
        if not exe:
            continue
        if exe == target or os.path.basename(exe) == "neura":
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
        if "neura-ui-server.py" in cmd or "scripts/neuraui" in cmd:
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
    """Kill all neura processes, sibling UI-server processes, then this server.
    Only sends signals — no files are touched — so `neura` restarts cleanly."""
    others = neura_processes() + ui_server_processes()
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
        sys.stderr.write(f"[neura-ui] {self.client_address[0]} {fmt % args}\n")

    def _send_json(self, payload, status: int = 200) -> None:
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _send_bytes(self, body: bytes, content_type: str, status: int = 200, *, no_cache: bool = False) -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        if no_cache:
            self.send_header("Cache-Control", "no-cache, no-store, must-revalidate")
            self.send_header("Pragma", "no-cache")
        self.end_headers()
        self.wfile.write(body)

    def _sse_write(self, obj) -> bool:
        try:
            self.wfile.write(b"data: " + json.dumps(obj).encode() + b"\n\n")
            self.wfile.flush()
            return True
        except (BrokenPipeError, ConnectionResetError, TimeoutError, ValueError):
            return False

    def stream_subtext(self, body: dict) -> None:
        """Stream live thought narration from the local Neura OSS model as SSE."""
        message = (body.get("message") or "").strip()
        context = body.get("context") or []
        base, model = subtext_sidecar_target()

        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "keep-alive")
        self.send_header("Access-Control-Allow-Origin", "*")
        self.end_headers()

        if not message:
            self._sse_write({"type": "done"})
            return

        transcript_lines = []
        for turn in context[-4:]:
            role = turn.get("role", "user")
            text = (turn.get("content") or turn.get("text") or "").strip()
            if text:
                transcript_lines.append(f"{role}: {text}")
        transcript_lines.append(f"user: {message}")
        # We surface the model's own chain-of-thought (the `reasoning` field) as
        # the live "thinking". So the task itself is simply to think about the
        # user's latest message — that produces genuine first-person reasoning
        # about the question rather than meta-notes about writing notes.
        payload = {
            "model": model,
            "messages": [
                {
                    "role": "system",
                    "content": (
                        "You are the inner monologue of the Neura assistant. Think "
                        "concisely, in the first person, about the user's latest "
                        "message: what they want, what matters, and how to respond. "
                        "The assistant HAS live web search, fetch, and tool access, "
                        "so assume it can look up current/real-time info — never "
                        "conclude that it 'can't access live data'. Do not address "
                        "the user; just think."
                    ),
                },
                {"role": "user", "content": "\n".join(transcript_lines)},
            ],
            "max_tokens": 96,
            "temperature": 0.3,
            "stream": True,
            # Ignored by the /v1 route (kept for forward-compat); warmth is held
            # by the native-endpoint prewarm at startup.
            "keep_alive": "30m",
        }
        req = urllib.request.Request(
            base + "/chat/completions",
            data=json.dumps(payload).encode(),
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        accumulated = ""
        try:
            with urllib.request.urlopen(req, timeout=60) as resp:
                for raw in resp:
                    line = raw.decode("utf-8", "ignore").strip()
                    if not line.startswith("data:"):
                        continue
                    data = line[len("data:"):].strip()
                    if not data or data == "[DONE]":
                        continue
                    try:
                        obj = json.loads(data)
                        delta = obj["choices"][0].get("delta", {})
                    except (ValueError, KeyError, IndexError):
                        continue
                    # gpt-oss via Ollama streams the live thinking in `reasoning`
                    # (content stays empty until the thought resolves).
                    piece = (
                        delta.get("reasoning")
                        or delta.get("reasoning_content")
                        or delta.get("content")
                        or ""
                    )
                    if not piece:
                        continue
                    accumulated += piece
                    words = accumulated.split()[-8:]
                    if not self._sse_write({
                        "type": "frame",
                        "phase": "thinking",
                        "text": accumulated.strip(),
                        "words": words,
                    }):
                        return
        except (urllib.error.URLError, urllib.error.HTTPError, TimeoutError, OSError) as exc:
            self._sse_write({"type": "error", "error": f"local model observer unavailable: {exc}"})
        self._sse_write({"type": "done"})

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
        if path == "/api/knowledge":
            query = parse_qs(urlparse(self.path).query)
            project = (query.get("project") or [None])[0]
            return self._send_json(build_knowledge_state(project))
        if path == "/api/knowledge/concept":
            query = parse_qs(urlparse(self.path).query)
            project = (query.get("project") or [None])[0]
            concept_id = (query.get("id") or [""])[0]
            return self._send_json(build_concept_detail(project, concept_id))
        if path == "/api/sidecar-status":
            return self._send_json(build_sidecar_status())
        if path == "/api/subtext-config":
            return self._send_json(build_subtext_config())
        if path == "/api/projects":
            return self._send_json({"projects": list_projects()})
        if path == "/api/projects/suggest-path":
            query = parse_qs(urlparse(self.path).query)
            name = (query.get("name") or [None])[0]
            try:
                return self._send_json(suggest_project_path(name))
            except Exception as exc:
                return self._send_json({"error": str(exc)}, status=400)
        if path == "/api/workspace":
            try:
                path_str = default_workspace_path()
                return self._send_json({
                    "path": path_str,
                    "name": "Workspace",
                })
            except Exception as exc:
                return self._send_json({"error": str(exc)}, status=400)
        if path == "/api/fs/browse":
            query = parse_qs(urlparse(self.path).query)
            browse_path = (query.get("path") or [None])[0]
            try:
                return self._send_json(browse_filesystem(browse_path))
            except ValueError as exc:
                return self._send_json({"error": str(exc)}, status=400)
            except Exception as exc:
                return self._send_json({"error": str(exc)}, status=500)
        if path == "/api/chats":
            query = parse_qs(urlparse(self.path).query)
            project_path = (query.get("project") or query.get("project_path") or [None])[0]
            return self._send_json(
                {"serverName": SERVER_NAME, "chats": list_chats(project_path=project_path)}
            )
        if path.startswith("/api/chats/"):
            sid = path[len("/api/chats/"):]
            chat = load_chat(sid)
            if chat is None:
                return self.send_error(404, "Unknown chat session")
            return self._send_json(chat)
        if path == "/api/dapp":
            query = parse_qs(urlparse(self.path).query)
            project_path = (query.get("project_path") or [None])[0]
            if not project_path:
                return self._send_json({"error": "project_path is required"}, status=400)
            try:
                normalized = normalize_project_path(project_path)
                root = ensure_dapp(normalized)
                return self._send_json(
                    {
                        "projectPath": normalized,
                        "projectId": project_id_for_path(normalized),
                        "root": str(root),
                        "files": list_dapp_files(normalized),
                    }
                )
            except Exception as exc:
                return self._send_json({"error": str(exc)}, status=400)
        if path == "/api/dapp/revision":
            query = parse_qs(urlparse(self.path).query)
            project_path = (query.get("project_path") or [None])[0]
            if not project_path:
                return self._send_json({"error": "project_path is required"}, status=400)
            try:
                normalized = normalize_project_path(project_path)
                return self._send_json({
                    "projectPath": normalized,
                    "revision": dapp_revision(normalized),
                })
            except Exception as exc:
                return self._send_json({"error": str(exc)}, status=400)
        if path == "/api/dapp/library":
            try:
                return self._send_json({"templates": list_dapp_library_entries()})
            except Exception as exc:
                return self._send_json({"error": str(exc)}, status=500)
        if path == "/api/dapp/themes":
            try:
                return self._send_json({"themes": list_dapp_theme_entries()})
            except Exception as exc:
                return self._send_json({"error": str(exc)}, status=500)
        if path == "/api/dapp/components":
            try:
                return self._send_json({"components": dc.list_component_entries()})
            except Exception as exc:
                return self._send_json({"error": str(exc)}, status=500)
        if path.startswith("/api/dapp/library/"):
            tpl_id = path[len("/api/dapp/library/") :].strip("/")
            if not tpl_id:
                return self._send_json({"error": "template id required"}, status=400)
            entry = get_dapp_library_entry(tpl_id)
            if entry is None:
                return self.send_error(404, "Unknown template")
            return self._send_json(entry)
        if path == "/api/dapp/history":
            query = parse_qs(urlparse(self.path).query)
            project_path = (query.get("project_path") or [None])[0]
            session_id = (query.get("session_id") or [None])[0]
            if not project_path or not session_id:
                return self._send_json({"error": "project_path and session_id required"}, status=400)
            try:
                snapshots = list_dapp_history(project_path, session_id)
                return self._send_json({"snapshots": snapshots})
            except Exception as exc:
                return self._send_json({"error": str(exc)}, status=400)
        if path == "/api/dapp/diff":
            query = parse_qs(urlparse(self.path).query)
            project_path = (query.get("project_path") or [None])[0]
            session_id = (query.get("session_id") or [None])[0]
            turn_raw = (query.get("turn") or ["0"])[0]
            if not project_path or not session_id:
                return self._send_json({"error": "project_path and session_id required"}, status=400)
            try:
                turn = int(turn_raw)
                diff = get_dapp_turn_diff(project_path, session_id, turn)
                return self._send_json(diff)
            except Exception as exc:
                return self._send_json({"error": str(exc)}, status=400)
        if path == "/api/dapp/file":
            query = parse_qs(urlparse(self.path).query)
            project_path = (query.get("project_path") or [None])[0]
            rel_path = (query.get("path") or [None])[0]
            if not project_path or not rel_path:
                return self._send_json({"error": "project_path and path are required"}, status=400)
            try:
                return self._send_json(read_dapp_file(project_path, rel_path))
            except ValueError as exc:
                return self._send_json({"error": str(exc)}, status=404)
            except Exception as exc:
                return self._send_json({"error": str(exc)}, status=400)
        if path.startswith("/api/dapp/preview/"):
            project_id = path[len("/api/dapp/preview/") :].strip("/")
            project_path = project_path_from_id(project_id)
            if not project_path:
                return self.send_error(404, "Unknown project")
            try:
                body = dapp_preview_html(project_path, project_id)
                return self._send_bytes(body, "text/html; charset=utf-8", no_cache=True)
            except Exception as exc:
                return self._send_json({"error": str(exc)}, status=400)
        if path.startswith("/api/dapp/static/"):
            rest = path[len("/api/dapp/static/") :]
            if "/" not in rest:
                return self.send_error(404, "Missing dapp file path")
            project_id, rel_path = rest.split("/", 1)
            try:
                body, mime = read_dapp_static(project_id, rel_path)
                return self._send_bytes(body, mime, no_cache=True)
            except ValueError:
                return self.send_error(404, "Dapp file not found")
            except Exception as exc:
                return self._send_json({"error": str(exc)}, status=400)
        if path.startswith("/api/"):
            return self.send_error(404, "Unknown Neura API endpoint")
        if path == "/" or (DIST_DIR / path.lstrip("/")).exists():
            return super().do_GET()
        # SPA fallback.
        self.path = "/index.html"
        return super().do_GET()

    def do_DELETE(self) -> None:  # noqa: N802
        path = urlparse(self.path).path
        if path == "/api/projects":
            try:
                length = int(self.headers.get("Content-Length", "0"))
                body = json.loads(self.rfile.read(length) or b"{}")
            except Exception as exc:
                return self._send_json({"error": f"bad request: {exc}"}, status=400)
            project_path = (body.get("path") or body.get("project_path") or "").strip()
            if not project_path:
                return self._send_json({"error": "path is required"}, status=400)
            try:
                return self._send_json(delete_project(project_path))
            except ValueError as exc:
                return self._send_json({"error": str(exc)}, status=400)
            except Exception as exc:
                return self._send_json({"error": str(exc)}, status=500)
        if path.startswith("/api/dapp/library/"):
            tpl_id = path[len("/api/dapp/library/") :].strip("/")
            if not tpl_id or tpl_id in ("pin", "unpin", "apply"):
                return self._send_json({"error": "invalid template id"}, status=400)
            if delete_dapp_library_entry(tpl_id):
                return self._send_json({"ok": True, "templateId": tpl_id})
            return self._send_json({"error": "template not found"}, status=404)
        return self.send_error(404, "Unknown Neura API endpoint")

    def do_PATCH(self) -> None:  # noqa: N802
        path = urlparse(self.path).path
        if not path.startswith("/api/chats/"):
            return self.send_error(404, "Unknown Neura API endpoint")
        session_id = path[len("/api/chats/") :].strip("/")
        if not session_id:
            return self.send_error(404, "Unknown chat session")
        try:
            length = int(self.headers.get("Content-Length", "0"))
            body = json.loads(self.rfile.read(length) or b"{}")
        except Exception as exc:
            return self._send_json({"error": f"bad request: {exc}"}, status=400)
        title = (body.get("title") or "").strip()
        if not title:
            return self._send_json({"error": "title is required"}, status=400)
        if read_session_snapshot(session_id) is None and session_id not in load_ui_chat_meta().get("chats", {}):
            return self._send_json({"error": "Unknown chat session"}, status=404)
        locked = bool(body.get("lock", True))
        saved = set_chat_title(session_id, title, locked=locked, source="user")
        return self._send_json(
            {
                "id": session_id,
                "title": saved,
                "titleLocked": locked,
                "titleSource": "user",
            }
        )

    def do_PUT(self) -> None:  # noqa: N802
        path = urlparse(self.path).path
        if path != "/api/dapp/file":
            return self.send_error(404, "Unknown Neura API endpoint")
        try:
            length = int(self.headers.get("Content-Length", "0"))
            body = json.loads(self.rfile.read(length) or b"{}")
        except Exception as exc:
            return self._send_json({"error": f"bad request: {exc}"}, status=400)
        project_path = (body.get("project_path") or "").strip()
        rel_path = (body.get("path") or "").strip()
        if not project_path or not rel_path:
            return self._send_json({"error": "project_path and path are required"}, status=400)
        try:
            saved = write_dapp_file(
                project_path,
                rel_path,
                str(body.get("content") or ""),
                session_id=(body.get("session_id") or "").strip() or None,
            )
            return self._send_json(saved)
        except ValueError as exc:
            return self._send_json({"error": str(exc)}, status=400)
        except Exception as exc:
            return self._send_json({"error": str(exc)}, status=500)

    def do_POST(self) -> None:  # noqa: N802
        path = urlparse(self.path).path
        if path == "/api/subtext-stream":
            try:
                length = int(self.headers.get("Content-Length", "0"))
                body = json.loads(self.rfile.read(length) or b"{}")
            except Exception:
                body = {}
            return self.stream_subtext(body)
        if path == "/api/dapp/activate":
            try:
                length = int(self.headers.get("Content-Length", "0"))
                body = json.loads(self.rfile.read(length) or b"{}")
            except Exception as exc:
                return self._send_json({"error": f"bad request: {exc}"}, status=400)
            project_path = (body.get("project_path") or "").strip()
            session_id = (body.get("session_id") or "").strip()
            if not project_path or not session_id:
                return self._send_json({"error": "project_path and session_id are required"}, status=400)
            try:
                result = activate_session_dapp(project_path, session_id)
                return self._send_json(result)
            except Exception as exc:
                return self._send_json({"error": str(exc)}, status=400)
        if path == "/api/dapp/prefetch":
            try:
                length = int(self.headers.get("Content-Length", "0"))
                body = json.loads(self.rfile.read(length) or b"{}")
            except Exception as exc:
                return self._send_json({"error": f"bad request: {exc}"}, status=400)
            project_path = (body.get("project_path") or "").strip()
            session_id = (body.get("session_id") or "").strip()
            user_text = (body.get("user_text") or body.get("message") or "").strip()
            if not project_path or not session_id or not user_text:
                return self._send_json({"error": "project_path, session_id, and user_text are required"}, status=400)
            try:
                result = prefetch_dapp_from_user_text(
                    project_path,
                    session_id,
                    user_text,
                    chat_title=(body.get("chat_title") or "").strip() or None,
                )
                return self._send_json(result)
            except Exception as exc:
                return self._send_json({"error": str(exc)}, status=400)
        if path == "/api/dapp/library/pin":
            try:
                length = int(self.headers.get("Content-Length", "0"))
                body = json.loads(self.rfile.read(length) or b"{}")
            except Exception as exc:
                return self._send_json({"error": f"bad request: {exc}"}, status=400)
            template_id = (body.get("template_id") or body.get("templateId") or "").strip()
            if not template_id:
                return self._send_json({"error": "template_id is required"}, status=400)
            if pin_library_template(template_id):
                return self._send_json({"ok": True, "templateId": template_id, "pinned": True})
            return self._send_json({"error": "template not found"}, status=404)
        if path == "/api/dapp/library/unpin":
            try:
                length = int(self.headers.get("Content-Length", "0"))
                body = json.loads(self.rfile.read(length) or b"{}")
            except Exception as exc:
                return self._send_json({"error": f"bad request: {exc}"}, status=400)
            template_id = (body.get("template_id") or body.get("templateId") or "").strip()
            if not template_id:
                return self._send_json({"error": "template_id is required"}, status=400)
            if unpin_library_template(template_id):
                return self._send_json({"ok": True, "templateId": template_id, "pinned": False})
            return self._send_json({"error": "template not pinned"}, status=404)
        if path == "/api/dapp/themes/apply":
            try:
                length = int(self.headers.get("Content-Length", "0"))
                body = json.loads(self.rfile.read(length) or b"{}")
            except Exception as exc:
                return self._send_json({"error": f"bad request: {exc}"}, status=400)
            project_path = (body.get("project_path") or "").strip()
            session_id = (body.get("session_id") or "").strip()
            theme_id = (body.get("theme_id") or body.get("themeId") or "").strip()
            if not project_path or not session_id or not theme_id:
                return self._send_json({"error": "project_path, session_id, theme_id required"}, status=400)
            if apply_dapp_theme(project_path, session_id, theme_id):
                theme = get_dapp_theme_entry(theme_id)
                return self._send_json({"ok": True, "themeId": theme_id, "theme": theme})
            return self._send_json({"error": "theme not found"}, status=404)
        if path == "/api/dapp/library/apply":
            try:
                length = int(self.headers.get("Content-Length", "0"))
                body = json.loads(self.rfile.read(length) or b"{}")
            except Exception as exc:
                return self._send_json({"error": f"bad request: {exc}"}, status=400)
            project_path = (body.get("project_path") or "").strip()
            session_id = (body.get("session_id") or "").strip()
            template_id = (body.get("template_id") or body.get("templateId") or "").strip()
            if not project_path or not session_id or not template_id:
                return self._send_json({"error": "project_path, session_id, template_id required"}, status=400)
            entry = get_dapp_library_entry(template_id)
            if not entry:
                return self._send_json({"error": "template not found"}, status=404)
            try:
                if apply_library_template(project_path, session_id, entry):
                    sync_dapp_prompt_overlay(project_path, read_fork_files(project_path, session_id))
                    return self._send_json({"ok": True, "templateId": template_id})
            except Exception as exc:
                return self._send_json({"error": str(exc)}, status=400)
            return self._send_json({"error": "apply failed"}, status=500)
        if path == "/api/dapp/undo":
            try:
                length = int(self.headers.get("Content-Length", "0"))
                body = json.loads(self.rfile.read(length) or b"{}")
            except Exception as exc:
                return self._send_json({"error": f"bad request: {exc}"}, status=400)
            project_path = (body.get("project_path") or "").strip()
            session_id = (body.get("session_id") or "").strip()
            turn = int(body.get("turn") or 0)
            if not project_path or not session_id or turn <= 0:
                return self._send_json({"error": "project_path, session_id, turn required"}, status=400)
            try:
                if undo_dapp_snapshot(project_path, session_id, turn):
                    return self._send_json({"ok": True, "turn": turn})
            except Exception as exc:
                return self._send_json({"error": str(exc)}, status=400)
            return self._send_json({"error": "snapshot not found"}, status=404)
        if path == "/api/projects":
            try:
                length = int(self.headers.get("Content-Length", "0"))
                body = json.loads(self.rfile.read(length) or b"{}")
            except Exception as exc:
                return self._send_json({"error": f"bad request: {exc}"}, status=400)
            if body.get("auto"):
                try:
                    project = ensure_default_project()
                except ValueError as exc:
                    return self._send_json({"error": str(exc)}, status=400)
                return self._send_json(project, status=201)
            project_path = (body.get("path") or "").strip()
            if not project_path:
                return self._send_json({"error": "path is required"}, status=400)
            try:
                project = add_project(project_path, body.get("name"))
            except ValueError as exc:
                return self._send_json({"error": str(exc)}, status=400)
            return self._send_json(project, status=201)
        if path == "/api/shutdown":
            self._send_json({"ok": True, "message": "neura shutting down"})
            try:
                self.wfile.flush()
            except Exception:
                pass
            # Defer slightly so this response reaches the browser before we die.
            threading.Timer(0.35, shutdown_everything).start()
            return
        if path != "/api/chat":
            return self.send_error(404, "Unknown Neura API endpoint")
        try:
            length = int(self.headers.get("Content-Length", "0"))
            body = json.loads(self.rfile.read(length) or b"{}")
        except Exception as exc:
            return self._send_json({"error": f"bad request: {exc}"}, status=400)

        message = (body.get("message") or "").strip()
        if not message:
            return self._send_json({"error": "message is required"}, status=400)
        session_id = body.get("session_id") or None
        working_dir = (body.get("working_dir") or body.get("project_path") or "").strip() or None
        if not working_dir:
            working_dir = resolve_project_path(body.get("project_id"), None)
        if not working_dir and not session_id:
            try:
                working_dir = ensure_default_project()["path"]
            except Exception:
                working_dir = default_workspace_path()

        result = run_chat_turn(session_id, message, working_dir=working_dir)
        status = 200 if "error" not in result else 502
        return self._send_json(result, status=status)


def ensure_built() -> None:
    if (DIST_DIR / "index.html").exists():
        return
    subprocess.check_call(["npm", "install"], cwd=UI_DIR)
    subprocess.check_call(["npm", "run", "build"], cwd=UI_DIR)


def detach_stdout_from_caller() -> None:
    """Parent may close our stdout pipe after reading NEURA_UI_URL; keep running."""
    try:
        sys.stdout.flush()
    except BrokenPipeError:
        pass
    try:
        with open(os.devnull, "w") as devnull:
            os.dup2(devnull.fileno(), 1)
    except OSError:
        pass


def find_port(preferred: int) -> int:
    for port in range(preferred, preferred + 20):
        with socket.socket() as sock:
            # Match ThreadingHTTPServer.allow_reuse_address so a port lingering
            # in TIME_WAIT (e.g. right after a restart) is not falsely skipped —
            # otherwise the server drifts to 8769 and the browser can't find it.
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            try:
                sock.bind(("127.0.0.1", port))
                return port
            except OSError:
                continue
    raise RuntimeError("No free localhost port found")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=int(os.environ.get("NEURA_UI_PORT", "8768")))
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--open", action="store_true", help="Open the UI in the default browser")
    parser.add_argument("--voice", action="store_true", help="Open the UI with voice capture enabled")
    args = parser.parse_args()

    if not args.no_build:
        ensure_built()
    port = find_port(args.port)
    url = f"http://{args.host}:{port}"
    if args.voice:
        url += "?voice=1"
    print(f"NEURA_UI_URL={url}", flush=True)
    detach_stdout_from_caller()
    if args.open:
        threading.Timer(0.2, lambda: webbrowser.open(url)).start()
    initialize_live_updates()
    prewarm_subtext_sidecar()
    ensure_dapp_library()
    ensure_dapp_themes()
    bin_path = neura_bin()
    if Path(bin_path).is_file():
        print(f"NEURA_BIN={bin_path}", flush=True)
    else:
        print(f"NEURA_BIN_MISSING={bin_path}", flush=True)
    httpd = ThreadingHTTPServer((args.host, port), Handler)
    httpd.serve_forever()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
