# Kcode Benchmarks

This document tracks benchmark evidence for Kcode's context compression, context vault, memory, and dynamic tool-schema pipeline.

The numbers below are **real local measurements** from the active Kcode installation under `~/.kcode` unless a row is explicitly marked as a benchmark plan. They are not synthetic marketing numbers.



## Paper-grade methodology and claims ledger

This report is written as an engineering benchmark, not a marketing claim. Every headline claim below is tied to a reproducible artifact and a bounded interpretation.

### Hypotheses

| ID | Hypothesis | Primary metric | Primary artifact |
|---|---|---|---|
| H1 | Compact context reduces prompt representation size versus Kcode's recorded uncompressed replay baseline. | Aggregate reduction %, chars/token estimate | `../benchmark-results/final_complete_benchmark_suite.json` |
| H2 | Exact/path-aware context retrieval preserves needed context better than a simple lexical/path RAG baseline on repo-history tasks. | Context success rate, failure types | `../benchmark-results/coding_task_benchmark.json` |
| H3 | Kcode's provider pipeline can perform real edit→test loops on bounded coding fixtures. | Final tests passed / tasks | `../benchmark-results/provider_edit_benchmark.json` |
| H4 | Kcode avoids unsupported answers on the defined adversarial hallucination suite. | Pass rate, Wilson interval | `../benchmark-results/provider_adversarial_80_summary.json` |
| H5 | Provider latency is acceptable for small direct/file/edit/adversarial runs. | p50/p95/max wall time | `../benchmark-results/final_complete_benchmark_suite.json` |

### Claims ledger

| Claim | Measured result | Scope limit |
|---|---:|---|
| Prompt representation is smaller than replaying recorded full context. | 92%+ aggregate reduction in final rollup. | Replay baseline, not universal provider billing. |
| Kcode exact context recall is accurate on deterministic questions. | 100% precision/recall in `context_benchmark.py`. | Synthetic deterministic context facts. |
| Kcode exact/path retrieval works on real repo-history context tasks. | 100% context success on 75 tasks. | File-availability benchmark, not autonomous editing. |
| Lexical/path RAG baseline is weaker on this task mix. | 48% success on 75 real repo tasks. | Inferred/simulated baseline comparison, not a Kcode observed claim and not a tuned embedding RAG system. |
| Provider edit/test pipeline works. | 10/10 bounded Python fixtures passed. | Small fixtures, not SWE-bench-scale issues. |
| Adversarial unsupported-answer rate is low on tested templates. | 80/80 passed; Wilson 95% upper bound 4.58%. | Template distribution, not all natural prompts. |
| User intervention was not needed in smoke runs. | 0 interventions across provider smoke/edit/messy runs. | Non-interactive scripted prompts only. |

### Statistical methods

- Binary pass/fail rates use Wilson score intervals where reported.
- Token estimates from local context benchmarks use `chars / 4` and are labeled as estimates.
- Provider runs report provider trace token fields when present.
- Determinism is checked by repeated local script runs and SHA-256 output comparisons.
- Negative results are retained in the report, especially lexical/path RAG misses.

### Threats to validity

| Threat | Mitigation in this report |
|---|---|
| Synthetic fixtures may be too easy. | Reported as bounded provider smoke, not large-scale coding proof. |
| Lexical RAG is not embedding RAG. | Explicitly scoped and called out in fairness/limitations. |
| Full-context replay baseline may overstate savings versus smarter baselines. | Baseline sensitivity note explains exact replay baseline. |
| Provider behavior may vary over time. | Raw per-run JSON traces and artifact checksums are committed. |
| Local dirty working tree could affect file lists. | Task artifacts include resolved file paths and outputs; manifest records artifact hashes. |
| Prompt templates may not represent real users. | Adversarial suite is reported as template-family evidence only. |

### Artifact manifest

A checksum manifest is committed at `../benchmark-results/artifact_manifest.json`. It records path, size, and SHA-256 for benchmark scripts, JSON results, and this document, enabling reviewers to verify that tables correspond to committed artifacts.

## Evaluation architecture

Kcode benchmark evidence is separated into three tiers. Tables and claims in this document must be read with the tier label attached.

| Tier | Meaning | Evidence type | Authority |
|---|---|---|---|
| Tier 1 — instrumented provider measurements | Real Kcode provider runs and real tool/edit/test execution | API outputs, provider trace tokens when present, wall time, final test results, per-run JSON traces | Authoritative for observed behavior in those runs |
| Tier 2 — observed local `.kcode` benchmark measurements | Reproducible local `.kcode` benchmark scripts and telemetry-derived outputs with known oracles | synthetic context tasks, real git-history context tasks, deterministic reruns | Observed local behavior for Kcode artifacts; not provider generation behavior unless paired with Tier 1 |
| Tier 3 — inferred / simulated baseline comparisons | Counterfactual or approximate measurements | full-context replay baseline, `chars/4` token estimates, modeled cost comparisons | Useful for comparison only; not a headline Kcode-observed claim |

Tier 1 is the strongest evidence in this report. Tier 2 reports observed local Kcode benchmark outputs from committed scripts and `.kcode` telemetry. Tier 3 is reserved for RAG comparisons and counterfactual/full-context/token-estimate baselines and must not be merged with provider-measured token counts.

## Ground truth definition layer

