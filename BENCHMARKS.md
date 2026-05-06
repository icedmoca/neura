# Kcode Benchmarks

This document tracks benchmark evidence for Kcode's context compression, context vault, memory, and dynamic tool-schema pipeline.

The numbers below are **real local measurements** from the active Kcode installation under `~/.kcode` unless a row is explicitly marked as a benchmark plan. They are not synthetic marketing numbers.

## Measurement snapshot

| Field | Value |
|---|---:|
| Source checkout | `~/.kcode/build-src/kcode` |
| Repo HEAD when measured | `4812e47` |
| Installed binary reported | `kcode v0.10.168-dev (6dc42ed, dirty)` |
| Interlang telemetry file | `~/.kcode/interlang-stats.jsonl` |
| Telemetry size | 2.6 MiB |
| Telemetry events parsed | 3,716 |
| Non-zero compression events | 3,662 |
| Tool schema functions in source | 42 |
| Rust tests in source tree | 2,554 |

> Note: the installed binary reported an older embedded commit than the docs HEAD because the working tree had later docs-only commits after the binary was built. The benchmarked compression/tool behavior comes from the `.kcode` telemetry and current source tree.

## Token usage vs full-context baseline

The baseline is a conservative full-context approximation: send the original recorded context blocks directly instead of replacing eligible blocks with compact refs or encoded blocks.

| Metric | Measured value |
|---|---:|
| Approx original chars | 6,087,468,238 |
| Approx encoded chars | 440,808,870 |
| Approx chars avoided | 5,646,659,368 |
| Aggregate reduction | 92.76% |
| Median chars saved per non-zero event | 1,146,141 |
| p95 chars saved per non-zero event | 4,436,244 |
| Max chars saved per event | 4,436,244 |

Approximate token savings depend on tokenizer and provider formatting. A rough chars/4 estimate implies about **1.41B tokens avoided** across the recorded telemetry. Use provider-side token accounting for billing-grade numbers.

## Short / medium / long session reduction

Events were bucketed by approximate original context size.

| Bucket | Original size/event | Events | Original chars | Saved chars | Reduction |
|---|---:|---:|---:|---:|---:|
| Short | 1 to 12k chars | 69 | 598,778 | 264,655 | 44.20% |
| Medium | 12k to 80k chars | 125 | 5,404,946 | 4,439,885 | 82.14% |
| Long | 80k+ chars | 3,468 | 6,081,464,514 | 5,641,954,828 | 92.77% |

Interpretation: compression is intentionally modest on short turns, where exact context is cheap and safer. Savings become large in long-running sessions where repeated tool output, logs, diffs, and older turns dominate cost.

## Tool-schema overhead and pruning

Kcode contains 42 tool schema definitions in source. Sending every schema on every turn creates a large fixed cost, especially for direct-answer questions.

Current behavior:

- tool-like turns receive relevant tool families up front,
- direct-answer turns keep only core always-on tools plus `tool_expand`,
- `tool_expand` lets the model request more schemas if the first-pass classifier was too conservative.

This is designed to reduce fixed prompt overhead without disabling tools. Regression coverage includes dynamic tool-filter tests for web/file/browser/direct-answer cases.


## Deterministic context-layer benchmark run

A reproducible local benchmark harness now lives at `scripts/context_benchmark.py`. It does not call a remote model. Instead, it isolates the context layer and compares three strategies over 80 synthetic-but-deterministic context blocks and 12 queries:

- **full_context:** sends every block and always has the answer when it exists,
- **kcode_exact:** sends compact refs and rehydrates the exact block by ID/alias/query,
- **lexical_rag:** retrieves the top 3 blocks with simple bag-of-words lexical scoring.

This benchmark directly covers task success vs token cost, context recall accuracy, and unsupported-answer/hallucination behavior for the retrieval layer.

| Strategy | Queries | Success rate | Hallucination rate | Miss rate | Prompt chars | Est. prompt tokens | Est. tokens/success |
|---|---:|---:|---:|---:|---:|---:|---:|
| Full context | 12 | 100.00% | 0.00% | 0.00% | 98,796 | 24,699 | 2,058.25 |
| Kcode exact refs | 12 | 100.00% | 0.00% | 0.00% | 38,351 | 9,588 | 798.98 |
| Lexical RAG | 12 | 66.67% | 33.33% | 16.67% | 5,634 | 1,408 | 176.06 |

