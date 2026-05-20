# Dynamic Latent Operational Recurrence + Invariant Translation Layers

Kcode now includes a deterministic, inspectable latent operational recurrence layer. The implementation lives in:

- `src/latent_operational_recurrence.rs`
- `src/cli/latent.rs`
- CLI command: `kcode kcode-latent ...`

## What this layer does

The layer converts operational events into a stable low-dimensional vector, applies recurrence over time, translates events into invariant matches, preserves temporal provenance, and reports drift/anti-sludge signals.

It is intentionally **not opaque model magic**. The vector schema is fixed, deterministic, serialized as JSON, and covered by tests.

## CLI

```bash
kcode kcode-latent status
kcode kcode-latent vector
kcode kcode-latent observe build success --tag test --tag validation --tool cargo --latent-provider openai
kcode kcode-latent translate build success --tag test --tag validation
kcode kcode-latent drift
kcode kcode-latent remap 1
kcode kcode-latent invariants
kcode kcode-latent provenance
kcode kcode-latent temporal
kcode kcode-latent influence build success --tag test
kcode kcode-latent report --output ~/Desktop/latent_report.md
kcode kcode-latent learn build success --tag test --tag validation --tool cargo
kcode kcode-latent learned-vectors
kcode kcode-latent attractors
kcode kcode-latent counterfactual build success --tag test --alternate-tag validation --alternate-tag provenance
kcode kcode-latent doctrine
kcode kcode-latent immune
kcode kcode-latent topology
kcode kcode-latent convergence
kcode kcode-latent evolution-report --output ~/Desktop/latent_evolution_report.md
kcode kcode-latent ingest build success --tag test --tag validation --tool cargo --source cli
kcode kcode-latent learn-now --limit 32
kcode kcode-latent background-status
kcode kcode-latent samples
kcode kcode-latent outcomes
kcode kcode-latent doctrines
kcode kcode-latent pause
kcode kcode-latent resume
```

## State

Default state path:

```text
~/.kcode/latent_operational_state.json
```

Override for tests or isolated runs:

```bash
KCODE_LATENT_STATE=/tmp/kcode-latent.json kcode kcode-latent status
```

## Recurrence model

```mermaid
flowchart LR
    E[Operational event] --> Enc[Deterministic encoder]
    Enc --> Gate[Influence gate]
    Gate -->|accepted| Rec[Recurrent update]
    Rec --> Vec[Latent vector]
    E --> Inv[Invariant translation]
    E --> Temp[Temporal memory]
    Temp --> Prov[Provenance records]
    Temp --> Sludge[Anti-sludge report]
```

## Invariants

Default invariant translations include:

- validate before done,
- preserve user intent,
- avoid irreversible actions,
- track provenance.

Each invariant has a canonical expression and required tags. Translation returns a confidence score and explanation.

## Guardrails

- Low-signal events are rejected.
- Near-duplicate influence is rejected.
- Temporal memory is capped to prevent unbounded sludge.
- Anti-sludge reporting surfaces duplicate and low-signal ratios.
- Schema remap is explicit and versioned.

## Validation

Core tests cover:

- deterministic event encoding,
- recurrence update behavior,
- invariant translation matching,
- influence gate rejection for empty signal.

## Live operational fabric commands

```bash
kcode kcode-latent fabric-status
kcode kcode-latent fabric-events
kcode kcode-latent fabric-report --output ~/Desktop/live_operational_fabric_report.md
kcode kcode-latent fabric-pause
kcode kcode-latent fabric-resume
kcode kcode-latent fabric-ping
```

The fabric emits live user-message, provider request/response, tool, token, local sidecar token-estimate, memory bridge, and background latent learning events. Events are persisted under `~/.kcode/live_operational_fabric/events.jsonl` and bridged into the latent background sample queue.

Automatic background adaptation is enabled by default: every live fabric event opportunistically runs a bounded latent background cycle. Set `KCODE_LIVE_FABRIC_AUTO_CYCLE=0` to disable it for debugging.

## Latent memory bank commands

```bash
kcode kcode-latent latent-memory-status
kcode kcode-latent latent-memory-blocks
kcode kcode-latent latent-memory-report --output ~/Desktop/latent_memory_report.md
kcode kcode-latent latent-memory-usefulness
```

Latent memory stores ctx-style blocks for stable attractors, noise patterns, validation doctrine, useful drift synthesis, and operational lessons. Background learning consults this bank before vector updates to suppress duplicates, down-rank noise, anchor excessive drift, and preserve useful drift as synthesis memory.

## Operational policy influence commands

```bash
kcode kcode-latent policy-status
kcode kcode-latent policy-rules
kcode kcode-latent policy-decide test-validation final-answer
kcode kcode-latent policy-audit
kcode kcode-latent policy-report --output ~/Desktop/policy_influence_report.md
kcode kcode-latent policy-domains
```

Policy influence is gated by latent memory usefulness: low-confidence memories do not become policy, policy decisions are audited, and observe-only mode is available in state for safe rollout.

## Policy outcome credit commands

```bash
kcode kcode-latent policy-credit-report --output ~/Desktop/policy_credit_report.md
kcode kcode-latent policy-credit-assign <audit-id> success
```

Policy outcome credit assigns success/failure back to policy audits, updates rule confidence, and propagates usefulness back into the source latent memory when a rule came from memory.
