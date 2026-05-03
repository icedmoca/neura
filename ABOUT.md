# About Kcode

Kcode is a local-first coding agent harness. It combines a remote frontier model, local tools, context compression, persistent memory, and a local GGUF sidecar model into one terminal workflow.

The short version:

- **Remote model:** does the main reasoning and response generation, for example GPT-5.5.
- **Kcode harness:** owns tools, files, terminal commands, browser/mouse automation, memory, token-saving context transforms, and runtime orchestration.
- **Local model sidecar:** helps with routing, memory extraction, summaries, critique, and bridge telemetry.
- **Context diet / interlang:** saves tokens by replacing old low-value exact context with compact summaries and rehydratable references.
- **Memory system:** keeps useful facts, preferences, and project state outside the main context window, then injects relevant memory when needed.

---

## 1. High-level architecture

```mermaid
flowchart TD
    U[User] --> T[Kcode Terminal UI]
    T --> A[Kcode Agent Runtime]

    A --> P[Prompt Builder]
    A --> M[Memory System]
    A --> C[Context Diet / Interlang]
    A --> L[Local Model Bridge]
    A --> Tools[Tool Layer]

    P --> C
    M --> P
    C --> R[Remote Model Provider\nGPT-5.5 / OpenAI / compatible APIs]
    L --> LM[Local GGUF Sidecar\nkcode-oss-20b-mxfp4]
    Tools --> FS[Files / Shell / Browser / Git / Gmail / etc.]

    R --> A
    LM --> L
    L --> A
    A --> T
    T --> U
```

Kcode is not just a wrapper around an LLM API. It is the orchestration layer that decides what context to send, which tools are available, when to compact old data, when to recall memory, when to call the local sidecar, and how to persist useful information.

---

## 2. How token savings work

Kcode saves tokens primarily by preventing old, bulky context from being resent verbatim every turn.

Normal chat systems often resend a growing transcript:

```mermaid
flowchart LR
    T1[Turn 1 context] --> T2[Turn 2 context includes Turn 1]
    T2 --> T3[Turn 3 context includes Turns 1-2]
    T3 --> T4[Turn 4 context includes Turns 1-3]
    T4 --> Big[Large expensive prompt]
```

Kcode instead uses a **context diet**. Old low-value blocks, tool output, logs, repeated text, and already-seen content are replaced by compact references.

```mermaid
flowchart TD
    Raw[Raw old context\nlarge tool logs, previous messages, outputs] --> Analyze[Context Diet Analyzer]
    Analyze --> Keep[Keep recent/high-value context exact]
    Analyze --> Compress[Compress old/low-value blocks]
    Compress --> Ref[ctx / il:seen reference\nhash + summary + original size]
    Keep --> Prompt[Final prompt]
    Ref --> Prompt
    Prompt --> Remote[Remote model]
```

A compacted block looks conceptually like this:

```xml
<ctx v=1 k="old-tool-result" id="ctx:hash" h="hash" n=279518 c="0.66" p="high" ar="true" t="build,error" s="lines=...; files=...; first=..." />
```

That means the model still sees the removed block type, stable id/hash, original
size, confidence, priority, semantic topics, and deterministic summary. The exact
text is not repeated by default; the decoder prompt defines `.ctx_get id=...` as
the recovery path when exact old content matters.

### Token-saving modes

Kcode has an interlang/context compression path with modes such as safe, verified, aggressive, and ultra. The main active behavior is:

1. **Recent context stays exact.** The newest messages and current task details remain readable.
2. **Old bulky context gets summarized.** Long tool results, repeated logs, and old low-value content become compact `<ctx>` references.
3. **Seen content can become a reference.** If exact content was already provided earlier, later turns can use `<il:seen>` rather than resending it.
4. **The model can request exact text.** If a summary is insufficient, it can request `.ctx_get id=...`.
5. **Auto-restore is relevance-gated.** Kcode only proactively restores exact excerpts when the old block's topics match the latest real user turn.
6. **Stats are local-first.** Kcode logs original chars, encoded chars, saved chars, estimated saved tokens, and exact local-tokenizer estimates when available. Stats reminders are only injected for token/context-related turns.

Current ultra-mode defaults are tuned for long GPT-5.5 style coding sessions:

| Setting | Default | Purpose |
|---|---:|---|
| `KCODE_CONTEXT_DIET_TRIGGER_TOKENS` | `24000` | Start replacing old bulky blocks once the prompt is roughly this large. |
| `KCODE_CONTEXT_DIET_RECENT_MESSAGES` | `8` | Keep the newest messages exact so current task details remain visible. |
| `KCODE_CONTEXT_DIET_MIN_BLOCK_CHARS` | `420` | Old text/tool/reasoning blocks at or above this size can become `<ctx>` refs. |

