# Kcode operations guide

This guide explains how to operate and validate Kcode while developing it.

## Daily development loop

```bash
cargo fmt
cargo check --lib
python3 scripts/validate_docs.py
```

Then run focused tests for the touched subsystem.

Examples:

```bash
cargo test --lib operational_repair_learning
cargo test --lib adaptive_cognition
cargo test --lib local_model
cargo test --lib info_widget_usage
```

## Commit discipline

- Keep changes buildable.
- Commit source, tests, and generated documentation inventory together.
- Do not commit secrets or local machine-specific credentials.
- Prefer focused commits that explain the architectural purpose.

## Documentation workflow

If you add or rename any of the following, refresh the inventory:

- binary in `src/bin`;
- provider file in `src/provider`;
- public module declaration;
- public slash command using `RegisteredCommand::public`.

Commands:

```bash
python3 scripts/validate_docs.py --write-inventory
python3 scripts/validate_docs.py
```

## Provider operations

Provider issues usually fall into one of these categories:

| Category | Typical signal | Validation |
| --- | --- | --- |
| Auth | 401/403, missing key, expired token | Auth/account command or provider smoke prompt. |
| Catalog | missing model, stale model list | Refresh catalog or choose explicit model. |
| Streaming | malformed SSE, partial response | Provider parser tests and a smoke stream. |
| Rate limit | 429 or quota messages | Retry policy, fallback, or provider switch. |
| Compatibility | request rejected by provider | Inspect adapter request shaping. |

Provider changes should stay inside `src/provider` unless the runtime contract itself changes.

## Local LM Studio operations

LM Studio setup lives in `docs/INSTALL.md#lm-studio-and-local-openai-compatible-models`.

Operational checks:

```text
/kcode-local-model
```

Benchmark check:

```bash
cargo run --bin kcode-bench -- \
  --local-provider lmstudio \
  --local-url http://127.0.0.1:1234/v1 \
  --local-model '<model-id>'
```

Record local model ID, quantization, hardware, and endpoint URL when comparing benchmark runs.

## Operational repair learning operations

The repair learning subsystem is deterministic and safe to test without a model provider.

Core data types:

- `FailureObservation`
- `RepairAttempt`
- `RepairMotif`
- `FailureClass`
- `ReplayGate`

Recommended workflow when a recurring failure appears:

1. Capture the command, stderr, exit code, and touched files.
2. Classify with `operational_repair_learning`.
3. Record a repair attempt after applying a fix.
4. Use the replay gate to decide validation intensity.
5. Mirror the motif into adaptive cognition for future prompt memory.

## Replay gate interpretation

| Gate | Meaning | Example validation |
| --- | --- | --- |
| `Skip` | No actionable failure. | None. |
| `Smoke` | Cheap validation is enough. | Provider health check or single request. |
| `Focused` | Validate the failing subsystem. | One test filter, parser test, or cargo check. |
| `Full` | Recurring build/test failure. | Broader test suite or benchmark replay. |

## TUI operations

TUI changes should be validated with rendering tests or a focused compile/test pass. Sidebars and widgets are often snapshot-like, so avoid changing visible strings casually unless tests and docs are updated.

## Failure handling policy

- Reproduce before fixing when feasible.
- Prefer deterministic tests over model-dependent assertions.
- For external-provider issues, record the provider, endpoint, model, request class, and exact status/error text.
- For local-model issues, record LM Studio version, model ID, quantization, URL, and hardware constraints.
