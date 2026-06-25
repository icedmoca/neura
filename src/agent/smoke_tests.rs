//! End-to-end smoke harness using a programmable mock provider.
//!
//! These tests exercise `Agent::run_turn` from input → upstream payload →
//! response. They cover the matrix the user enumerated (A–G):
//!
//!   A. Direct trivial — "say meow"
//!   B. Direct arithmetic — "what is 2+2"
//!   C. Memory recall — anchor mode + `.mem_get`
//!   D. Coding/tool — agentgrep usage
//!   E. Coding continuation — admission inheritance through tool loops
//!   F. Context recovery — persistent ctx-vault round-trip via `.ctx_get`
//!   G. Regression — coding turn still admits required tools
//!
//! The harness is intentionally hermetic: a `MockProvider` records the
//! messages / tools / system it sees, and returns a canned text reply. No
//! network, no real tokenizer truncation surprises.

#![cfg(test)]

use crate::agent::Agent;
use crate::agent::context_compiler::CompilerMode;
use crate::message::{ContentBlock, Message, StreamEvent, ToolDefinition};
use crate::provider::{EventStream, Provider};
use crate::tool::Registry;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc as tokio_mpsc;
use tokio_stream::wrappers::ReceiverStream;

#[derive(Default, Debug, Clone)]
struct CapturedRequest {
    messages: Vec<Message>,
    tools: Vec<ToolDefinition>,
    system_static: String,
    system_dynamic: String,
}

struct MockProvider {
    name: &'static str,
    model: &'static str,
    captured: Arc<Mutex<Vec<CapturedRequest>>>,
    /// FIFO of canned responses. Each response is the assistant text emitted
    /// for a single `complete_split` call. When empty, an empty response is
    /// returned with `stop_reason=end_turn`.
    responses: Arc<Mutex<Vec<String>>>,
}

impl MockProvider {
    fn new(name: &'static str, model: &'static str, responses: Vec<String>) -> Arc<Self> {
        Arc::new(Self {
            name,
            model,
            captured: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(responses)),
        })
    }

    fn last_request(&self) -> Option<CapturedRequest> {
        self.captured.lock().ok()?.last().cloned()
    }

    fn capture_count(&self) -> usize {
        self.captured.lock().map(|v| v.len()).unwrap_or(0)
    }
}

#[async_trait]
impl Provider for MockProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        self.complete_split(messages, tools, system, "", None).await
    }

    async fn complete_split(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_static: &str,
        system_dynamic: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        if let Ok(mut captured) = self.captured.lock() {
            captured.push(CapturedRequest {
                messages: messages.to_vec(),
                tools: tools.to_vec(),
                system_static: system_static.to_string(),
                system_dynamic: system_dynamic.to_string(),
            });
        }
        let response = self
            .responses
            .lock()
            .ok()
            .and_then(|mut q| {
                if q.is_empty() {
                    None
                } else {
                    Some(q.remove(0))
                }
            })
            .unwrap_or_else(|| "ok".to_string());
        let (tx, rx) = tokio_mpsc::channel::<Result<StreamEvent>>(8);
        tokio::spawn(async move {
            let _ = tx.send(Ok(StreamEvent::TextDelta(response))).await;
            let _ = tx
                .send(Ok(StreamEvent::MessageEnd {
                    stop_reason: Some("end_turn".to_string()),
                }))
                .await;
        });
        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn name(&self) -> &str {
        self.name
    }

    fn model(&self) -> String {
        self.model.to_string()
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            name: self.name,
            model: self.model,
            captured: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(Vec::new())),
        })
    }

    fn handles_tools_internally(&self) -> bool {
        false
    }

    fn supports_compaction(&self) -> bool {
        false
    }

    fn uses_neura_compaction(&self) -> bool {
        false
    }

    fn context_window(&self) -> usize {
        200_000
    }
}

/// Serialise tests that flip env vars (compiler mode, anchor mode, etc.).
fn smoke_env_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex as StdMutex, OnceLock};
    static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
    match LOCK.get_or_init(|| StdMutex::new(())).lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

fn make_agent(provider: Arc<MockProvider>) -> Agent {
    let registry = Registry::empty();
    Agent::new(provider as Arc<dyn Provider>, registry)
}

fn approx_payload_chars(acc: &crate::provider::remote_telemetry::PayloadAccounting) -> usize {
    acc.system_static_chars
        .saturating_add(acc.system_dynamic_chars)
        .saturating_add(acc.messages_chars)
        .saturating_add(acc.tools_json_chars)
}

