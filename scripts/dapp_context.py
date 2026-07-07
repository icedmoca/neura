"""Conversation-driven dapp context — entities, template slots, and live-data hints.

Nothing in this module assumes a fixed city, ticker, or topic. All values are
resolved from the active chat (and optional title) at compose/runtime time.
"""
from __future__ import annotations

import html
import re
from dataclasses import dataclass, field
from typing import Any

import dapp_engine as de

KEYWORD_STOPWORDS = frozenset({
    "what", "how", "does", "about", "please", "tell", "show", "give", "want", "need",
    "can", "you", "the", "and", "for", "this", "that", "with", "from", "when", "where",
    "who", "why", "are", "was", "were", "will", "would", "could", "should", "have",
    "has", "had", "just", "like", "know", "get", "got", "hey", "hello", "thanks",
    "thank", "also", "into", "your", "mine", "our", "their", "them", "they", "chat",
    "project", "neura", "dapp", "make", "help", "whats", "up", "gif", "gifs",
})

TOPIC_ALIASES: dict[str, list[str]] = {
    "weather": ["forecast", "temperature", "rain", "snow", "sunny", "humidity", "clima"],
    "forecast": ["weather", "temperature"],
    "fox": ["ylvis", "whatdoesthefoxsay"],
    "song": ["music", "lyrics", "spotify", "youtube"],
    "music": ["song", "spotify", "youtube", "album", "artist"],
    "todo": ["task", "checklist", "tasks"],
    "stock": ["stocks", "market", "ticker", "nasdaq", "nyse"],
    "bitcoin": ["btc", "crypto"],
    "ethereum": ["eth"],
    "recipe": ["cook", "cooking", "ingredients", "food"],
}

CATEGORY_HINTS: dict[str, list[str]] = {
    "weather": ["weather", "forecast", "temperature", "rain", "snow", "humidity", "wind", "clima"],
    "music": ["song", "music", "spotify", "youtube", "lyrics", "album", "artist", "fox", "ylvis"],
    "finance": ["stock", "stocks", "crypto", "bitcoin", "market", "ticker", "nasdaq"],
    "food": ["recipe", "restaurant", "food", "cook", "ingredients", "menu"],
    "travel": ["flight", "hotel", "map", "airport", "trip", "travel"],
    "tasks": ["todo", "task", "checklist", "calendar", "reminder"],
    "video": ["video", "youtube", "movie", "trailer", "watch", "embed", "gif", "gifs"],
    "code": ["code", "function", "python", "javascript", "rust", "debug", "error"],
    "math": ["math", "plus", "minus", "multiply", "divide", "calculate", "sum"],
}

SYMBOL_NAME_TO_TICKER: dict[str, str] = {
    "bitcoin": "BTC",
    "btc": "BTC",
    "ethereum": "ETH",
    "eth": "ETH",
    "dogecoin": "DOGE",
    "doge": "DOGE",
}

TICKER_STOPWORDS = frozenset({
    "THE", "AND", "FOR", "YOU", "ARE", "WAS", "HAS", "HAD", "NOT", "BUT", "ALL",
    "CAN", "HER", "HIS", "OUR", "OUT", "DAY", "GET", "USE", "NEW", "NOW", "WAY",
})

LOCATION_PATTERNS = (
    re.compile(r"\bweather(?:\s+in|\s+for|\s+at|\s+near)?\s+([A-Za-z][A-Za-z\s,'-]{1,48})", re.I),
    re.compile(r"\b(?:in|at|near|for)\s+([A-Z][a-z]+(?:\s+[A-Z][a-z]+){0,2})"),
    re.compile(r"\b(?:in|at|near|for)\s+([a-z]{3,}(?:\s+[a-z]{3,})?)", re.I),
)


@dataclass
class ResolvedEntities:
    location: str = ""
    symbol: str = ""
    topic: str = ""
    category: str | None = None
    keywords: list[str] = field(default_factory=list)


