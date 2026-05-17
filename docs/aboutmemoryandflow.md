# Kcode Memory and Conversation Flow

This document maps the high-level flow of a Kcode agent turn: from the moment the user sends input, through context assembly, memory retrieval, sidecar/tool execution, model output, summarization/compaction, and finally into the next user input cycle.

The diagram is intentionally comprehensive and GitHub-ready. It focuses on the agent lifecycle rather than every internal helper function.

## End-to-End Flow

```mermaid
flowchart TD
    %% =========================
    %% ENTRY
    %% =========================
    A([User sends input]) --> B[Client / Harness receives message]
    B --> C{Message contains attachments, ctx blocks, or tool results?}

    C -- Yes --> C1[Parse structured context blocks]
    C1 --> C2[Normalize attachments, screenshots, files, old-text summaries]
    C2 --> D[Create new conversation turn]

    C -- No --> D[Create new conversation turn]

    %% =========================
    %% SESSION / CONVERSATION STATE
    %% =========================
    D --> E[Load active session state]
    E --> E1[Read conversation history]
    E --> E2[Read workspace and cwd metadata]
    E --> E3[Read model/config/provider settings]
    E --> E4[Read available tools and permissions]

    E1 --> F[Conversation manager]
    E2 --> F
    E3 --> F
    E4 --> F

    %% =========================
    %% MEMORY SOURCES
    %% =========================
    F --> G{Memory and context lookup}

    G --> G1[Short-term turn history]
    G --> G2[Old-text / compressed prior transcript]
    G --> G3[Persistent user/project memory]
    G --> G4[Workspace files and repo metadata]
    G --> G5[Tool state: background jobs, opened files, previous results]
    G --> G6[Side panel / UI context if available]

    G1 --> H[Context candidate pool]
    G2 --> H
    G3 --> H
    G4 --> H
    G5 --> H
    G6 --> H

    %% =========================
    %% CONTEXT BUDGETING
    %% =========================
    H --> I[Context ranking and budgeting]
    I --> I1[Prefer current user request]
    I --> I2[Preserve active system/developer instructions]
    I --> I3[Keep recent high-signal messages]
    I --> I4[Include relevant memory summaries]
    I --> I5[Include relevant file/tool snippets]
    I --> I6[Drop, summarize, or compress low-signal text]

    I1 --> SM[Self-model / operational cognition update]
    I2 --> SM
    I3 --> SM
    I4 --> SM
    I5 --> SM
    I6 --> SM

    %% =========================
    %% SELF-MODEL INTEGRATION
    %% =========================
    SM --> SM1[Convert operational observations into CognitiveSignal values]
    SM1 --> SM2[Update SelfModel domain assessments]
    SM2 --> SM3[Compute global OperationalState and score]
    SM3 --> SM4[Produce RoutingBias, repair hints, and pause-for-repair guidance]
    SM4 --> J[Prompt assembly]

    %% =========================
    %% PROMPT STACK
    %% =========================
    J --> K[Final model input]
    K --> K1[System instructions]
    K --> K2[Developer instructions]
    K --> K3[Harness/tool instructions]
    K --> K4[Conversation memory and summaries]
    K --> K5[Recent messages]
    K --> K6[Current user request]
    K --> K7[Tool schemas and available channels]

    K1 --> L[Model reasoning step]
    K2 --> L
    K3 --> L
    K4 --> L
    K5 --> L
    K6 --> L
    K7 --> L
    SM4 --> L

    %% =========================
    %% MODEL DECISION
    %% =========================
    L --> M{Model decides next action}
    M --> M0{Operational guidance says pause or repair first?}
    M0 -- Yes --> RR[Repair / replay / context rebuild path]
    RR --> U
    M0 -- No --> M1[Proceed with normal plan]

    M1 -- Direct answer --> N[Draft final response]

    M1 -- Needs file inspection --> O[Call read/ls/grep/bash or expanded code tools]
    M1 -- Needs web info --> P[Call websearch/webfetch]
    M1 -- Needs image generation/edit --> Q[Call image_gen]
    M1 -- Needs browser/UI validation --> R[Call browser/open/screenshot tools if available]
    M1 -- Needs long-running work --> S[Start background task]
    M1 -- Needs code modification --> T[Edit/write/patch files]

    %% =========================
    %% TOOL EXECUTION / SIDECAR
    %% =========================
    O --> U[Tool sidecar / harness executes request]
    P --> U
    Q --> U
    R --> U
    S --> U
    T --> U

    U --> U1[Validate command permissions and environment]
    U1 --> U2[Execute tool outside model]
    U2 --> U3[Capture stdout/stderr/files/images/status]
    U3 --> U4[Return structured tool result]

    U4 --> V[Append tool result to conversation]
    V --> W{More work needed?}

    W -- Yes --> H
    W -- No --> N

    %% =========================
    %% RESPONSE
    %% =========================
    N --> X[Response safety and instruction check]
    X --> Y[Send output to user]

    %% =========================
    %% POST-TURN STORAGE
    %% =========================
    Y --> Z[Post-turn persistence]
    Z --> Z1[Store user message]
    Z --> Z2[Store assistant response]
    Z --> Z3[Store tool calls and tool results]
    Z --> Z4[Update session metadata]
    Z --> Z5[Update background task registry]

    %% =========================
    %% MEMORY UPDATE
    %% =========================
    Z --> AA{Should memory be updated?}

    AA -- Stable preference / durable fact --> AA1[Write persistent memory candidate]
    AA -- Project state / task progress --> AA2[Write session/project memory]
    AA -- Too much transcript / token pressure --> AA3[Create compressed summary]
    AA -- No durable signal --> AA4[Do not persist extra memory]

    AA1 --> AB[Memory store]
    AA2 --> AB
    AA3 --> AB
    AA4 --> AC[End of turn]
    AB --> AC

    %% =========================
    %% LOOP
    %% =========================
    AC --> AD([Next user input])
    AD --> A

    %% =========================
    %% BACKGROUND LOOP
    %% =========================
    S --> BG[Background process continues asynchronously]
    BG --> BG1[Emit progress/checkpoints]
    BG1 --> BG2[Harness records task state]
    BG2 --> BG3{User/model waits, tails, or resumes?}
    BG3 -- Wait/tail/status --> U4
    BG3 -- Later turn --> G5

    %% =========================
    %% ERROR LOOP
    %% =========================
    U2 --> ERR{Tool failure?}
    ERR -- Yes --> ERR1[Capture error, exit code, logs]
    ERR1 --> V
    ERR -- No --> U3
```

