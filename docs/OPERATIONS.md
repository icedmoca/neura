# Kcode operations manual

This manual describes how to operate Kcode as a living coding-agent system. It is written for maintainers who need to make changes, diagnose failures, validate behavior, and keep documentation truthful.

## 1. Operational model

Kcode operation has four loops:

1. **Interaction loop**: user enters intent through TUI or CLI.
2. **Execution loop**: agent runtime selects provider, executes tools, streams results, and mutates workspace when appropriate.
3. **Validation loop**: focused checks confirm that changes are correct.
4. **Learning loop**: adaptive cognition and operational repair learning retain compact signals.

A healthy Kcode change should preserve all four loops. If a change improves behavior but cannot be validated or documented, it is incomplete.

## 2. Updating Kcode from inside the TUI

Kcode includes a local `/update` slash command. It checks the local git `HEAD` against `origin/main`. If they match, it reports that Kcode is already current. If GitHub has a newer commit, it runs the GitHub installer path and reports that Kcode should be restarted to use the updated binary.

```text
/update
```

Operational notes:

- `/update` requires the checkout to have an `origin` remote with `main`.
- It performs a `git fetch origin main --quiet` before comparing commits.
- It does not hot-swap the running process; restart Kcode after a successful update.
- If the installer fails, the command reports stdout/stderr in the TUI.

## 4. Daily maintainer workflow

```bash
cargo fmt
cargo check --lib
python3 scripts/validate_docs.py
```

Then run focused tests. Examples:

```bash
cargo test --lib operational_repair_learning
cargo test --lib adaptive_cognition
cargo test --lib local_model
cargo test --lib info_widget_usage
```

For provider parser changes, run the provider-specific tests if present. For TUI rendering changes, run the relevant TUI test filter. For docs changes, always run `scripts/validate_docs.py`.

## 4. Validation strategy

Kcode uses validation tiers:

| Tier | When to use | Examples |
| --- | --- | --- |
| Format | Any Rust change | `cargo fmt --check` |
| Compile | Any code change | `cargo check --lib` |
| Focused unit | Single subsystem | `cargo test --lib operational_repair_learning` |
| Integration-ish | Provider/TUI/tool flows | provider parser tests, TUI state tests |
| Smoke | External endpoint/tool | local model check, provider health prompt |
| Benchmark | Performance/provider comparison | `kcode-bench`, `tui-bench` |

The goal is not to run the biggest possible suite every time. The goal is to select the smallest validation that actually proves the change, then broaden when risk increases.

## 5. Provider operations

Provider changes are operationally sensitive because failures may come from code, credentials, upstream availability, catalog drift, rate limits, or model behavior.

### Provider diagnostic checklist

1. Identify provider adapter file under `src/provider`.
2. Confirm request shape and headers.
3. Confirm selected model ID and provider routing prefix.
4. Check account/auth state.
5. Check catalog refresh logic if model discovery failed.
6. Check streaming parser if partial output or event errors occur.
7. Run a cheap smoke prompt before a long task.

### Provider failure taxonomy

| Failure | Symptoms | Repair direction |
| --- | --- | --- |
| Auth | 401, 403, expired token | refresh account/auth path |
| Catalog | model missing, stale picker | refresh catalog, explicit model selection |
| Stream parse | output truncation, malformed SSE | provider parser fix/test |
| Rate limit | 429, quota messages | retry/fallback/account switch |
| Compatibility | provider rejects request fields | adapter-specific request shaping |
| Routing | wrong provider/model chosen | model route or picker metadata fix |

## 6. Local sidecar and LM Studio operations

LM Studio setup is documented in `docs/INSTALL.md`. Operationally, the local sidecar model is best treated as a cheap support worker.

Good sidecar tasks:

- summarize long logs;
- compress noisy tool output;
- generate routing hints;
- produce lightweight critique;
- help with memory compaction;
- run local OpenAI-compatible smoke checks;
- benchmark local model behavior.

Risky sidecar tasks:

- high-stakes code architecture decisions on weak local models;
- security-sensitive reasoning without review;
- assuming tool-call capability when the local model was not trained for it;
- replacing validation with plausible summaries.

### Local model smoke check

```text
/kcode-local-model
```

### Local benchmark example

```bash
cargo run --bin kcode-bench -- \
  --local-provider lmstudio \
  --local-url http://127.0.0.1:1234/v1 \
  --local-model '<model-id>'
```

Record model ID, quantization, hardware, URL, and prompt class when comparing runs.

## 7. Tool operations

Tools can mutate the workspace. Treat tool changes as operational changes, not just API changes.

Tool operation principles:

- prefer noninteractive commands;
- preserve user work;
- avoid destructive actions without explicit confirmation;
- capture stderr/stdout for failure learning;
- validate edits after applying them;
- keep long-running jobs observable.

For shell commands, prefer commands that time out or finish predictably. Avoid interactive prompts unless the harness can answer them.

## 8. Adaptive cognition operations

Adaptive cognition should store compact, high-signal data. Do not turn it into an unbounded transcript sink.

Good memory records:

- recurring failure signatures;
- successful repair summaries;
- validation outcomes;
- provider/tool operational signals;
- durable repository facts.

Bad memory records:

- raw long logs without compression;
- secrets;
- one-off irrelevant errors;
- speculative claims with no validation;
- duplicated transcript chunks.

## 9. Operational repair learning operations

Repair learning is deterministic. Use it when a failure and repair attempt can be represented explicitly.

Workflow:

1. Capture `FailureObservation` with summary, stderr, command, exit code, and touched files.
2. Classify failure.
3. Apply repair.
4. Record `RepairAttempt` with outcome and validation.
5. Let recurrence/confidence update.
6. Use replay gate to select future validation intensity.

Replay gates:

| Gate | Meaning | Typical validation |
| --- | --- | --- |
| `Skip` | Not actionable | None |
| `Smoke` | Cheap external/tool/provider check | health prompt, endpoint check |
| `Focused` | Subsystem-specific proof | one test filter, `cargo check` |
| `Full` | Recurring build/test failure | broad suite or benchmark replay |

## 10. TUI operations

TUI changes affect user trust quickly. Validate:

- command registry descriptions;
- input behavior;
- model/account picker rendering;
- sidebar text;
- status lines;
- keyboard handling;
- tests that assert visible output.

The rainbow context `∞` is an intentional UI choice. Do not reintroduce precise-looking token bars unless the measurement is genuinely precise and provider-correct.

## 11. Documentation operations

Documentation is part of the system.

After changing source structure:

```bash
python3 scripts/validate_docs.py --write-inventory
python3 scripts/validate_docs.py
```

Docs should distinguish:

- implemented behavior;
- recommended operation;
- limitation/tradeoff;
- future extension.

Do not claim provider capability unless the adapter supports it.

## 12. Release/readiness checklist

Before calling a phase complete:

- code formatted;
- focused tests passed;
- compile check passed when relevant;
- docs updated;
- generated inventory refreshed if needed;
- changes committed;
- push completed;
- final response names commit and validation.

## 13. Incident response playbook

When Kcode breaks:

1. Stop making broad changes.
2. Reproduce with the smallest command/test.
3. Classify failure type.
4. Inspect recent commits and changed subsystem.
5. Apply minimal repair.
6. Validate with focused check.
7. Record or update repair motif if recurring.
8. Broaden validation if build/test behavior was affected.

## 14. `/improve` operational posture

`/improve` is for bounded recursive self-improvement. It should be used with validation and review. Good `/improve` tasks are scoped, reversible, and testable. Bad `/improve` tasks are vague rewrites, destructive actions, or large migrations without checkpoints.
