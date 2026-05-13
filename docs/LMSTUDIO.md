# LM Studio / local OpenAI-compatible models

Kcode includes a local model diagnostics path for LM Studio and other OpenAI-compatible local servers.

## Start LM Studio

1. Open LM Studio.
2. Load a chat/instruct model.
3. Start the local server. The default endpoint is usually `http://127.0.0.1:1234/v1`.

## Check from Kcode

Inside the TUI, run:

```text
/kcode-local-model
```

The command checks the local `/v1/models` and `/v1/chat/completions` endpoints and reports endpoint health, model availability, and a tiny completion smoke test.

## Benchmark from CLI

```bash
cargo run --bin kcode-bench -- \
  --local-provider lmstudio \
  --local-url http://127.0.0.1:1234/v1 \
  --local-model '<model-id-from-lm-studio>'
```

You can also benchmark any OpenAI-compatible local server by changing `--local-provider` and `--local-url`.

## Environment variables

The local helper code honors the existing local model configuration path. Prefer explicit CLI flags for repeatable benchmark artifacts, especially when comparing cloud providers with LM Studio.
