# Kcode Context Efficiency Statistics

_Last updated: 2026-05-03T07:04:27-07:00_

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

## Current safety and efficiency behavior

Current Kcode uses several layers to avoid wasting remote-model context while preserving exact evidence:

1. **Recent context stays exact.** Current task details and the newest messages are not dieted away.
2. **Old bulky context becomes compact refs.** Long old text, tool output, logs, and repeated content become `<ctx>` or `<il:seen>` references.
3. **Exact text remains local.** Every `<ctx>` ref maps to exact original content in the local vault for `.ctx_get` rehydration.
4. **Refs are compact.** Current refs avoid repeating long policy/request strings on every block:

```xml
<ctx v=1 k="old-tool-result" id="ctx:..." h="..." n=8507 c="0.56" p="high" ar="true" t="build,error" s="lines=...; files=[...]; first=..." />
```

5. **Auto-restore is intent and topic gated.** Low-confidence/high-priority blocks are not restored merely because they are important. Kcode now requires concrete exact/debug/fix/failure intent plus semantic-topic overlap with the latest real user turn.
6. **Generic self-test/statistics turns stay compact.** Words like `test`, `build`, `token`, or `memory` no longer auto-restore old exact code by themselves. This specifically prevents a reload/self-test/statistics request from pulling unrelated prompt or memory source snippets into the prompt.
7. **Auto-restore is bounded.** Current defaults restore at most one exact excerpt and at most about 1,800 chars proactively.
8. **Stats reminders are gated.** Compression stats are written locally every time, but model-visible stats reminders are only added when the user asks about token/context/compression/ctx/interlang/rehydration.
9. **Tool output caps are stricter.** A single tool output is capped more aggressively and uses a short truncation notice rather than a long explanatory paragraph.

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
- exact `.ctx_get` rehydration still works,
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

The current implementation is intentionally more conservative about what it sends to the remote model: exact old content stays retrievable, but proactive exact restore now requires concrete intent and relevance to the latest real user turn.
