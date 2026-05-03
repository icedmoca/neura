# Kcode Context Efficiency Statistics

_Last updated: 2026-05-03T06:38:32-07:00_

This document summarizes live Kcode context-compression accounting from this machine and the current safeguards that keep long GPT-style coding sessions token-efficient without deleting exact evidence.

## Data source and methodology

Primary data source:

```text
/home/dad/.kcode/interlang-stats.jsonl
```

Each JSONL row is emitted by Kcode's interlang/context-diet path when a turn compresses or references context. The counters are pre-provider local accounting, not provider billing records. Character-based token estimates use the conservative `chars / 4` approximation. When the local tokenizer is available, Kcode also records exact local-tokenizer token counts for original and encoded blocks.

Important caveats:

- These are local prompt-preparation statistics, not OpenAI invoice numbers.
- Historical rows include behavior before the latest topic-gated auto-restore patch.
- Exact tokenizer counts are local-model tokenizer counts, useful for relative savings but not guaranteed identical to every remote provider tokenizer.
- Auto-rehydration counters include historical pre-patch behavior. Current source caps proactive exact restore to one intent-aware, topic-relevant excerpt and records skipped candidates.

## Executive summary

| Metric | All recorded events | Latest 50 events |
|---|---:|---:|
| Compaction events | 3,083 | 50 |
| Original chars represented | 1,886,329,423 | 34,139,685 |
| Encoded chars sent | 195,518,448 | 4,992,219 |
| Saved chars | 1,690,810,975 | 29,147,466 |
| Character reduction | 89.63% | 85.38% |
| Estimated tokens saved | 422,702,718 | 7,286,867 |
| Avg estimated tokens saved/event | 137,107.60 | 145,737.34 |
| Exact local-tokenizer tokens saved | 539,050,823 | 10,708,458 |
| Avg exact tokens saved/event | 174,846.20 | 214,169.16 |
| Blocks encoded | 348,253 | 7,809 |
| Diet blocks | 327,914 | 7,379 |
| Seen-ref blocks | 20,064 | 430 |
| Raw context avoided, estimated tokens | 471,109,029 | 8,534,939 |

## Latest 200-event trend

| Metric | Latest 200 events |
|---|---:|
| Original chars represented | 86,558,733 |
| Encoded chars sent | 12,358,278 |
| Character reduction | 85.72% |
| Estimated tokens saved | 18,550,108 |
| Exact local-tokenizer tokens saved | 27,524,056 |
| Blocks encoded | 18,980 |
| Diet blocks | 17,430 |
| Seen-ref blocks | 1,550 |

## Current safety and efficiency behavior

Current Kcode uses several layers to avoid wasting remote-model context while preserving exact evidence:

1. **Recent context stays exact.** Current task details and the newest messages are not dieted away.
2. **Old bulky context becomes compact refs.** Long old text, tool output, logs, and repeated content become `<ctx>` or `<il:seen>` references.
3. **Exact text remains local.** Every `<ctx>` ref maps to exact original content in the local vault for `.ctx_get` rehydration.
4. **Refs are compact.** Current refs avoid repeating long policy/request strings on every block:

```xml
<ctx v=1 k="old-tool-result" id="ctx:..." h="..." n=8507 c="0.56" p="high" ar="true" t="build,error" s="lines=...; files=[...]; first=..." />
```

5. **Auto-restore is topic-gated.** Low-confidence/high-priority blocks are not restored merely because they are important. Kcode now requires intent-aware semantic-topic overlap with the latest real user turn.
6. **Auto-restore is bounded.** Current defaults restore at most one exact excerpt and at most about 1,800 chars proactively.
7. **Stats reminders are gated.** Compression stats are written locally every time, but model-visible stats reminders are only added when the user asks about token/context/compression/ctx/interlang/rehydration.
8. **Tool output caps are stricter.** A single tool output is capped more aggressively and uses a short truncation notice rather than a long explanatory paragraph.

## Why topic-gated auto-restore matters