def expand_keywords(keywords: list[str]) -> list[str]:
    expanded = list(keywords)
    for keyword in keywords:
        for alias in TOPIC_ALIASES.get(keyword, []):
            if alias not in expanded:
                expanded.append(alias)
        for root, aliases in TOPIC_ALIASES.items():
            if keyword in aliases and root not in expanded:
                expanded.append(root)
    return expanded[:40]


def extract_entity_tokens(text: str) -> list[str]:
    if not text:
        return []
    tokens: list[str] = []
    for match in re.findall(r"\b[A-Z][a-z]+(?:\s+[A-Z][a-z]+){0,2}\b", text):
        compact = match.lower().replace(" ", "")
        if len(compact) >= 3 and compact not in tokens:
            tokens.append(compact)
        spaced = match.strip()
        if spaced and spaced not in tokens:
            tokens.append(spaced)
    return tokens[:12]


def conversation_blob(
    messages: list[dict],
    *,
    chat_title: str | None = None,
    include_assistant: bool = False,
) -> str:
    parts: list[str] = []
    if chat_title:
        parts.append(chat_title)
    for msg in messages:
        if msg.get("role") != "user":
            continue
        text = (msg.get("text") or "").strip()
        if text:
            parts.append(text)
    if include_assistant:
        for msg in reversed(messages):
            if msg.get("role") != "assistant":
                continue
            text = (msg.get("text") or "").strip()
            if text:
                parts.append(text[:700])
            break
    return "\n".join(parts)


def keywords_from_messages(
    messages: list[dict],
    chat_title: str | None = None,
    *,
    include_assistant: bool = False,
) -> list[str]:
    blob = conversation_blob(messages, chat_title=chat_title, include_assistant=include_assistant).lower()
    words = re.findall(r"[a-z0-9]{3,}", blob)
    if include_assistant:
        for msg in reversed(messages):
            if msg.get("role") != "assistant":
                continue
            words.extend(extract_entity_tokens(msg.get("text") or ""))
            break
    out: list[str] = []
    for word in words:
        if word in KEYWORD_STOPWORDS or word in out:
            continue
        out.append(word)
    return expand_keywords(out)[:32]


def detect_category(keywords: list[str]) -> str | None:
    expanded = set(expand_keywords(keywords))
    best_category = None
    best_overlap = 0
    for category, hints in CATEGORY_HINTS.items():
        overlap = len(expanded & set(hints))
        if overlap > best_overlap:
            best_overlap = overlap
            best_category = category
    return best_category if best_overlap >= 1 else None


def _category_vocab() -> set[str]:
    vocab: set[str] = set()
    for hints in CATEGORY_HINTS.values():
        vocab.update(hints)
    return vocab


TRAILING_FILLER = frozenset({
    "please", "thanks", "thank", "you", "today", "tomorrow", "now", "right", "there",
    "here", "too", "also", "again", "pls", "thx",
})


def _clean_place(name: str) -> str:
    cleaned = re.sub(r"[?.!,;:].*$", "", name.strip())
    cleaned = re.sub(r"\s+", " ", cleaned)
    parts = [p for p in cleaned.split() if p.lower() not in TRAILING_FILLER]
    return " ".join(parts).strip()


def resolve_location(blob: str, keywords: list[str], *, category: str | None = None) -> str:
    weather_intent = (
        category == "weather"
        or "weather" in blob.lower()
        or "forecast" in blob.lower()
        or "temperature" in blob.lower()
    )
    if not weather_intent:
        return ""

    for pattern in LOCATION_PATTERNS:
        match = pattern.search(blob)
        if not match:
            continue
        place = _clean_place(match.group(1))
        if place and place.lower() not in KEYWORD_STOPWORDS:
            return place

    category_vocab = _category_vocab()
    for token in extract_entity_tokens(blob):
        low = token.lower() if isinstance(token, str) else str(token).lower()
        if low in KEYWORD_STOPWORDS or low in category_vocab:
            continue
        return token if isinstance(token, str) and " " in token else token.title()

    for kw in keywords:
        low = kw.lower()
        if low in KEYWORD_STOPWORDS or low in category_vocab:
            continue
        if len(low) >= 3 and low.isalpha():
            return kw.title()
    return ""


