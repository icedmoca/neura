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

    I1 --> J[Prompt assembly]
    I2 --> J
    I3 --> J
    I4 --> J
    I5 --> J
    I6 --> J

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

    %% =========================
    %% MODEL DECISION
    %% =========================
    L --> M{Model decides next action}

    M -- Direct answer --> N[Draft final response]

    M -- Needs file inspection --> O[Call read/ls/grep/bash or expanded code tools]
    M -- Needs web info --> P[Call websearch/webfetch]
    M -- Needs image generation/edit --> Q[Call image_gen]
    M -- Needs browser/UI validation --> R[Call browser/open/screenshot tools if available]
    M -- Needs long-running work --> S[Start background task]
    M -- Needs code modification --> T[Edit/write/patch files]

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

## Summary

Kcode’s flow is best understood as a loop:

1. **Input arrives.**
2. **Context and memory are assembled.**
3. **The model reasons.**
4. **Tools execute outside the model.**
5. **Results return to the model.**
6. **The assistant outputs an answer or continues work.**
7. **The turn is stored, summarized, and possibly written to memory.**
8. **The next user input starts the loop again.**

That loop is what lets the agent remain coherent across multiple tool calls, file edits, screenshots, background jobs, and follow-up requests.
