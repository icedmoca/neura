"""Premade dapp component registry — instant compose before LLM."""
from __future__ import annotations

from typing import Any

import dapp_context as dc
import dapp_engine as de

DEFAULT_DAPP_COMPONENTS: list[dict[str, Any]] = [
    {
        "id": "comp-math-hero",
        "title": "Math Hero",
        "category": "math",
        "kind": "panel",
        "pinned": True,
        "keywords": ["math", "plus", "minus", "multiply", "divide", "calculate", "sum", "equation"],
        "html": (
            '<article class="neura-widget neura-widget--panel neura-comp neura-comp--math" '
            'data-neura-widget="panel" data-neura-id="math-hero">'
            '<p class="neura-comp__label">Calculator</p>'
            '<h2 class="neura-comp__hero">{{expr}}</h2>'
            '<p class="neura-comp__result">{{expr}} = <strong>{{result}}</strong></p>'
            "</article>"
        ),
    },
    {
        "id": "comp-weather-live",
        "title": "Live Weather",
        "category": "weather",
        "kind": "panel",
        "pinned": True,
        "keywords": ["weather", "forecast", "temperature", "rain", "snow", "humidity", "wind", "clima"],
        "html": (
            '<article class="neura-widget neura-widget--panel neura-comp neura-comp--weather" '
            'data-neura-widget="panel" data-neura-id="weather-live">'
            '<p class="neura-comp__label">Live weather · {{location_display}}</p>'
            '<div id="neura-live" class="neura-live">{{weather_body}}</div>'
            "</article>"
        ),
        "bootstrap": "weather",
    },
    {
        "id": "comp-music-card",
        "title": "Music Card",
        "category": "music",
        "kind": "card",
        "keywords": ["song", "music", "lyrics", "spotify", "youtube", "album", "artist", "fox", "listen", "gif", "gifs", "meme"],
        "html": (
            '<article class="neura-widget neura-widget--card neura-comp neura-comp--music" '
            'data-neura-widget="card" data-neura-id="music-card">'
            '<p class="neura-comp__label">Now playing</p>'
            '<h2 class="neura-comp__hero">{{title}}</h2>'
            '<p class="neura-comp__sub">{{subtitle}}</p>'
            '<button type="button" class="neura-action" data-neura-action="ask">Ask about this track</button>'
            "</article>"
        ),
    },
    {
        "id": "comp-finance-ticker",
        "title": "Finance Ticker",
        "category": "finance",
        "kind": "card",
        "keywords": ["stock", "stocks", "crypto", "bitcoin", "market", "ticker", "price", "nasdaq"],
        "html": (
            '<article class="neura-widget neura-widget--card neura-comp neura-comp--finance" '
            'data-neura-widget="card" data-neura-id="finance-ticker">'
            '<p class="neura-comp__label">Market · {{symbol_display}}</p>'
            '<div id="neura-live" class="neura-live">{{finance_body}}</div>'
            "</article>"
        ),
        "bootstrap": "finance",
    },
    {
        "id": "comp-media-gallery",
        "title": "Media Gallery",
        "category": "video",
        "kind": "panel",
        "keywords": ["gif", "gifs", "meme", "memes", "image", "photo", "gallery", "fox", "foxes", "picture", "pictures"],
        "html": (
            '<article class="neura-widget neura-widget--panel neura-comp neura-comp--media" '
            'data-neura-widget="panel" data-neura-id="media-gallery">'
            '<p class="neura-comp__label">Gallery · {{title}}</p>'
            '<div class="neura-comp__body">{{assistant_preview}}</div>'
            '<button type="button" class="neura-action" data-neura-action="ask">Show more in chat</button>'
            "</article>"
        ),
    },
    {
        "id": "comp-assistant-insight",
        "title": "Assistant Insight",
        "category": "chat",
        "kind": "card",
        "pinned": True,
        "keywords": ["answer", "explain", "summary", "insight", "response"],
        "html": (
            '<article class="neura-widget neura-widget--card neura-comp neura-comp--insight" '
            'data-neura-widget="card" data-neura-id="assistant-insight">'
            '<p class="neura-comp__label">Neura</p>'
            '<p class="neura-comp__body">{{assistant_preview}}</p>'
            "</article>"
        ),
    },
    {
        "id": "comp-action-chips",
        "title": "Action Chips",
        "category": "shell",
        "kind": "row",
        "pinned": True,
        "keywords": ["action", "chips", "quick", "suggest"],
        "html": (
            '<nav class="neura-widget neura-widget--row neura-comp neura-comp--chips" '
            'data-neura-widget="row" data-neura-id="action-chips">{{chips}}</nav>'
        ),
    },
    {
        "id": "comp-todo-board",
        "title": "Todo Board",
        "category": "tasks",
        "kind": "panel",
        "keywords": ["todo", "task", "checklist", "reminder", "list"],
        "html": (
            '<article class="neura-widget neura-widget--panel neura-comp neura-comp--todo" '
            'data-neura-widget="panel" data-neura-id="todo-board">'
            '<p class="neura-comp__label">Tasks</p>'
            '<ul class="neura-comp__list">{{todo_items}}</ul>'
            '<button type="button" class="neura-action" data-neura-action="ask">Add a task via chat</button>'
            "</article>"
        ),
    },
    {
        "id": "comp-code-snippet",
        "title": "Code Snippet",
        "category": "code",
        "kind": "panel",
        "keywords": ["code", "function", "python", "javascript", "rust", "debug", "error", "compile"],
        "html": (
            '<article class="neura-widget neura-widget--panel neura-comp neura-comp--code" '
            'data-neura-widget="panel" data-neura-id="code-snippet">'
            '<p class="neura-comp__label">Code</p>'
            '<pre class="neura-comp__code">{{code_preview}}</pre>'
            "</article>"
        ),
    },
]

