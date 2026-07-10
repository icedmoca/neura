"""Dapp intelligence: validation, snapshots, diff, matching, bootstrap, chat context."""
from __future__ import annotations

import difflib
import hashlib
import json
import math
import re
import time
from collections import Counter
from pathlib import Path
from typing import Any, Callable

DAPP_CONTEXT_MARKER = "[Neura UI — live dapp preview context]"
MAX_DAPP_FILE_BYTES = 500_000

DAPP_BOOTSTRAP_WEATHER = """// NEURA live weather bootstrap
(function () {
  const root = document.getElementById("neura-live") || document.body;
  const loc = window.__NEURA_BOOTSTRAP__ && window.__NEURA_BOOTSTRAP__.location;
  const box = document.createElement("section");
  box.className = "neura-live neura-live--weather";
  if (!loc) {
    box.innerHTML = "<p class=\\"neura-live__hint\\">Mention a city in chat to load live weather.</p>";
    root.prepend(box);
    return;
  }
  box.innerHTML = "<p class=\\"neura-live__loading\\">Loading live weather…</p>";
  root.prepend(box);
  fetch("https://wttr.in/" + encodeURIComponent(loc) + "?format=j1")
    .then((r) => r.json())
    .then((data) => {
      const cur = (data.current_condition && data.current_condition[0]) || {};
      const days = (data.weather || []).slice(0, 3);
      box.innerHTML =
        "<h2>Live · " + loc + "</h2>" +
        "<p class=\\"neura-live__now\\">" + (cur.temp_F || "?") + "°F · " + (cur.weatherDesc && cur.weatherDesc[0].value || "") + "</p>" +
        days.map((d) => "<div class=\\"neura-live__day\\">" + d.date + ": " + d.mintempF + "–" + d.maxtempF + "°F</div>").join("");
    })
    .catch(() => { box.innerHTML = "<p class=\\"neura-live__err\\">Weather unavailable</p>"; });
})();
"""

DAPP_BOOTSTRAP_FINANCE = """// NEURA live quote bootstrap
(function () {
  const sym = window.__NEURA_BOOTSTRAP__ && window.__NEURA_BOOTSTRAP__.symbol;
  const root = document.getElementById("neura-live") || document.body;
  const box = document.createElement("section");
  box.className = "neura-live neura-live--finance";
  if (!sym) {
    box.innerHTML = "<p class=\\"neura-live__hint\\">Mention a ticker or crypto symbol in chat.</p>";
    root.prepend(box);
    return;
  }
  box.innerHTML = "<p>Loading " + sym + "…</p>";
  root.prepend(box);
  fetch("https://query1.finance.yahoo.com/v8/finance/chart/" + encodeURIComponent(sym))
    .then((r) => r.json())
    .then((data) => {
      const meta = data.chart && data.chart.result && data.chart.result[0] && data.chart.result[0].meta;
      if (!meta) throw new Error("no data");
      box.innerHTML = "<h2>" + sym + "</h2><p class=\\"neura-live__price\\">" + (meta.regularMarketPrice || "?") + " " + (meta.currency || "") + "</p>";
    })
    .catch(() => { box.innerHTML = "<p>Quote unavailable for " + sym + "</p>"; });
})();
"""

DAPP_BOOTSTRAP_REACT = """<!-- NEURA React sandbox (CDN) -->
<script crossorigin src="https://unpkg.com/react@18/umd/react.production.min.js"></script>
<script crossorigin src="https://unpkg.com/react-dom@18/umd/react-dom.production.min.js"></script>
<script src="https://unpkg.com/@babel/standalone/babel.min.js"></script>
<div id="neura-react-root"></div>
"""


def keyword_vector(keywords: list[str]) -> dict[str, float]:
    counts = Counter(keywords)
    total = sum(counts.values()) or 1
    return {word: count / total for word, count in counts.items()}


def cosine_similarity(a: dict[str, float], b: dict[str, float]) -> float:
    if not a or not b:
        return 0.0
    keys = set(a) | set(b)
    dot = sum(a.get(k, 0.0) * b.get(k, 0.0) for k in keys)
    na = math.sqrt(sum(v * v for v in a.values()))
    nb = math.sqrt(sum(v * v for v in b.values()))
    if na == 0 or nb == 0:
        return 0.0
    return dot / (na * nb)


