# Operational maturity model

Kcode is evolving from a terminal coding assistant into an adaptive operational agent. The maturity model below reflects implemented capabilities and near-term extension points.

## Level 1: deterministic execution

- TUI and CLI entry points.
- Provider request/stream handling.
- Tool execution with visible results.
- Focused unit and rendering tests.

## Level 2: diagnostics and observability

- Provider diagnostics and failover paths.
- Local LM Studio/OpenAI-compatible health checks.
- Bench binaries for repeatable provider/local model evaluation.
- Source-backed implementation inventory.

## Level 3: adaptive memory

- Adaptive cognition store for local execution signals.
- Prompt-memory selection and retrieval decisions.
- Operational repair motifs mirrored into adaptive cognition.

## Level 4: repair learning

- Failure classification by operational class.
- Recurrence tracking.
- Confidence calibration from repair outcomes.
- Replay gates that recommend smoke, focused, or full validation.

## Level 5: closed-loop repair operations

Future work should connect learned motifs directly to replay harnesses, benchmark suites, and TUI reporting so recurring operational failures automatically propose the smallest safe validation plan.