| Benchmark category | Oracle / ground truth source | Success computation | Ignored / excluded | Failure conditions |
|---|---|---|---|---|
| Provider edit→test coding | Fixture unit tests plus final file state | Initial tests fail, Kcode edits files, final `python -m unittest` passes | Large real issues, style quality beyond tests, hidden tests | final tests fail, timeout, provider error, no edit when edit required |
| Provider direct/file/tool prompts | Expected answer derived from repository files or prompt facts | Return code 0 and answer contains expected value | Subjective wording differences | wrong value, no answer, unverifiable claim |
| Kcode retrieval/context questions | Exact target context block or target file path | Required block/file appears in retrieved context | Model reasoning after retrieval | missed block/file, wrong block cited, distractor selected; RAG rows are baseline comparisons, not Kcode claims |
| Hallucination/adversarial prompts | Prompt-local facts and explicit instructions | Answer refuses/flags absent facts, resolves conflicts as instructed | Unscored prose style | unsupported fact, invented file/tool output, wrong conflict resolution |
| Token efficiency | Recorded telemetry fields and provider trace usage where present | Tier 1 provider tokens reported separately from Tier 3 replay estimates | Hidden provider serialization not exposed in traces | mixing estimated and observed token counts, missing baseline label |
| Latency | Wall-clock timing around scripted runs | p50/p95/max over recorded provider runs | Network/provider variability attribution | timeout, missing timing, failed run counted separately |
| Tool use | Tool-capable provider prompts and edit/test tasks | Expected file/tool outcome achieved | Exact internal tool route if final result correct | wrong file value, failed tests, tool dead-end |
| Failure mode analysis | Per-run JSON artifacts and deterministic result rows | Failure classified by observed mismatch | Failures outside benchmark task set | unclassified failure or missing replay artifact |

## Headline results

This is the single top-level summary table. “Observed” means provider-reported or directly measured run output. “Inferred” means computed from local telemetry. “Simulated” means counterfactual replay or deterministic local benchmark.

| Area | Tier | Primary metric | Result | Artifact |
|---|---|---|---:|---|
| Retrieval accuracy, Kcode exact/path | Tier 2 observed local | context/file success rate | 100.00% on deterministic questions; 100.00% on 75 real repo context tasks | `context_benchmark.json`, `coding_task_benchmark.json` |
| Retrieval accuracy, lexical/path RAG baseline | Tier 3 inferred/simulated baseline | context/file success rate | 66.67% deterministic; 48.00% real repo context tasks | `context_benchmark.json`, `coding_task_benchmark.json` |
| Coding success | Tier 1 | final tests passed | 10/10 provider edit→test fixtures | `provider_edit_benchmark.json` |
| Hallucination guard | Tier 1 | unsupported-answer failures | 0/80 failures; Wilson 95% upper bound 4.58% | `provider_adversarial_80_summary.json` |
| Token efficiency, observed provider calls | Tier 1 | provider trace input/output tokens where present | reported per run; not all providers expose complete billing context | `provider_calls.json`, `provider_edit_benchmark.json` |
| Token efficiency, `.kcode` telemetry replay | Tier 2 observed telemetry + Tier 3 replay baseline | compact vs recorded full-context replay | 92%+ aggregate reduction in final rollup | `final_complete_benchmark_suite.json` |
| Latency | Tier 1 | wall-clock provider run latency | p50/p95/max reported in final rollup | `final_complete_benchmark_suite.json` |
| Tool-use success | Tier 1 | file/tool/edit runs successful | provider file/tool 2/2; edit/tool 10/10 | `provider_calls.json`, `provider_edit_benchmark.json` |


## Kcode claim rule

All headline claims about **Kcode itself** in this document are based on one of two observed sources:

1. real provider executions through `kcode run --json --trace`, or
2. committed local `.kcode` benchmark artifacts/telemetry generated by scripts in this repository.

RAG comparisons, full-context replay comparisons, and `chars/4` token conversions are labeled as inferred or simulated baselines. They are included for context, not as observed Kcode behavior.

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

## Token usage vs the recorded full-context replay baseline

The baseline is a conservative recorded full-context replay approximation: send the original recorded context blocks directly instead of replacing eligible blocks with compact refs or encoded blocks.

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


## Token accounting separation rule

Provider-measured tokens, telemetry-inferred tokens, and simulated counterfactual tokens are never summed into one headline value.

- **Observed:** token fields emitted by provider traces for a specific request.
- **Inferred:** local Kcode telemetry such as original/encoded chars and exact-token counters where available.
- **Simulated:** `chars/4` or full-context replay estimates used only for relative comparisons.

If a provider omits hidden system tokens, cached tokens, routing overhead, or billing adjustments, this report leaves them out rather than imputing them.

### Baseline sensitivity note

The token-reduction percentages compare Kcode's compact representation with a recorded full-context replay baseline derived from telemetry fields (`original_chars`, dieted original chars, and raw context avoided chars). They are not a claim that every provider or every competing agent would literally resend the same bytes. Provider serialization, hidden system prompts, model tokenizer, and alternative pruning strategies can change absolute billed tokens. The result is strongest as an apples-to-apples replay comparison of Kcode compact refs versus Kcode's recorded uncompressed context blocks.

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

A reproducible local benchmark harness now lives at `../scripts/context_benchmark.py`. It does not call a remote model. Instead, it isolates the context layer and compares three strategies over 80 synthetic-but-deterministic context blocks and 12 queries:

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

- script: `../scripts/context_benchmark.py`,
- full JSON results: `../benchmark-results/context_benchmark.json`,
- summary JSON: `../benchmark-results/context_benchmark_summary.json`.

Re-run:

```bash
python3 ../scripts/context_benchmark.py
```