def summarize_dapp_files(files: dict[str, str], *, title: str | None = None, category: str | None = None) -> str:
    parts: list[str] = []
    if title:
        parts.append(title)
    elif category:
        parts.append(f"a {category} interface")
    html = files.get("index.html", "")
    headings = re.findall(r"<h[12][^>]*>([^<]{3,80})</h[12]>", html, re.I)
    if headings:
        parts.append("showing " + ", ".join(headings[:3]))
    if category == "weather" and "weather" not in " ".join(parts).lower():
        parts.append("with weather/forecast content")
    if not parts:
        parts.append("a contextual project dashboard")
    return "The user currently sees " + " ".join(parts) + " in the live dapp preview panel behind chat."


def build_dapp_chat_context(
    files: dict[str, str],
    *,
    template_title: str | None = None,
    category: str | None = None,
) -> str:
    if not files or not any(files.values()):
        return ""
    summary = summarize_dapp_files(files, title=template_title, category=category)
    return (
        f"{DAPP_CONTEXT_MARKER}\n"
        f"{summary}\n"
        "Reference this preview naturally when helpful (e.g. \"check the panel behind chat\"). "
        "Do not repeat this note verbatim to the user.\n"
        "The dapp lives at `.neura/dapp/` in this project; you may edit it with file tools when the user asks to change the UI.\n"
        "Auto-generation may also update the dapp — prefer editing the existing fork over rebuilding from scratch.\n\n"
        "[User message]\n"
    )


def inject_dapp_context(message: str, context_prefix: str) -> str:
    if not context_prefix:
        return message
    return context_prefix + message


def strip_reasoning_leak(text: str) -> str:
    """Remove model reasoning that some local servers fuse into the answer.

    gpt-oss/Harmony models emit an `analysis` channel before the `final`
    channel; when the serving template doesn't split channels the answer
    arrives as one blob ("…analysis…assistantfinalAnswer" or with literal
    `<|channel|>final<|message|>` markers). Keep only the final channel when
    such a marker is present; also drop 💭-prefixed legacy thinking lines.
    """
    if not text:
        return text
    explicit = "<|channel|>final<|message|>"
    idx = text.rfind(explicit)
    if idx >= 0:
        text = text[idx + len(explicit):].lstrip()
    else:
        # The stripped-token leak form glues "assistantfinal" directly onto
        # the end of the analysis ("…concisely.assistantfinalAnswer"). Only
        # treat it as a marker when it is glued (non-whitespace before it),
        # so ordinary prose containing the word sequence survives.
        idx = text.rfind("assistantfinal")
        if idx > 0 and not text[idx - 1].isspace():
            text = text[idx + len("assistantfinal"):].lstrip()
    if "💭" in text:
        kept = [line for line in text.splitlines() if not line.lstrip().startswith("💭")]
        text = "\n".join(kept).strip()
    return text


def sanitize_chat_display_text(text: str) -> str:
    """Strip internal Neura UI prefixes from messages shown in chat."""
    if not text:
        return text
    text = strip_reasoning_leak(text)
    if "You generate contextual Neura project dapp files from chat transcripts" in text:
        if "Reply with ONLY valid JSON" in text:
            return ""
    if DAPP_CONTEXT_MARKER in text:
        user_tag = "[User message]\n"
        tag_idx = text.find(user_tag)
        if tag_idx >= 0:
            return text[tag_idx + len(user_tag) :].strip()
        # Assistant echo of injected preview context — hide from chat UI.
        if "no task/request came through" in text.lower():
            return ""
        if "what would you like me to change or build in the preview" in text.lower():
            return ""
        return ""
    return text


SIMPLE_ARITHMETIC_RE = re.compile(
    r"(?:what'?s?|whats|calculate|compute|solve)?\s*(\d+)\s*([+\-*/×x])\s*(\d+)",
    re.I,
)


def parse_simple_arithmetic(text: str) -> tuple[str, int] | None:
    match = SIMPLE_ARITHMETIC_RE.search((text or "").strip())
    if not match:
        return None
    left, op, right = int(match.group(1)), match.group(2), int(match.group(3))
    operators = {
        "+": lambda a, b: a + b,
        "-": lambda a, b: a - b,
        "*": lambda a, b: a * b,
        "×": lambda a, b: a * b,
        "x": lambda a, b: a * b,
        "/": lambda a, b: a // b if b else 0,
    }
    fn = operators.get(op)
    if not fn:
        return None
    expr = f"{left} {op} {right}"
    return expr, fn(left, right)