def resolve_symbol(blob: str, keywords: list[str], *, category: str | None = None) -> str:
    finance_intent = (
        category == "finance"
        or any(h in blob.lower() for h in ("stock", "stocks", "ticker", "crypto", "bitcoin", "market", "price"))
    )
    if not finance_intent:
        return ""

    for match in re.finditer(r"\$([A-Z]{1,5})\b", blob):
        return match.group(1)
    for match in re.finditer(r"\b([A-Z]{2,5})\b", blob):
        sym = match.group(1)
        if sym not in TICKER_STOPWORDS:
            return sym
    for kw in keywords:
        mapped = SYMBOL_NAME_TO_TICKER.get(kw.lower())
        if mapped:
            return mapped
        if re.fullmatch(r"[a-z]{2,5}", kw):
            return kw.upper()
    return ""


def resolve_entities(
    messages: list[dict],
    *,
    chat_title: str | None = None,
    category: str | None = None,
) -> ResolvedEntities:
    blob = conversation_blob(messages, chat_title=chat_title, include_assistant=True)
    keywords = keywords_from_messages(messages, chat_title, include_assistant=True)
    cat = category or detect_category(keywords)
    last_user = ""
    for msg in reversed(messages):
        if msg.get("role") == "user":
            last_user = (msg.get("text") or "").strip()
            break
    topic = chat_title or last_user[:80] or ""
    return ResolvedEntities(
        location=resolve_location(blob, keywords, category=cat),
        symbol=resolve_symbol(blob, keywords, category=cat),
        topic=topic,
        category=cat,
        keywords=keywords,
    )


def suggest_action_chips(
    *,
    category: str | None,
    last_user: str,
    parsed_math: tuple[str, int] | None,
    entities: ResolvedEntities,
) -> list[str]:
    if parsed_math:
        return ["Show steps", "Try another", "Graph it"]
    lowered = last_user.lower()
    if category == "weather" and entities.location:
        return [f"Hourly · {entities.location}", "5-day forecast", "Change city"]
    if category == "finance" and entities.symbol:
        return [f"Chart · {entities.symbol}", "Compare peers", "Set alert"]
    if category == "music":
        return ["Play preview", "Show lyrics", "Similar tracks"]
    if category == "video" or "gif" in lowered:
        return ["More like this", "Full screen", "Search again"]
    if category == "tasks":
        return ["Add task", "Mark done", "Clear done"]
    if category == "code":
        return ["Explain code", "Fix errors", "Add tests"]
    if last_user.strip():
        snippet = last_user.strip().split("\n", 1)[0][:32]
        return [f"More on: {snippet}", "Go deeper", "Summarize"]
    return []


