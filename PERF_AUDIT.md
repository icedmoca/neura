# Kcode interactive performance audit

This file records the current latency-sensitive paths in Kcode and the concrete optimization status as of this audit.

## High-confidence findings

### TUI rendering

- Main render entry point: `src/tui/mod.rs::render_frame`.
- Rendering delegates to snapshot-based state through `TuiState`, so most widgets do not do filesystem or process work while drawing.
- Message/body rendering already has cache coverage and tests under `src/tui/ui_tests/basic/body_cache.rs`.
- UI message rendering cache is bounded with an LRU-like `VecDeque` in `src/tui/ui_messages_cache.rs`, avoiding unbounded growth in long sessions.

### Sessions

- Session persistence is journaled rather than rewriting the full transcript for every small append.
- Hot append path uses `storage::append_json_line_fast`, avoiding pretty JSON and minimizing per-message persistence overhead.
- Periodic checkpoints compact the journal to bound replay cost.

### Info widgets

- Info widget rendering consumes `InfoWidgetData` snapshots from `TuiState::info_widget_data`.
- Git, memory, usage, and similar data are passed in through state, not recomputed from the filesystem or subprocesses in the draw path.

## Remaining work that should be done with measurements

1. Add opt-in frame timing telemetry around `render_frame` once a repo-wide tracing/logging dependency convention is chosen.
2. Add an integration benchmark that replays a large transcript and records p50/p95 frame render time.
3. Add a typing-latency harness that feeds key events and measures key-to-redraw latency.
4. If measurements show slow frames, optimize the specific widget/body renderer reported by the harness rather than guessing.

## Validation notes

- `cargo check` succeeds for this audit state.
- Targeted `cargo test ui_messages_cache --lib` and `cargo test session --lib` currently hit a rustc internal compiler error in this workspace, before test assertions run. This should be tracked separately from app performance.