## Key Concepts

### 1. Conversation Turn

A turn begins when the user sends input. Kcode wraps that input with the current session state, available tools, active instructions, and relevant context. The model does not see the entire filesystem or entire past transcript by default. It sees a curated prompt assembled from the most useful pieces.

### 2. Memory Layers

Kcode-style memory can be understood as several layers:

| Layer | Purpose | Typical contents |
|---|---|---|
| Immediate turn context | What is happening right now | Current user request, recent assistant replies, latest tool output |
| Short-term session memory | Keep the active task coherent | Current files, todo/progress, background task IDs, recent decisions |
| Compressed transcript / old-text | Preserve older conversation without overflowing context | Summaries of previous messages and important facts |
| Persistent memory | Durable user/project facts | Preferences, recurring workflows, stable project details |
| Workspace context | Facts from the actual environment | Files, git state, code search results, screenshots, local configs |
| Tool state | Non-language-model execution state | Background jobs, command outputs, browser screenshots, generated artifacts |

### 3. Sidecar / Harness Role

The model decides what should happen, but tools run outside the model in the harness or sidecar layer. That separation is important:

- The model proposes a tool call.
- The harness validates and executes it.
- The result is captured as structured output.
- The result is appended back into the conversation.
- The model reasons over the new result and either continues or answers.

This is why a failed command, screenshot, file read, or background task output becomes part of the next reasoning step.

### 4. Context Budgeting

Because model context is finite, Kcode must decide what to include. The usual priority order is:

1. System and developer instructions.
2. The current user request.
3. Recent high-signal conversation.
4. Relevant memory summaries.
5. Relevant tool outputs.
6. Relevant file snippets.
7. Older or lower-signal history, usually compressed or omitted.

When the transcript grows too large, older messages may be converted into compact `old-text` style summaries. The model can still use their important content, but not necessarily every exact token.

### 5. Tool Result Loop

A single user request can involve many model/tool cycles:

```mermaid
sequenceDiagram
    participant U as User
    participant H as Kcode Harness
    participant M as Model
    participant T as Tool / Sidecar
    participant FS as Files / Browser / Shell

    U->>H: Send request
    H->>H: Assemble context and memory
    H->>M: Prompt with instructions, memory, tools, user input
    M->>H: Tool call request
    H->>T: Execute validated tool call
    T->>FS: Read, write, search, browse, run command
    FS-->>T: Raw result
    T-->>H: Structured tool result
    H->>M: Continue with tool result in context
    M->>H: More tool calls or final answer
    H-->>U: Output
    H->>H: Persist turn, summarize if needed, update memory
```

