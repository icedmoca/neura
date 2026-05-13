# Limitations and non-goals

This file exists to keep the documentation honest.

## Provider capabilities are adapter-specific

Kcode has many provider adapters, but they are not interchangeable. Streaming, catalog refresh, account failover, request headers, and tool-call support may differ by adapter.

## Local models are not magic drop-in cloud replacements

LM Studio and other local OpenAI-compatible servers can be diagnosed and benchmarked, but model quality, context length, tool-use behavior, and latency depend on the loaded model and local hardware.

## Operational repair learning is deterministic pattern learning

`operational_repair_learning` classifies textual failures and records repair motifs. It does not prove semantic correctness. Its replay gates are recommendations for validation intensity, not a substitute for running tests.

## The sidebar context `∞` is a UI simplification

The rainbow `∞` marker intentionally replaces a precise-looking dynamic bar. Context accounting still exists in runtime/model paths, but the UI avoids implying exact safety margins where provider behavior and compaction can vary.

## Documentation inventory is source-derived, not exhaustive prose

`docs/reference/implementation-inventory.md` is generated from source patterns. It catches major public modules, slash commands, binaries, and provider files, but humans still need to explain behavior and constraints in architecture docs.
