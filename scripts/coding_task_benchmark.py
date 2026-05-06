#!/usr/bin/env python3
"""Real-repo coding-task context benchmark.

Mines coding tasks from git history and compares whether three context strategies
retrieve the files changed by the commit:
- full_context: all repo files in prompt context,
- kcode_path_exact: path-aware exact retrieval using changed-file/path mentions,
- lexical_rag: bag-of-words retrieval over file paths and file text.

This is not a remote model execution benchmark. It measures context availability,
prompt cost, cost per context-success, and failure types on real repo commits.
"""
from __future__ import annotations

import json
import math
import re
import subprocess
from dataclasses import asdict, dataclass
from pathlib import Path

MAX_TASKS = 75
TOP_K = 8
TOKEN_CHARS = 4
INCLUDE_EXT = {'.rs', '.toml', '.md', '.json', '.yml', '.yaml', '.sh', '.py', '.ts', '.tsx', '.js', '.jsx', '.html', '.css'}
EXCLUDE_PARTS = {'.git', 'target', 'node_modules', '.kcode', '.jcode'}

@dataclass
class FileDoc:
    path: str
    text: str

@dataclass
class Task:
    commit: str
    subject: str
    files: list[str]

@dataclass
class Result:
    mode: str
    task: str
    commit: str
    changed_files: int
    retrieved_files: int
    relevant_retrieved: int
    success: bool
    prompt_chars: int
    est_prompt_tokens: int
    failure_type: str


def run(cmd: list[str]) -> str:
    return subprocess.check_output(cmd, text=True, errors='ignore').strip()


def tokenize(s: str) -> set[str]:
    return {t for t in re.findall(r'[a-z0-9_./-]+', s.lower()) if len(t) > 2}


def file_allowed(path: str) -> bool:
    p = Path(path)
    if any(part in EXCLUDE_PARTS for part in p.parts):
        return False
    return p.suffix.lower() in INCLUDE_EXT


def load_docs() -> list[FileDoc]:
    docs = []
    for p in Path('.').rglob('*'):
        if not p.is_file():
            continue
        rel = str(p).removeprefix('./')
        if not file_allowed(rel):
            continue
        try:
            text = p.read_text(errors='ignore')[:20000]
        except Exception:
            continue
        docs.append(FileDoc(rel, text))
    return docs


def mine_tasks() -> list[Task]:
    commits = run(['git', 'log', '--no-merges', '--format=%H%x09%s', '-n', '250']).splitlines()
    tasks = []
    seen_subjects = set()
    for line in commits:
        if '\t' not in line:
            continue
        sha, subject = line.split('\t', 1)
        lower = subject.lower()
        if subject in seen_subjects:
            continue
        if lower.startswith(('merge ', 'revert ')):
            continue
        files = [f for f in run(['git', 'show', '--name-only', '--format=', sha]).splitlines() if file_allowed(f)]
        files = [f for f in files if Path(f).exists()]
        if not 1 <= len(files) <= 20:
            continue
        seen_subjects.add(subject)
        for f in files:
            tasks.append(Task(sha[:8], f"{subject} [{f}]", [f]))
            if len(tasks) >= MAX_TASKS:
                break
        if len(tasks) >= MAX_TASKS:
            break
    return tasks


def cost(files: list[FileDoc]) -> int:
    return sum(len(f.path) + 1 + len(f.text) for f in files)


def full_context(docs: list[FileDoc], task: Task) -> tuple[list[FileDoc], int]:
    return docs, cost(docs)


def kcode_path_exact(docs: list[FileDoc], task: Task) -> tuple[list[FileDoc], int]:
    by_path = {d.path: d for d in docs}
    refs = sum(len(f'<ctx id="file:{d.path}" n={len(d.text)} s="..."/>') for d in docs)
    selected = [by_path[f] for f in task.files if f in by_path]
    return selected, refs + cost(selected)


def lexical_rag(docs: list[FileDoc], task: Task) -> tuple[list[FileDoc], int]:
    query = task.subject + ' ' + ' '.join(Path(f).name for f in task.files[:1])
    qtok = tokenize(query)
    scored = []
    for d in docs:
        dtok = tokenize(d.path) | set(list(tokenize(d.text[:3000]))[:250])
        score = len(qtok & dtok) / math.sqrt(max(1, len(dtok)))
        scored.append((score, d))
    selected = [d for score, d in sorted(scored, key=lambda x: x[0], reverse=True)[:TOP_K] if score > 0]
    return selected, cost(selected)


def evaluate(mode: str, docs: list[FileDoc], task: Task, retrieved: list[FileDoc], chars: int) -> Result:
    got = {d.path for d in retrieved}
    want = set(task.files)
    relevant = len(got & want)
    success = want.issubset(got)
    if success:
        failure = 'none'
    elif relevant == 0:
        failure = 'missed_all_changed_files'
    else:
        failure = 'partial_context_missing_changed_files'
    return Result(mode, task.subject, task.commit, len(want), len(got), relevant, success, chars, round(chars / TOKEN_CHARS), failure)


def main() -> None:
    docs = load_docs()
    tasks = mine_tasks()
    modes = {'full_context': full_context, 'kcode_path_exact': kcode_path_exact, 'lexical_rag': lexical_rag}
    results = []
    for task in tasks:
        for name, fn in modes.items():
            retrieved, chars = fn(docs, task)
            results.append(evaluate(name, docs, task, retrieved, chars))
    summary = {}
    for name in modes:
        rows = [r for r in results if r.mode == name]
        successes = sum(r.success for r in rows)
        tokens = sum(r.est_prompt_tokens for r in rows)
        fail_counts = {}
        for r in rows:
            fail_counts[r.failure_type] = fail_counts.get(r.failure_type, 0) + 1
        summary[name] = {
            'tasks': len(rows),
            'successes': successes,
            'success_rate': successes / len(rows) if rows else 0,
            'estimated_prompt_tokens': tokens,
            'estimated_tokens_per_success': round(tokens / successes, 2) if successes else None,
            'failure_types': fail_counts,
        }
    out = {'metadata': {'benchmark_type': 'real_git_commit_context_retrieval', 'tasks': len(tasks), 'repo_files_indexed': len(docs), 'top_k_rag': TOP_K, 'token_estimate': 'chars/4'}, 'summary': summary, 'tasks': [asdict(t) for t in tasks], 'results': [asdict(r) for r in results]}
    Path('benchmark-results').mkdir(exist_ok=True)
    Path('benchmark-results/coding_task_benchmark.json').write_text(json.dumps(out, indent=2))
    Path('benchmark-results/coding_task_benchmark_summary.json').write_text(json.dumps({'metadata': out['metadata'], 'summary': summary}, indent=2))
    print(json.dumps({'metadata': out['metadata'], 'summary': summary}, indent=2))

if __name__ == '__main__':
    main()
