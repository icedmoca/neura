# Long-Horizon Operational Pressure + Continuous Cognition Stress Infrastructure

This phase adds bounded stress infrastructure above the self-model and semantic operational layer.

Implemented in:

```text
src/long_horizon_pressure.rs
```

Exported through:

```rust
pub mod long_horizon_pressure;
```

## Purpose

The layer gives Kcode deterministic infrastructure for answering:

- How does operational cognition behave across many steps?
- When do drift, compression, and convergence become concerning?
- Do tool failure bursts create repair pressure?
- Do provider latency or context saturation affect semantic state?
- Can stress results be summarized in a stable report?

It is intentionally finite and caller-driven. It is not an autonomous daemon.

## Architecture

```mermaid
flowchart TD
    A[StressScenario] --> B[StressSample stream]
    B --> C[LongHorizonStressRunner]
    C --> D[OperationalCognition]
    D --> E[SelfModel]
    E --> F[SemanticOperationalState]
    F --> G[PressureReading]
    G --> H[LongHorizonReport]

    H --> I[pass/fail threshold check]
    H --> J[markdown summary]
    H --> K[telemetry/benchmark consumers]
```

## Core Types

| Type | Purpose |
|---|---|
| `StressScenario` | Built-in finite scenario class |
| `StressSample` | One step of stress input, mapping to operational events |
| `HorizonConfig` | Bounds and thresholds for a run |
| `PressureReading` | Per-step semantic/operational pressure snapshot |
| `LongHorizonReport` | Aggregate result across a bounded horizon |
| `LongHorizonStressRunner` | Finite runner that ingests samples and produces a report |
| `generate_scenario` | Deterministic scenario generator for tests/docs/benchmarks |

## Built-In Scenarios

```mermaid
mindmap
  root((StressScenario))
    Baseline
      nominal context
      successful tools
    ContextSaturation
      rising token pressure
      decreasing context confidence
    ToolFailureBurst
      intermittent failed tool runs
      repair pressure
    ProviderLatency
      growing provider latency
      routing pressure
    MemoryStaleness
      stale retrieval ratio
      memory confidence decay
    MixedLongHorizon
      context pressure
      intermittent tools
      provider latency samples
```

## Bounded Run Loop

```mermaid
sequenceDiagram
    participant R as LongHorizonStressRunner
    participant S as StressSample
    participant OC as OperationalCognition
    participant SM as SelfModel
    participant SL as Semantic Layer
    participant PR as PressureReading
    participant LR as LongHorizonReport

    loop up to HorizonConfig.max_steps
        S->>R: sample events
        R->>OC: ingest OperationalEvent values
        OC->>SM: update domain assessments
        SM->>SL: abstract_semantic_state
        SL-->>R: SemanticOperationalState
        R->>PR: record score, label, metrics, warnings
    end
    R->>LR: aggregate readings
```

## Pressure Metrics

Each reading records:

- step
- scenario
- `OperationalState`
- `SemanticLabel`
- semantic metrics
  - coherence
  - drift
  - convergence
  - compression
- score
- warnings

Warnings are generated for:

- high drift
- high compression
- low convergence
- repair pause recommendation

## Validation

Targeted command:

```bash
cargo test long_horizon_pressure --lib
```

Focused coverage:

- baseline pressure passes default thresholds
- mixed horizon respects `max_steps`
- tool failure bursts produce repair pressure
- markdown reports include final semantic state and recent readings
