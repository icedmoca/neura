# Install Neura: source, environment, and local model setup

This guide covers installing Neura from source, preparing the development environment, configuring PATH, validating the checkout, and setting up optional LM Studio/local OpenAI-compatible model support.

## 1. Requirements

Neura is a Rust project. You need:

- Git;
- Rust stable toolchain;
- a Unicode-capable terminal;
- platform build tools;
- network access for crates and provider/local model endpoints.

Recommended developer tools:

- `ripgrep` for fast source search;
- `python3` for docs validation;
- `pkg-config` and SSL headers on Linux;
- WSL2 for Windows development if native Rust tooling is inconvenient.

## 2. Install Rust

Use rustup unless your environment has a managed Rust toolchain:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustc --version
cargo --version
```

## 3. Clone and build

```bash
git clone https://github.com/icedmoca/neura.git
cd neura
cargo build --release
```

Debug build:

```bash
cargo build
cargo run
```

Release binary path:

```bash
target/release/neura
```

## 4. PATH configuration

For a one-session PATH update:

```bash
export PATH="$PWD/target/release:$PATH"
```

For persistent shell use, add the absolute path to your shell profile:

```bash
export PATH="/absolute/path/to/neura/target/release:$PATH"
```

Then reload:

```bash
exec "$SHELL" -l
```

## 5. Installer script

From a remote checkout path:

```bash
curl -fsSL https://raw.githubusercontent.com/icedmoca/neura/main/install.sh | bash
exec "$SHELL" -l
neura
```

From a local clone:

```bash
./install.sh
neura
```

Inspect `install.sh` before running it if you are operating in a constrained or security-sensitive environment.

## 6. Platform notes

### Linux

Install typical build dependencies. Debian/Ubuntu example:

```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libssl-dev git curl python3
```

### macOS

Install Xcode Command Line Tools:

```bash
xcode-select --install
```

Then install Rust and build normally.

### Windows and WSL

WSL2 is recommended. Build inside the Linux environment. If interacting with Windows-hosted LM Studio, see the WSL networking notes below.

## 7. Development validation

After installation:

```bash
cargo fmt --check
cargo check --lib
python3 scripts/validate_docs.py
```

Focused subsystem checks:

```bash
cargo test --lib operational_repair_learning
cargo test --lib adaptive_cognition
cargo test --lib local_model
```

## 8. Provider credentials

Provider-specific credentials are adapter-dependent. General rules:

- never commit secrets;
- prefer built-in auth/account flows when available;
- keep provider smoke tests cheap;
- record provider/model IDs when diagnosing failures;
- treat account failover behavior as provider-specific.

## 9. LM Studio and local OpenAI-compatible models

Neura includes local model diagnostics for LM Studio and other OpenAI-compatible local servers.

### 9.1 Start LM Studio

1. Open LM Studio.
2. Download or select a chat/instruct model.
3. Load the model.
4. Start the local server.
5. Confirm the base URL. LM Studio commonly uses:

```text
http://127.0.0.1:1234/v1
```

### 9.2 Check from Neura

Inside the TUI:

```text
/neura-local-model
```

The command checks `/v1/models` and `/v1/chat/completions`, then reports endpoint health, model availability, and a small completion smoke test.

### 9.3 Benchmark local model behavior

```bash
cargo run --bin neura-bench -- \
  --local-provider lmstudio \
  --local-url http://127.0.0.1:1234/v1 \
  --local-model '<model-id-from-lm-studio>'
```

### 9.4 Choosing local models

For sidecar support, prioritize:

- fast inference;
- good summarization;
- coding/log familiarity;
- reliable instruction following;
- context length appropriate for log compression.

A smaller fast model can be better as a sidecar than a huge slow model because the sidecar's job is often compression and support, not primary reasoning.

### 9.5 Troubleshooting

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| Connection refused | Server stopped or wrong port | Start server and verify URL |
| No models listed | No model loaded | Load model in LM Studio |
| Timeout | Model too large or hardware saturated | Try smaller quantization/model |
| Bad answers | Weak local model | Use sidecar for summaries, not final reasoning |
| WSL cannot connect | Windows/WSL networking boundary | Use Windows host IP or adjust firewall |

## 10. WSL networking for LM Studio

If Neura runs in WSL and LM Studio runs on Windows, `127.0.0.1` may not always point where you expect. Try:

```bash
cat /etc/resolv.conf
```

or inspect the Windows host IP visible from WSL. Then pass the URL explicitly to benchmark commands.

## 11. Documentation inventory after install

Validate docs after changes:

```bash
python3 scripts/validate_docs.py --write-inventory
python3 scripts/validate_docs.py
```

Commit generated inventory updates with the code changes that caused them.