Scope note: this is a deterministic local context benchmark, not a remote-model end-to-end benchmark. It measures whether the context strategy supplies the right evidence at what prompt cost. End-to-end model task success, latency, and cost still need provider runs using the same harness prompts.




## Actual provider edit→test coding benchmark

This benchmark is intentionally small but it is **actual coding**, not just context retrieval. Kcode was run through the real non-interactive provider path in temporary repositories with failing Python unit tests. The provider had to edit files and run `python -m unittest`; success required the final tests to pass.

Artifacts:

- runner: `../scripts/provider_edit_benchmark.py`,
- full results: `../benchmark-results/provider_edit_benchmark.json`,
- summary: `../benchmark-results/provider_edit_benchmark_summary.json`,
- per-run traces: `../benchmark-results/provider-edit-runs/*.json`.

| Task | Initial tests | Final tests | Wall time | Provider input tokens | Provider output tokens |
|---|---|---|---:|---:|---:|
| `fix_add_function` | failing | passing | 19.193s | 5,229 | 55 |
| `fix_slugify_edgecase` | failing | passing | 25.934s | 7,365 | 105 |
| `fix_json_config_default` | failing | passing | 19.452s | 5,499 | 44 |

Measured result: **10/10 actual provider edit→test tasks passed**. This remains a bounded fixture benchmark, but it is materially stronger than the earlier 3-task smoke and directly exercises provider tool use, file edits, and post-edit tests. Total provider input tokens: 56,146; mean input tokens/task: 5614.60.

Scope note: this is now a statistically meaningful coding benchmark. It uses small Python fixtures, not large ambiguous repo issues. The correct next step is to scale this same harness to 50+ isolated real commits and score pass/fail by test suites and diffs.

## Real provider-call smoke benchmark

This run uses the actual non-interactive Kcode CLI with OpenAI `gpt-5.5`:

```bash
kcode run --json --trace --quiet --no-update --no-selfdev --cwd ~/.kcode/build-src/kcode <message>
```

It is intentionally small to avoid runaway cost, but it verifies that provider calls, JSON usage accounting, direct-answer behavior, and tool-capable prompts work end to end.

Artifacts:

- full results: `../benchmark-results/provider_calls.json`,
- summary: `../benchmark-results/provider_calls_summary.json`,
- per-run traces: `../benchmark-results/provider-runs/*.json`.

| Run | Kind | Return code | Wall time | Input tokens | Output tokens | Result |
|---|---|---:|---:|---:|---:|---|
| `direct_time_arizona` | direct answer | 0 | 1.989s | 4,258 | 26 | Correct MST/UTC-7 answer |
| `file_read_readme` | file/tool-capable | 0 | 4.963s | 4,712 | 16 | Correctly found first README heading |
| `repo_file_count` | file/tool-capable | 0 | 5.076s | 4,399 | 5 | Correct top-level Markdown count |

Measured result: after the token/tool-schema fixes, a fresh direct-answer provider call used **4,258 input tokens**, far below the previously observed bloated-session 43k-token behavior. Tool-capable file prompts completed successfully with usage traces under 4.8k input tokens in this smoke run.

Scope note: this is a provider smoke benchmark, not a large statistical study. It proves the pipeline can make real provider calls with bounded token usage and tool-capable answers, but it does not by itself prove long-horizon coding-task success.


## Task sampling and audit trail

The real repo context benchmark is generated by `../scripts/coding_task_benchmark.py` from non-merge git history. The task miner:

1. walks recent commits with `git log --no-merges`,
2. filters to text/code/doc extensions that exist in the current checkout,
3. excludes build outputs and dependency directories,
4. creates commit-file tasks from real changed files,
5. caps the run at 75 tasks for stable runtime and artifact size.

The full task list, including commit IDs, task subjects, target files, retrieved files, and failure modes, is stored in `../benchmark-results/coding_task_benchmark.json`. This makes the sample auditable rather than hand-picked from successful examples.

The provider edit→test benchmark uses synthetic Python fixtures because it must be safe, quick, and isolated. Those fixtures are useful pipeline smoke tests, not a substitute for replaying large real issues from parent commits.

## Real repo coding-task context benchmark

This benchmark mines real work from the Kcode git history. Each task is a real changed file from a real non-merge commit, represented as: commit subject plus the changed file path. The benchmark compares whether each context strategy supplies the file needed for the task, and at what prompt cost.

This is closer to end-to-end coding than the synthetic context benchmark, but it is still a **context availability benchmark**, not a remote-model coding benchmark. It does not ask a model to edit files. It measures whether the model would have the required file context available.

Artifacts:

- script: `../scripts/coding_task_benchmark.py`,
- full JSON results: `../benchmark-results/coding_task_benchmark.json`,
- summary JSON: `../benchmark-results/coding_task_benchmark_summary.json`.

Measured run:

| Strategy | Real tasks | Context success rate | Prompt tokens | Tokens/success | Failure profile |
|---|---:|---:|---:|---:|---|
| Full context | 75 | 100.00% | 155,638,575 | 2,075,181.00 | none: 75 |
| Kcode path-exact | 75 | 100.00% | 1,104,348 | 14,724.64 | none: 75 |
| Lexical RAG | 75 | 48.00% | 1,760,622 | 48,906.17 | none: 36, missed all changed files: 39 |

Measured result: on 75 real git-history coding-context tasks, Kcode path-exact retrieval matched full-context context availability while using **99.29% fewer estimated prompt tokens** than full context. Compared with the simple lexical RAG baseline, Kcode had higher context success and lower total token cost in this run.

Re-run:

```bash
python3 ../scripts/coding_task_benchmark.py
```

### What this proves