def build_math_widget_html(expr: str, result: int) -> str:
    return (
        f'<article class="neura-widget neura-widget--card" data-neura-widget="card" data-neura-id="math-result">'
        f'<h2 class="neura-math__expr">{expr}</h2>'
        f'<p class="neura-math__eq">{expr} = <strong>{result}</strong></p>'
        f'<button type="button" class="neura-action" data-neura-action="ask">Ask another</button>'
        f"</article>"
    )


def try_local_math_widget_patch(
    messages: list[dict],
    files: dict[str, str],
    *,
    topic_label: str | None = None,
) -> dict[str, str] | None:
    last_user = ""
    for msg in reversed(messages):
        if msg.get("role") == "user":
            last_user = (msg.get("text") or "").strip()
            break
    parsed = parse_simple_arithmetic(last_user)
    if not parsed:
        return None
    expr, result = parsed
    widget_html = build_math_widget_html(expr, result)
    topic_inner = (
        f'<div class="neura-math">'
        f"{widget_html}"
        f'<p class="neura-widget__placeholder">Quick math widget — updates instantly as you ask.</p>'
        f"</div>"
    )
    patched = apply_ai_shell_wrapper(files or NEURA_AI_SHELL_FILES, topic_label=topic_label or expr)
    index = patched.get("index.html", "")
    patched["index.html"] = index.replace(
        '<p class="neura-widget__placeholder">Dynamic AI widgets appear here as you chat.</p>',
        topic_inner,
    )
    extra_css = """
.neura-math { display: grid; gap: 12px; }
.neura-math__expr { margin: 0; font-size: 28px; letter-spacing: 0.08em; text-transform: uppercase; }
.neura-math__eq { margin: 0; font-size: 20px; }
"""
    patched["style.css"] = patched.get("style.css", "") + extra_css
    return patched


def validate_dapp_files(files: dict[str, str]) -> tuple[bool, str]:
    if not files:
        return False, "empty files"
    if "index.html" not in files:
        return False, "missing index.html"
    html = files.get("index.html", "")
    if len(html.strip()) < 40:
        return False, "index.html too short"
    if "<html" not in html.lower() and "<body" not in html.lower() and "<main" not in html.lower():
        return False, "index.html missing html/body/main structure"
    for path, content in files.items():
        if len(content.encode("utf-8")) > MAX_DAPP_FILE_BYTES:
            return False, f"{path} exceeds size limit"
        if ".." in path.split("/"):
            return False, f"invalid path {path}"
    return True, "ok"


def compute_dapp_diff(before: dict[str, str], after: dict[str, str]) -> dict[str, Any]:
    all_paths = sorted(set(before) | set(after))
    files_diff: dict[str, Any] = {}
    for path in all_paths:
        old = before.get(path, "")
        new = after.get(path, "")
        if old == new:
            continue
        lines = list(
            difflib.unified_diff(
                old.splitlines(keepends=True),
                new.splitlines(keepends=True),
                fromfile=f"{path} (before)",
                tofile=f"{path} (after)",
                lineterm="",
            )
        )
        files_diff[path] = {
            "unified": "".join(lines),
            "beforeSize": len(old),
            "afterSize": len(new),
        }
    return {
        "changedFiles": list(files_diff.keys()),
        "files": files_diff,
        "summary": f"{len(files_diff)} file(s) changed",
    }


def snapshot_fork(fork_dir: Path, turn: int, *, meta: dict[str, Any] | None = None) -> Path:
    snap_root = fork_dir / "snapshots"
    snap_root.mkdir(parents=True, exist_ok=True)
    snap_dir = snap_root / f"turn_{turn:04d}"
    snap_dir.mkdir(parents=True, exist_ok=True)
    for item in fork_dir.iterdir():
        if not item.is_file() or item.name == "meta.json":
            continue
        if item.parent.name == "snapshots":
            continue
        (snap_dir / item.name).write_text(item.read_text(errors="replace"))
    payload = {"turn": turn, "createdAt": time.time(), **(meta or {})}
    (snap_dir / "snapshot.json").write_text(json.dumps(payload, indent=2))
    index_path = snap_root / "index.json"
    history: list[dict] = []
    if index_path.is_file():
        try:
            history = json.loads(index_path.read_text()).get("snapshots") or []
        except Exception:
            history = []
    history.append({"turn": turn, "createdAt": payload["createdAt"], "meta": meta or {}})
    index_path.write_text(json.dumps({"snapshots": history[-50:]}, indent=2))
    return snap_dir


def list_fork_snapshots(fork_dir: Path) -> list[dict]:
    index_path = fork_dir / "snapshots" / "index.json"
    if not index_path.is_file():
        return []
    try:
        return json.loads(index_path.read_text()).get("snapshots") or []
    except Exception:
        return []