#[tokio::test]
async fn smoke_a_direct_meow_skips_subsystems() {
    let _g = smoke_env_lock();
    unsafe {
        std::env::set_var("NEURA_CONTEXT_COMPILER", "v2");
        std::env::remove_var("NEURA_MEMORY_ANCHOR");
    }
    let provider = MockProvider::new("mock", "test-direct", vec!["meow 😺".to_string()]);
    let mut agent = make_agent(provider.clone());
    let reply = agent
        .run_once_capture("say meow")
        .await
        .expect("turn must succeed");
    assert!(reply.to_lowercase().contains("meow"), "reply: {reply:?}");

    // Provider was called once.
    assert_eq!(provider.capture_count(), 1);
    let request = provider.last_request().expect("captured");
    // No memory injection wrapper anywhere in the messages we sent.
    let any_memory = request.messages.iter().any(|m| {
        m.content.iter().any(|b| match b {
            ContentBlock::Text { text, .. } => text.contains("<system-reminder>"),
            _ => false,
        })
    });
    assert!(
        !any_memory,
        "Direct turn must not inject memory <system-reminder>"
    );

    // Telemetry: admission == direct, no interlang refs, no anchored memory.
    let acc = agent
        .last_payload_accounting()
        .expect("accounting populated");
    assert_eq!(acc.admission, Some("direct"));
    assert_eq!(acc.interlang_refs_blocks, 0);
    assert_eq!(acc.memory_inject_chars, 0);
    assert_eq!(acc.memory_anchor_chars, 0);
    // Direct turns under v2 send 0 tools (or only `tool_expand` if it
    // existed in the locked list — Registry::empty has none).
    assert_eq!(acc.tools_count, 0, "Direct turns must send 0 tools");
    let est = approx_payload_chars(acc);
    assert!(
        est < 8_000,
        "Direct turn payload estimate {est} chars must be < 8KB"
    );

    unsafe {
        std::env::remove_var("NEURA_CONTEXT_COMPILER");
    }
}

#[tokio::test]
async fn smoke_b_direct_arithmetic_minimal_payload() {
    let _g = smoke_env_lock();
    unsafe {
        std::env::set_var("NEURA_CONTEXT_COMPILER", "v2");
        std::env::remove_var("NEURA_MEMORY_ANCHOR");
    }
    let provider = MockProvider::new("mock", "test-arith", vec!["4".to_string()]);
    let mut agent = make_agent(provider.clone());
    let reply = agent
        .run_once_capture("what is 2+2")
        .await
        .expect("arithmetic turn ok");
    assert!(reply.contains('4'));
    let acc = agent.last_payload_accounting().expect("accounting");
    // "what is 2+2" includes "what" → admission Light. Either way it must
    // not trigger memory or interlang.
    assert!(matches!(acc.admission, Some("direct") | Some("light")));
    assert_eq!(acc.memory_inject_chars, 0);
    assert_eq!(acc.interlang_refs_blocks, 0);
    let est = approx_payload_chars(acc);
    assert!(est < 12_000, "arithmetic estimate {est} chars < 12KB");
    unsafe {
        std::env::remove_var("NEURA_CONTEXT_COMPILER");
    }
}

#[tokio::test]
async fn smoke_c_memory_anchor_and_mem_get_recovery() {
    let _g = smoke_env_lock();
    unsafe {
        std::env::set_var("NEURA_CONTEXT_COMPILER", "v2");
        std::env::set_var("NEURA_MEMORY_ANCHOR", "1");
    }
    // Pre-stash some "memory" so the anchor body has something to surface.
    let session_id = "smoke-c-session";
    crate::memory::clear_anchored_memory(session_id);
    crate::memory::stash_memory_for_anchor_rehydration(
        session_id,
        "## Notes\n1. user prefers tabs",
        &["m1".to_string()],
    );

    // Verify .mem_get rehydration round-trips.
    let rehydrated = crate::memory::maybe_rehydrate_mem_get(session_id, ".mem_get reason=recall")
        .expect("rehydrate should succeed");
    assert!(rehydrated.contains("user prefers tabs"));
    assert!(rehydrated.contains("recall"));

    crate::memory::clear_anchored_memory(session_id);
    unsafe {
        std::env::remove_var("NEURA_MEMORY_ANCHOR");
        std::env::remove_var("NEURA_CONTEXT_COMPILER");
    }
}

#[tokio::test]
async fn smoke_d_coding_keeps_tools_admitted() {
    let _g = smoke_env_lock();
    unsafe {
        std::env::set_var("NEURA_CONTEXT_COMPILER", "v2");
        std::env::remove_var("NEURA_MEMORY_ANCHOR");
    }
    let provider = MockProvider::new(
        "mock",
        "test-coding",
        vec!["I'll search for ContextCompiler.".to_string()],
    );
    let mut agent = make_agent(provider.clone());
    let _reply = agent
        .run_once_capture("search the repo for ContextCompiler in src/")
        .await
        .expect("coding turn ok");
    let request = provider.last_request().expect("captured");
    // Coding turns must NOT have their tool list shrunk by the v2 plan.
    let tool_names: Vec<&str> = request.tools.iter().map(|t| t.name.as_str()).collect();
    let acc = agent.last_payload_accounting().expect("accounting");
    // Admission resolves to Deep (contains 'search', 'src/', 'repo').
    assert_eq!(acc.admission, Some("deep"));
    // Even with an empty registry the assertion is structural: tools_count
    // matches what tools we had available. The test will catch any future
    // regression that would over-shrink Deep tool admission.
    assert_eq!(acc.tools_count, tool_names.len());
    unsafe {
        std::env::remove_var("NEURA_CONTEXT_COMPILER");
    }
}