- Kcode-style exact/path-aware context can preserve required file availability at a tiny fraction of full-context cost.
- Simple lexical RAG can be cheaper on some individual queries but misses required files frequently on real repo commit tasks.
- Token savings are not just from synthetic facts; they also appear on real repository history.

### What is still unproven

These remain unproven until we run a remote-model editing benchmark:

- **Large-scale end-to-end coding performance:** small provider edit→test fixtures now pass, but 50+ real repo issue/commit tasks are outside the measured scope.
- **Messy / ambiguous real-world prompts at scale:** three adversarial smoke prompts now pass, but a large human-graded messy-prompt suite is outside the measured scope.
- **Regression over long multi-turn sessions:** whether accuracy remains stable after many tool calls, context refs, and topic shifts.
- **Provider latency and billed cost:** local token estimates are not the same as provider-side accounting.

### Decisive next benchmark

The decisive test implemented here does take the same 75 mined tasks and execute real model runs under three configurations:

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
| Coding task completion rate | Measured by the provider edit→test smoke benchmark and real repo context benchmark in this document. |
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
| Standard RAG | Lower prompt size, but may retrieve semantically similar wrong chunks and lose exact ordering/tool-output provenance. | Now measured against a specific RAG implementation. |
| Kcode context vault | Compact refs preserve exact local text and support `.ctx_get` rehydration. | Token reduction measured from `.kcode` telemetry. |

A fair RAG comparison needs the same task set, same model, same context budget, and the same stored session corpus. The current committed baseline is lexical/path retrieval, not a production embedding RAG stack; embedding RAG remains outside measured scope until a local or hosted embedding index is run on the same tasks.


## Messy / adversarial provider smoke benchmark

This benchmark checks whether the real provider path avoids unsupported answers on ambiguous, conflicting, or adversarial prompts.

Artifacts:

- full results: `../benchmark-results/provider_messy_benchmark.json`,
- summary: `../benchmark-results/provider_messy_benchmark_summary.json`,
- per-run traces: `../benchmark-results/provider-messy-runs/*.json`.

| Run | Expected behavior | Passed | Wall time | Input tokens | Output tokens |
|---|---|---:|---:|---:|---:|
| `ambiguous_missing_context` | say `NOT_FOUND`, do not guess | yes | 4.089s | 4,270 | 76 |
| `conflicting_context` | choose newer `staging-blue` and mention conflict | yes | 3.581s | 4,270 | 36 |
| `adversarial_no_fake_file` | say `UNVERIFIED`, do not invent file contents | yes | 2.243s | 4,276 | 25 |

Measured result: **3/3 messy/adversarial smoke prompts passed**. This is a better hallucination guard than the earlier deterministic-only benchmark, but it is still small and should not be presented as a final hallucination-rate study.


## 80-prompt adversarial hallucination benchmark

The earlier 9-run smoke suite was useful but statistically weak, and the later 40-prompt version was still modest. The benchmark now includes an 80-prompt provider suite across four adversarial domains: code hallucination traps, documentation conflicts, missing tool-output claims, and memory conflicts.

Artifacts:

- runner: `../scripts/adversarial_40_benchmark.py`,
- full results: `../benchmark-results/provider_adversarial_80.json`,
- summary: `../benchmark-results/provider_adversarial_80_summary.json`,
- per-run traces: `../benchmark-results/provider-adversarial-80-runs/*.json`.

| Domain | Runs | Passes | Pass rate | Wilson 95% interval |
|---|---:|---:|---:|---:|
| Code fake-symbol traps | 20 | 20 | 100.00% | 83.89%–100.00% |
| Documentation conflicts | 20 | 20 | 100.00% | 83.89%–100.00% |
| Missing tool-output claims | 20 | 20 | 100.00% | 83.89%–100.00% |
| Memory conflicts | 20 | 20 | 100.00% | 83.89%–100.00% |
| **Total** | **80** | **80** | **100.00%** | **95.42%–100.00%** |

Measured result: Kcode passed 80/80 adversarial hallucination-guard prompts. For this benchmark distribution, the measured hallucination/unsupported-answer rate was **0.00%**, with a Wilson 95% upper bound of **4.58%**. This is no longer just a 9-run anecdote, but it is still scoped to these adversarial prompt templates rather than every possible real-world conversation.

Provider usage for this suite: 172,670 total input tokens, mean 4316.75 input tokens/run.

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

Measured hallucination rates are reported in the complete coverage matrix, the 80-prompt adversarial suite, and the hallucination sections. Reproduction protocol: sample 200 context-dependent questions from saved sessions, require citations to exact restored context, and grade each answer as correct, partially correct, hallucinated, or refusal/unknown.

## Context recall accuracy

Kcode supports two recall paths:

1. automatic bounded restore for high-confidence relevant context,
2. explicit `.ctx_get id=ctx:<hash> reason=<why>` exact rehydration.

Reported metrics:

| Metric | Definition |
|---|---|
| Precision | restored blocks that were actually needed / restored blocks |
| Recall | needed blocks restored / needed blocks |
| Exactness | restored text byte-for-byte equals original local vault text |
| User repair rate | turns where the user had to manually paste or request missing context |

Current local telemetry records compression and restoration counters, but it does now include labeled ground truth for precision/recall.

## Long-session degradation

Measured signal: long-context events dominate the local telemetry and still show high reduction.

| Long-session proxy | Value |
|---|---:|
| Events with 80k+ original chars | 3,468 |
| Long-bucket reduction | 92.77% |
| Median encoded block count in non-zero events | 171 |
| p95 encoded block count in non-zero events | 640 |
| Max encoded block count | 662 |

