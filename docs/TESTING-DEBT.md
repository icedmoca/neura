# TUI test-suite debt

Status as of this document: the non-TUI suites are fully green
(`knowledge` 14, `memory` 48, `memory_graph` 34, `agent` 66, `cli` 82,
`provider` 246). The `tui::` suite has **40 deterministic failures** (down
from 44) that predate the knowledge/cognition work and stem from a TUI
redesign (IL Stats panel, footer savings suffix, picker routes, paste/undo
handling) whose tests were not updated, plus tests that read real machine
state.

## Fixed already

- `configured_auth_test_targets…` — OpenRouter availability is resolved from
  `OPENROUTER_API_KEY`/key files at call time, not the fixture; the test now
  pins the env var (serialized).
- `tui_launch` spawn tests — mutated global `PATH`/`NEURA_*` without
  serialization; now hold a shared env lock.
- `build_turn_footer` ×2 — the footer emitted a noisy "IL ultra saved 0"
  suffix when nothing was saved; suffix is now omitted at zero (code fix).
- Hermeticity groundwork: `interlang::stats_path()` and `config::config()`
  no longer read the developer's real `~/.neura` under `cfg(test)` —
  machine-local stats/settings previously leaked into rendered frames.

## Remaining clusters (40 tests, all fail deterministically even serial)

1. **Frame-layout family (~14)** — background-task card, native scrollbar,
   copy badges ×3, file-activity repaint ×2, prompt preview, scroll
   indicator, pinned splitter, mouse pan, pending-split status, usage card:
   frames now render the redesigned "IL Stats" info panel where the tests
   expect other content. Needs per-test re-baselining against the current
   intentional layout.
2. **Model-picker family (4)** — openrouter/openai route prefix, local
   quantized entry, no-routes guidance, review-model preference: picker
   entries/routes changed shape.
3. **Input/session family (~8)** — ctrl-z undo returns empty, paste
   expansion + multiple pastes (index OOB — genuine bug), soft-interrupt
   requeue, startup stub, context command pin, improve/improve-plan prompt
   text drift, session-context snapshot.
4. **Update/reload family (2)** — `has_newer_binary` + reload-request read
   the real launcher path and `~/.neura/builds` (one test *writes* a stub
   into the real launcher path if missing). Needs a NEURA_HOME temp harness.
5. **Markdown/mermaid/render-cache family (~10)** — centered blockquotes,
   wrapped code gutter, mermaid width-cache variants, image height estimate,
   side-panel clamps ×2 + probe + fallback, prep-cache ×2, narrow version
   display, auth-doctor markdown.
6. **Async harness (1)** — `remote_startup_done…` panics with "no reactor
   running" (needs `#[tokio::test]` or a runtime guard).

## Suggested approach

Fix cluster 4 and the paste OOB first (real bugs / dangerous test side
effects), then re-baseline cluster 1 in one sitting with
`cargo test --lib 'tui::' -- --test-threads=1` and the actual frames from
failure output, then clusters 2/3/5 test-by-test. Estimated one focused
session.
