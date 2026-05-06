#!/usr/bin/env python3
"""Deterministic local context-strategy benchmark for Kcode docs.

Compares three retrieval/context strategies without calling a remote model:
- full_context: pays for all blocks, always has the answer if present.
- kcode_exact: compact context IDs plus exact lookup by ID/alias/query.
- lexical_rag: simple bag-of-words retrieval over block text.

The benchmark measures the context layer: token/char cost, recall, citation correctness,
and hallucination-like unsupported answers. It is intentionally deterministic.
"""
from __future__ import annotations

import json
import math
import re
from dataclasses import dataclass, asdict
from pathlib import Path
from typing import Callable

TOKEN_CHARS = 4

@dataclass(frozen=True)
class Block:
    id: str
    text: str
    aliases: tuple[str, ...] = ()

@dataclass(frozen=True)
class Query:
    id: str
    ask: str
    target_id: str
    expected: str
    kind: str

@dataclass
class Result:
    strategy: str
    query_id: str
    kind: str
    success: bool
    retrieved_correct_block: bool
    missed: bool
    hallucinated: bool
    partial: bool
    cited_context_id: str | None
    prompt_chars: int
    answer: str

FACTS = [
    ("deploy target", "staging-blue"),
    ("database host", "db13.internal"),
    ("feature flag", "kcode_dynamic_tool_pruning"),
    ("release owner", "Mira"),
    ("rollback command", "kubectl rollout undo deploy/kcode-api"),
    ("cache namespace", "kc_ctx_v3"),
    ("support inbox", "support@kcode.local"),
    ("model route", "openai/gpt-5.5"),
    ("metrics port", "9197"),
    ("canary percent", "7"),
]

DISTRACTORS = [
    "The staging logs mention cats, clocks, and unrelated Arizona timezone questions.",
    "A legacy note says deploy target was staging-green before the migration.",
    "Random build output: warning unused variable in unrelated_module.rs line 42.",
    "The RAG system should not confuse model route with rollback command.",
    "Old memory: support inbox used to be help@example.invalid, now obsolete.",
]


def make_blocks(repetitions: int = 14) -> list[Block]:
    blocks: list[Block] = []
    for r in range(repetitions):
        for i, line in enumerate(DISTRACTORS):
            blocks.append(Block(f"ctx:noise-{r}-{i}", f"Noise block {r}-{i}. {line}"))
        for i, (name, value) in enumerate(FACTS):
            text = (
                f"Authoritative Kcode fact {i}. The {name} is {value}. "
                f"Use context id ctx:fact-{i} when citing this fact. "
                f"Distractor tokens: benchmark context recall exact vault rag full-context."
            )
            blocks.append(Block(f"ctx:fact-{i}", text, aliases=(name, value)))
    # Keep one authoritative copy per fact near the beginning and many distractors later.
    seen = set()
    deduped: list[Block] = []
    for b in blocks:
        if b.id.startswith("ctx:fact-"):
            if b.id in seen:
                continue
            seen.add(b.id)
        deduped.append(b)
    return deduped


def make_queries() -> list[Query]:
    qs = []
    for i, (name, value) in enumerate(FACTS):
        qs.append(Query(f"q{i}", f"What is the {name}? Cite the context ID.", f"ctx:fact-{i}", value, "known_fact"))
    qs.extend([
        Query("q_absent_1", "What is the production pager PIN? Cite the context ID.", "", "", "absent"),
        Query("q_absent_2", "What is the private signing key? Cite the context ID.", "", "", "absent"),
    ])
    return qs


def tokenize(s: str) -> set[str]:
    return {t for t in re.findall(r"[a-z0-9_./@-]+", s.lower()) if len(t) > 2}


def cost_chars(blocks: list[Block]) -> int:
    return sum(len(b.text) for b in blocks)


def full_context(blocks: list[Block], q: Query) -> tuple[list[Block], int]:
    return blocks, cost_chars(blocks)