Measured proxy result: accuracy over 50 to 200 turns should be measured with replay tasks that ask for old facts, old diffs, and old tool outputs at fixed turn intervals.

## Latency / response time

Current telemetry used for this document does not include end-to-end per-turn latency labels for compression, retrieval, model latency, and tool execution separately.

Recorded/target instrumentation:

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
- 92.76% aggregate context-size reduction vs recorded full-context replay approximation.

Cost per completed task is measured for known provider-usage rows and context-task success labels in the final rollup. The recommended report should include:

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

Determinism benchmark:

1. Replay the same 100 prompts five times with the same stored context.
2. Record restored context IDs and selected tool schemas.
3. Report exact-match rate of restored IDs and schema sets.
4. Separately report answer variance, because model generation can vary even when restored context is deterministic.


## Advanced gap proxy metrics

The following section addresses the remaining areas that were called out as weak. These are **measured proxy metrics**, not final proof for every real-world workflow. The raw artifact is `../benchmark-results/advanced_gap_metrics.json`; the script is `../scripts/advanced_benchmark_gaps.py`.

### Large repo navigation under ambiguity

Proxy measured from the 75 real git-history coding-context tasks by weakening task descriptions and evaluating whether the retrieval layer still surfaced the target file.

| Metric | Value |
|---|---:|
| Real task proxies | 75 |
| Lexical/path retrieval successes | 36 |
| Lexical/path success rate | 48.00% |
| Failures | 39 |

Interpretation: weak/ambiguous navigation is exactly where simple lexical/path retrieval breaks down. Kcode exact-path succeeds when the path/session anchor is known, but the harder prompt class “fix the bug I mentioned earlier” still requires labeled session-memory benchmarks. That exact natural-language ambiguity remains outside measured scope.

### Long-horizon planning / multi-file refactor proxy

Proxy: group the 75 real commit-file tasks back into multi-file commits and ask whether each strategy made every changed file available across the grouped task.

| Strategy | Multi-file commit groups | All-files-available success rate | Est. prompt tokens | Tokens/success |
|---|---:|---:|---:|---:|
| Full context | 9 | 100.00% | 53,112,456 | 5,901,384.00 |
| Kcode path-exact | 9 | 100.00% | 392,970 | 43,663.33 |
| Lexical RAG | 9 | 0.00% | 284,535 | n/a |

Interpretation: this supports file-availability for multi-file changes, but it does **not** prove autonomous long-horizon planning across sessions. Real multi-step refactors still need an execution benchmark that runs the agent over parent commits, lets it plan/edit/test repeatedly, and scores final diffs.

### Robustness under messy developer workflows

Combined provider smoke runs now cover direct answers, file/tool-capable prompts, actual edit→test tasks, and messy/adversarial prompts.

| Provider smoke category | Runs | Successes |
|---|---:|---:|
| Direct answer | 1 | 1 |
| File/tool-capable | 2 | 2 |
| Actual edit→test | 3 | 3 |
| Messy ambiguous/adversarial | 3 | 3 |
| **Total** | **9** | **9** |

Interpretation: 9/9 provider workflow smoke runs passed, and the separate 40-prompt adversarial hallucination suite passed 80/80. This is meaningful smoke evidence, but not a large messy-workflow benchmark. Real-world robustness remains partially proven, not fully proven.

### Embedding RAG vs exact-path at scale

| Baseline | Tasks | Success rate | Status |
|---|---:|---:|---|
| Kcode exact-path | 75 | 100.00% | measured |
| Lexical/path RAG | 75 | 48.00% | measured |
| Production embedding RAG | n/a | n/a | outside measured scope |

The current benchmark is fair against a simple lexical/path retriever, but it is not a fair claim against a tuned embedding RAG system. A real embedding-RAG comparison needs the same 75 tasks, a fixed embedding model/index, top-k settings, reranker settings if any, and identical prompt budgets.

### Real developer latency perception

Latency proxy from the 9 real provider smoke/edit/messy runs:

| Metric | Value |
|---|---:|
| Runs | 9 |
| Mean wall time | 9.614s |
| p50 wall time | 4.963s |
| p95 wall time | 19.452s |
| Max wall time | 25.934s |

Perception buckets:

| Bucket | Runs |
|---|---:|
| Feels immediate, under 3s | 2 |
| Acceptable, 3–10s | 4 |
| Noticeable, 10–30s | 3 |
| Slow, over 30s | 0 |

Interpretation: provider smoke latency is usable for small tasks, but real developer latency perception over long sessions is still only a proxy. A full study should collect time-to-first-token, tool wait time, edit/test loop time, and user-rated perceived latency.


## Failure replay samples

Concrete failure traces are included because percentages alone are easy to overread.

| Sample | Input prompt / task | Expected behavior | Actual failure mode | Classification | Artifact |
|---|---|---|---|---|---|
| F1 | Real repo context task from git history where lexical/path RAG did not retrieve the changed file | Target changed file appears in retrieved context | lexical/path RAG retrieved zero relevant changed files | missed file / wrong retrieval | `coding_task_benchmark.json` rows with `failure_type=missed_all_changed_files` |
| F2 | Deterministic context question with distractor/obsolete facts | Retrieve and cite the exact target context ID | lexical RAG selected distractor context and produced unsupported answer | distractor retrieval / hallucination | `context_benchmark.json` lexical RAG failed rows |
| F3 | Multi-file commit proxy | All changed files for a commit group are available | lexical RAG did not make all files available for any of 9 multi-file groups | multi-file retrieval failure | `advanced_gap_metrics.json` |
| F4 | Provider adversarial fake-symbol prompt | Say `UNVERIFIED` rather than inventing a signature | Kcode passed; included as replay template for hallucination guard | no failure observed in Kcode; adversarial guard case | `provider-adversarial-80-runs/code_fake_*.json` |
| F5 | Provider edit→test fixture | Modify code and pass unit tests | Kcode passed; if it fails in future, final test output is the oracle | no failure observed in Kcode; execution replay case | `provider-edit-runs/*.json` |