def restore_fork_snapshot(fork_dir: Path, turn: int) -> bool:
    snap_dir = fork_dir / "snapshots" / f"turn_{turn:04d}"
    if not snap_dir.is_dir():
        return False
    for item in snap_dir.iterdir():
        if item.name in ("snapshot.json",):
            continue
        if item.is_file():
            (fork_dir / item.name).write_text(item.read_text(errors="replace"))
    return True


def inject_live_bootstrap(
    files: dict[str, str],
    category: str | None,
    keywords: list[str],
    *,
    messages: list[dict] | None = None,
    chat_title: str | None = None,
) -> dict[str, str]:
    import dapp_context as dc

    out = dict(files)
    html = out.get("index.html", "")
    js = out.get("app.js", "")
    bootstrap = (
        dc.bootstrap_payload(messages or [], chat_title=chat_title, category=category)
        if messages
        else {}
    )
    if category == "weather" and bootstrap.get("location"):
        if "neura-live" not in html and "<body" in html.lower():
            html = re.sub(r"(<body[^>]*>)", r"\1<div id=\"neura-live\"></div>", html, count=1, flags=re.I)
        if "NEURA live weather bootstrap" not in js:
            js = DAPP_BOOTSTRAP_WEATHER + "\n" + js
    elif category == "finance" and bootstrap.get("symbol"):
        if "neura-live" not in html and "<body" in html.lower():
            html = re.sub(r"(<body[^>]*>)", r"\1<div id=\"neura-live\"></div>", html, count=1, flags=re.I)
        if "NEURA live quote bootstrap" not in js:
            js = DAPP_BOOTSTRAP_FINANCE + "\n" + js
    if bootstrap:
        inject = f"window.__NEURA_BOOTSTRAP__ = {json.dumps(bootstrap)};\n"
        if "__NEURA_BOOTSTRAP__" not in js:
            js = inject + js
    if category in ("tasks", "finance", "weather", "music", "video") and "react.production.min.js" not in html:
        if "<head>" in html.lower() and "neura-react-root" not in html:
            html = html.replace("<head>", "<head>" + DAPP_BOOTSTRAP_REACT, 1)
    out["index.html"] = html
    out["app.js"] = js
    return out


def enrich_library_entry(entry: dict, keywords: list[str]) -> dict:
    entry = dict(entry)
    entry["keywordVector"] = keyword_vector(keywords)
    return entry


def score_template_tfidf(query_keywords: list[str], template: dict) -> float:
    query_vec = keyword_vector(query_keywords)
    stored = template.get("keywordVector")
    if isinstance(stored, dict) and stored:
        tpl_vec = {k: float(v) for k, v in stored.items()}
    else:
        tpl_vec = keyword_vector(list(template.get("keywords") or []))
    return cosine_similarity(query_vec, tpl_vec)


def build_prompt_overlay(summary: str, category: str | None) -> str:
    lines = [
        "# Neura UI Dapp (auto-maintained)",
        "",
        summary,
        "",
        "When the user asks to change the dapp, preview, or panel UI:",
        "- Edit files under `.neura/dapp/` (and the session fork under `.neura/dapp-forks/`).",
        "- Make minimal targeted edits; do not wipe unrelated sections.",
        "- The UI auto-syncs dapp files from chat context; avoid fighting auto-generation.",
    ]
    if category:
        lines.append(f"- Current dapp category: **{category}**.")
    return "\n".join(lines) + "\n"


NEURA_AI_SHELL_ID = "neura-ai"

NEURA_BRIDGE_JS = """/* NEURA host ↔ dapp bridge (do not remove) */
(function () {
  const listeners = [];
  window.NEURA_BRIDGE = {
    sendAction(intent, payload) {
      parent.postMessage({ source: "neura-dapp", type: "dapp_action", intent, payload: payload || {} }, "*");
    },
    sendChat(text) {
      parent.postMessage({ source: "neura-dapp", type: "dapp_chat", text: String(text || "") }, "*");
    },
    sendWidgetAction(widgetId, action, payload) {
      parent.postMessage({
        source: "neura-dapp",
        type: "widget_action",
        widgetId: String(widgetId || ""),
        action: String(action || ""),
        payload: payload || {},
      }, "*");
    },
    requestTheme(themeId) {
      parent.postMessage({ source: "neura-dapp", type: "theme_request", themeId: String(themeId || "") }, "*");
    },
    onHostMessage(handler) {
      if (typeof handler === "function") listeners.push(handler);
    },
  };
  window.addEventListener("message", (event) => {
    const data = event.data;
    if (!data || data.source !== "neura-host") return;
    listeners.forEach((handler) => {
      try { handler(data); } catch (_) { /* ignore */ }
    });
  });
  parent.postMessage({ source: "neura-dapp", type: "dapp_ready" }, "*");
})();
"""

