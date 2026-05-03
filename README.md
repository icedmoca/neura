hi
# Kcode

> Kcode lets you run long, tool heavy coding sessions without blowing up token costs by compressing old context into references and only restoring exact data when needed, reducing hallucinations by grounding the model in real, retrievable source data instead of guesswork.

---

Kcode is a local first coding agent harness with a terminal UI, tool orchestration, memory/context management, and an optional local GGUF sidecar model.

[![Hugging Face](https://img.shields.io/badge/Hugging%20Face-icedmoca%2Fkcode--oss--20b--mxfp4-yellow?logo=huggingface)](https://huggingface.co/icedmoca/kcode-oss-20b-mxfp4)

For a detailed architecture explanation, see [ABOUT.md](ABOUT.md).

For details on hallucination mitigation, exact context rehydration, and real token-saving data, see [HALLUCINATION_MITIGATION.md](HALLUCINATION_MITIGATION.md).

## One-command install

Run this in a terminal:

```bash
curl -fsSL https://raw.githubusercontent.com/icedmoca/kcode/main/install/install.sh | bash
```

The installer will:

1. Clone Kcode from `https://github.com/icedmoca/kcode` into `~/.kcode/build-src/kcode`.
2. Download the local sidecar model from Hugging Face:
   `https://huggingface.co/icedmoca/kcode-oss-20b-mxfp4`.
3. Store the model inside your Kcode home at:
   `~/.kcode/models/gguf/kcode-oss-20b-mxfp4.gguf`.
4. Build the `kcode` binary with Cargo.
5. Install and register the bundled Chromium MCP bridge.
6. Install command wrappers into `~/.local/bin/kcode` and `~/.local/bin/jcode`.

After install:

```bash
kcode --version
kcode
```

If `kcode` is not found, add `~/.local/bin` to your PATH:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

## Requirements

- Linux or macOS shell environment
- `git`
- `curl`
- Rust/Cargo
- Enough disk space for the GGUF model and Rust build artifacts

On Ubuntu/Debian:

```bash
sudo apt-get update
sudo apt-get install -y git curl build-essential pkg-config libssl-dev
curl https://sh.rustup.rs -sSf | sh
```

## Installer options

You can customize installation with environment variables:

```bash
# Install somewhere other than ~/.kcode
KCODE_HOME="$HOME/.kcode-dev" bash install/install.sh

# Install command wrappers somewhere other than ~/.local/bin
KCODE_BIN_DIR="$HOME/bin" bash install/install.sh

# Clone from a fork
KCODE_REPO_URL="https://github.com/yourname/kcode.git" bash install/install.sh

# Skip downloading the model
KCODE_SKIP_MODEL=1 bash install/install.sh

# Skip installing the bundled Chromium MCP bridge
KCODE_SKIP_CHROMIUM_MCP=1 bash install/install.sh

# Debug build instead of release
KCODE_BUILD_PROFILE=debug bash install/install.sh
```

## Bundled Chromium MCP bridge

The repository includes the Chromium MCP bridge in `vendor/chromium-agent-bridge`.
The installer copies it into `~/.kcode/chromium-agent-bridge` and writes this MCP
registration to `~/.kcode/mcp.json`:

```json
{
  "servers": {
    "chromium-agent-bridge": {
      "command": "~/.kcode/chromium-agent-bridge/chromium-agent-bridge-mcp",
      "args": [],
      "env": {},
      "shared": true
    }
  }
}
```

The bundled bridge includes the MCP server script, local WebSocket bridge, Chrome
extension source, and packaged extension zip. Chrome still requires one manual
browser step after install: open `chrome://extensions`, enable Developer mode,
choose **Load unpacked**, and select `~/.kcode/chromium-agent-bridge/extension`.

## Context diet and usage refresh knobs

Kcode enables interlang/context-diet compression by default. In ultra mode, old
large context is replaced with local `<ctx>` references while recent task context
stays exact. Useful runtime overrides:

```bash
# Disable interlang/context-diet compression
KCODE_INTERLANG_COMPACT=0 kcode

# Compression mode: safe, verified, aggressive, ultra
KCODE_INTERLANG_MODE=ultra kcode

# Start dieting after this approximate prompt size. Default: 24000.
KCODE_CONTEXT_DIET_TRIGGER_TOKENS=24000 kcode

# Keep this many newest messages exact. Default: 8.
KCODE_CONTEXT_DIET_RECENT_MESSAGES=8 kcode

# Minimum old block size eligible for context diet. Default: 420 chars.
KCODE_CONTEXT_DIET_MIN_BLOCK_CHARS=420 kcode
```

Token-savings accounting is appended to `~/.kcode/interlang-stats.jsonl`. OpenAI
ChatGPT limit data is refreshed aggressively for the sidebar: Kcode treats the
OpenAI usage cache as stale after about 30 seconds and requests a refresh after
completed turns, so 5-hour and weekly usage can update during an existing
session.

Kcode also adds confidence and priority metadata to context references. When a
summary is low-confidence or high-priority, Kcode can proactively inject a small
exact excerpt before the next model call instead of relying only on the model to
notice the summary and request `.ctx_get`. Sensitive-looking content is not
auto-injected; it still requires an explicit exact-context request.

## OpenAI OAuth, API keys, and failover

Kcode can use OpenAI through ChatGPT/Codex OAuth accounts stored in
`~/.kcode/openai-auth.json`, a stored OpenAI platform API key, or automatic
failover. Manage this from the account command UI:

```bash
# Choose credential behavior
/account openai auth-mode oauth     # OAuth/subscription only
/account openai auth-mode api_key   # platform API key only
/account openai auth-mode auto      # OAuth first, retry with API key on clear limit errors

# Store or clear a platform API key in ~/.kcode/openai-auth.json (0600)
/account openai api-key sk-...
/account openai api-key clear
```

In `auto` mode, Kcode retries a failed OAuth/subscription request once through
`https://api.openai.com/...` Bearer auth when the error clearly looks like an
OAuth/subscription limit or quota response. It does not silently use the API key
when auth mode is `oauth`.

## Local sidecar model

The default local model identity is:

```text
kcode-oss-20b-mxfp4
```

The installer downloads:

```text
~/.kcode/models/gguf/kcode-oss-20b-mxfp4.gguf
```

from:

```text
https://huggingface.co/icedmoca/kcode-oss-20b-mxfp4
```

Compatibility aliases are created for older names:

```text
gpt-oss-20b-mxfp4_moe.gguf
jcode-gpt-oss-20b.gguf
kcode-oss-20b-mxfp4
```

## Repository safety

This GitHub repository should contain source code and installer files only. Runtime state, logs, credentials, build outputs, and model files belong under the user's local `~/.kcode` directory and are ignored by `.gitignore`.

## Development

```bash
git clone https://github.com/icedmoca/kcode.git
cd kcode
cargo check
cargo build --release --bin kcode
```