def build_compose_context(
    messages: list[dict],
    *,
    chat_title: str | None = None,
    category: str | None = None,
) -> dict[str, str]:
    entities = resolve_entities(messages, chat_title=chat_title, category=category)
    last_user = ""
    last_assistant = ""
    for msg in reversed(messages):
        if msg.get("role") == "user" and not last_user:
            last_user = (msg.get("text") or "").strip()
        if msg.get("role") == "assistant" and not last_assistant:
            last_assistant = (msg.get("text") or "").strip()
        if last_user and last_assistant:
            break

    parsed = de.parse_simple_arithmetic(last_user)
    ctx: dict[str, str] = {
        "location": entities.location,
        "location_display": entities.location or "—",
        "location_hint": "mention a city in chat",
        "symbol": entities.symbol,
        "symbol_display": entities.symbol or "—",
        "symbol_hint": "mention a ticker in chat",
        "title": entities.topic[:80] + ("…" if len(entities.topic) > 80 else ""),
        "subtitle": last_user[:120] if last_user else "",
        "expr": "?",
        "result": "?",
        "assistant_preview": "",
        "code_preview": "",
        "todo_items": "",
        "chips": "",
    }

    if parsed:
        expr, result = parsed
        ctx["expr"] = expr
        ctx["result"] = str(result)

    if last_assistant:
        preview = last_assistant.replace("\n", " ").strip()
        ctx["assistant_preview"] = preview[:280] + ("…" if len(preview) > 280 else "")

    chip_labels = suggest_action_chips(
        category=entities.category,
        last_user=last_user,
        parsed_math=parsed,
        entities=entities,
    )
    ctx["chips"] = "".join(
        f'<button type="button" class="neura-chip" data-neura-action="ask">{label}</button>'
        for label in chip_labels[:4]
    )

    code_match = re.search(r"```[\w]*\n([\s\S]{10,400}?)\n```", last_assistant)
    if code_match:
        ctx["code_preview"] = code_match.group(1).strip()

    todo_lines = [line.strip("-•* ").strip() for line in last_assistant.splitlines() if line.strip().startswith(("-", "•", "*"))]
    if todo_lines:
        ctx["todo_items"] = "".join(f"<li>{html.escape(line)}</li>" for line in todo_lines[:8])
    elif last_user and any(w in last_user.lower() for w in ("todo", "task", "checklist")):
        ctx["todo_items"] = f"<li>{last_user[:120]}</li>"

    return ctx


def bootstrap_payload(
    messages: list[dict],
    *,
    chat_title: str | None = None,
    category: str | None = None,
) -> dict[str, str]:
    entities = resolve_entities(messages, chat_title=chat_title, category=category)
    payload: dict[str, str] = {}
    if category == "weather" and entities.location:
        payload["location"] = entities.location
    if category == "finance" and entities.symbol:
        payload["symbol"] = entities.symbol
    return payload


def live_data_hints(
    category: str | None,
    messages: list[dict],
    *,
    chat_title: str | None = None,
    keywords: list[str] | None = None,
) -> str:
    if not category:
        return ""
    entities = resolve_entities(messages, chat_title=chat_title, category=category)
    if category == "weather":
        if entities.location:
            target = entities.location
        else:
            return (
                "Live data (when relevant):\n"
                "- Extract the city/region from the conversation before fetching weather.\n"
                "- Use wttr.in or open-meteo; show loading/error states if location is unknown.\n"
            )
        return (
            "Live data (required when relevant):\n"
            f"- Fetch live weather for **{target}** via `https://wttr.in/{target}?format=j1` or open-meteo.\n"
            "- Show current temp, conditions, and a short multi-day forecast.\n"
        )
    if category == "finance":
        if entities.symbol:
            sym = entities.symbol
        else:
            return (
                "Live data (when relevant):\n"
                "- Extract ticker/crypto symbol from the conversation before fetching quotes.\n"
            )
        return (
            "Live data (required when relevant):\n"
            f"- Fetch quote data for **{sym}** (e.g. Yahoo chart API).\n"
            "- Show price, daily change, and a tiny sparkline if possible.\n"
        )
    if category == "music":
        return (
            "Live data (required when relevant):\n"
            "- Derive artist/track from the chat; include YouTube/Spotify links for that topic.\n"
        )
    if category == "food":
        return (
            "Live data (optional):\n"
            "- Use place names from the chat for map/recipe links — do not assume a default city.\n"
        )
    if category == "tasks":
        return (
            "Interactivity (required):\n"
            "- Build a working in-browser todo/checklist seeded from chat content.\n"
            "- Persist state in localStorage keyed to this chat topic.\n"
        )
    if category == "video":
        return (
            "Live embeds (required when relevant):\n"
            "- Build gallery/embeds for the subject the user asked about (e.g. gifs, videos).\n"
        )
    return ""
