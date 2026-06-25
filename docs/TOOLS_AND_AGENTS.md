# Neura Tools, Agents, and MCPs

Neura is not just a chat box. It is a tool-running agent harness. This document lists the built-in capabilities that ship with Neura and how to think about them.

## The big picture

Neura gives the model access to controlled tools for real work:

- files and folders,
- shell commands,
- code search,
- patching/editing,
- browser automation,
- web search/fetch,
- Gmail,
- memory,
- goals/todos,
- subagents and swarms,
- MCP servers,
- local mouse/screenshot automation,
- benchmark/debug helpers.

Neura also prunes tool schemas dynamically. Simple direct-answer turns do not need to pay for every tool description. Tool-heavy turns get relevant tools, and `tool_expand` can request more if needed.

## Core file and code tools

| Tool | What it does |
|---|---|
| `read` | read text files, PDFs, and images |
| `write` | write a file |
| `edit` | replace text in a file |
| `multiedit` | apply multiple replacements to one file |
| `patch` | apply unified diffs |
| `apply_patch` | apply Codex/Neura-style patches |
| `ls` | list directory contents |
| `glob` | find files by glob |
| `grep` | lightweight regex search |
| `agentgrep` | code/file search with more structured output |
| `codesearch` | search code examples and docs |
| `lsp` | LSP-style symbol operation stub/future integration |
| `open` | open or reveal files, folders, and URLs |

## Shell, build, and background execution

| Tool | What it does |
|---|---|
| `bash` | run shell commands |
| `batch` | run multiple independent tool calls in parallel |
| `bg` | manage background jobs |
| `debug_socket` | debugging helper for local socket workflows |
| `invalid` | report malformed tool usage |

## Browser, web, and UI tools

| Tool | What it does |
|---|---|
| `browser` | browser automation through the harness bridge |
| `mouse` | local mouse/screenshot automation |
| `websearch` | search the web |
| `webfetch` | fetch a URL as text/markdown/html |
| `side_panel` | write/focus side-panel pages |
| `open` | open files/folders/URLs for the user |

## Communication and external services

| Tool | What it does |
|---|---|
| `gmail` | search/read/draft/send/label Gmail messages |
| `conversation_search` | search the current conversation history |
| `session_search` | search past chat sessions |

## Memory, planning, and task management

| Tool | What it does |
|---|---|
| `memory` | remember/recall/search/tag/link durable memories |
| `todo` | manage the visible todo list |
| `goal` | create and update longer-running goals |
| `schedule` | schedule a task for later |
| `skill_manage` | load/list/reload/read Hermes-style skills; use `.skill_get name=<skill> reason=<why>` for on-demand exact rehydration |

### Hermes-style skills

Neura discovers skills as directories containing a `SKILL.md` file. The prompt only injects a compact skill anchor with the available skill names, not the full skill bodies. When exact instructions are needed, the model should emit:

```text
.skill_get name=<skill-name> reason=<why this skill is needed>
```

Neura intercepts that request, rehydrates the exact `SKILL.md` content into a system reminder, and retries the turn. This mirrors `.mem_get`/`.ctx_get` token discipline: advertise availability cheaply, then load large skill text only on demand. For immediate tool-driven inspection, call `skill_manage` with `action="list"` or `action="read"`.

## Agent and swarm tools

| Tool | What it does |
|---|---|
| `subagent` | run a focused subagent task |
| `swarm` | coordinate multiple agents/sessions/channels/tasks |
| `send_message` | ambient communication helper |
| `request_permission` | permission request helper for ambient flows |
| `schedule_ambient` | schedule ambient/shared work |
| `end_ambient_cycle` | end an ambient cycle |

## MCP tools

| Tool | What it does |
|---|---|
| `mcp` | list/connect/disconnect/reload MCP servers |
| bundled Chromium MCP bridge | browser automation server/extension included with Neura |

The bundled Chromium bridge ships in:

```text
vendor/chromium-agent-bridge
```

The installer copies it to:

```text
~/.neura/chromium-agent-bridge
```

and registers it in:

```text
~/.neura/mcp.json
```

See [INSTALL.md](INSTALL.md) for the Chrome extension setup step.

## Local sidecar agent/model

Neura can install a local GGUF sidecar model:

```text
neura-oss-20b-mxfp4
```

The sidecar is not the main remote reasoning model. It helps with local support tasks such as:

- routing,
- memory extraction,
- summaries,
- critique,
- bridge telemetry,
- local helper workflows.

## Context tools and exact recall

Neura's context system is also part of its tool story:

- old bulky context can become compact local refs,
- summaries are breadcrumbs, not source of truth,
- exact local text can be restored when needed,
- sensitive-looking context is not auto-injected,
- token/context telemetry is recorded locally.

This is what lets Neura run long sessions without blindly resending every old log, diff, and tool result.

## Tool safety model

Neura can do real work, so the safest practice is:

- inspect before editing,
- prefer reversible file changes,
- run tests before claiming success,
- avoid destructive commands unless explicitly requested,
- keep credentials and runtime state in `~/.neura`, not the repository.

## Full tool list extracted from source

Current built-in tool names include:

```text
agentgrep
apply_patch
bash
batch
bg
browser
codesearch
conversation_search
debug_socket
edit
glob
gmail
goal
grep
invalid
ls
lsp
mcp
memory
mouse
multiedit
open
patch
read
schedule
selfdev
session_search
side_panel
skill_manage
subagent
swarm
todo
webfetch
websearch
write
```

Some internal/ambient/test helpers exist in source as well, but the list above is the practical user-facing set.