Before the latest patch, Kcode could auto-restore exact old blocks just because they were low-confidence or high-priority. In practice this meant old installer logs, GitHub API responses, or build diffs could be injected into unrelated turns. That protected correctness, but sometimes wasted tokens.

Current behavior is more selective:

```text
restore_exact = !sensitive
  && (low_confidence || high_priority)
  && topics_overlap_latest_real_user_turn
  && within_small_excerpt_budget
```

This keeps the anti-hallucination escape hatch while avoiding unrelated old evidence being resent.


## Distribution and worst-case view

Averages can hide spikes, so Kcode also tracks percentile-style views from the local event stream. These numbers are historical and include pre-topic-gating behavior, but they show the scale of individual turns.

| Per-event metric | p50 | p95 | p99 | Max |
|---|---:|---:|---:|---:|
| Estimated tokens saved | 106,674 | 355,053 | 355,053 | 355,053 |
| Exact local-tokenizer tokens saved | 142,459 | 453,279 | 453,279 | 453,279 |

Historical proactive auto-restore character volume:

| Metric | Value |
|---|---:|
| p95 auto-restored chars/event | 5,556 |
| Max auto-restored chars/event | 5,556 |

Current source is stricter than much of this historical data: proactive restore now requires intent-aware topic overlap, records candidate/skipped counters, and exposes optional debug logging with `KCODE_CTX_REHYDRATE_DEBUG=1`.

## Auto-rehydration historical counters

| Metric | All recorded events | Latest 50 events |
|---|---:|---:|
| Low-confidence blocks detected | 5,645 | 2,364 |
| Auto-rehydrated blocks | 773 | 140 |
| Auto-rehydrated chars | 1,406,549 | 253,075 |

These counters are historical and include pre-topic-gating behavior. After the current patch, unrelated old blocks should remain summarized unless the model explicitly requests `.ctx_get` or the latest user turn has concrete exact/debug/fix intent plus overlapping block topics.

## Validation performed

The current implementation was validated with targeted tests and compile checks:

```text
cargo test -q interlang --lib
cargo check -q
cargo build --release -q
```

Relevant regression coverage includes:

- unrelated old installer/build context does **not** auto-restore into a token-efficiency/context-strategy turn,
- related installer/build-error turns can still auto-restore one bounded exact excerpt,
- exact `.ctx_get` rehydration still works,
- large old turns are dieted while recent messages remain exact,
- vault references and seen references still save space.

## How to inspect local stats

```bash
# Raw event stream
less ~/.kcode/interlang-stats.jsonl

# Total events
wc -l ~/.kcode/interlang-stats.jsonl

# Recent entries
tail -n 20 ~/.kcode/interlang-stats.jsonl | jq .
```

Useful fields:

| Field | Meaning |
|---|---|
| `original_chars` | Characters represented before compression/reference replacement. |
| `encoded_chars` | Characters sent after compression/reference replacement. |
| `saved_tokens_estimate` | Approximate local estimate using chars/4. |
| `exact_saved_tokens` | Local-tokenizer token delta when tokenizer data is available. |
| `diet_blocks` | Blocks replaced by context diet. |
| `seen_ref_blocks` | Repeated/already-seen blocks represented by references. |
| `raw_context_avoided_tokens_estimate` | Estimate of raw old context not resent. |
| `low_confidence_blocks` | Blocks scored as needing extra care. |
| `auto_rehydrated_blocks` | Exact excerpts proactively restored. Historical rows may include pre-topic-gating behavior. |

## Bottom line

Across 3,083 recorded compaction events, Kcode represented about 1,886,329,423 original characters as 195,518,448 encoded characters, a 89.63% character reduction. The conservative estimate is 422,702,718 tokens saved, while local-tokenizer accounting shows 539,050,823 exact tokens saved.

The current implementation is intentionally more conservative about what it sends to the remote model: exact old content stays retrievable, but proactive exact restore now requires relevance to the latest real user turn.