COMPONENT_MATCH_THRESHOLD = 0.35


def _keywords_from_messages(messages: list[dict], chat_title: str | None = None) -> list[str]:
    return dc.keywords_from_messages(messages, chat_title)


def _score_component(keywords: list[str], component: dict) -> float:
    comp_keys = set(component.get("keywords") or [])
    query = set(keywords)
    if not query or not comp_keys:
        return 0.0
    overlap = len(query & comp_keys)
    if overlap == 0:
        return 0.12 if component.get("pinned") else 0.0
    base = overlap / max(len(query), len(comp_keys), 1)
    if component.get("pinned"):
        base += 0.08
    category = component.get("category")
    if category and category in " ".join(keywords):
        base += 0.1
    return min(1.0, base)


def build_compose_context(messages: list[dict], *, chat_title: str | None = None) -> dict[str, str]:
    entities = dc.resolve_entities(messages, chat_title=chat_title)
    ctx = dc.build_compose_context(messages, chat_title=chat_title, category=entities.category)
    ctx["weather_body"] = (
        ""
        if entities.location
        else f'<p class="neura-comp__body">{ctx["location_hint"]}</p>'
    )
    ctx["finance_body"] = (
        ""
        if entities.symbol
        else f'<p class="neura-comp__body">{ctx["symbol_hint"]}</p>'
    )
    return ctx


def match_components(
    messages: list[dict],
    *,
    chat_title: str | None = None,
    keywords: list[str] | None = None,
) -> list[dict]:
    keys = keywords or _keywords_from_messages(messages, chat_title)
    if de.parse_simple_arithmetic(last_user_text(messages)):
        forced = [c for c in DEFAULT_DAPP_COMPONENTS if c["id"] == "comp-math-hero"]
        if forced:
            return forced
    scored: list[tuple[float, dict]] = []
    for comp in DEFAULT_DAPP_COMPONENTS:
        if comp["id"] == "comp-math-hero" and not de.parse_simple_arithmetic(last_user_text(messages)):
            continue
        if comp.get("category") in ("shell", "chat"):
            continue
        score = _score_component(keys, comp)
        overlap = len(set(keys) & set(comp.get("keywords") or []))
        if score < COMPONENT_MATCH_THRESHOLD:
            if not (comp.get("pinned") and overlap > 0):
                continue
        scored.append((score, comp))
    scored.sort(key=lambda item: (-item[0], str(item[1].get("title") or "")))
    picked: list[dict] = []
    seen_categories: set[str] = set()
    for score, comp in scored:
        cat = str(comp.get("category") or comp["id"])
        if cat in seen_categories:
            continue
        picked.append({**comp, "matchScore": round(score, 3)})
        seen_categories.add(cat)
        if len(picked) >= 3:
            break
    if not picked:
        return []
    ids = {c["id"] for c in picked}
    for required in ("comp-action-chips", "comp-assistant-insight"):
        if required not in ids:
            extra = next((c for c in DEFAULT_DAPP_COMPONENTS if c["id"] == required), None)
            if extra:
                picked.append({**extra, "matchScore": 1.0})
    return picked