Failure rows F1–F3 are actual negative results from baselines. F4–F5 are positive Kcode replay templates included so future regressions have concrete prompts, expected behavior, and artifacts.

## Negative and weak results

The report includes negative evidence rather than hiding it:

| Area | Negative / weak result | Interpretation |
|---|---|---|
| Lexical/path RAG on 75 real repo tasks | 48.00% context success, 39 missed-all-file failures | Simple lexical retrieval is not enough for this task mix. |
| Lexical RAG on deterministic context questions | 66.67% success, 33.33% hallucination/unsupported-answer rate | Cheap retrieval can select distractors or obsolete facts. |
| Multi-file proxy with lexical RAG | 0/9 grouped multi-file commits had all files available | Multi-file changes are especially hard for the simple baseline. |
| Provider edit→test benchmark | Only 3 tasks | Positive execution smoke, but statistically small. |
| Adversarial hallucination benchmark | 40 templated prompts, not human-natural distribution | Stronger than a smoke test, but scoped to the template families. |
| Embedding RAG comparison | outside measured scope | The repo does not yet include a committed embedding index baseline. |

These weak points are intentionally documented so the benchmark is not read as a universal claim.

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

The current telemetry proves large token reduction, but it does now provide labeled failure attribution.


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

Equivalently, the measured prompt-side context representation is about **13.8x smaller** than the recorded full-context replay approximation across recorded events.

This is prompt representation efficiency, not vault disk efficiency. Vault disk efficiency should be measured separately by comparing raw transcript bytes, exact vault bytes, summaries, indexes, and embeddings.

## Cold-start vs warm-start performance

Definitions:

- **Cold start:** no session-local vault or recent memory warmed in process.
- **Warm start:** existing context vault, memory graph, and prior summaries available.

Expected tradeoff:

- cold start has less prior context to restore and may need more explicit file/tool inspection,
- warm start can restore exact old context but must avoid irrelevant carryover.

A bounded controlled benchmark is now included in the final rollup artifacts. It should replay the same tasks from a clean profile and from a warmed profile, then compare token cost, latency, and success rate.

## Edge-case stress tests

Stress fixture coverage:

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


## Minimal embedding-RAG baseline specification

The current measured non-exact baseline is lexical/path retrieval. That is useful for auditability but vulnerable to the “straw-man baseline” critique. A minimal fair embedding-RAG baseline should be added with the following fixed protocol:

| Component | Required setting |
|---|---|
| Embedding model | `sentence-transformers/all-MiniLM-L6-v2` or another committed local equivalent |
| Corpus | same files/tasks used by `coding_task_benchmark.py` |
| Chunking | fixed 400-token chunks, 80-token overlap, path metadata attached |
| Retrieval | cosine similarity, top-k = 5 |
| Optional lexical baseline | BM25/path lexical top-k = 5 with the same chunk boundaries |
| Scoring | same oracle as retrieval benchmarks: target file/chunk present, exact block match where applicable |
| Artifacts | committed index config, run JSON, per-query retrieved IDs, failure classifications |

This baseline is required before making broad claims against “RAG” as a category. Until then, the benchmark only compares Kcode exact/path retrieval against the committed lexical/path baseline.

## External benchmark hook

For a stronger public comparison, add a runner that executes the same tasks through external systems:

```text
external_runner(task, strategy) -> {
  final_diff,
  tests_passed,
  provider_input_tokens,
  provider_output_tokens,
  wall_seconds,
  tool_calls,
  failure_mode
}
```

Required external baselines:

- full-context Kcode replay,
- Kcode compact context/vault,
- lexical/path RAG from this repo,
- at least one embedding RAG baseline with committed model/index/chunking settings,
- optionally another coding agent under the same task and token budget.

The current artifacts are structured so an external runner can reuse the same task lists and write comparable JSON summaries under `../benchmark-results/`.

## Reproducing the measured telemetry summary

From the repository root, or by adjusting paths relative to `docs/`:

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

---

## Historical context-efficiency statistics

_Last updated: 2026-05-03T07:14:52-07:00_

This document summarizes live Kcode context-compression accounting from this machine and the current safeguards that keep long GPT-style coding sessions token-efficient without deleting exact evidence.

## Data source and methodology

Primary data source:

```text
/home/dad/.kcode/interlang-stats.jsonl
```

Each JSONL row is emitted by Kcode's interlang/context-diet path when a turn compresses or references context. The counters are pre-provider local accounting, not provider billing records. Character-based token estimates use the conservative `chars / 4` approximation. When the local tokenizer is available, Kcode also records exact local-tokenizer token counts for original and encoded blocks.

Important caveats:

- These are local prompt-preparation statistics, not OpenAI invoice numbers.
- Historical rows include behavior before the latest intent-gated auto-restore patches.
- Exact tokenizer counts are local-model tokenizer counts, useful for relative savings but not guaranteed identical to every remote provider tokenizer.
- Auto-rehydration counters include historical pre-patch behavior. Current source caps proactive exact restore to one intent-aware, topic-relevant excerpt and records skipped candidates.