### 6. Background Tasks

Long-running jobs are not just normal shell calls. They can continue while the conversation proceeds.

```mermaid
flowchart LR
    A[Model starts background task] --> B[Harness launches process]
    B --> C[Task emits logs/progress/checkpoints]
    C --> D[Background registry stores state]
    D --> E{Later interaction}
    E --> F[Wait for completion]
    E --> G[Tail logs]
    E --> H[Check status]
    E --> I[Use result in future prompt]
```

### 7. Output to Next Input Loop

The important loop is:

```mermaid
flowchart LR
    A[User input] --> B[Context + memory assembly]
    B --> C[Model reasoning]
    C --> D[Tool calls if needed]
    D --> C
    C --> E[Assistant output]
    E --> F[Persistence, summaries, memory updates]
    F --> G[Next user input]
    G --> B
```

Every assistant output and tool result can influence the next user input because the session state is updated after each turn.

## Practical Reading of the Flow

If the user says, “fix this bug,” the flow usually looks like this:

1. User sends the request.
2. Kcode loads session history, repo state, and relevant memory.
3. The model decides it needs to inspect files.
4. The harness reads/searches files.
5. Tool results are returned to the model.
6. The model edits code.
7. The harness writes files and runs tests.
8. Test output returns to the model.
9. The model iterates until tests pass or a blocker is found.
10. The assistant reports what changed.
11. Kcode stores the final state, tool outputs, and any useful summary for future turns.

## Why Memory Matters

Memory prevents the agent from treating every message as a totally fresh session. It lets the system preserve:

- What the user asked for earlier.
- What files were created or modified.
- What tests passed or failed.
- What decisions were already made.
- Which background tasks are still running.
- What user preferences should continue to apply.

But memory is also controlled. Not every detail should become durable memory. Temporary logs, failed exploratory commands, and low-value text are usually better kept only in session history or compressed summaries.


## Self-Model Integration Added in the New Phase

Kcode now has an explicit self-model substrate in the Rust crate:

- `src/self_model.rs`
- exported through `src/lib.rs` as `pub mod self_model;`

This phase added a deterministic operational cognition layer that can be used by routing, repair, replay, benchmarking, slash-command surfaces, and future telemetry. The goal is not to make the agent “sentient.” The goal is to give Kcode a structured way to reason about its own operational condition while it is working.

### New Core Types

| Type | Role |
|---|---|
| `SelfModel` | Snapshot of Kcode’s current operational state across cognitive domains |
| `CognitiveDomain` | Functional area being assessed, such as context assembly, memory retrieval, tool execution, provider routing, repair, replay, benchmarking, and user interaction |
| `OperationalState` | Health state: `Nominal`, `Watch`, `Degraded`, or `Blocked` |
| `CognitiveSignal` | One normalized observation about a domain |
| `DomainAssessment` | Aggregated health score and state for one domain |
| `RoutingBias` | Router-facing output such as prefer low latency, prefer high context, or avoid tool-heavy plans |
| `OperationalEvent` | Higher-level event submitted by systems such as context compilation, tool runs, provider decisions, repair attempts, replay checks, or benchmark samples |
| `OperationalCognition` | Facade that ingests events and keeps the `SelfModel` updated |
| `OperationalGuidance` | Compact guidance object for routing, repair, replay, benchmark, command, or telemetry consumers |

### Self-Model Domain Map

```mermaid
flowchart TD
    A[OperationalEvent] --> B[OperationalCognition::ingest]
    B --> C[OperationalEvent::into_signal]
    C --> D[CognitiveSignal]
    D --> E[SelfModel::observe]
    E --> F[DomainAssessment]
    F --> G[OperationalState per domain]
    F --> H[Score per domain]
    G --> I[Global OperationalState]
    H --> J[Global score]
    I --> K[OperationalGuidance]
    J --> K
    K --> L[RoutingBias]
    K --> M[Repair hints]
    K --> N[should_pause_for_repair]

    L --> O[Provider / model routing]
    M --> P[Repair and recovery path]
    N --> Q[Pause risky chaining when degraded]
```

### Cognitive Domains

The implemented cognitive domains are:

```mermaid
mindmap
  root((Kcode SelfModel))
    ContextAssembly
      token pressure
      context confidence
      latency
    MemoryRetrieval
      stale summaries
      retrieval confidence
      lookup latency
    ToolExecution
      success/failure
      error rate
      tool latency
    ProviderRouting
      model/provider confidence
      provider latency
    Repair
      repair attempt success
      repair failure pressure
    Replay
      deterministic replay checks
      artifact capture pressure
    Benchmarking
      pass rate
      load
    UserInteraction
      ambiguity
      interaction load
```

### Operational Events to Signals

The new integration facade accepts operational events and turns them into normalized cognitive signals:

```mermaid
flowchart LR
    A[ContextCompiled] --> S1[ContextAssembly signal]
    B[MemoryLookup] --> S2[MemoryRetrieval signal]
    C[ToolRun] --> S3[ToolExecution signal]
    D[ProviderDecision] --> S4[ProviderRouting signal]
    E[RepairAttempt] --> S5[Repair signal]
    F[ReplayCheck] --> S6[Replay signal]
    G[BenchmarkSample] --> S7[Benchmarking signal]
    H[UserTurn] --> S8[UserInteraction signal]

    S1 --> M[SelfModel]
    S2 --> M
    S3 --> M
    S4 --> M
    S5 --> M
    S6 --> M
    S7 --> M
    S8 --> M
```

### How Scores Become Operational State

Each domain receives a computed score derived from:

- confidence
- load
- error rate
- optional latency penalty

The score maps to an operational state:

```mermaid
flowchart TD
    A[Domain score] --> B{Score range}
    B -- ">= 0.82" --> C[Nominal]
    B -- "0.62 to 0.82" --> D[Watch]
    B -- "0.35 to 0.62" --> E[Degraded]
    B -- "< 0.35" --> F[Blocked]

    C --> G[Normal execution]
    D --> H[Continue, but bias routing/repair]
    E --> I[Pause risky chaining and prefer repair]
    F --> J[Blocked operational state]
```

### Routing Bias

The self-model produces `RoutingBias`, which can be consumed by provider/model routing or planning code.

```mermaid
flowchart TD
    A[SelfModel] --> B[RoutingBias]
    B --> C{Provider latency high?}
    C -- Yes --> C1[prefer_low_latency]
    B --> D{Context assembly under pressure?}
    D -- Yes --> D1[prefer_high_context]
    B --> E{Tool execution degraded?}
    E -- Yes --> E1[avoid_tool_heavy_plan]
```

Examples:

- If provider routing has high latency, routing can prefer lower-latency options.
- If context assembly is under pressure, routing can prefer models or modes with better context capacity.
- If tool execution is degraded, planning can avoid long chains of dependent tool calls and validate more often.

### Repair Guidance

The self-model also produces repair hints. These are deterministic strings based on degraded domains.

```mermaid
flowchart LR
    A[Degraded ContextAssembly] --> B[Rebuild context with stricter relevance filtering]
    C[Degraded MemoryRetrieval] --> D[Refresh memory retrieval and verify stale summaries]
    E[Degraded ToolExecution] --> F[Prefer smaller tool calls and validate outputs before chaining]
    G[ProviderRouting Watch/Degraded] --> H[Re-evaluate provider/model routing]
    I[Replay Watch/Degraded] --> J[Capture replay artifacts before further mutation]
```

The `OperationalGuidance` object exposes:

- `state`
- `score`
- `routing_bias`
- `repair_hints`
- `should_pause_for_repair`

### Updated Turn Loop With Self-Model

```mermaid
sequenceDiagram
    participant U as User
    participant H as Harness
    participant C as Context/Memory
    participant SM as SelfModel / OperationalCognition
    participant M as Model
    participant T as Tools
    participant R as Repair/Replay

    U->>H: Send input
    H->>C: Load session, memory, files, tool state
    C-->>H: Context candidates
    H->>SM: Submit ContextCompiled / MemoryLookup events
    SM-->>H: OperationalGuidance
    H->>M: Prompt with context + guidance
    M->>H: Plan or tool call
    H->>SM: Submit ProviderDecision / UserTurn events
    alt Guidance says degraded or blocked
        H->>R: Repair, replay, or context rebuild
        R-->>H: Recovery result
        H->>SM: Submit RepairAttempt / ReplayCheck events
    else Normal execution
        H->>T: Execute tool calls
        T-->>H: Tool result
        H->>SM: Submit ToolRun events
    end
    H->>M: Continue with result and updated context
    M-->>H: Final answer
    H-->>U: Output
    H->>SM: Post-turn operational update
    H->>C: Persist turn, summaries, memory updates
```