DEFAULT_DAPP_THEMES: list[dict] = [
    {
        "id": "brutalist-bw",
        "title": "Brutalist B&W",
        "description": "Sharp monochrome Neura default",
        "category": "minimal",
        "pinned": True,
        "vars": {
            "--neura-bg": "#000000",
            "--neura-fg": "#ffffff",
            "--neura-muted": "#737373",
            "--neura-border": "#404040",
            "--neura-accent": "#ffffff",
            "--neura-card-bg": "#0a0a0a",
            "--neura-radius": "0px",
            "--neura-font": "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
        },
    },
    {
        "id": "neon-terminal",
        "title": "Neon Terminal",
        "description": "Green phosphor cyberdeck",
        "category": "retro",
        "pinned": False,
        "vars": {
            "--neura-bg": "#020805",
            "--neura-fg": "#39ff14",
            "--neura-muted": "#1a6630",
            "--neura-border": "#145522",
            "--neura-accent": "#39ff14",
            "--neura-card-bg": "#041208",
            "--neura-radius": "0px",
            "--neura-font": "ui-monospace, monospace",
        },
    },
    {
        "id": "paper-minimal",
        "title": "Paper Minimal",
        "description": "Light editorial cards",
        "category": "light",
        "pinned": False,
        "vars": {
            "--neura-bg": "#f5f2eb",
            "--neura-fg": "#1a1a1a",
            "--neura-muted": "#6b6560",
            "--neura-border": "#d4cfc4",
            "--neura-accent": "#1a1a1a",
            "--neura-card-bg": "#ffffff",
            "--neura-radius": "2px",
            "--neura-font": "Georgia, 'Times New Roman', serif",
        },
    },
    {
        "id": "midnight-glass",
        "title": "Midnight Glass",
        "description": "Soft dark panels with glow",
        "category": "modern",
        "pinned": False,
        "vars": {
            "--neura-bg": "#070b14",
            "--neura-fg": "#e8eefc",
            "--neura-muted": "#7b879f",
            "--neura-border": "#243047",
            "--neura-accent": "#6ea8ff",
            "--neura-card-bg": "#0d1424",
            "--neura-radius": "8px",
            "--neura-font": "system-ui, sans-serif",
        },
    },
]