Measured result: Kcode exact refs matched full-context success and hallucination behavior while using **61.18% fewer estimated prompt tokens** than full context on this benchmark. Lexical RAG was cheapest, but it missed or hallucinated on several queries because the top lexical hits can be distractors or obsolete facts.

Artifacts:

- script: `scripts/context_benchmark.py`,
- full JSON results: `benchmark-results/context_benchmark.json`,
- summary JSON: `benchmark-results/context_benchmark_summary.json`.

Re-run:

```bash
python3 scripts/context_benchmark.py
```

Caveat: this is a deterministic local context benchmark, not a remote-model end-to-end benchmark. It measures whether the context strategy supplies the right evidence at what prompt cost. End-to-end model task success, latency, and cost still need provider runs using the same harness prompts.




## Actual provider edit→test coding benchmark

This benchmark is intentionally small but it is **actual coding**, not just context retrieval. Kcode was run through the real non-interactive provider path in temporary repositories with failing Python unit tests. The provider had to edit files and run `python -m unittest`; success required the final tests to pass.

Artifacts:

- runner: `scripts/provider_edit_benchmark.py`,
- full results: `benchmark-results/provider_edit_benchmark.json`,
- summary: `benchmark-results/provider_edit_benchmark_summary.json`,
- per-run traces: `benchmark-results/provider-edit-runs/*.json`.

| Task | Initial tests | Final tests | Wall time | Provider input tokens | Provider output tokens |
|---|---|---|---:|---:|---:|
| `fix_add_function` | failing | passing | 19.193s | 5,229 | 55 |
| `fix_slugify_edgecase` | failing | passing | 25.934s | 7,365 | 105 |
| `fix_json_config_default` | failing | passing | 19.452s | 5,499 | 44 |

Measured result: **3/3 actual provider edit→test tasks passed**. This is still a small smoke benchmark, but it directly addresses the earlier weakness that context-only benchmarks do not prove coding success.

Caveat: this is not yet a statistically meaningful coding benchmark. It uses small Python fixtures, not large ambiguous repo issues. The correct next step is to scale this same harness to 50+ isolated real commits and score pass/fail by test suites and diffs.

## Real provider-call smoke benchmark

This run uses the actual non-interactive Kcode CLI with OpenAI `gpt-5.5`:

```bash
kcode run --json --trace --quiet --no-update --no-selfdev --cwd ~/.kcode/build-src/kcode <message>
```

It is intentionally small to avoid runaway cost, but it verifies that provider calls, JSON usage accounting, direct-answer behavior, and tool-capable prompts work end to end.

Artifacts:

- full results: `benchmark-results/provider_calls.json`,
- summary: `benchmark-results/provider_calls_summary.json`,
- per-run traces: `benchmark-results/provider-runs/*.json`.

| Run | Kind | Return code | Wall time | Input tokens | Output tokens | Result |
|---|---|---:|---:|---:|---:|---|
| `direct_time_arizona` | direct answer | 0 | 1.989s | 4,258 | 26 | Correct MST/UTC-7 answer |
| `file_read_readme` | file/tool-capable | 0 | 4.963s | 4,712 | 16 | Correctly found first README heading |
| `repo_file_count` | file/tool-capable | 0 | 5.076s | 4,399 | 5 | Correct top-level Markdown count |

Measured result: after the token/tool-schema fixes, a fresh direct-answer provider call used **4,258 input tokens**, far below the previously observed bloated-session 43k-token behavior. Tool-capable file prompts completed successfully with usage traces under 4.8k input tokens in this smoke run.

Caveat: this is a provider smoke benchmark, not a large statistical study. It proves the pipeline can make real provider calls with bounded token usage and tool-capable answers, but it does not by itself prove long-horizon coding-task success.

## Real repo coding-task context benchmark

This benchmark mines real work from the Kcode git history. Each task is a real changed file from a real non-merge commit, represented as: commit subject plus the changed file path. The benchmark compares whether each context strategy supplies the file needed for the task, and at what prompt cost.

This is closer to end-to-end coding than the synthetic context benchmark, but it is still a **context availability benchmark**, not a remote-model coding benchmark. It does not ask a model to edit files. It measures whether the model would have the required file context available.

Artifacts:

- script: `scripts/coding_task_benchmark.py`,
- full JSON results: `benchmark-results/coding_task_benchmark.json`,
- summary JSON: `benchmark-results/coding_task_benchmark_summary.json`.

Measured run:

| Strategy | Real tasks | Context success rate | Prompt tokens | Tokens/success | Failure profile |
|---|---:|---:|---:|---:|---|
| Full context | 75 | 100.00% | 155,638,575 | 2,075,181.00 | none: 75 |
| Kcode path-exact | 75 | 100.00% | 1,104,348 | 14,724.64 | none: 75 |
| Lexical RAG | 75 | 48.00% | 1,760,622 | 48,906.17 | none: 36, missed all changed files: 39 |

Measured result: on 75 real git-history coding-context tasks, Kcode path-exact retrieval matched full-context context availability while using **99.29% fewer estimated prompt tokens** than full context. Compared with the simple lexical RAG baseline, Kcode had higher context success and lower total token cost in this run.

Re-run:

```bash
python3 scripts/coding_task_benchmark.py
```

### What this proves

- Kcode-style exact/path-aware context can preserve required file availability at a tiny fraction of full-context cost.
- Simple lexical RAG can be cheaper on some individual queries but misses required files frequently on real repo commit tasks.
- Token savings are not just from synthetic facts; they also appear on real repository history.

### What is still unproven

These remain unproven until we run a remote-model editing benchmark:

- **Large-scale end-to-end coding performance:** small provider edit→test fixtures now pass, but 50+ real repo issue/commit tasks remain unmeasured.
- **Messy / ambiguous real-world prompts at scale:** three adversarial smoke prompts now pass, but a large human-graded messy-prompt suite remains unmeasured.
- **Regression over long multi-turn sessions:** whether accuracy remains stable after many tool calls, context refs, and topic shifts.
- **Provider latency and billed cost:** local token estimates are not the same as provider-side accounting.

### Decisive next benchmark

The decisive test should take the same 75 mined tasks and execute real model runs under three configurations:

1. full context,
2. Kcode context vault/path-exact retrieval,
3. lexical/semantic RAG.

For each task, start from the parent commit, let the agent modify the repo, and score:

- task success by diff and tests,
- provider input/output tokens,
- wall-clock latency,
- number of tool calls,
- user intervention count,
- hallucinated file/function/tool-output claims,
- failure mode category.

That benchmark is more expensive, but it is the correct way to prove end-to-end coding performance rather than context availability alone.

## Task success rate

| Benchmark | Current status |
|---|---|
| Unit/regression suite | 2,554 Rust tests present in tree. Focused token pipeline tests were run during implementation. |
| Coding task completion rate | Not yet measured as a controlled benchmark. |
| Feature implemented and tests passing | Verified for the context/memory/tool-pruning changes with focused tests and release builds. |

Recommended controlled protocol:

1. Select 50 to 100 real coding tasks from the repo issue history or curated fixtures.
2. Run each task under three modes: no compression, Kcode compression, and standard RAG-only retrieval.
3. Score success by `cargo test`, expected file diff, and human review for requirements coverage.
4. Report completion rate, partial completion rate, and regressions.

## Comparison with no-compression and standard RAG setups

| Setup | Expected behavior | Measured locally? |
|---|---|---|
| No compression / full context | Highest exact-context availability, highest token cost, hard context-window failure in long sessions. | Baseline approximated from original chars in telemetry. |
| Standard RAG | Lower prompt size, but may retrieve semantically similar wrong chunks and lose exact ordering/tool-output provenance. | Not yet measured against a specific RAG implementation. |
| Kcode context vault | Compact refs preserve exact local text and support `.ctx_get` rehydration. | Token reduction measured from `.kcode` telemetry. |

A fair RAG comparison needs the same task set, same model, same context budget, and the same stored session corpus. The current committed baseline is lexical/path retrieval, not a production embedding RAG stack; embedding RAG remains **UNMEASURED** until a local or hosted embedding index is run on the same tasks.


## Messy / adversarial provider smoke benchmark

This benchmark checks whether the real provider path avoids unsupported answers on ambiguous, conflicting, or adversarial prompts.

Artifacts:

- full results: `benchmark-results/provider_messy_benchmark.json`,
- summary: `benchmark-results/provider_messy_benchmark_summary.json`,
- per-run traces: `benchmark-results/provider-messy-runs/*.json`.

| Run | Expected behavior | Passed | Wall time | Input tokens | Output tokens |
|---|---|---:|---:|---:|---:|
| `ambiguous_missing_context` | say `NOT_FOUND`, do not guess | yes | 4.089s | 4,270 | 76 |
| `conflicting_context` | choose newer `staging-blue` and mention conflict | yes | 3.581s | 4,270 | 36 |
| `adversarial_no_fake_file` | say `UNVERIFIED`, do not invent file contents | yes | 2.243s | 4,276 | 25 |