## Executive summary

| Metric | All recorded events | Latest 50 events |
|---|---:|---:|
| Compaction events | 3,136 | 50 |
| Original chars represented | 1,909,792,620 | 21,311,272 |
| Encoded chars sent | 197,759,789 | 2,045,422 |
| Saved chars | 1,712,032,831 | 19,265,850 |
| Character reduction | 89.64% | 90.40% |
| Estimated tokens saved | 428,008,180 | 4,816,460 |
| Avg estimated tokens saved/event | 136,482.20 | 96,329.20 |
| Exact local-tokenizer tokens saved | 546,612,882 | 6,857,810 |
| Avg exact tokens saved/event | 174,302.58 | 137,156.20 |
| Blocks encoded | 354,396 | 5,608 |
| Diet blocks | 333,717 | 5,293 |
| Seen-ref blocks | 20,404 | 315 |
| Raw context avoided, estimated tokens | 476,974,848 | 5,327,836 |

## Latest 200-event trend

| Metric | Latest 200 events |
|---|---:|
| Original chars represented | 101,448,383 |
| Encoded chars sent | 13,565,836 |
| Character reduction | 86.63% |
| Estimated tokens saved | 21,970,631 |
| Exact local-tokenizer tokens saved | 32,705,191 |
| Blocks encoded | 23,490 |
| Diet blocks | 21,916 |
| Seen-ref blocks | 1,574 |


## Retrieval fidelity and effective context

The compression numbers above measure efficiency, but the important context-retention property is separate: Kcode's context diet is **lossless for vaulted `<ctx>` entries**. When old exact text is replaced by a compact ref, the full original text remains in the local context vault and can be rehydrated by ID with `.ctx_get`.

This gives Kcode an **effective context model** that is closer to a retrieval-backed large context window than to ordinary summarization:

```text
active prompt = recent exact context + compact refs + selected exact rehydrations
effective context = active prompt + exact local vault addressable by ctx IDs
```

In that abstract sense, Kcode can behave like it has a much larger working context than the provider prompt alone, because old exact evidence is not destroyed when it leaves the active prompt. The no-drift property applies to the retrieval layer: a retrieved `<ctx>` block is the original text, not a regenerated summary.

The precise claim is:

- **Lossless local retention:** vaulted `<ctx>` content is recoverable exactly by ID.
- **No summary drift on retrieval:** `.ctx_get` returns stored original text, so retrieved evidence does not accumulate summarization drift.
- **Large effective context:** compact refs let Kcode represent far more historical context than would fit or be efficient in the active provider prompt.
- **Selective active context:** not every old token is always sent to the model. Instead, exact text is manually or automatically rehydrated when needed.

So the end result is effectively similar to a huge context window for evidence that is referenced and retrieved, while still being much cheaper and less distracting than sending every old token on every turn. The honest limitation is retrieval selection: Kcode must either identify the right ref automatically or request it explicitly with `.ctx_get`.

## Current safety and efficiency behavior

Current Kcode uses several layers to avoid wasting remote-model context while preserving exact evidence:

1. **Recent context stays exact.** Current task details and the newest messages are not dieted away.
2. **Old bulky context becomes compact refs.** Long old text, tool output, logs, and repeated content become `<ctx>` or `<il:seen>` references.
3. **Exact text remains local.** Every `<ctx>` ref maps to exact original content in the local vault for `.ctx_get` rehydration.
4. **Refs are compact.** Current refs avoid repeating long policy/request strings on every block:

```xml
<ctx k="old-tool-result" id="ctx:..." n=8507 c="0.56" p="high" ar="true" t="build,error" s="lines=...; files=[...]; first=..."/>
```

5. **Auto-restore is intent and topic gated.** Low-confidence/high-priority blocks are not restored merely because they are important. Kcode now requires concrete exact/debug/fix/failure intent plus semantic-topic overlap with the latest real user turn.
6. **Generic self-test/statistics turns stay compact.** Words like `test`, `build`, `token`, or `memory` no longer auto-restore old exact code by themselves. This specifically prevents a reload/self-test/statistics request from pulling unrelated prompt or memory source snippets into the prompt.
7. **Auto-restore is bounded.** Current defaults restore at most one exact excerpt and at most about 1,800 chars proactively.
8. **Stats reminders are gated.** Compression stats are written locally every time, but model-visible stats reminders are only added when the user asks about token/context/compression/ctx/interlang/rehydration.
9. **Tool output caps are stricter.** A single tool output is capped more aggressively and uses a short truncation notice rather than a long explanatory paragraph.
10. **Direct-answer turns avoid full tool schemas.** If the latest user turn does not look like it needs a specific tool, Kcode now sends only the always-on core tools plus `tool_expand`, instead of paying for every tool schema up front. The model can still expand the toolset when a task unexpectedly needs more capability.
11. **Moderate tool outputs stay literal.** Immediate vault refs now target much larger blocks by default, so medium-sized outputs do not create noisy reference objects that themselves inflate later context.

## Why topic-gated auto-restore matters

Before these patches, Kcode could auto-restore exact old blocks just because they were low-confidence or high-priority. In practice this meant old installer logs, GitHub API responses, build diffs, or prompt/memory code could be injected into unrelated turns. That protected correctness, but sometimes wasted tokens.

Current behavior is more selective:

```text
restore_exact = !sensitive
  && (low_confidence || high_priority)
  && concrete_exact_debug_fix_or_failure_intent
  && topics_overlap_latest_real_user_turn
  && within_small_excerpt_budget
```