NEURA_WIDGET_JS = """/* NEURA immersive widget runtime — chat woven into the dapp canvas */
(function () {
  function esc(text) {
    return String(text || "")
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;");
  }

  const label = document.getElementById("neura-topic-label");
  const themeLabel = document.getElementById("neura-theme-label");
  const grid = document.getElementById("neura-widget-grid");
  const chatStream = document.getElementById("neura-chat-stream");
  const composerDock = document.getElementById("neura-composer-dock");
  let state = { immersive: false, sending: false, messages: [], topic: "", components: [] };

  function applyTheme(themeId, vars) {
    if (themeId) document.body.dataset.neuraTheme = themeId;
    if (themeLabel && themeId) themeLabel.textContent = themeId;
    Object.entries(vars || {}).forEach(([key, value]) => {
      document.documentElement.style.setProperty(key, String(value));
    });
  }

  function renderComponentRail(ids) {
    const rail = document.getElementById("neura-component-rail");
    if (!rail || !ids || !ids.length) return;
    const badges = ids.map((id) => {
      const label = String(id || "").replace(/^comp-/, "").replace(/-/g, " ");
      return `<span class="neura-component-badge">${esc(label)}</span>`;
    }).join("");
    if (!rail.querySelector(".neura-component-badge")) {
      rail.insertAdjacentHTML("beforeend", badges);
    }
  }

  function bindComposer(root) {
    const form = root && root.querySelector("#neura-chat-form");
    const input = root && root.querySelector("#neura-chat-input");
    if (!form || !input) return;
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      const text = input.value.trim();
      if (!text || !window.NEURA_BRIDGE) return;
      input.value = "";
      window.NEURA_BRIDGE.sendChat(text);
    });
  }

  function renderImmersiveChat() {
    document.body.classList.toggle("neura-ai-shell--immersive", Boolean(state.immersive));
    if (!state.immersive) {
      if (composerDock) composerDock.hidden = true;
      return;
    }
    if (composerDock) {
      composerDock.hidden = false;
      composerDock.innerHTML = `
        <form class="neura-composer-dock__form" id="neura-chat-form">
          <div class="neura-composer-dock__glow"></div>
          <textarea id="neura-chat-input" rows="1" placeholder="${state.sending ? "Neura is thinking…" : "Talk to the canvas…"}"></textarea>
          <button type="submit" aria-label="Send">↑</button>
        </form>`;
      bindComposer(composerDock);
    }
    if (!chatStream) return;
    const msgs = state.messages || [];
    const cards = msgs.map((m, i) => {
      const role = m.role === "user" ? "user" : "assistant";
      return `<article class="neura-widget neura-widget--msg neura-widget--msg-${role}" data-neura-widget="msg-${role}" data-neura-id="msg-${i}">
        <header class="neura-msg__head">${role === "user" ? "You" : "Neura"}</header>
        <div class="neura-msg__body">${esc(m.text)}</div>
      </article>`;
    }).join("");
    const typing = state.sending
      ? '<article class="neura-widget neura-widget--msg neura-widget--msg-assistant neura-widget--typing"><div class="neura-msg__body">…</div></article>'
      : "";
    chatStream.innerHTML = cards + typing;
    chatStream.scrollTop = chatStream.scrollHeight;
    renderComponentRail(state.components || []);
  }

  function upsertHostWidgets(widgets) {
    if (!grid || !Array.isArray(widgets)) return;
    widgets.forEach((widget) => {
      if (!widget || !widget.id || widget.kind === "topic" || widget.kind === "chat") return;
      let node = grid.querySelector(`[data-neura-id="${widget.id}"]`);
      if (!node) {
        node = document.createElement("article");
        node.dataset.neuraId = widget.id;
        const stream = document.getElementById("neura-chat-stream");
        grid.insertBefore(node, stream || null);
      }
      node.className = `neura-widget neura-widget--${widget.kind || "card"}`;
      node.dataset.neuraWidget = widget.kind || "card";
      node.innerHTML = widget.html || "";
    });
  }

  function handleHostMessage(msg) {
    if (msg.type === "theme_apply") applyTheme(msg.themeId, msg.vars);
    if (msg.type === "chat_update" || msg.type === "sync_state") {
      if (msg.topic && label) label.textContent = msg.topic;
      state = {
        immersive: Boolean(msg.immersive ?? msg.chatInline ?? state.immersive),
        sending: Boolean(msg.sending),
        messages: msg.messages || state.messages || [],
        topic: msg.topic || state.topic || "",
        components: msg.components || msg.activeComponents || state.components || [],
      };
      renderImmersiveChat();
    }
    if (msg.type === "topic_html" && msg.html) {
      const panel = document.getElementById("neura-topic-panel");
      if (panel) panel.innerHTML = msg.html;
    }
    if (msg.type === "widgets_update" && msg.widgets) upsertHostWidgets(msg.widgets);
    if (msg.type === "sync_state" && msg.widgets) upsertHostWidgets(msg.widgets);
    if (msg.type === "components_active" && msg.components) {
      state.components = msg.components;
      renderComponentRail(state.components);
    }
  }

  document.querySelectorAll("[data-neura-action]").forEach((node) => {
    node.addEventListener("click", () => {
      if (!window.NEURA_BRIDGE) return;
      const widgetId = node.closest("[data-neura-id]")?.dataset.neuraId || "";
      window.NEURA_BRIDGE.sendWidgetAction(widgetId, node.dataset.neuraAction || "click", {
        label: node.textContent || "",
      });
    });
  });

  if (window.NEURA_BRIDGE) {
    window.NEURA_BRIDGE.onHostMessage(handleHostMessage);
  }
})();
"""

