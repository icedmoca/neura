# Kcode

Kcode is a local-first coding agent harness with a terminal UI, tool orchestration, memory/context management, and an optional local GGUF sidecar model.

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
5. Install command wrappers into `~/.local/bin/kcode` and `~/.local/bin/jcode`.

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

# Debug build instead of release
KCODE_BUILD_PROFILE=debug bash install/install.sh
```

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
