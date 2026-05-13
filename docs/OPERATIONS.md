# Operations guide

This guide covers day-to-day operation of Kcode as implemented in this repository.

## Install and update

See `INSTALL.md` for installation details. From a source checkout, the common development loop is:

```bash
cargo fmt
cargo check --lib
cargo test --lib <focused_filter>
```

## Common binaries

The generated inventory in `docs/reference/implementation-inventory.md` lists binaries from `src/bin`.

Common examples:

```bash
cargo run --bin kcode-bench -- --help
cargo run --bin tui-bench -- --help
```

## Local LM Studio diagnostics

See `docs/LMSTUDIO.md`.

Short version:

1. Start LM Studio local server, usually `http://127.0.0.1:1234/v1`.
2. In the TUI, run `/kcode-local-model`.
3. For benchmarks, use `kcode-bench --local-provider lmstudio --local-url http://127.0.0.1:1234/v1 --local-model <model-id>`.

## Operational repair learning

Operational repair learning classifies failures into build, test, runtime, provider, auth, network, tooling, context, or unknown classes. It assigns a replay gate:

- `Skip`: no meaningful failure.
- `Smoke`: cheap validation for external/provider/tooling conditions.
- `Focused`: targeted validation for runtime/context/test/build issues.
- `Full`: recurring build/test failures that need stronger regression coverage.

Learned repair motifs are mirrored into adaptive cognition as execution signals. The slash command registry includes `/kcode-repair-memory` for user-facing discovery of the feature.

## Documentation validation

Run:

```bash
python3 scripts/validate_docs.py
```

Use `--write-inventory` to refresh `docs/reference/implementation-inventory.{json,md}` after adding binaries, provider files, public modules, or slash commands.
