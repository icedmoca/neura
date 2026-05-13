# Install Kcode

This guide covers installing Kcode from source, preparing the runtime environment, and configuring optional local LM Studio/OpenAI-compatible model support.

## Requirements

- Rust toolchain, preferably current stable.
- Git.
- A terminal with Unicode support.
- Platform build tools:
  - Linux: `build-essential`, `pkg-config`, SSL development headers as needed by your distro.
  - macOS: Xcode Command Line Tools.
  - Windows: WSL2 is recommended for the primary development path. Native Windows may require additional MSVC tooling.

## Clone and build

```bash
git clone https://github.com/icedmoca/kcode.git
cd kcode
cargo build --release
```

The release binary is produced under:

```bash
target/release/kcode
```

For development, use the debug build:

```bash
cargo build
cargo run
```

## Add Kcode to PATH

Example for bash or zsh:

```bash
export PATH="$PWD/target/release:$PATH"
```

To persist it, add the export line to `~/.bashrc`, `~/.zshrc`, or your shell profile after replacing `$PWD` with the absolute repository path.

## Development validation

Run these after a checkout and before committing behavior changes:

```bash
cargo fmt
cargo check --lib
python3 scripts/validate_docs.py
```

Run focused tests for changed subsystems. Examples:

```bash
cargo test --lib operational_repair_learning
cargo test --lib adaptive_cognition
cargo test --lib local_model
```

## Optional provider credentials

Provider-specific authentication depends on the adapter and command path. Use the built-in auth/account flows where available. The provider files under `src/provider` are the implementation source of truth for provider-specific behavior.

General guidance:

- Keep API keys out of shell history when possible.
- Prefer provider-specific auth commands or config files over ad-hoc exports.
- Do not commit secrets into the repository.
- Test provider changes with smoke prompts before relying on long-running tasks.

## LM Studio and local OpenAI-compatible models

Kcode includes local model diagnostics for LM Studio and other OpenAI-compatible local servers.

### Start LM Studio

1. Open LM Studio.
2. Download or select a chat/instruct model.
3. Load the model.
4. Start the local server.
5. Confirm the base URL. LM Studio commonly uses:

```text
http://127.0.0.1:1234/v1
```

### Check from Kcode

Inside the TUI, run:

```text
/kcode-local-model
```

The command checks the local `/v1/models` and `/v1/chat/completions` endpoints and reports endpoint health, model availability, and a small completion smoke test.

### Benchmark from the CLI

```bash
cargo run --bin kcode-bench -- \
  --local-provider lmstudio \
  --local-url http://127.0.0.1:1234/v1 \
  --local-model '<model-id-from-lm-studio>'
```

You can benchmark any OpenAI-compatible local server by changing `--local-provider` and `--local-url`.

### Local model troubleshooting

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| Connection refused | LM Studio server is not running or uses another port. | Start the server and verify the URL. |
| No models listed | No model is loaded or the endpoint is not OpenAI-compatible. | Load a model and check `/v1/models`. |
| Completion timeout | Model is too large or hardware is saturated. | Try a smaller quantized model or increase timeout in the caller. |
| Poor tool behavior | Local model lacks tool-use/coding capability. | Use local models for diagnostics/benchmarks, or select a stronger model. |

### Environment and repeatability

Prefer explicit benchmark flags for repeatability. Environment variables and default local settings are useful for interactive work, but benchmark artifacts should record provider, URL, model ID, and relevant hardware notes.

## WSL notes

If running Kcode in WSL and LM Studio on Windows, `127.0.0.1` may not always refer to the Windows host depending on networking mode. If the local model check fails:

1. Confirm LM Studio is listening on the Windows side.
2. Try the Windows host IP visible from WSL.
3. Ensure firewalls allow local connections.
4. Prefer explicit `--local-url` in benchmark commands.

## Updating source-backed docs

After adding binaries, provider files, public modules, or slash commands:

```bash
python3 scripts/validate_docs.py --write-inventory
python3 scripts/validate_docs.py
```

Commit generated inventory updates with the code change that caused them.