NEURA_AI_SHELL_FILES: dict[str, str] = {
    "index.html": """<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>NEURA · AI</title>
  <link rel="stylesheet" href="style.css" />
</head>
<body class="neura-ai-shell" data-neura-theme="brutalist-bw">
  <header class="neura-ai-shell__head">
    <span class="neura-ai-shell__brand">NEURA</span>
    <span id="neura-topic-label" class="neura-ai-shell__topic">AI Workspace</span>
    <span id="neura-theme-label" class="neura-ai-shell__theme"></span>
  </header>
  <main class="neura-canvas">
    <div id="neura-component-rail" class="neura-component-rail"></div>
    <div id="neura-widget-grid" class="neura-widget-grid">
      <section id="neura-topic-panel" class="neura-widget neura-widget--topic" data-neura-widget="topic">
        <p class="neura-widget__placeholder">Dynamic AI widgets appear here as you chat.</p>
      </section>
      <section id="neura-chat-stream" class="neura-chat-stream" aria-label="Immersive chat stream"></section>
    </div>
    <div id="neura-composer-dock" class="neura-composer-dock" hidden aria-label="Immersive composer"></div>
  </main>
  <script src="neura-bridge.js"></script>
  <script src="neura-widget.js"></script>
  <script src="app.js"></script>
</body>
</html>
""",
    "style.css": """* { box-sizing: border-box; }
:root {
  --neura-bg: #000;
  --neura-fg: #fff;
  --neura-muted: #737373;
  --neura-border: #404040;
  --neura-accent: #fff;
  --neura-card-bg: #0a0a0a;
  --neura-radius: 0px;
  --neura-font: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
}
body.neura-ai-shell {
  margin: 0;
  min-height: 100vh;
  font-family: var(--neura-font);
  background: var(--neura-bg);
  color: var(--neura-fg);
  display: flex;
  flex-direction: column;
}
.neura-ai-shell__head {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 16px;
  border-bottom: 1px solid var(--neura-border);
}
.neura-ai-shell__brand {
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.14em;
  text-transform: uppercase;
}
.neura-ai-shell__topic { font-size: 11px; color: var(--neura-muted); letter-spacing: 0.06em; }
.neura-ai-shell__theme { margin-left: auto; font-size: 10px; color: var(--neura-muted); text-transform: uppercase; }
.neura-canvas {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-height: 0;
}
.neura-widget-grid {
  flex: 1;
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
  gap: 12px;
  padding: 16px;
  overflow: auto;
  align-content: start;
}
.neura-widget {
  border: 1px solid var(--neura-border);
  background: var(--neura-card-bg);
  border-radius: var(--neura-radius);
  padding: 12px;
}
.neura-widget--topic { grid-column: 1 / -1; }
.neura-component-rail {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  padding: 8px 16px 0;
}
.neura-component-badge {
  font-size: 9px;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  padding: 4px 8px;
  border: 1px solid var(--neura-border);
  color: var(--neura-muted);
}
.neura-chat-stream {
  grid-column: 1 / -1;
  display: flex;
  flex-direction: column;
  gap: 10px;
  max-height: 50vh;
  overflow: auto;
  padding-top: 4px;
}
body.neura-ai-shell--immersive .neura-chat-stream { max-height: none; flex: 1; }
.neura-widget--msg { padding: 10px 12px; }
.neura-widget--msg-user { margin-left: 12%; border-color: color-mix(in srgb, var(--neura-accent) 40%, var(--neura-border)); }
.neura-widget--msg-assistant { margin-right: 8%; }
.neura-msg__head { font-size: 9px; letter-spacing: 0.12em; text-transform: uppercase; color: var(--neura-muted); margin-bottom: 6px; }
.neura-msg__body { font-size: 13px; line-height: 1.55; white-space: pre-wrap; }
.neura-widget--typing { opacity: 0.7; }
.neura-composer-dock {
  position: sticky;
  bottom: 0;
  padding: 12px 16px 16px;
  background: linear-gradient(transparent, var(--neura-bg) 30%);
  z-index: 5;
}
.neura-composer-dock__form {
  position: relative;
  display: flex;
  gap: 8px;
  align-items: flex-end;
  padding: 10px 12px;
  border: 1px solid var(--neura-border);
  background: color-mix(in srgb, var(--neura-card-bg) 88%, transparent);
  backdrop-filter: blur(8px);
  border-radius: var(--neura-radius);
}
.neura-composer-dock__form textarea {
  flex: 1;
  min-height: 44px;
  max-height: 120px;
  border: none;
  background: transparent;
  color: var(--neura-fg);
  font: inherit;
  font-size: 13px;
  resize: none;
  outline: none;
}
.neura-composer-dock__form button {
  width: 36px;
  height: 36px;
  border: 1px solid var(--neura-accent);
  background: var(--neura-accent);
  color: var(--neura-bg);
  cursor: pointer;
  font-size: 16px;
  line-height: 1;
}
body.neura-ai-shell--immersive .neura-ai-shell__head {
  background: color-mix(in srgb, var(--neura-bg) 92%, transparent);
  backdrop-filter: blur(6px);
}
.neura-widget__placeholder { color: var(--neura-muted); font-size: 12px; line-height: 1.6; margin: 0; }
.neura-live { margin-bottom: 12px; padding: 12px; border: 1px solid var(--neura-border); }
button, .neura-widget button {
  padding: 8px 12px;
  border: 1px solid var(--neura-accent);
  background: var(--neura-accent);
  color: var(--neura-bg);
  font: inherit;
  cursor: pointer;
}
button.neura-action { margin-top: 8px; margin-right: 8px; }
""",
    "app.js": """/* Custom topic widget hooks — use NEURA_BRIDGE.sendChat() / sendWidgetAction() */
(function () {
  document.querySelectorAll("[data-neura-action]").forEach((node) => {
    node.addEventListener("click", () => {
      if (!window.NEURA_BRIDGE) return;
      const widgetId = node.closest("[data-neura-id]")?.dataset.neuraId || "";
      window.NEURA_BRIDGE.sendWidgetAction(widgetId, node.dataset.neuraAction || "click", {
        label: node.textContent || "",
      });
    });
  });
})();
""",
    "neura-bridge.js": NEURA_BRIDGE_JS,
    "neura-widget.js": NEURA_WIDGET_JS,
    "README.md": """# NEURA AI Shell

Composable dapp canvas with inline chat widget + dynamic AI widgets.
Do not remove `neura-bridge.js`, `neura-widget.js`, or `#neura-widget-grid`.
""",
}