### What This Phase Tightened

Before this phase, memory, context, routing, repair, replay, and benchmarks could exist as separate concerns. The new self-model gives them a common operational vocabulary:

```mermaid
flowchart TD
    A[Context system] --> SM[SelfModel]
    B[Memory system] --> SM
    C[Tool runner] --> SM
    D[Provider router] --> SM
    E[Repair system] --> SM
    F[Replay system] --> SM
    G[Benchmarks] --> SM
    H[User interaction layer] --> SM

    SM --> I[Unified state]
    SM --> J[Unified score]
    SM --> K[Unified routing bias]
    SM --> L[Unified repair hints]
    SM --> M[Unified pause signal]
```

This makes future features easier because each subsystem can submit operational events instead of inventing its own isolated health model.

### Validation Added

The implementation includes focused tests for:

- nominal self-model state
- degraded tool execution producing repair hints
- provider latency producing low-latency routing bias
- repeated observations being averaged
- operational events updating domains and guidance
- failed tool events requesting a repair pause

The targeted validation command used was:

```bash
cargo test self_model --lib --quiet
```

Expected result:

```text
6 passed; 0 failed
```

## Updated Practical Reading of the Flow

With self-model integration, a “fix this bug” request now has an extra operational cognition layer:

1. User sends the request.
2. Kcode loads session history, repo state, and relevant memory.
3. Context and memory systems emit operational events.
4. `OperationalCognition` updates the `SelfModel`.
5. The model receives normal context plus operational guidance.
6. If the self-model reports degraded tool execution, context pressure, or provider latency, the agent can adjust its plan.
7. The harness reads/searches files.
8. Tool results are returned and tool success/failure updates the self-model.
9. The model edits code.
10. Tests run and results update operational state.
11. If needed, repair/replay guidance triggers recovery before continuing.
12. The assistant reports what changed.
13. Kcode stores the final state, tool outputs, summaries, and any useful memory updates.

## Operational Cognition Verbalization + Semantic State Abstraction

A later phase adds `src/semantic_operational_layer.rs`, a deterministic layer above `SelfModel` that turns operational cognition into bounded semantic state and safe verbalizations.

```mermaid
flowchart LR
    A[SelfModel] --> B[SemanticOperationalState]
    B --> C[SemanticMetrics: coherence, drift, convergence, compression]
    B --> D[Semantic labels: stable, monitoring, compressed, recovering, blocked]
    B --> E[Compact verbalization]
    B --> F[Diagnostic verbalization]
    B --> G[Machine verbalization]
```

This gives routing, repair, replay, telemetry, benchmarks, and future slash/status commands a shared vocabulary for describing operational state without relying on prompt-only explanations. See [`docs/semantic_operational_layer.md`](semantic_operational_layer.md).


## Long-Horizon Operational Pressure + Continuous Cognition Stress Infrastructure

The newest phase adds `src/long_horizon_pressure.rs`, a bounded stress infrastructure layer above `SelfModel` and `SemanticOperationalState`. It lets Kcode simulate finite multi-step operational pressure and produce reports without creating an autonomous daemon.

```mermaid
flowchart TD
    A[StressScenario] --> B[StressSample stream]
    B --> C[LongHorizonStressRunner]
    C --> D[OperationalCognition]
    D --> E[SelfModel]
    E --> F[SemanticOperationalState]
    F --> G[PressureReading]
    G --> H[LongHorizonReport]
```

It introduces built-in scenarios for baseline operation, context saturation, tool failure bursts, provider latency, memory staleness, and mixed long-horizon pressure. Each bounded run records drift, compression, convergence, semantic labels, operational state, repair pressure, and warning conditions.

See [`docs/long_horizon_report.md`](long_horizon_report.md).

## Summary

Kcode’s flow is best understood as a loop:

1. **Input arrives.**
2. **Context and memory are assembled.**
3. **Operational events update the self-model.**
4. **The self-model produces routing bias, repair hints, and pause guidance.**
5. **The model reasons with both task context and operational guidance.**
6. **Tools execute outside the model.**
7. **Results return to the model and update operational state.**
8. **The assistant outputs an answer or continues work.**
9. **The turn is stored, summarized, and possibly written to memory.**
10. **The next user input starts the loop again.**

That loop is what lets the agent remain coherent across multiple tool calls, file edits, screenshots, background jobs, and follow-up requests.