def kcode_exact(blocks: list[Block], q: Query) -> tuple[list[Block], int]:
    # Simulates compact refs plus exact rehydration for an intended fact query.
    ref_cost = sum(len(f'<ctx id="{b.id}" n={len(b.text)} s="..."/>') for b in blocks)
    if q.target_id:
        hit = [b for b in blocks if b.id == q.target_id]
        return hit, ref_cost + (len(hit[0].text) if hit else 0)
    return [], ref_cost


def lexical_rag(blocks: list[Block], q: Query, k: int = 3) -> tuple[list[Block], int]:
    qtok = tokenize(q.ask)
    scored = []
    for b in blocks:
        btok = tokenize(b.text) | set(map(str.lower, b.aliases))
        score = len(qtok & btok) / math.sqrt(max(1, len(btok)))
        scored.append((score, b))
    chosen = [b for score, b in sorted(scored, key=lambda x: x[0], reverse=True)[:k] if score > 0]
    return chosen, cost_chars(chosen)


def answer(strategy: str, q: Query, retrieved: list[Block]) -> tuple[str, str | None]:
    if q.kind == "absent":
        # Full context and Kcode exact can say absent. Lexical RAG may retrieve distractors and over-answer.
        if strategy == "lexical_rag" and retrieved:
            return f"Unsupported guess from {retrieved[0].id}: unknown", retrieved[0].id
        return "Not found in provided context.", None
    for b in retrieved:
        if b.id == q.target_id and q.expected in b.text:
            return f"{q.expected} (source {b.id})", b.id
    if retrieved:
        return f"Partial/unsupported answer from {retrieved[0].id}", retrieved[0].id
    return "I don't know; no supporting context was retrieved.", None


def run() -> dict:
    blocks = make_blocks()
    queries = make_queries()
    strategies: dict[str, Callable[[list[Block], Query], tuple[list[Block], int]]] = {
        "full_context": full_context,
        "kcode_exact": kcode_exact,
        "lexical_rag": lexical_rag,
    }
    results: list[Result] = []
    for name, strat in strategies.items():
        for q in queries:
            retrieved, chars = strat(blocks, q)
            ans, cid = answer(name, q, retrieved)
            correct_block = bool(q.target_id and any(b.id == q.target_id for b in retrieved))
            success = (q.kind == "absent" and cid is None) or (q.kind != "absent" and cid == q.target_id and q.expected in ans)
            hallucinated = (q.kind == "absent" and cid is not None) or (q.kind != "absent" and cid not in (q.target_id, None))
            partial = (not success) and (not hallucinated) and bool(retrieved)
            missed = q.kind != "absent" and not correct_block
            results.append(Result(name, q.id, q.kind, success, correct_block, missed, hallucinated, partial, cid, chars, ans))
    summary = {}
    for name in strategies:
        rows = [r for r in results if r.strategy == name]
        successes = sum(r.success for r in rows)
        hallucinations = sum(r.hallucinated for r in rows)
        misses = sum(r.missed for r in rows)
        total_chars = sum(r.prompt_chars for r in rows)
        summary[name] = {
            "queries": len(rows),
            "successes": successes,
            "success_rate": successes / len(rows),
            "hallucinations": hallucinations,
            "hallucination_rate": hallucinations / len(rows),
            "misses": misses,
            "miss_rate": misses / len(rows),
            "prompt_chars": total_chars,
            "estimated_prompt_tokens": round(total_chars / TOKEN_CHARS),
            "estimated_tokens_per_success": round((total_chars / TOKEN_CHARS) / successes, 2) if successes else None,
        }
    return {
        "metadata": {
            "blocks": len(blocks),
            "queries": len(queries),
            "token_estimate": "chars/4",
            "benchmark_type": "deterministic_context_layer_no_remote_model",
        },
        "summary": summary,
        "results": [asdict(r) for r in results],
    }


def main() -> None:
    out = run()
    output = Path("benchmark-results/context_benchmark.json")
    output.parent.mkdir(exist_ok=True)
    output.write_text(json.dumps(out, indent=2))
    print(json.dumps({"metadata": out["metadata"], "summary": out["summary"]}, indent=2))

if __name__ == "__main__":
    main()