def theme_by_id(theme_id: str) -> dict | None:
    for theme in DEFAULT_DAPP_THEMES:
        if theme.get("id") == theme_id:
            return dict(theme)
    return None


def is_ai_shell_html(html: str) -> bool:
    lower = (html or "").lower()
    return "neura-ai-shell" in lower and "neura-widget-grid" in lower


def extract_topic_html(html: str) -> str:
    lower = html.lower()
    marker = 'id="neura-topic-panel"'
    idx = lower.find(marker)
    if idx < 0:
        body_match = re.search(r"<body[^>]*>(.*)</body>", html, re.I | re.S)
        return body_match.group(1).strip() if body_match else html
    chunk = html[idx:]
    open_end = chunk.find(">")
    if open_end < 0:
        return html
    inner = chunk[open_end + 1 :]
    close = inner.lower().find("</section>")
    if close < 0:
        close = inner.lower().find("</main>")
    return inner[:close].strip() if close >= 0 else inner.strip()


def _replace_topic_label(html: str, topic_label: str) -> str:
    def repl(match: re.Match[str]) -> str:
        return f"{match.group(1)}{topic_label}{match.group(3)}"

    return re.sub(
        r'(<span id="neura-topic-label"[^>]*>)(.*?)(</span>)',
        repl,
        html,
        count=1,
        flags=re.S,
    )


def _replace_body_theme(html: str, theme_id: str) -> str:
    def repl(match: re.Match[str]) -> str:
        return f'{match.group(1)} data-neura-theme="{theme_id}">'

    return re.sub(r"(<body[^>]*)>", repl, html, count=1, flags=re.I)


def apply_ai_shell_wrapper(files: dict[str, str], *, topic_label: str | None = None) -> dict[str, str]:
    """Wrap generated content in the NEURA AI shell when needed."""
    out = dict(NEURA_AI_SHELL_FILES)
    index = files.get("index.html", "")
    if is_ai_shell_html(index):
        out.update({
            k: v for k, v in files.items()
            if k in ("index.html", "style.css", "app.js", "neura-bridge.js", "neura-widget.js")
        })
        if topic_label and 'id="neura-topic-label"' in out.get("index.html", ""):
            out["index.html"] = _replace_topic_label(out["index.html"], topic_label)
        return out
    topic_inner = extract_topic_html(index)
    shell_index = out["index.html"]
    shell_index = shell_index.replace(
        '<p class="neura-widget__placeholder">Dynamic AI widgets appear here as you chat.</p>',
        topic_inner or '<p class="neura-widget__placeholder">Dynamic AI widgets appear here as you chat.</p>',
    )
    if topic_label:
        shell_index = _replace_topic_label(shell_index, topic_label)
    out["index.html"] = shell_index
    if files.get("style.css"):
        out["style.css"] = NEURA_AI_SHELL_FILES["style.css"] + "\n\n/* --- topic styles --- */\n" + files["style.css"]
    if files.get("app.js"):
        out["app.js"] = NEURA_AI_SHELL_FILES["app.js"] + "\n" + files["app.js"]
    return out