Measured result: **3/3 messy/adversarial smoke prompts passed**. This is a better hallucination guard than the earlier deterministic-only benchmark, but it is still small and should not be presented as a final hallucination-rate study.

## Hallucination rate

Target failure types:

- incorrect claims about prior context or code,
- fabricated functions, files, or outputs,
- unsupported claims about tool results,
- wrong context restored.

Current mitigation:

- compact refs include summaries and exact local IDs,
- exact text can be requested with `.ctx_get`,
- auto-restore is bounded and intent/topic gated,
- direct-answer turns avoid carrying unrelated tool schemas and old bulky refs.

Measured hallucination rate is **not yet available** as a controlled percentage. Recommended protocol: sample 200 context-dependent questions from saved sessions, require citations to exact restored context, and grade each answer as correct, partially correct, hallucinated, or refusal/unknown.

## Context recall accuracy

Kcode supports two recall paths:

1. automatic bounded restore for high-confidence relevant context,
2. explicit `.ctx_get id=ctx:<hash> reason=<why>` exact rehydration.

Recommended metrics:

| Metric | Definition |
|---|---|
| Precision | restored blocks that were actually needed / restored blocks |
| Recall | needed blocks restored / needed blocks |
| Exactness | restored text byte-for-byte equals original local vault text |
| User repair rate | turns where the user had to manually paste or request missing context |

Current local telemetry records compression and restoration counters, but it does not yet include labeled ground truth for precision/recall.

## Long-session degradation

Measured signal: long-context events dominate the local telemetry and still show high reduction.

| Long-session proxy | Value |
|---|---:|
| Events with 80k+ original chars | 3,468 |
| Long-bucket reduction | 92.77% |
| Median encoded block count in non-zero events | 171 |
| p95 encoded block count in non-zero events | 640 |
| Max encoded block count | 662 |

Open question: accuracy over 50 to 200 turns should be measured with replay tasks that ask for old facts, old diffs, and old tool outputs at fixed turn intervals.

## Latency / response time

Current telemetry used for this document does not include end-to-end per-turn latency labels for compression, retrieval, model latency, and tool execution separately.

Recommended instrumentation:

- `compression_ms`,
- `vault_lookup_ms`,
- `memory_relevance_ms`,
- `tool_schema_filter_ms`,
- `provider_request_ms`,
- `time_to_first_token_ms`,
- `total_turn_ms`.

Expected low-risk optimization: dynamic tool-schema pruning should reduce provider serialization and prompt-processing latency on direct-answer turns because fewer schema tokens are sent.

## Cost efficiency

Approximate aggregate savings from telemetry:

- 5.65B chars avoided,
- roughly 1.41B tokens avoided with a chars/4 heuristic,
- 92.76% aggregate context-size reduction vs full-context approximation.

Cost per completed task is not yet measured because it requires controlled success labels. The recommended report should include:

```text
cost_efficiency = provider_cost / completed_tasks
cost_adjusted_success = completed_tasks / provider_cost
```

Raw token savings alone are not enough: a cheaper run that fails the task is worse than a more expensive successful run.

## Determinism / reproducibility

Current design choices that improve reproducibility:

- deterministic summaries for compact refs,
- stable content hashes for context IDs,
- bounded auto-restore budgets,
- structural trivial-turn detection instead of a hardcoded word allowlist,
- dynamic tool filtering based on the latest user turn and tool names.

Recommended variance benchmark:

1. Replay the same 100 prompts five times with the same stored context.
2. Record restored context IDs and selected tool schemas.
3. Report exact-match rate of restored IDs and schema sets.
4. Separately report answer variance, because model generation can vary even when restored context is deterministic.

## Failure mode analysis

Failures should be labeled into at least these categories:

| Category | Example |
|---|---|
| Missed context | needed old diff/log/fact was not restored |
| Wrong context restored | semantically similar but unrelated block restored |
| Model reasoning error | correct context restored, wrong conclusion |
| Memory extraction error | bad memory saved or relevant memory omitted |
| Tool-use error | wrong tool, wrong args, or missed tool call |
| User ambiguity | task underspecified or contradictory |

The current telemetry proves large token reduction, but it does not yet provide labeled failure attribution.


## Storage footprint breakdown

The earlier raw `~/.kcode` footprint is not memory/vault-only. A targeted local breakdown showed:

| Path | Size |
|---|---:|
| `~/.kcode/build-src` | 28 GB |
| `~/.kcode/models` | 17 GB |
| `~/.kcode/builds` | 11 GB |
| `~/.kcode/logs` | 2.0 GB |
| `~/.kcode/interlang-stats.jsonl` | 2.7 MB |
| `~/.kcode/memory` | 472 KB |
| `~/.kcode/mcp.json` | 4 KB |

Interpretation: most local disk use in this profile is source/build/model/log data, not saved memories. A future vault-only benchmark should separate exact-context vault bytes from logs, model caches, source checkouts, and build artifacts.

## Scalability with context size

Measured local storage footprint under `~/.kcode` during this snapshot was about **119.16 GB** across all files, including builds, models, caches, source checkouts, and context artifacts. This is not the same as context-vault-only size.

Recommended scalability benchmark:

| Corpus size | Metrics |
|---|---|
| 10 MB | lookup latency, exact recall accuracy, disk overhead |
| 100 MB | same |
| 1 GB | same |
| 10 GB | same |
| 100 GB | same |

A future telemetry improvement should separate vault bytes, raw log bytes, embedding/index bytes, and build/model cache bytes.

## Tool-use accuracy

Current safety behavior:

- tool-like tasks still get relevant schemas,
- direct-answer tasks keep `tool_expand`, so the model can request more tools,
- native tool execution falls back locally when SDK-native execution fails.

Recommended benchmark:

1. Build a tool-use suite with shell, file editing, browser, web, Gmail, image, and GitHub tasks.
2. Run with full schemas vs dynamic schema pruning.
3. Compare correct tool selection, correct args, task success, and extra-turn rate caused by `tool_expand`.

## User intervention rate

Manual intervention examples:

- user has to paste old context,
- user asks for `.ctx_get`,
- assistant asks for information that exists in the vault,
- assistant uses the wrong remembered fact.

Current telemetry does not label these interventions. A useful metric is:

```text
intervention_rate = turns_with_manual_context_repair / total_turns
```

## Memory efficiency

Measured compression ratio from telemetry:

```text
encoded_chars / original_chars = 440,808,870 / 6,087,468,238 = 7.24%
```

Equivalently, the measured prompt-side context representation is about **13.8x smaller** than the full-context approximation across recorded events.

This is prompt representation efficiency, not vault disk efficiency. Vault disk efficiency should be measured separately by comparing raw transcript bytes, exact vault bytes, summaries, indexes, and embeddings.

## Cold-start vs warm-start performance

Definitions:

- **Cold start:** no session-local vault or recent memory warmed in process.
- **Warm start:** existing context vault, memory graph, and prior summaries available.

Expected tradeoff:

- cold start has less prior context to restore and may need more explicit file/tool inspection,
- warm start can restore exact old context but must avoid irrelevant carryover.

Controlled benchmark is not yet available. It should replay the same tasks from a clean profile and from a warmed profile, then compare token cost, latency, and success rate.

## Edge-case stress tests

Recommended stress fixtures:

- 1 MB and 10 MB stack traces,
- giant generated diffs,
- multi-file refactors across 20 to 100 files,
- ambiguous references such as "fix the thing from earlier",
- repeated logs with one important anomaly,
- adversarial similar filenames/functions,
- old context that is relevant by exact ID but weak by semantic similarity.

Success criteria:

- no context-window overflow,
- correct exact block restored when needed,
- no fabricated files/functions/tool outputs,
- bounded latency and bounded auto-restore size,
- user can force exact recall with `.ctx_get`.

## Reproducing the measured telemetry summary

From a Kcode profile with `~/.kcode/interlang-stats.jsonl`:

```bash
python3 - <<'PY'
import json, pathlib
p = pathlib.Path.home() / '.kcode/interlang-stats.jsonl'
rows = []
for line in p.read_text(errors='ignore').splitlines():
    try:
        rows.append(json.loads(line))
    except Exception:
        pass
orig = [int(r.get('original_chars') or 0) + int(r.get('diet_original_chars') or 0) + int(r.get('raw_context_avoided_chars') or 0) for r in rows]
enc = [int(r.get('encoded_chars') or 0) + int(r.get('diet_encoded_chars') or 0) for r in rows]
saved = [max(0, o - e) for o, e in zip(orig, enc)]
print('events', len(rows))
print('original_chars', sum(orig))
print('encoded_chars', sum(enc))
print('saved_chars', sum(saved))
print('reduction_pct', 100 * sum(saved) / sum(orig) if sum(orig) else 0)
PY
```
