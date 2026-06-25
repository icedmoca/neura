# Neura Message Flow Test

This diagram captures the current high-level flow when a user sends a message to Neura, plus a technical suggestion for making responses feel even more immediate.

```mermaid
sequenceDiagram
    autonumber
    actor User
    participant UI as Cockpit UI<br/>ui/src/App.tsx
    participant API as Desktop HTTP API<br/>crates/neura-desktop/src/main.rs
    participant Session as Session Manager<br/>single_session/session_data
    participant Agent as Codex Agent Runtime
    participant Store as Chat/Session State

    User->>UI: Type prompt and press Enter
    UI->>UI: Read textarea ref, clear input immediately
    UI->>API: POST /api/chats/{chatId}/message { text }
    API->>Session: Resolve chat mode and session id

    alt single-session mode
        Session->>Agent: send_single_session_message(text)
    else persisted chat mode
        Session->>Store: append user message
        Session->>Agent: launch/resume Codex session with prompt
    end

    Agent-->>Session: emits events and assistant output
    Session->>Store: persist transcript/status updates
    API-->>UI: JSON response / refreshed chat state
    UI->>UI: refreshState(), render assistant response
```

## Suggested improvement

The current flow is simple and reliable, but the UI still waits on request/refresh boundaries for some updates. A more responsive design would push agent events to the cockpit as they happen.

```mermaid
flowchart TD
    A[User sends prompt] --> B[UI posts message]
    B --> C[API accepts message quickly]
    C --> D[Background agent task starts]
    C --> E[UI opens SSE/WebSocket event stream]
    D --> F[Agent emits token/tool/status events]
    F --> G[Server broadcasts normalized chat events]
    G --> H[UI incrementally patches visible transcript]
    H --> I[Final event marks turn complete]

    subgraph Benefit
        J[Lower perceived latency]
        K[No polling refresh loop]
        L[Tool progress appears immediately]
    end

    H --> J
    G --> K
    F --> L
```