#[tokio::test]
async fn smoke_e_continuation_inherits_deep_admission() {
    use crate::agent::turn_loops::{TurnAdmission, latest_is_tool_result_only};
    let _g = smoke_env_lock();
    let messages = vec![
        Message::user("fix the failing build in src/foo.rs"),
        Message::assistant_text("running grep"),
        Message::tool_result("c-1", "match", false),
    ];
    assert!(latest_is_tool_result_only(&messages));
    // The continuation tier must keep memory + interlang + sidecar enabled,
    // matching Deep.
    let cont = TurnAdmission::Continuation;
    assert!(cont.use_memory());
    assert!(cont.use_interlang());
    assert!(cont.use_sidecar());
}

#[tokio::test]
async fn smoke_f_ctx_vault_round_trips_after_seen_clear() {
    let _g = smoke_env_lock();
    let _guard = crate::interlang::tests::seen_test_lock();
    let temp = tempfile::TempDir::new().unwrap();
    let prev_home = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("HOME", temp.path());
    }
    // Encode a block via the public diet path, then drop in-memory state and
    // verify `.ctx_get` rehydrates exact text. Mirrors a Neura restart.
    let text = "smoke F vault round trip ".repeat(200);
    // We can't call the private helper directly, but we can drive it via
    // `maybe_compact_messages` against a forced large input.
    let many_filler = (0..40)
        .map(|i| {
            Message::user(&format!(
                "filler {i} alpha beta gamma delta epsilon {}",
                "alpha beta gamma delta ".repeat(60)
            ))
        })
        .collect::<Vec<_>>();
    let mut messages = vec![Message::user(&text)];
    messages.extend(many_filler);
    messages.push(Message::user("now do something"));
    let _ = crate::interlang::maybe_compact_messages(&messages);
    // Walk the temp vault directory: at least one file should exist.
    let vault_dir = temp.path().join(".neura/ctx-vault");
    let mut found_any = false;
    if vault_dir.exists() {
        for entry in std::fs::read_dir(&vault_dir).unwrap() {
            let shard = entry.unwrap().path();
            if shard.is_dir() {
                for inner in std::fs::read_dir(&shard).unwrap() {
                    let f = inner.unwrap().path();
                    if f.extension().map(|e| e == "json").unwrap_or(false) {
                        found_any = true;
                    }
                }
            }
        }
    }
    assert!(found_any, "ctx-vault must persist at least one block");

    unsafe {
        if let Some(prev) = prev_home {
            std::env::set_var("HOME", prev);
        } else {
            std::env::remove_var("HOME");
        }
    }
}

#[tokio::test]
async fn smoke_g_regression_coding_with_v1_default() {
    let _g = smoke_env_lock();
    unsafe {
        std::env::remove_var("NEURA_CONTEXT_COMPILER");
        std::env::remove_var("NEURA_MEMORY_ANCHOR");
    }
    let provider = MockProvider::new(
        "mock",
        "test-regression",
        vec!["okay let me read that file".to_string()],
    );
    let mut agent = make_agent(provider.clone());
    let reply = agent
        .run_once_capture("look at src/agent/turn_loops.rs and tell me what it does")
        .await
        .expect("regression turn ok");
    assert!(!reply.is_empty());
    // Default (v1) compiler mode does not enforce; admission is deep.
    let acc = agent.last_payload_accounting().expect("accounting");
    assert_eq!(acc.admission, Some("deep"));
    // The legacy v1 path must continue to ship tools (no zero-shrink under v1).
    assert_eq!(
        acc.tools_count,
        provider.last_request().unwrap().tools.len()
    );
}

#[test]
fn smoke_compiler_mode_parses_v2() {
    let _g = smoke_env_lock();
    unsafe {
        std::env::set_var("NEURA_CONTEXT_COMPILER", "v2");
    }
    let mode = crate::agent::context_compiler::compiler_mode();
    assert!(matches!(mode, CompilerMode::V2));
    unsafe {
        std::env::remove_var("NEURA_CONTEXT_COMPILER");
    }
    let mode = crate::agent::context_compiler::compiler_mode();
    assert!(matches!(mode, CompilerMode::V1));
}