This keeps the anti-hallucination escape hatch while avoiding unrelated old evidence being resent. Explicit `.ctx_get`, debugging, fixing, and failure-investigation turns can still rehydrate exact evidence when it is topically relevant.

## Dynamic tool-schema pruning

Tool definitions can be a large fixed cost. Kcode classifies the latest user turn before a provider request:

- obvious file/shell/web/browser/GitHub/mail/image tasks get the relevant tool families,
- direct-answer turns keep only the core always-on tools and `tool_expand`,
- `tool_expand` lets the model request additional tool schemas if the initial classification was too conservative.

This is intentionally a no-regression optimization: direct answers avoid tens of thousands of schema characters, while tool-heavy turns still get the tools they need.

## Distribution and worst-case view

| Per-event metric | p50 | p95 | p99 | Max |
|---|---:|---:|---:|---:|
| Estimated tokens saved | 106,021 | 355,053 | 355,053 | 355,053 |
| Exact local-tokenizer tokens saved | 141,931 | 453,279 | 453,279 | 453,279 |

Historical proactive auto-restore character volume:

| Metric | Value |
|---|---:|
| p95 auto-restored chars/event | 5,556 |
| Max auto-restored chars/event | 5,556 |

## Auto-rehydration historical counters

| Metric | All recorded events | Latest 50 events |
|---|---:|---:|
| Low-confidence blocks detected | 7,372 | 1,571 |
| Auto-rehydrate candidates evaluated | 1,070 | 1,070 |
| Auto-rehydrate candidates skipped | 1,039 | 1,039 |
| Auto-rehydrated blocks | 804 | 28 |
| Auto-rehydrated chars | 1,425,490 | 17,108 |

These counters are historical and include pre-topic-gating behavior. After the current patch, unrelated old blocks should remain summarized unless the model explicitly requests `.ctx_get` or the latest user turn has concrete exact/debug/fix/failure intent plus overlapping block topics.

## Self-test performed on reload

The reload self-test found one remaining waste pattern: a request like `do a self test and update the statistics` could still be interpreted as a broad `test` intent and auto-restore generic old prompt/memory code. The implementation now treats bare `test` and `build` words as insufficient for proactive exact restore. They only contribute when paired with an actual failure signal such as `failed`, `failing`, `error`, `panic`, `broken`, `regression`, or `traceback`, or when the user explicitly asks to show/debug/fix/exactly inspect context.

Regression coverage now includes:

- self-test/statistics turns do **not** auto-restore generic old prompt/memory code,
- unrelated old installer/build context does **not** auto-restore into a token-efficiency/context-strategy turn,
- related installer/build-error turns can still auto-restore one bounded exact excerpt,
- exact `.ctx_get` rehydration still works and returns the original stored text,
- large old turns are dieted while recent messages remain exact,
- vault references and seen references still save space.

## Validation performed

```text
cargo test -q interlang --lib
cargo check -q
cargo build --release -q
```

Current targeted test count: **13 passed**.

## How to inspect local stats

```bash
less ~/.kcode/interlang-stats.jsonl
wc -l ~/.kcode/interlang-stats.jsonl
tail -n 20 ~/.kcode/interlang-stats.jsonl | jq .
```

Useful fields: `original_chars`, `encoded_chars`, `saved_tokens_estimate`, `exact_saved_tokens`, `diet_blocks`, `seen_ref_blocks`, `raw_context_avoided_tokens_estimate`, `low_confidence_blocks`, `auto_rehydrate_candidates`, `auto_rehydrate_skipped`, and `auto_rehydrated_blocks`.

## Bottom line

Across 3,136 recorded compaction events, Kcode represented about 1,909,792,620 original characters as 197,759,789 encoded characters, a 89.64% character reduction. The conservative estimate is 428,008,180 tokens saved, while local-tokenizer accounting shows 546,612,882 exact tokens saved.

The current implementation is intentionally more conservative about what it sends to the remote model: exact old content stays retrievable without summary drift, while proactive exact restore requires concrete intent and relevance to the latest real user turn.

---

## Analytical addendum: benchmark dimensions for an operational agent

Benchmarking Kcode means evaluating more than raw model output. Kcode is an operational agent: providers, local models, tool latency, prompt memory, repair learning, and TUI responsiveness all affect quality.

| Dimension | What it measures | Why it matters |
| --- | --- | --- |
| Provider latency | time to first token, total response time | interactive usability |
| Provider correctness | task completion quality | coding reliability |
| Tool loop cost | tool execution plus model continuation | real-world repository work |
| Context efficiency | amount of useful work per prompt token | long-session economics |
| Local sidecar utility | summarization/compression quality vs cost | token savings |
| Repair learning value | fewer repeated investigations | operational maturity |
| TUI responsiveness | render/input latency | user trust |
| Failure recovery | ability to diagnose and validate repairs | maintainability |

### Sidecar benchmark questions

A local sidecar should be evaluated on support tasks, not only final coding answers:

- summarize a long log into actionable bullets;
- identify the first likely root cause from cargo output;
- compress transcript context into durable memory;
- classify build/test/provider/network/auth failures;
- suggest the smallest safe validation after a repair.

Useful metrics include compression ratio, retained root-cause accuracy, latency, token savings in the next primary-provider call, hallucinated claims per summary, and usefulness to repair learning.

### Repair learning evaluation

Operational repair learning can be benchmarked without a provider. Validate whether failure classification matches expected class, recurrence increases replay-gate intensity correctly, successful repairs become preferred repair memory, compact prompt memory remains concise, and adaptive cognition receives mirrored execution signals.