These can be overridden per session. Lower values save more tokens but may cause
more `.ctx_get` rehydration requests when exact old content becomes important.

```mermaid
sequenceDiagram
    participant A as Kcode Agent
    participant D as Context Diet
    participant R as Remote Model
    participant V as Local Context Vault

    A->>D: Prepare messages for next turn
    D->>D: Keep recent context exact
    D->>V: Store exact old block by hash
    D->>D: Score confidence + priority
    D-->>A: Replace old block with compact ctx ref
    opt Low confidence or high priority AND topic-relevant to latest user turn
        D-->>A: Auto-inject one bounded exact excerpt
    end
    A->>R: Send smaller prompt
    R-->>A: Response
    alt Remote needs exact old block
        R-->>A: .ctx_get id=ctx:...
        A->>V: Fetch exact content
        V-->>A: Exact original block
        A->>R: Rehydrate exact block
    end
```

### Why prompts are still not tiny

Even after large savings, a final prompt may still be tens of thousands of characters because it includes:

- system/developer instructions,
- tool schemas,
- recent turns,
- current task details,
- memory reminders,
- compact summaries,
- and safety/protocol instructions.

The important part is that old raw context may be hundreds of thousands of characters, while the sent compact references may only be a small fraction of that.

---

## 3. Interlang and context vault references

Kcode uses a lightweight inter-language protocol inspired by context references and deterministic compression.

Main reference types:

| Type | Purpose |
|---|---|
| `<ctx ... />` | A local context-vault reference for old content stored by Kcode. |
| `<il:seen ... />` | A reference to exact content already seen earlier in the session. |
| `<il:v1>...</il>` | A compact encoding for repetitive lines or path prefixes. |
| `.ctx_get id=...` | A request for Kcode to rehydrate exact hidden content. |

```mermaid
flowchart TD
    B[Large block] --> H[Stable hash]
    B --> S[Deterministic summary]
    B --> Vault[Store exact block locally]
    H --> Ref[Compact reference]
    S --> Ref
    Ref --> Prompt[Prompt sent to remote model]
    Vault --> Rehydrate[Exact rehydration if requested]
```

This is designed to be conservative: summaries are useful for normal reasoning, but exact hidden text is not invented. If exact lines matter, the model is instructed to ask for rehydration.

---

## 4. How memory works

Kcode memory is separate from raw chat history. Instead of depending only on the current prompt, Kcode can store durable facts and retrieve them later.

Memory can include:

- user preferences,
- project facts,
- implementation decisions,
- corrections,
- entities,
- useful summaries,
- and task-specific state.

```mermaid
flowchart TD
    Chat[Conversation / tool activity] --> Extract[Memory extraction]
    Extract --> Candidate[Candidate memories]
    Candidate --> Promote[Promotion / confidence / filtering]
    Promote --> Store[Memory store]
    Store --> Search[Semantic / keyword recall]
    Search --> Prompt[Relevant memory injected into prompt]
```

### Memory lifecycle

```mermaid
sequenceDiagram
    participant U as User
    participant A as Kcode Agent
    participant M as Memory Store
    participant L as Local Sidecar
    participant R as Remote Model

    U->>A: Gives instruction or preference
    A->>L: Ask sidecar/extractor for memory candidates
    L-->>A: Candidate facts/preferences
    A->>M: Store promoted memory
    U->>A: Later related task
    A->>M: Search relevant memories
    M-->>A: Relevant facts/preferences
    A->>R: Prompt includes only relevant memory
    R-->>A: Response uses remembered context
```

### Why memory saves tokens

Without memory, important facts must stay in the chat transcript forever. With memory, Kcode can store compact durable facts and only inject relevant ones.

For example, instead of resending a long conversation about a project rename, memory can store:

```text
User renamed Jcode to Kcode. Active Kcode home is ~/.kcode. Legacy ~/.jcode compatibility matters.
```

That is much cheaper than carrying the entire rename conversation forever.

---

## 5. Local model bridge

The local model bridge is the layer between Kcode and the local GGUF sidecar model.

Default local sidecar model identity:

```text
kcode-oss-20b-mxfp4
```

Default local file:

```text
~/.kcode/models/gguf/kcode-oss-20b-mxfp4.gguf
```

Installer source:

```text
https://huggingface.co/icedmoca/kcode-oss-20b-mxfp4
```

The bridge can help with:

- pre-routing,
- local critique,
- memory extraction,
- summarization,
- prompt/exchange logging,
- and local-only assistant support tasks.

```mermaid
flowchart LR
    A[Kcode Agent] --> Bridge[Local Model Bridge]
    Bridge --> Runner[llama.cpp runner]
    Runner --> GGUF[kcode-oss-20b-mxfp4.gguf]
    GGUF --> Runner
    Runner --> Bridge
    Bridge --> A
```

