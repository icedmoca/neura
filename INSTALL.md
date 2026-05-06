# Installing and Configuring Kcode

This page is the practical setup guide. If you just want the fast path, use the first section and ignore the rest until you need customization.

## Fast install for normal people

Open a terminal and run:

```bash
curl -fsSL https://raw.githubusercontent.com/icedmoca/kcode/main/install/install.sh | bash
```

Then start Kcode:

```bash
kcode
```

Check the version:

```bash
kcode --version
```

If your shell says `kcode: command not found`, add `~/.local/bin` to your PATH:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

To make that permanent, add the same line to your shell config, such as `~/.bashrc`, `~/.zshrc`, or your shell profile.

## What gets installed

By default, the installer:

1. clones or updates Kcode into `~/.kcode/build-src/kcode`,
2. builds the `kcode` binary with Cargo,
3. installs wrappers into `~/.local/bin/kcode` and `~/.local/bin/jcode`,
4. downloads the optional local GGUF sidecar model,
5. installs the bundled Chromium MCP bridge,
6. writes logs to `~/.kcode/logs/install-YYYYMMDD-HHMMSS.log`.

The installer treats `~/.kcode/build-src/kcode` as an installer-managed cache. If that cache is locally diverged, it backs it up and clones a clean copy instead of failing on Git internals.

## Requirements

You need:

- Linux or macOS shell environment,
- `git`,
- `curl`,
- Rust/Cargo,
- enough disk space for Rust build artifacts and the optional GGUF model.

Ubuntu/Debian setup:

```bash
sudo apt-get update
sudo apt-get install -y git curl build-essential pkg-config libssl-dev
curl https://sh.rustup.rs -sSf | sh
```

## Installer options

Customize installation with environment variables:

```bash
# Install somewhere other than ~/.kcode
KCODE_HOME="$HOME/.kcode-dev" bash install/install.sh

# Install command wrappers somewhere other than ~/.local/bin
KCODE_BIN_DIR="$HOME/bin" bash install/install.sh

# Clone from a fork
KCODE_REPO_URL="https://github.com/yourname/kcode.git" bash install/install.sh

# Skip downloading the local sidecar model
KCODE_SKIP_MODEL=1 bash install/install.sh

# Skip installing the bundled Chromium MCP bridge
KCODE_SKIP_CHROMIUM_MCP=1 bash install/install.sh

# Debug build instead of release
KCODE_BUILD_PROFILE=debug bash install/install.sh
```

## Updating Kcode

The simple path is to rerun the installer:

```bash
curl -fsSL https://raw.githubusercontent.com/icedmoca/kcode/main/install/install.sh | bash
```

Inside Kcode, `/reload` can switch to a newer installed build when the installed build layout contains a newer candidate. If you are actively modifying Kcode source, a full rebuild/reinstall may be clearer.

## OpenAI OAuth, API keys, and failover

Kcode can use OpenAI through:

- ChatGPT/Codex OAuth accounts,
- a stored OpenAI platform API key,
- automatic OAuth-to-API-key failover when configured.

Manage OpenAI auth from inside Kcode:

```bash
# Choose credential behavior
/account openai auth-mode oauth     # OAuth/subscription only
/account openai auth-mode api_key   # platform API key only
/account openai auth-mode auto      # OAuth first, retry with API key on clear limit errors

# Store or clear a platform API key in ~/.kcode/openai-auth.json (0600)
/account openai api-key sk-...
/account openai api-key clear
```

In `auto` mode, Kcode retries a failed OAuth/subscription request once through OpenAI platform Bearer auth when the error clearly looks like a subscription limit or quota response. It does **not** silently use the API key when auth mode is `oauth`.

Credentials are stored under your local `~/.kcode` directory, not in the GitHub repository.

## Local sidecar model

The default local model identity is:

```text
kcode-oss-20b-mxfp4
```

The installer downloads it to:

```text
~/.kcode/models/gguf/kcode-oss-20b-mxfp4.gguf
```

from:

```text
https://huggingface.co/icedmoca/kcode-oss-20b-mxfp4
```

Compatibility aliases may also exist for older names:

```text
gpt-oss-20b-mxfp4_moe.gguf
jcode-gpt-oss-20b.gguf
kcode-oss-20b-mxfp4
```

Skip the model download if you only want remote-provider operation:

```bash
KCODE_SKIP_MODEL=1 bash install/install.sh
```

## Chromium MCP bridge setup

Kcode includes a Chromium MCP bridge in:

```text
vendor/chromium-agent-bridge
```

The installer copies it to:

```text
~/.kcode/chromium-agent-bridge
```

and registers it in:

```text
~/.kcode/mcp.json
```

Chrome still requires one manual step:

1. open `chrome://extensions`,
2. enable Developer mode,
3. choose **Load unpacked**,
4. select `~/.kcode/chromium-agent-bridge/extension`.

Skip the bundled bridge if you do not need browser automation:

```bash
KCODE_SKIP_CHROMIUM_MCP=1 bash install/install.sh
```

## Context and token-saving knobs

Kcode enables context compression by default. These knobs are useful when debugging cost/context behavior:

```bash
# Disable interlang/context-diet compression
KCODE_INTERLANG_COMPACT=0 kcode

# Compression mode: safe, verified, aggressive, ultra
KCODE_INTERLANG_MODE=ultra kcode

# Start dieting after this approximate prompt size
KCODE_CONTEXT_DIET_TRIGGER_TOKENS=24000 kcode

# Keep this many newest messages exact
KCODE_CONTEXT_DIET_RECENT_MESSAGES=8 kcode

# Minimum old block size eligible for context diet
KCODE_CONTEXT_DIET_MIN_BLOCK_CHARS=420 kcode
```

Token-savings accounting is appended to:

```text
~/.kcode/interlang-stats.jsonl
```

## Important local paths

| Path | Purpose |
|---|---|
| `~/.kcode/build-src/kcode` | installer-managed source checkout |
| `~/.kcode/builds` | installed Kcode builds |
| `~/.kcode/models` | optional local GGUF model files |
| `~/.kcode/logs` | installer/runtime logs |
| `~/.kcode/memory` | local memory data |
| `~/.kcode/openai-auth.json` | local OpenAI auth config |
| `~/.kcode/interlang-stats.jsonl` | context/token-saving telemetry |
| `~/.kcode/mcp.json` | MCP server config |

## Uninstalling

Kcode is local-first. To remove it, delete the install home and wrappers:

```bash
rm -rf ~/.kcode
rm -f ~/.local/bin/kcode ~/.local/bin/jcode
```

If you installed into custom locations, remove those paths instead.