def last_user_text(messages: list[dict]) -> str:
    for msg in reversed(messages):
        if msg.get("role") == "user":
            return (msg.get("text") or "").strip()
    return ""


def render_component_html(component: dict, ctx: dict[str, str]) -> str:
    html = str(component.get("html") or "")
    for key, value in ctx.items():
        html = html.replace("{{" + key + "}}", str(value))
    return html


def compose_dapp_from_components(
    components: list[dict],
    messages: list[dict],
    *,
    topic_label: str | None = None,
    existing_files: dict[str, str] | None = None,
) -> dict[str, str] | None:
    if not components:
        return None
    ctx = build_compose_context(messages, chat_title=topic_label)
    fragments = [render_component_html(c, ctx) for c in components if c.get("id") != "comp-action-chips"]
    chips = next((c for c in components if c.get("id") == "comp-action-chips"), None)
    chips_html = render_component_html(chips, ctx) if chips else ""
    topic_inner = "\n".join(fragments) if fragments else (
        '<p class="neura-widget__placeholder">Dynamic AI widgets appear here as you chat.</p>'
    )
    base = dict(existing_files or de.NEURA_AI_SHELL_FILES)
    patched = de.apply_ai_shell_wrapper(base, topic_label=topic_label)
    index = patched.get("index.html", "")
    index = index.replace(
        '<p class="neura-widget__placeholder">Dynamic AI widgets appear here as you chat.</p>',
        topic_inner,
    )
    if chips_html and "neura-component-rail" in index:
        index = index.replace(
            '<div id="neura-component-rail" class="neura-component-rail"></div>',
            f'<div id="neura-component-rail" class="neura-component-rail">{chips_html}</div>',
        )
    patched["index.html"] = index
    extra_css = """
.neura-comp__label { margin: 0 0 8px; font-size: 10px; letter-spacing: 0.12em; text-transform: uppercase; color: var(--neura-muted); }
.neura-comp__hero { margin: 0; font-size: 24px; letter-spacing: 0.06em; }
.neura-comp__result { margin: 8px 0 0; font-size: 18px; }
.neura-comp__body { margin: 0; line-height: 1.55; font-size: 13px; white-space: pre-wrap; }
.neura-comp__sub { margin: 4px 0 0; color: var(--neura-muted); font-size: 12px; }
.neura-comp__code { margin: 0; padding: 10px; overflow: auto; border: 1px solid var(--neura-border); background: #000; font-size: 11px; }
.neura-comp__list { margin: 0; padding-left: 18px; }
.neura-comp--chips { display: flex; flex-wrap: wrap; gap: 8px; border: none; background: transparent; padding: 0; }
.neura-chip { padding: 6px 10px; border: 1px solid var(--neura-border); background: transparent; color: var(--neura-fg); font: inherit; font-size: 11px; cursor: pointer; }
.neura-live__hint { margin: 0; font-size: 12px; color: var(--neura-muted); }
"""
    patched["style.css"] = patched.get("style.css", "") + extra_css
    category = components[0].get("category") if components else None
    keywords = _keywords_from_messages(messages, topic_label)
    patched = de.inject_live_bootstrap(
        patched,
        category,
        keywords,
        messages=messages,
        chat_title=topic_label,
    )
    ok, _ = de.validate_dapp_files(patched)
    return patched if ok else None


def list_component_entries() -> list[dict]:
    return [
        {
            "id": c["id"],
            "title": c.get("title"),
            "category": c.get("category"),
            "kind": c.get("kind"),
            "pinned": bool(c.get("pinned")),
            "keywords": c.get("keywords") or [],
        }
        for c in DEFAULT_DAPP_COMPONENTS
    ]