### Bridge logs

Kcode can record local bridge telemetry such as:

- upstream provider,
- upstream model,
- local model identity,
- prompt size,
- response size,
- prompt summaries,
- and memory promotion events.

This is useful for debugging whether compression is actually reducing what gets sent to the remote provider.

```mermaid
flowchart TD
    Prompt[Prepared prompt] --> Account[Token / char accounting]
    Account --> Log[api-exchanges.jsonl / stats]
    Prompt --> Remote[Remote model]
    Local[Local sidecar outputs] --> Log
    Remote --> Response[Response]
    Response --> Log
```

---

## 6. Tool layer

Kcode gives the agent controlled access to local tools. Depending on configuration, this can include:

- shell commands,
- file reads/writes/patches,
- code search,
- browser automation,
- mouse/screenshot automation,
- Gmail helpers,
- background tasks,
- TODO tracking,
- memory management,
- and multi-agent/swarm coordination.

```mermaid
flowchart TD
    Agent[Kcode Agent] --> Policy[Tool policy / harness]
    Policy --> Shell[Shell]
    Policy --> Files[Files / patches]
    Policy --> Browser[Browser]
    Policy --> Memory[Memory]
    Policy --> Git[Git]
    Policy --> Background[Background jobs]
    Shell --> Results[Tool results]
    Files --> Results
    Browser --> Results
    Memory --> Results
    Git --> Results
    Background --> Results
    Results --> Agent
```

Tool output can be large, so tool results are one of the biggest targets for context diet compression.

---

## 7. Install layout

After running the installer, the normal layout is:

```text
~/.kcode/
  build-src/kcode/              # cloned source repo
  builds/current/               # active installed build
  builds/stable/                # stable installed build
  models/gguf/
    kcode-oss-20b-mxfp4.gguf    # local sidecar model
  local-model-bridge/           # local bridge logs/state
  memory-store/                 # persistent memory
```

Command wrappers are installed to:

```text
~/.local/bin/kcode
~/.local/bin/jcode   # compatibility alias
```

```mermaid
flowchart TD
    Repo[GitHub repo\nicedmoca/kcode] --> Clone[~/.kcode/build-src/kcode]
    HF[Hugging Face model] --> Model[~/.kcode/models/gguf/kcode-oss-20b-mxfp4.gguf]
    Clone --> Build[cargo build --release]
    Build --> Current[~/.kcode/builds/current/kcode]
    Current --> Bin[~/.local/bin/kcode]
    Model --> Bridge[Local model bridge]
```

---

## 8. One-command install

```bash
curl -fsSL https://raw.githubusercontent.com/icedmoca/kcode/main/install/install.sh | bash
```

The installer:

1. clones `https://github.com/icedmoca/kcode`,
2. downloads `kcode-oss-20b-mxfp4.gguf` from Hugging Face,
3. builds Kcode,
4. installs `kcode` into `~/.local/bin`,
5. creates compatibility aliases,
6. and stores everything under `~/.kcode`.

---

## 9. Design goals

Kcode is designed around a few practical goals:

- **Spend remote tokens on useful current context, not old logs.**
- **Keep exact old context available locally when needed.**
- **Use memory for durable facts instead of bloating the transcript.**
- **Use a local model for cheap helper work.**
- **Keep the main remote model focused on high-value reasoning.**
- **Make the system observable with accounting logs and stats.**

```mermaid
mindmap
  root((Kcode))
    Token savings
      Context diet
      Interlang refs
      Exact rehydration
      Accounting stats
    Memory
      Preferences
      Project facts
      Corrections
      Recall
    Local sidecar
      Routing
      Summaries
      Critique
      Memory extraction
    Tools
      Shell
      Files
      Browser
      Git
    Install
      GitHub source
      Hugging Face GGUF
      ~/.kcode runtime
```

---

## 10. Practical example

A user asks a tiny follow-up like:

```text
ok did it work?
```

A normal transcript-based system might resend a large amount of previous tool output. Kcode can instead send:

- the recent exact messages,
- compact summaries of old tool output,
- memory facts relevant to the task,
- and references for exact old content if needed.

```mermaid
flowchart LR
    Tiny[Small user message] --> Builder[Prompt builder]
    Old[Huge old context] --> Diet[Context diet]
    Diet --> Compact[Compact ctx refs]
    Memory[Relevant memory] --> Builder
    Compact --> Builder
    Recent[Recent exact context] --> Builder
    Builder --> Smaller[Smaller useful prompt]
    Smaller --> Remote[Remote model]
```

That is the core idea: Kcode keeps the useful state, but avoids paying to resend every byte of old context every turn.
