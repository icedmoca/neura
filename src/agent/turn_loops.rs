use super::*;
use crate::runtime_ledger::{self, RuntimeReceiptKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TurnAdmission {
    /// Trivial turn (e.g., "meow"); skip memory / interlang / sidecar.
    Direct,
    /// Conversational/general turn; memory + interlang on, sidecar off.
    Light,
    /// Code/repo/debug/coordination turn; full pipeline on.
    Deep,
    /// Tool-loop continuation; inherits the originating turn's tier behaviour
    /// (memory + interlang + sidecar) but skips re-classification work and is
    /// recorded distinctly in telemetry.
    Continuation,
}

impl TurnAdmission {
    pub(crate) fn use_memory(self) -> bool {
        matches!(
            self,
            TurnAdmission::Light | TurnAdmission::Deep | TurnAdmission::Continuation
        )
    }
    pub(crate) fn use_interlang(self) -> bool {
        matches!(
            self,
            TurnAdmission::Light | TurnAdmission::Deep | TurnAdmission::Continuation
        )
    }
    pub(crate) fn use_sidecar(self) -> bool {
        matches!(self, TurnAdmission::Deep | TurnAdmission::Continuation)
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            TurnAdmission::Direct => "direct",
            TurnAdmission::Light => "light",
            TurnAdmission::Deep => "deep",
            TurnAdmission::Continuation => "continuation",
        }
    }
}

/// One-pass scan of the provider message vector that produces all the
/// per-block stats the turn loop needs: total chars per block kind, presence
/// of `<ctx_auto_exact>` markers, total interlang ref chars, and the top-N
/// largest blocks by char count. Replaces several independent O(history)
/// walks (`messages_contain_auto_exact`, the `<ctx>` detection inside
/// `log_pre_provider_payload`, the `aggregate_message_chars_for_payload_diet`
/// duplicate, etc.).
#[derive(Debug, Default, Clone)]
pub(crate) struct MessageWalkStats {
    pub total_chars: usize,
    pub text_chars: usize,
    pub tool_use_chars: usize,
    pub tool_result_chars: usize,
    pub reasoning_chars: usize,
    pub image_chars: usize,
    pub interlang_refs_chars: usize,
    pub interlang_refs_blocks: usize,
    pub contains_auto_exact: bool,
    pub top_blocks: Vec<crate::provider::remote_telemetry::TopBlockEntry>,
}

const TOP_BLOCK_LIMIT: usize = 3;

pub(crate) fn walk_messages(messages: &[Message]) -> MessageWalkStats {
    let mut stats = MessageWalkStats::default();
    for (idx, message) in messages.iter().enumerate() {
        let role = match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        for block in &message.content {
            let (kind, chars, payload_for_hash) = match block {
                ContentBlock::Text { text, .. } => {
                    stats.text_chars += text.len();
                    if text_contains_auto_exact(text) {
                        stats.contains_auto_exact = true;
                    }
                    if text_contains_interlang_ref(text) {
                        stats.interlang_refs_chars += text.len();
                        stats.interlang_refs_blocks += 1;
                    }
                    ("text", text.len(), text.as_str())
                }
                ContentBlock::Reasoning { text } => {
                    stats.reasoning_chars += text.len();
                    if text_contains_auto_exact(text) {
                        stats.contains_auto_exact = true;
                    }
                    if text_contains_interlang_ref(text) {
                        stats.interlang_refs_chars += text.len();
                        stats.interlang_refs_blocks += 1;
                    }
                    ("reasoning", text.len(), text.as_str())
                }
                ContentBlock::ToolResult { content, .. } => {
                    stats.tool_result_chars += content.len();
                    if text_contains_auto_exact(content) {
                        stats.contains_auto_exact = true;
                    }
                    if text_contains_interlang_ref(content) {
                        stats.interlang_refs_chars += content.len();
                        stats.interlang_refs_blocks += 1;
                    }
                    ("tool_result", content.len(), content.as_str())
                }
                ContentBlock::ToolUse { name, input, .. } => {
                    let input_str = input.to_string();
                    let chars = name.len() + input_str.len();
                    stats.tool_use_chars += chars;
                    ("tool_use", chars, "")
                }
                ContentBlock::Image { data, .. } => {
                    stats.image_chars += data.len();
                    ("image", data.len(), "")
                }
                ContentBlock::OpenAICompaction { encrypted_content } => {
                    stats.text_chars += encrypted_content.len();
                    ("openai_compaction", encrypted_content.len(), "")
                }
            };
            stats.total_chars += chars;
            consider_top_block(
                &mut stats.top_blocks,
                kind,
                chars,
                payload_for_hash,
                role,
                idx,
            );
        }
    }
    stats
}

fn text_contains_interlang_ref(text: &str) -> bool {
    text.contains("<ctx ")
        || text.contains("<il:seen")
        || text.contains("<ctx_candidate")
        || text.contains("<il:v1>")
}

fn consider_top_block(
    top: &mut Vec<crate::provider::remote_telemetry::TopBlockEntry>,
    kind: &'static str,
    chars: usize,
    payload: &str,
    role: &'static str,
    message_index: usize,
) {
    // Keep top vector sorted descending by chars. Drop the smallest when full.
    if chars == 0 {
        return;
    }
    let entry = crate::provider::remote_telemetry::TopBlockEntry {
        kind,
        chars,
        hash: short_block_hash(payload),
        role,
        message_index,
    };
    if top.len() < TOP_BLOCK_LIMIT {
        top.push(entry);
        top.sort_by(|a, b| b.chars.cmp(&a.chars));
        return;
    }
    if let Some(smallest) = top.last() {
        if smallest.chars >= chars {
            return;
        }
    }
    top.pop();
    top.push(entry);
    top.sort_by(|a, b| b.chars.cmp(&a.chars));
}

fn short_block_hash(payload: &str) -> String {
    if payload.is_empty() {
        return String::new();
    }
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(payload.as_bytes());
    hex::encode(&digest[..6])
}

/// Returns true when the latest message in `messages` is a tool-result-only
/// user turn — i.e., the model is in a tool loop, not responding to a fresh
/// user request. Used for continuation-turn admission inheritance.
pub(crate) fn latest_is_tool_result_only(messages: &[Message]) -> bool {
    let Some(last) = messages.last() else {
        return false;
    };
    if last.role != Role::User {
        return false;
    }
    if last.content.is_empty() {
        return false;
    }
    last.content
        .iter()
        .all(|block| matches!(block, ContentBlock::ToolResult { .. }))
}

fn classify_turn_admission(messages: &[Message]) -> TurnAdmission {
    let Some(latest) = latest_user_text_for_payload_diet(messages) else {
        return TurnAdmission::Light;
    };
    let trimmed = latest.trim();
    let lower = trimmed.to_ascii_lowercase();
    if trimmed.len() <= 120
        && !trimmed.contains('?')
        && !contains_any_for_payload_diet(
            &lower,
            &["what", "how", "why", "explain", "tell me about"],
        )
        && !trimmed.contains('\n')
        && !trimmed.contains("```")
        && !contains_any_for_payload_diet(
            &lower,
            &[
                "fix", "debug", "build", "test", "error", "failed", "repo", "code", "file", "src/",
                "docs/", ".rs", ".py", ".md", "continue", "previous", "earlier", "above", "that",
                "those", "memory", "token", "context", "why", "how many", "browser", "click",
                "search", "web", "email", "gmail", "commit", "push",
            ],
        )
    {
        return TurnAdmission::Direct;
    }
    if contains_any_for_payload_diet(
        &lower,
        &[
            "fix",
            "debug",
            "build",
            "test",
            "error",
            "failed",
            "repo",
            "code",
            "file",
            "src/",
            "docs/",
            ".rs",
            ".py",
            ".md",
            "continue",
            "previous",
            "earlier",
            "above",
            "browser",
            "click",
            "search",
            "web",
            "email",
            "gmail",
            "commit",
            "push",
            "benchmark",
        ],
    ) || trimmed.len() > 800
        || trimmed.contains('\n')
        || trimmed.contains("```")
    {
        TurnAdmission::Deep
    } else {
        TurnAdmission::Light
    }
}

fn latest_user_turn_mentions_context_stats(
    messages: impl AsRef<[crate::message::Message]>,
) -> bool {
    let messages = messages.as_ref();
    messages
        .iter()
        .rev()
        .find_map(|message| {
            if message.role != crate::message::Role::User {
                return None;
            }
            let text = message
                .content
                .iter()
                .filter_map(|block| match block {
                    crate::message::ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
                .to_ascii_lowercase();
            let trimmed = text.trim();
            if trimmed.is_empty() || trimmed.starts_with("<system-reminder>") {
                None
            } else {
                Some(
                    trimmed.contains("token")
                        || trimmed.contains("context")
                        || trimmed.contains("compact")
                        || trimmed.contains("compression")
                        || trimmed.contains("interlang")
                        || trimmed.contains("ctx")
                        || trimmed.contains("rehydrat"),
                )
            }
        })
        .unwrap_or(false)
}

impl Agent {
    /// Run turns until no more tool calls
    /// Maximum number of context-limit compaction retries before giving up.
    pub(super) const MAX_CONTEXT_LIMIT_RETRIES: u32 = 5;
    pub(super) const MAX_INCOMPLETE_CONTINUATION_ATTEMPTS: u32 = 3;

    pub(super) async fn run_turn(&mut self, print_output: bool) -> Result<String> {
        self.set_log_context();
        crate::interlang::reset_retrieval_turn();
        let mut final_text = String::new();
        let trace = trace_enabled();
        let mut context_limit_retries = 0u32;
        let mut incomplete_continuations = 0u32;
        // Phase 2: continuation-turn admission inheritance. The first loop
        // iteration classifies normally; subsequent iterations whose latest
        // message is a tool-result-only continuation reuse that admission so
        // the model stays on a coherent budget for the duration of a tool loop.
        let mut originating_admission: Option<TurnAdmission> = None;
        let mut skill_anchor_injected = false;

        loop {
            if !skill_anchor_injected {
                let skills_registry = self.registry.skills();
                let skills = skills_registry.read().await;
                if let Some(anchor) = crate::skill::build_skill_anchor(&skills) {
                    self.add_message(
                        Role::User,
                        vec![ContentBlock::Text {
                            text: anchor,
                            cache_control: None,
                        }],
                    );
                }
            }
            skill_anchor_injected = true;

            let repaired = self.repair_missing_tool_outputs();
            if repaired > 0 {
                logging::warn(&format!(
                    "Recovered {} missing tool output(s) before API call",
                    repaired
                ));
            }
            let (messages, compaction_event) = self.messages_for_provider();
            if let Some(event) = compaction_event {
                // Reset cache tracker and tool lock on compaction since the message history changes
                self.cache_tracker.reset();
                self.locked_tools = None;
                if print_output {
                    let tokens_str = event
                        .pre_tokens
                        .map(|t| format!(" ({} tokens)", t))
                        .unwrap_or_default();
                    println!("📦 Context compacted ({}){}", event.trigger, tokens_str);
                }
            }

            let tools = self.tool_definitions().await;
            let messages: std::sync::Arc<[Message]> = messages.into();
            // Local sidecar/context admission happens before memory, interlang, and sidecar routing.
            let classified_admission = classify_turn_admission(&messages);
            let admission =
                if latest_is_tool_result_only(&messages) && originating_admission.is_some() {
                    TurnAdmission::Continuation
                } else {
                    originating_admission = Some(classified_admission);
                    classified_admission
                };
            // Per-turn telemetry id, scoped via thread-local so deep call sites
            // (SSE handler, async sinks) can attach it without signature churn.
            let turn_id = crate::provider::remote_telemetry::new_turn_id();
            let _turn_guard = crate::provider::remote_telemetry::TurnIdGuard::install(&turn_id);
            // Phase 5 — compile a `TurnPlan` from the same inputs the legacy
            // logic uses. Under v1/shadow, the plan is computed for telemetry
            // only. Under v2, the plan drives tool admission, interlang
            // budget, memory plan, history clamp, and sidecar pre-route.
            let plan = {
                use crate::agent::context_compiler::{
                    CompileInputs, MemoryPlanInput, compile_turn_plan,
                };
                let tool_names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
                let pending_for_inputs = None::<MemoryPlanInput>;
                let inputs = CompileInputs {
                    admission,
                    messages: &messages,
                    tool_names: &tool_names,
                    classified_intent_tool_names: &[],
                    system_static_chars: 0,
                    system_dynamic_chars: 0,
                    provider_context_window: Some(self.provider.context_window()),
                    memory_pending: pending_for_inputs,
                    interlang_refs_chars: 0,
                    history_chars_observed: 0,
                    locked_tools_chars: self.locked_tools_chars(),
                };
                compile_turn_plan(&inputs)
            };
            let plan_active = plan.should_apply();
            // Memory: respect the plan's run_memory bit when v2 is enforcing,
            // otherwise fall back to the per-tier default. v1 default is
            // identical to admission.use_memory().
            let run_memory = if plan_active {
                plan.run_memory
            } else {
                admission.use_memory()
            };
            // Non-blocking memory: uses pending result from last turn, spawns check for next turn
            let memory_pending = if run_memory {
                self.build_memory_prompt_nonblocking_shared(std::sync::Arc::clone(&messages), None)
            } else {
                None
            };
            // Use split prompt for better caching - static content cached, dynamic not
            let split_prompt = self.build_system_prompt_split(None);
            self.log_prompt_prefix_accounting(&split_prompt, &tools);

            // Check for client-side cache violations before memory injection.
            // Memory is an ephemeral suffix that changes each turn; tracking it would cause
            // false-positive violations every turn (prior turn's memory ≠ current history prefix).
            self.record_client_cache_request(&messages);

            // Inject memory as a user message at the end (preserves cache prefix)
            let mut messages_with_memory: Vec<Message> = messages.iter().cloned().collect();
            // Track which memory mode we ended up in, for telemetry below.
            let mut memory_anchor_chars_used = 0usize;
            if let Some(memory) = memory_pending.as_ref() {
                let memory_count = memory.count.max(1);
                let age_ms = memory.computed_at.elapsed().as_millis() as u64;
                crate::memory::record_injected_prompt(&memory.prompt, memory_count, age_ms);
                self.record_memory_injection_in_session(memory);
                let plan = crate::agent::context_compiler::plan_memory(
                    admission,
                    Some(memory.prompt.as_str()),
                    memory_count,
                    &memory.memory_ids,
                    latest_user_text_for_payload_diet(&messages).as_deref(),
                );
                match plan {
                    crate::agent::context_compiler::MemoryPlan::Inject {
                        prompt,
                        count: _count,
                        memory_ids,
                    } => {
                        logging::info(&format!(
                            "Memory injected as message ({} chars)",
                            prompt.len()
                        ));
                        let memory_msg =
                            format!("<system-reminder>\n{}\n</system-reminder>", prompt);
                        messages_with_memory.push(Message::user(&memory_msg));
                        crate::memory::stash_memory_for_anchor_rehydration(
                            &self.session.id,
                            &prompt,
                            &memory_ids,
                        );
                    }
                    crate::agent::context_compiler::MemoryPlan::Anchor {
                        count,
                        memory_ids: _memory_ids,
                    } => {
                        let anchor_text =
                            crate::agent::context_compiler::MemoryPlan::anchor_text(count);
                        memory_anchor_chars_used = anchor_text.len();
                        logging::info(&format!(
                            "Memory anchor injected ({} bytes; full prompt held for .mem_get)",
                            memory_anchor_chars_used
                        ));
                        messages_with_memory.push(Message::user(&anchor_text));
                        crate::memory::stash_memory_for_anchor_rehydration(
                            &self.session.id,
                            &memory.prompt,
                            &memory.memory_ids,
                        );
                    }
                    crate::agent::context_compiler::MemoryPlan::Skip => {
                        logging::info(
                            "Memory skipped for this turn by MemoryPlan (admission=Direct or no pending)",
                        );
                    }
                }
            }

            logging::info(&format!(
                "API call starting: {} messages, {} tools",
                messages_with_memory.len(),
                tools.len()
            ));
            let api_start = Instant::now();

            // Publish status for TUI to show during Task execution
            Bus::global().publish(BusEvent::SubagentStatus(SubagentStatus {
                session_id: self.session.id.clone(),
                status: "calling API".to_string(),
                model: Some(self.provider.model()),
            }));

            let stamped;
            let base_send_messages: &[Message] =
                if crate::config::config().features.message_timestamps {
                    stamped = Message::with_timestamps(&messages_with_memory);
                    &stamped
                } else {
                    &messages_with_memory
                };
            let interlang_messages;
            let mut interlang_dynamic_part: Option<String> = None;
            let interlang_active = if plan_active {
                plan.run_interlang
            } else {
                admission.use_interlang()
            };
            let send_messages: &[Message] = if interlang_active
                && crate::interlang::enabled()
                && plan.max_interlang_blocks.map(|cap| cap > 0).unwrap_or(true)
            {
                let budget = crate::interlang::CompactBudget {
                    max_blocks: plan.max_interlang_blocks,
                    recent_bytes: plan.recent_window_bytes,
                };
                let (encoded, stats) = crate::interlang::maybe_compact_messages_with_budget(
                    base_send_messages,
                    budget,
                );
                crate::interlang::record_stats(stats);
                if stats.blocks_encoded > 0 {
                    let report = stats.report_line();
                    logging::info(&report);
                    let interlang_prompt = if latest_user_turn_mentions_context_stats(messages) {
                        format!(
                            "{}{}",
                            crate::interlang::decoder_prompt(),
                            crate::interlang::realtime_stats_prompt(stats)
                        )
                    } else {
                        crate::interlang::decoder_prompt()
                    };
                    interlang_messages = encoded;
                    interlang_dynamic_part =
                        Some(format!("{}{}", split_prompt.dynamic_part, interlang_prompt));
                    &interlang_messages
                } else {
                    base_send_messages
                }
            } else {
                base_send_messages
            };
            let dynamic_part = interlang_dynamic_part
                .as_deref()
                .unwrap_or(&split_prompt.dynamic_part);
            // Single pass over the post-interlang send_messages: produces the
            // auto_exact flag, per-kind char totals, interlang ref accounting,
            // and the top-3 largest blocks for telemetry. Replaces several
            // independent block walks (auto_exact + budget + telemetry).
            let send_walk = walk_messages(send_messages);
            let send_walk_total_chars = send_walk.total_chars;
            let send_walk_contains_auto_exact = send_walk.contains_auto_exact;
            let send_admission_over_budget =
                send_walk_total_chars > final_prompt_message_budget_for_admission(send_messages);
            // Sidecar pre-route obeys the plan (which already accounts for
            // sidecar mode = decide/log/off).
            let run_sidecar = if plan_active {
                plan.run_sidecar_preroute
            } else {
                admission.use_sidecar()
            };
            if run_sidecar
                && !matches!(
                    crate::agent::context_compiler::sidecar_preroute_mode(),
                    crate::agent::context_compiler::SidecarPrerouteMode::Off
                )
            {
                crate::local_model::pre_route_async(send_messages);
            }
            let provider_payload;
            let provider_needs_sanitizer = send_walk_contains_auto_exact;
            let short_turn_admission_fires = send_admission_over_budget;
            // v2 history clamp: when the plan opts to clamp (Direct turns),
            // run the existing short-turn compactor unconditionally even if
            // budget hasn't been exceeded — Direct turns always benefit.
            let v2_clamp = plan_active && plan.clamp_history;
            let provider_messages =
                if short_turn_admission_fires || provider_needs_sanitizer || v2_clamp {
                    provider_payload = compact_provider_messages_for_short_turn(send_messages);
                    &provider_payload
                } else {
                    send_messages
                };
            // Compute final accounting from whichever message vector is going
            // to the provider. When the short-turn admission rewrote messages,
            // re-walk the (much smaller) compacted vector. Otherwise reuse the
            // earlier walk.
            let provider_walk = if short_turn_admission_fires || provider_needs_sanitizer {
                walk_messages(provider_messages)
            } else {
                send_walk
            };
            let memory_inject_chars = memory_pending.as_ref().map(|m| m.prompt.len()).unwrap_or(0);
            let memory_inject_count = memory_pending.as_ref().map(|m| m.count).unwrap_or(0);
            // Cached on lock (turn_execution::tool_definitions) so we avoid
            // re-serializing the tool list every turn just for telemetry.
            let tools_json_chars = self
                .locked_tools_chars()
                .unwrap_or_else(|| crate::message::ToolDefinition::aggregate_prompt_chars(&tools));
            let provider_model_for_telemetry = self.provider.model();
            let provider_context_window = Some(self.provider.context_window());
            let mut accounting = crate::provider::remote_telemetry::PayloadAccounting::default();
            accounting.admission = Some(admission.label());
            accounting.provider = self.provider.name().to_string();
            accounting.model = Some(provider_model_for_telemetry.clone());
            accounting.system_static_chars = split_prompt.static_part.len();
            accounting.system_dynamic_chars = dynamic_part.len();
            accounting.messages_chars = provider_walk.total_chars;
            accounting.messages_text_chars = provider_walk.text_chars;
            accounting.messages_tool_use_chars = provider_walk.tool_use_chars;
            accounting.messages_tool_result_chars = provider_walk.tool_result_chars;
            accounting.messages_reasoning_chars = provider_walk.reasoning_chars;
            accounting.messages_image_chars = provider_walk.image_chars;
            accounting.tools_json_chars = tools_json_chars;
            accounting.locked_tools_cached_chars = self.locked_tools_chars();
            accounting.tools_count = tools.len();
            accounting.interlang_refs_chars = provider_walk.interlang_refs_chars;
            accounting.interlang_refs_blocks = provider_walk.interlang_refs_blocks;
            accounting.memory_inject_chars = memory_inject_chars;
            accounting.memory_inject_count = memory_inject_count;
            accounting.memory_anchor_chars = memory_anchor_chars_used;
            accounting.compacted_short_turn = short_turn_admission_fires;
            accounting.top_blocks = provider_walk.top_blocks.clone();
            accounting.provider_context_window = provider_context_window;
            crate::provider::remote_telemetry::log_pre_provider_payload_with_accounting(
                self.provider.name(),
                provider_messages,
                &tools,
                &split_prompt.static_part,
                &dynamic_part,
                short_turn_admission_fires,
                Some(&accounting),
            );
            // Phase 5.D — surface the accounting + turn id on the agent so
            // debug callers can read the most-recent turn stats without
            // re-tailing the JSONL files.
            self.last_payload_accounting = Some(accounting.clone());
            self.last_turn_id = Some(turn_id.clone());
            // Per-turn trace record carries the admission decision and the
            // raw component sizes. Joinable to provider-reported usage via
            // `turn_id`.
            // Phase 2 — shadow telemetry: emit the budget/tool/memory plan
            // the v2 compiler *would* recommend for this turn so we can see
            // expected savings before flipping enforcement on. Cheap.
            crate::agent::context_compiler::record_shadow_admission(
                admission,
                &tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
                &Vec::<String>::new(),
                provider_walk.total_chars,
                memory_inject_chars,
                latest_user_text_for_payload_diet(provider_messages).as_deref(),
            );
            crate::provider::remote_telemetry::log_turn_trace(serde_json::json!({
                "event": "turn_summary",
                "admission": admission.label(),
                "admission_classified": classified_admission.label(),
                "session_id": self.session.id,
                "provider": self.provider.name(),
                "model": provider_model_for_telemetry,
                "provider_context_window": provider_context_window,
                "system_static_chars": split_prompt.static_part.len(),
                "system_dynamic_chars": dynamic_part.len(),
                "messages_total_chars": provider_walk.total_chars,
                "messages_text_chars": provider_walk.text_chars,
                "messages_tool_use_chars": provider_walk.tool_use_chars,
                "messages_tool_result_chars": provider_walk.tool_result_chars,
                "messages_reasoning_chars": provider_walk.reasoning_chars,
                "interlang_refs_chars": provider_walk.interlang_refs_chars,
                "interlang_refs_blocks": provider_walk.interlang_refs_blocks,
                "tools_json_chars": tools_json_chars,
                "tools_count": tools.len(),
                "memory_inject_chars": memory_inject_chars,
                "memory_inject_count": memory_inject_count,
                "short_turn_admission_fired": short_turn_admission_fires,
                "auto_exact_present": send_walk_contains_auto_exact,
                "top_blocks": provider_walk.top_blocks,
            }));
            self.last_status_detail = None;
            // v2 tool-subset enforcement: when the plan supplies a subset, the
            // request goes upstream with only those tools. The locked tool
            // list is still kept in `self.locked_tools` so `tool_expand` can
            // still resolve hidden tool names if the model asks.
            let provider_tools_owned: Vec<ToolDefinition>;
            let provider_tools: &[ToolDefinition] = if let Some(subset) = plan.tool_subset.as_ref()
            {
                if subset.is_empty() && !tools.is_empty() {
                    provider_tools_owned = Vec::new();
                    &provider_tools_owned
                } else if subset.iter().all(|n| tools.iter().any(|t| &t.name == n))
                    && subset.len() < tools.len()
                {
                    provider_tools_owned = tools
                        .iter()
                        .filter(|t| subset.iter().any(|n| n == &t.name))
                        .cloned()
                        .collect();
                    &provider_tools_owned
                } else {
                    &tools
                }
            } else {
                &tools
            };
            // Recompute tools_json telemetry to reflect the filtered subset.
            let provider_tools_chars =
                crate::message::ToolDefinition::aggregate_prompt_chars(provider_tools);
            crate::provider::remote_telemetry::log_turn_trace(serde_json::json!({
                "event": "turn_plan",
                "compiler_mode": format!("{:?}", plan.mode),
                "plan_active": plan_active,
                "admission": admission.label(),
                "tool_subset": plan.tool_subset.clone(),
                "tool_subset_active_count": provider_tools.len(),
                "tools_full_count": tools.len(),
                "tools_full_chars": tools_json_chars,
                "tools_subset_chars": provider_tools_chars,
                "max_interlang_blocks": plan.max_interlang_blocks,
                "recent_window_bytes": plan.recent_window_bytes,
                "memory_plan": match &plan.memory_plan {
                    crate::agent::context_compiler::MemoryPlan::Inject { .. } => "inject",
                    crate::agent::context_compiler::MemoryPlan::Anchor { .. } => "anchor",
                    crate::agent::context_compiler::MemoryPlan::Skip => "skip",
                },
                "run_memory": plan.run_memory,
                "run_interlang": plan.run_interlang,
                "run_sidecar_preroute": plan.run_sidecar_preroute,
                "clamp_history": plan.clamp_history,
                "confidence": plan.confidence,
            }));
            if runtime_ledger::enabled() {
                runtime_ledger::append_receipt_best_effort(
                    RuntimeReceiptKind::ProviderCall,
                    "start",
                    serde_json::json!({
                        "provider": self.provider.name(),
                        "model": self.provider.model(),
                        "session_id": self.session.id,
                    }),
                );
            }
            let provider_result = self
                .provider
                .complete_split(
                    provider_messages,
                    provider_tools,
                    &split_prompt.static_part,
                    &dynamic_part,
                    self.provider_session_id.as_deref(),
                )
                .await;
            if runtime_ledger::enabled() {
                runtime_ledger::append_receipt_best_effort(
                    RuntimeReceiptKind::ProviderCall,
                    "finish",
                    serde_json::json!({
                        "provider": self.provider.name(),
                        "model": self.provider.model(),
                        "session_id": self.session.id,
                        "ok": provider_result.is_ok(),
                    }),
                );
            }
            let mut stream = match provider_result {
                Ok(stream) => stream,
                Err(e) => {
                    if self.try_auto_compact_after_context_limit(&e.to_string()) {
                        context_limit_retries += 1;
                        if context_limit_retries > Self::MAX_CONTEXT_LIMIT_RETRIES {
                            logging::warn(
                                "Context-limit compaction retry limit reached; giving up",
                            );
                            return Err(anyhow::anyhow!(
                                "Context limit exceeded after {} compaction retries",
                                Self::MAX_CONTEXT_LIMIT_RETRIES
                            ));
                        }
                        continue;
                    }
                    return Err(e);
                }
            };

            // Successful API call - reset retry counter
            context_limit_retries = 0;

            logging::info(&format!(
                "API stream opened in {:.2}s",
                api_start.elapsed().as_secs_f64()
            ));

            Bus::global().publish(BusEvent::SubagentStatus(SubagentStatus {
                session_id: self.session.id.clone(),
                status: "streaming".to_string(),
                model: Some(self.provider.model()),
            }));

            let mut text_content = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut current_tool: Option<ToolCall> = None;
            let mut current_tool_input = String::new();
            let mut usage_input: Option<u64> = None;
            let mut usage_output: Option<u64> = None;
            let mut usage_cache_read: Option<u64> = None;
            let mut usage_cache_creation: Option<u64> = None;
            let mut saw_message_end = false;
            let mut stop_reason: Option<String> = None;
            let mut _thinking_start: Option<Instant> = None;
            let store_reasoning_content = self.provider.name() == "openrouter";
            let mut reasoning_content = String::new();
            // Track tool results from provider (already executed by Claude Code CLI)
            let mut sdk_tool_results: std::collections::HashMap<String, (String, bool)> =
                std::collections::HashMap::new();
            let mut openai_native_compaction: Option<(String, usize)> = None;

            let mut retry_after_compaction = false;
            while let Some(event) = stream.next().await {
                let event = match event {
                    Ok(event) => event,
                    Err(e) => {
                        let err_str = e.to_string();
                        if self.try_auto_compact_after_context_limit(&err_str) {
                            context_limit_retries += 1;
                            if context_limit_retries > Self::MAX_CONTEXT_LIMIT_RETRIES {
                                logging::warn(
                                    "Context-limit compaction retry limit reached; giving up",
                                );
                                return Err(anyhow::anyhow!(
                                    "Context limit exceeded after {} compaction retries",
                                    Self::MAX_CONTEXT_LIMIT_RETRIES
                                ));
                            }
                            retry_after_compaction = true;
                            break;
                        }
                        return Err(e);
                    }
                };

                match event {
                    StreamEvent::ThinkingStart => {
                        // Track start but don't print - wait for ThinkingDone
                        _thinking_start = Some(Instant::now());
                    }
                    StreamEvent::ThinkingDelta(thinking_text) => {
                        // Display reasoning content only if enabled
                        if print_output && crate::config::config().display.show_thinking {
                            println!("💭 {}", thinking_text);
                        }
                        if store_reasoning_content {
                            reasoning_content.push_str(&thinking_text);
                        }
                    }
                    StreamEvent::ThinkingEnd => {
                        // Don't print here - ThinkingDone has accurate timing
                        _thinking_start = None;
                    }
                    StreamEvent::ThinkingDone { duration_secs } => {
                        // Bridge provides accurate wall-clock timing
                        if print_output {
                            println!("Thought for {:.1}s\n", duration_secs);
                        }
                    }
                    StreamEvent::TextDelta(text) => {
                        if print_output {
                            print!("{}", text);
                            io::stdout().flush()?;
                        }
                        text_content.push_str(&text);
                    }
                    StreamEvent::ToolUseStart { id, name } => {
                        if trace {
                            eprintln!("\n[trace] tool_use_start name={} id={}", name, id);
                        }
                        if print_output {
                            print!("\n[{}] ", name);
                            io::stdout().flush()?;
                        }
                        current_tool = Some(ToolCall {
                            id,
                            name,
                            input: serde_json::Value::Null,
                            intent: None,
                        });
                        current_tool_input.clear();
                    }
                    StreamEvent::ToolInputDelta(delta) => {
                        current_tool_input.push_str(&delta);
                    }
                    StreamEvent::ToolUseEnd => {
                        if let Some(mut tool) = current_tool.take() {
                            // Parse the accumulated JSON
                            let tool_input =
                                serde_json::from_str::<serde_json::Value>(&current_tool_input)
                                    .unwrap_or(serde_json::Value::Null);
                            tool.input = tool_input.clone();
                            tool.intent = ToolCall::intent_from_input(&tool_input);

                            if trace {
                                if current_tool_input.trim().is_empty() {
                                    eprintln!("[trace] tool_input {} (empty)", tool.name);
                                } else if tool_input == serde_json::Value::Null {
                                    eprintln!(
                                        "[trace] tool_input {} (raw) {}",
                                        tool.name, current_tool_input
                                    );
                                } else {
                                    let pretty = serde_json::to_string_pretty(&tool_input)
                                        .unwrap_or_else(|_| tool_input.to_string());
                                    eprintln!("[trace] tool_input {} {}", tool.name, pretty);
                                }
                            }

                            if print_output {
                                // Show brief tool info
                                print_tool_summary(&tool);
                            }

                            tool_calls.push(tool);
                            current_tool_input.clear();
                        }
                    }
                    StreamEvent::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        // SDK already executed this tool, store the result
                        if trace {
                            eprintln!(
                                "[trace] sdk_tool_result id={} is_error={} content_len={}",
                                tool_use_id,
                                is_error,
                                content.len()
                            );
                        }
                        sdk_tool_results.insert(tool_use_id, (content, is_error));
                    }
                    StreamEvent::GeneratedImage {
                        id,
                        path,
                        metadata_path,
                        output_format,
                        revised_prompt,
                    } => {
                        if trace {
                            eprintln!(
                                "[trace] generated_image id={} format={} path={} metadata={}",
                                id,
                                output_format,
                                path,
                                metadata_path.as_deref().unwrap_or("none")
                            );
                        }
                        if print_output {
                            let summary = crate::message::generated_image_summary(
                                &path,
                                metadata_path.as_deref(),
                                &output_format,
                                revised_prompt.as_deref(),
                            );
                            eprintln!(
                                "\n[{}] {}",
                                crate::message::GENERATED_IMAGE_TOOL_NAME,
                                summary
                            );
                        }
                    }
                    StreamEvent::TokenUsage {
                        input_tokens,
                        output_tokens,
                        cache_read_input_tokens,
                        cache_creation_input_tokens,
                    } => {
                        if let Some(input) = input_tokens {
                            usage_input = Some(input);
                        }
                        if let Some(output) = output_tokens {
                            usage_output = Some(output);
                        }
                        if cache_read_input_tokens.is_some() {
                            usage_cache_read = cache_read_input_tokens;
                        }
                        if cache_creation_input_tokens.is_some() {
                            usage_cache_creation = cache_creation_input_tokens;
                        }
                        if let Some(input) = usage_input {
                            self.update_compaction_usage_from_stream(
                                input,
                                usage_cache_read,
                                usage_cache_creation,
                            );
                        }
                        if trace {
                            eprintln!(
                                "[trace] token_usage input={} output={} cache_read={} cache_write={}",
                                usage_input.unwrap_or(0),
                                usage_output.unwrap_or(0),
                                usage_cache_read.unwrap_or(0),
                                usage_cache_creation.unwrap_or(0)
                            );
                        }
                    }
                    StreamEvent::ConnectionType { connection } => {
                        if trace {
                            eprintln!("[trace] connection_type={}", connection);
                        }
                        crate::telemetry::record_connection_type(&connection);
                        self.last_connection_type = Some(connection);
                    }
                    StreamEvent::ConnectionPhase { phase } => {
                        if trace {
                            eprintln!("[trace] connection_phase={}", phase);
                        }
                    }
                    StreamEvent::StatusDetail { detail } => {
                        if trace {
                            eprintln!("[trace] status_detail={}", detail);
                        }
                        self.last_status_detail = Some(detail);
                    }
                    StreamEvent::MessageEnd {
                        stop_reason: reason,
                    } => {
                        saw_message_end = true;
                        if reason.is_some() {
                            stop_reason = reason;
                        }
                        // Don't break yet - wait for SessionId which comes after MessageEnd
                        // (but stream close will also end the loop for providers without SessionId)
                    }
                    StreamEvent::SessionId(sid) => {
                        if trace {
                            eprintln!("[trace] session_id {}", sid);
                        }
                        self.provider_session_id = Some(sid.clone());
                        self.session.provider_session_id = Some(sid);
                        // We've received session_id, can exit the loop now
                        if saw_message_end {
                            break;
                        }
                    }
                    StreamEvent::UpstreamProvider { provider } => {
                        // Log upstream provider for local trace output
                        if trace {
                            eprintln!("[trace] upstream_provider={}", provider);
                        }
                        self.last_upstream_provider = Some(provider);
                    }
                    StreamEvent::Compaction {
                        trigger,
                        pre_tokens,
                        openai_encrypted_content,
                    } => {
                        if let Some(encrypted_content) = openai_encrypted_content {
                            openai_native_compaction
                                .get_or_insert((encrypted_content, self.session.messages.len()));
                        }
                        if print_output {
                            let tokens_str = pre_tokens
                                .map(|t| format!(" ({} tokens)", t))
                                .unwrap_or_default();
                            println!("📦 Context compacted ({}){}", trigger, tokens_str);
                        }
                    }
                    StreamEvent::NativeToolCall {
                        request_id,
                        tool_name,
                        input,
                    } => {
                        // Execute native tool and send result back to SDK bridge
                        if trace {
                            eprintln!(
                                "[trace] native_tool_call request_id={} tool={}",
                                request_id, tool_name
                            );
                        }
                        let ctx = ToolContext {
                            session_id: self.session.id.clone(),
                            message_id: self.session.id.clone(),
                            tool_call_id: request_id.clone(),
                            working_dir: self.working_dir().map(PathBuf::from),
                            stdin_request_tx: self.stdin_request_tx.clone(),
                            graceful_shutdown_signal: Some(self.graceful_shutdown.clone()),
                            execution_mode: ToolExecutionMode::AgentTurn,
                        };
                        crate::telemetry::record_tool_call();
                        let evidence_paths =
                            crate::knowledge::evidence::candidate_paths(&tool_name, &input);
                        let tool_result = self.registry.execute(&tool_name, input, ctx).await;
                        if tool_result.is_err() {
                            crate::telemetry::record_tool_failure();
                        }
                        crate::knowledge::evidence::note_tool_outcome_paths(
                            &tool_name,
                            evidence_paths,
                            tool_result.is_ok(),
                        );
                        let native_result = match tool_result {
                            Ok(output) => NativeToolResult::success(request_id, output.output),
                            Err(e) => NativeToolResult::error(request_id, e.to_string()),
                        };
                        // Send result back to SDK bridge
                        if let Some(sender) = self.provider.native_result_sender() {
                            let _ = sender.send(native_result).await;
                        }
                    }
                    StreamEvent::Error {
                        message,
                        retry_after_secs,
                    } => {
                        if trace {
                            eprintln!("[trace] stream_error {}", message);
                        }
                        if self.try_auto_compact_after_context_limit(&message) {
                            context_limit_retries += 1;
                            if context_limit_retries > Self::MAX_CONTEXT_LIMIT_RETRIES {
                                logging::warn(
                                    "Context-limit compaction retry limit reached; giving up",
                                );
                                return Err(anyhow::anyhow!(
                                    "Context limit exceeded after {} compaction retries",
                                    Self::MAX_CONTEXT_LIMIT_RETRIES
                                ));
                            }
                            retry_after_compaction = true;
                            break;
                        }
                        return Err(StreamError::new(message, retry_after_secs).into());
                    }
                }
            }

            if retry_after_compaction {
                continue;
            }

            let api_elapsed = api_start.elapsed();
            logging::info(&format!(
                "API call complete in {:.2}s (input={} output={} cache_read={} cache_write={})",
                api_elapsed.as_secs_f64(),
                usage_input.unwrap_or(0),
                usage_output.unwrap_or(0),
                usage_cache_read.unwrap_or(0),
                usage_cache_creation.unwrap_or(0),
            ));

            if usage_input.is_some()
                || usage_output.is_some()
                || usage_cache_read.is_some()
                || usage_cache_creation.is_some()
            {
                crate::telemetry::record_token_usage(
                    usage_input.unwrap_or(0),
                    usage_output.unwrap_or(0),
                    usage_cache_read,
                    usage_cache_creation,
                );
            }

            if print_output
                && (usage_input.is_some()
                    || usage_output.is_some()
                    || usage_cache_read.is_some()
                    || usage_cache_creation.is_some())
            {
                let input = usage_input.unwrap_or(0);
                let output = usage_output.unwrap_or(0);
                let cache_read = usage_cache_read.unwrap_or(0);
                let cache_creation = usage_cache_creation.unwrap_or(0);
                let cache_str = if usage_cache_read.is_some() || usage_cache_creation.is_some() {
                    format!(
                        " cache_read: {} cache_write: {}",
                        cache_read, cache_creation
                    )
                } else {
                    String::new()
                };
                print!(
                    "\n[Tokens] upload: {} download: {}{}\n",
                    input, output, cache_str
                );
                io::stdout().flush()?;
            }

            // Store usage for debug queries
            self.last_usage = TokenUsage {
                input_tokens: usage_input.unwrap_or(0),
                output_tokens: usage_output.unwrap_or(0),
                cache_read_input_tokens: usage_cache_read,
                cache_creation_input_tokens: usage_cache_creation,
            };

            self.recover_text_wrapped_tool_call(&mut text_content, &mut tool_calls);

            // Add assistant message to history
            let mut content_blocks = Vec::new();
            if !text_content.is_empty() {
                content_blocks.push(ContentBlock::Text {
                    text: text_content.clone(),
                    cache_control: None,
                });
            }
            if store_reasoning_content && !reasoning_content.is_empty() {
                content_blocks.push(ContentBlock::Reasoning {
                    text: reasoning_content.clone(),
                });
            }
            for tc in &tool_calls {
                content_blocks.push(ContentBlock::ToolUse {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input: tc.input.clone(),
                });
            }

            let assistant_message_id = if !content_blocks.is_empty() {
                crate::telemetry::record_assistant_response();
                let token_usage = Some(crate::session::StoredTokenUsage {
                    input_tokens: self.last_usage.input_tokens,
                    output_tokens: self.last_usage.output_tokens,
                    cache_read_input_tokens: self.last_usage.cache_read_input_tokens,
                    cache_creation_input_tokens: self.last_usage.cache_creation_input_tokens,
                });
                let message_id =
                    self.add_message_ext(Role::Assistant, content_blocks, None, token_usage);
                self.push_embedding_snapshot_if_semantic(&text_content);
                crate::local_model::record_api_exchange_async(
                    send_messages,
                    &text_content,
                    self.provider.name(),
                    &self.provider.model(),
                );
                self.session.save()?;
                Some(message_id)
            } else {
                None
            };

            if let Some((encrypted_content, compacted_count)) = openai_native_compaction.take() {
                self.apply_openai_native_compaction(encrypted_content, compacted_count)?;
            }

            // If stop_reason indicates truncation (e.g. max_tokens), discard tool calls
            // with null/empty inputs since they were likely truncated mid-generation.
            // This prevents executing broken tool calls and instead requests a continuation.
            self.filter_truncated_tool_calls(
                stop_reason.as_deref(),
                &mut tool_calls,
                assistant_message_id.as_ref(),
            );

            // If no tool calls, we're done
            if tool_calls.is_empty() {
                if let Some(rehydrated) =
                    crate::interlang::maybe_rehydrate_context_search(&text_content)
                {
                    logging::info("Context vault .ctx_search fulfilled; retrying turn");
                    self.add_message(
                        Role::User,
                        vec![ContentBlock::Text {
                            text: rehydrated,
                            cache_control: None,
                        }],
                    );
                    self.session.save()?;
                    continue;
                }

                if let Some(rehydrated) = crate::interlang::maybe_rehydrate_response(&text_content)
                {
                    logging::info("Interlang exact context request fulfilled; retrying turn");
                    self.add_message(
                        Role::User,
                        vec![ContentBlock::Text {
                            text: rehydrated,
                            cache_control: None,
                        }],
                    );
                    self.session.save()?;
                    continue;
                }
                // Phase 3 — `.mem_get` rehydration. When the model emits
                // `.mem_get reason=<why>` and we previously injected only an
                // anchor, the full memory prompt was stashed by name; replay
                // it as a system-reminder so the next turn has authoritative
                // memory text without burning tokens on every prior turn.
                if let Some(rehydrated) =
                    crate::memory::maybe_rehydrate_mem_get(&self.session.id, &text_content)
                {
                    logging::info("Memory anchor .mem_get fulfilled; retrying turn");
                    self.add_message(
                        Role::User,
                        vec![ContentBlock::Text {
                            text: rehydrated,
                            cache_control: None,
                        }],
                    );
                    self.session.save()?;
                    continue;
                }
                {
                    let skills_registry = self.registry.skills();
                    let skills = skills_registry.read().await;
                    if let Some(rehydrated) =
                        crate::skill::maybe_rehydrate_skill_get(&skills, &text_content)
                    {
                        logging::info("Skill anchor .skill_get fulfilled; retrying turn");
                        self.add_message(
                            Role::User,
                            vec![ContentBlock::Text {
                                text: rehydrated,
                                cache_control: None,
                            }],
                        );
                        self.session.save()?;
                        continue;
                    }
                }
                if self.maybe_continue_incomplete_response(
                    stop_reason.as_deref(),
                    &mut incomplete_continuations,
                )? {
                    continue;
                }
                logging::info("Turn complete - no tool calls, returning");
                if print_output {
                    println!();
                }
                final_text = text_content;
                break;
            }

            logging::info(&format!(
                "Turn has {} tool calls to execute",
                tool_calls.len()
            ));

            // If provider handles tools internally (like Claude Code CLI), only run native tools locally
            if self.provider.handles_tools_internally() {
                tool_calls.retain(|tc| NEURA_NATIVE_TOOLS.contains(&tc.name.as_str()));
                if tool_calls.is_empty() {
                    logging::info("Provider handles tools internally - task complete");
                    break;
                }
                logging::info("Provider handles tools internally - executing native tools locally");
            }

            // Execute tools and add results
            let mut tool_results_dirty = false;
            for tc in tool_calls {
                self.validate_tool_allowed(&tc.name)?;

                let message_id = assistant_message_id
                    .clone()
                    .unwrap_or_else(|| self.session.id.clone());

                let is_native_tool = NEURA_NATIVE_TOOLS.contains(&tc.name.as_str());

                // Check if SDK already executed this tool
                if let Some((sdk_content, sdk_is_error)) = sdk_tool_results.remove(&tc.id) {
                    // For native tools, ignore SDK errors and execute locally
                    if is_native_tool && sdk_is_error {
                        if trace {
                            eprintln!(
                                "[trace] sdk_error_for_native_tool name={} id={}, executing locally",
                                tc.name, tc.id
                            );
                        }
                        // Fall through to local execution below
                    } else {
                        if trace {
                            eprintln!(
                                "[trace] using_sdk_result name={} id={} is_error={}",
                                tc.name, tc.id, sdk_is_error
                            );
                        }
                        if print_output {
                            print!("\n  → ");
                            let preview = if sdk_content.len() > 200 {
                                format!("{}...", crate::util::truncate_str(&sdk_content, 200))
                            } else {
                                sdk_content.clone()
                            };
                            println!("{}", preview.lines().next().unwrap_or("(done via SDK)"));
                        }

                        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                            session_id: self.session.id.clone(),
                            message_id: message_id.clone(),
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            status: if sdk_is_error {
                                ToolStatus::Error
                            } else {
                                ToolStatus::Completed
                            },
                            title: None,
                        }));

                        self.add_message(
                            Role::User,
                            vec![ContentBlock::ToolResult {
                                tool_use_id: tc.id,
                                content: sdk_content,
                                is_error: if sdk_is_error { Some(true) } else { None },
                            }],
                        );
                        tool_results_dirty = true;
                        continue;
                    }
                }

                // SDK didn't execute this tool, run it locally
                if print_output {
                    print!("\n  → ");
                    io::stdout().flush()?;
                }

                let ctx = ToolContext {
                    session_id: self.session.id.clone(),
                    message_id: message_id.clone(),
                    tool_call_id: tc.id.clone(),
                    working_dir: self.working_dir().map(PathBuf::from),
                    stdin_request_tx: self.stdin_request_tx.clone(),
                    graceful_shutdown_signal: Some(self.graceful_shutdown.clone()),
                    execution_mode: ToolExecutionMode::AgentTurn,
                };

                if trace {
                    eprintln!("[trace] tool_exec_start name={} id={}", tc.name, tc.id);
                }
                Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                    session_id: self.session.id.clone(),
                    message_id: message_id.clone(),
                    tool_call_id: tc.id.clone(),
                    tool_name: tc.name.clone(),
                    status: ToolStatus::Running,
                    title: None,
                }));

                logging::info(&format!("Tool starting: {}", tc.name));
                let tool_start = Instant::now();

                // Publish status for TUI to show during Task execution
                Bus::global().publish(BusEvent::SubagentStatus(SubagentStatus {
                    session_id: self.session.id.clone(),
                    status: format!("running {}", tc.name),
                    model: Some(self.provider.model()),
                }));

                let result = self.registry.execute(&tc.name, tc.input.clone(), ctx).await;
                crate::telemetry::record_tool_call();
                // Workspace edits are architectural observations: queue them
                // as knowledge evidence (folded in at the next sleep/sync).
                crate::knowledge::evidence::note_tool_outcome(&tc.name, &tc.input, result.is_ok());
                self.unlock_tools_if_needed(&tc.name);
                let tool_elapsed = tool_start.elapsed();
                logging::info(&format!(
                    "Tool finished: {} in {:.2}s",
                    tc.name,
                    tool_elapsed.as_secs_f64()
                ));

                match result {
                    Ok(output) => {
                        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                            session_id: self.session.id.clone(),
                            message_id: message_id.clone(),
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            status: ToolStatus::Completed,
                            title: output.title.clone(),
                        }));

                        if trace {
                            eprintln!(
                                "[trace] tool_exec_done name={} id={}\n{}",
                                tc.name, tc.id, output.output
                            );
                        }
                        if print_output {
                            let preview = if output.output.len() > 200 {
                                format!("{}...", crate::util::truncate_str(&output.output, 200))
                            } else {
                                output.output.clone()
                            };
                            println!("{}", preview.lines().next().unwrap_or("(done)"));
                        }

                        let blocks = tool_output_to_content_blocks(tc.id, output);
                        self.add_message_with_duration(
                            Role::User,
                            blocks,
                            Some(tool_elapsed.as_millis() as u64),
                        );
                        tool_results_dirty = true;
                    }
                    Err(e) => {
                        crate::telemetry::record_tool_failure();
                        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                            session_id: self.session.id.clone(),
                            message_id: message_id.clone(),
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            status: ToolStatus::Error,
                            title: None,
                        }));

                        let error_msg = format!("Error: {}", e);
                        if trace {
                            eprintln!(
                                "[trace] tool_exec_error name={} id={} {}",
                                tc.name, tc.id, error_msg
                            );
                        }
                        if print_output {
                            println!("{}", error_msg);
                        }
                        self.add_message_with_duration(
                            Role::User,
                            vec![ContentBlock::ToolResult {
                                tool_use_id: tc.id,
                                content: error_msg,
                                is_error: Some(true),
                            }],
                            Some(tool_elapsed.as_millis() as u64),
                        );
                        tool_results_dirty = true;
                    }
                }
            }

            if tool_results_dirty {
                self.session.save()?;
            }

            if print_output {
                println!();
            }

            // Check for soft interrupts (e.g. Telegram messages) and inject them for the next turn
            let injected = self.inject_soft_interrupts();
            if !injected.is_empty() {
                let total_chars: usize = injected.iter().map(|item| item.content.len()).sum();
                logging::info(&format!(
                    "Soft interrupt injected into headless turn ({} message(s), {} chars)",
                    injected.len(),
                    total_chars
                ));
            }
        }

        Ok(final_text)
    }
}

fn should_apply_final_prompt_admission(messages: &[Message]) -> bool {
    let Some(latest) = latest_user_text_for_payload_diet(messages) else {
        return false;
    };
    let lower = latest.to_ascii_lowercase();
    let simple = latest.len() <= 120
        && !latest.contains('\n')
        && !latest.contains("```")
        && !contains_any_for_payload_diet(
            &lower,
            &[
                "fix", "debug", "build", "test", "error", "failed", "repo", "code", "file", "src/",
                "docs/", ".rs", ".py", ".md", "continue", "previous", "earlier", "above", "that",
                "those", "memory", "token", "context", "why", "how many",
            ],
        );
    aggregate_message_chars_for_payload_diet(messages)
        > final_prompt_message_budget(&latest, simple)
}

/// Same gate as `should_apply_final_prompt_admission` but skips the per-block
/// re-walk: callers that already computed the total via `walk_messages` can
/// reuse it here. Only the latest user text is read to choose the budget.
pub(crate) fn final_prompt_message_budget_for_admission(messages: &[Message]) -> usize {
    let Some(latest) = latest_user_text_for_payload_diet(messages) else {
        return usize::MAX; // no user text → never trip the gate
    };
    let lower = latest.to_ascii_lowercase();
    let simple = latest.len() <= 120
        && !latest.contains('\n')
        && !latest.contains("```")
        && !contains_any_for_payload_diet(
            &lower,
            &[
                "fix", "debug", "build", "test", "error", "failed", "repo", "code", "file", "src/",
                "docs/", ".rs", ".py", ".md", "continue", "previous", "earlier", "above", "that",
                "those", "memory", "token", "context", "why", "how many",
            ],
        );
    final_prompt_message_budget(&latest, simple)
}

fn final_prompt_message_budget(latest: &str, simple: bool) -> usize {
    let lower = latest.to_ascii_lowercase();
    if simple {
        return 4_000;
    }
    if contains_any_for_payload_diet(
        &lower,
        &[
            "fix", "debug", "build", "test", "error", "failed", "repo", "code", "file", "src/",
            "docs/", ".rs", ".py", ".md", "continue", "previous", "earlier", "above",
        ],
    ) {
        48_000
    } else {
        12_000
    }
}

fn compact_provider_messages_for_short_turn(messages: &[Message]) -> Vec<Message> {
    let latest_user_idx = messages
        .iter()
        .rposition(|message| message.role == Role::User);
    let mut out = Vec::with_capacity(messages.len().min(8));
    let keep_from = messages.len().saturating_sub(6);
    for (idx, message) in messages.iter().enumerate() {
        if idx < keep_from && Some(idx) != latest_user_idx {
            continue;
        }
        let mut msg = message.clone();
        let preserve_exact = Some(idx) == latest_user_idx;
        if !preserve_exact {
            for block in &mut msg.content {
                match block {
                    ContentBlock::Text { text, .. }
                        if text.len() > 700 || text_contains_auto_exact(text) =>
                    {
                        *text = summarize_provider_block_for_payload_diet(text, "text");
                    }
                    ContentBlock::ToolResult { content, .. }
                        if content.len() > 500 || text_contains_auto_exact(content) =>
                    {
                        *content =
                            summarize_provider_block_for_payload_diet(content, "tool_result");
                    }
                    ContentBlock::Reasoning { text }
                        if text.len() > 500 || text_contains_auto_exact(text) =>
                    {
                        *text = summarize_provider_block_for_payload_diet(text, "reasoning");
                    }
                    ContentBlock::ToolUse { input, .. } if input.to_string().len() > 500 => {
                        *input = serde_json::json!({
                            "summary": "large tool input omitted for short direct turn"
                        });
                    }
                    _ => {}
                }
            }
        }
        out.push(msg);
    }
    out
}

fn messages_contain_auto_exact(messages: &[Message]) -> bool {
    messages.iter().any(|message| {
        message.content.iter().any(|block| match block {
            ContentBlock::Text { text, .. } => text_contains_auto_exact(text),
            ContentBlock::ToolResult { content, .. } => text_contains_auto_exact(content),
            ContentBlock::Reasoning { text } => text_contains_auto_exact(text),
            ContentBlock::ToolUse { input, .. } => text_contains_auto_exact(&input.to_string()),
            ContentBlock::Image { data, .. } => text_contains_auto_exact(data),
            ContentBlock::OpenAICompaction { encrypted_content } => {
                text_contains_auto_exact(encrypted_content)
            }
        })
    })
}

fn text_contains_auto_exact(text: &str) -> bool {
    text.contains("<ctx_auto_exact")
        || text.contains("Neura auto-restored one relevant exact excerpt")
}

fn summarize_provider_block_for_payload_diet(text: &str, kind: &str) -> String {
    let first = text.lines().next().unwrap_or_default().trim();
    format!(
        "<summary kind=\"{}\" lines=\"{}\" chars=\"{}\" first=\"{}\" />",
        kind,
        text.lines().count(),
        text.len(),
        escape_summary_attr_for_payload_diet(&crate::util::truncate_str(first, 160))
    )
}

fn escape_summary_attr_for_payload_diet(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn aggregate_message_chars_for_payload_diet(messages: &[Message]) -> usize {
    messages
        .iter()
        .flat_map(|message| message.content.iter())
        .map(|block| match block {
            ContentBlock::Text { text, .. } => text.len(),
            ContentBlock::Reasoning { text } => text.len(),
            ContentBlock::ToolResult { content, .. } => content.len(),
            ContentBlock::ToolUse { name, input, .. } => name.len() + input.to_string().len(),
            ContentBlock::Image { data, .. } => data.len(),
            ContentBlock::OpenAICompaction { encrypted_content } => encrypted_content.len(),
        })
        .sum()
}

fn latest_user_text_for_payload_diet(messages: &[Message]) -> Option<String> {
    messages.iter().rev().find_map(|message| {
        if message.role != Role::User {
            return None;
        }
        let text = message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        (!text.trim().is_empty()).then_some(text)
    })
}

fn contains_any_for_payload_diet(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[cfg(test)]
mod short_turn_payload_diet_tests {
    use super::*;
    use crate::runtime_ledger::{self, RuntimeReceiptKind};

    #[test]
    fn simple_turn_with_large_history_gets_compacted_provider_payload() {
        let mut messages = Vec::new();
        for idx in 0..12 {
            messages.push(Message::user(&format!(
                "old verbose context {idx}\n{}",
                "alpha beta gamma delta epsilon\n".repeat(250)
            )));
        }
        messages.push(Message::user("meow"));

        assert!(should_apply_final_prompt_admission(&messages));
        let compacted = compact_provider_messages_for_short_turn(&messages);
        assert!(compacted.len() <= 6);
        assert_eq!(
            latest_user_text_for_payload_diet(&compacted).as_deref(),
            Some("meow")
        );
        assert!(aggregate_message_chars_for_payload_diet(&compacted) < 8_000);
    }

    #[test]
    fn coding_turn_does_not_get_short_turn_payload_diet() {
        let messages = vec![
            Message::user(&"old diagnostic context\n".repeat(900)),
            Message::user("fix the failing build in src/provider/openai.rs"),
        ];
        assert!(!should_apply_final_prompt_admission(&messages));
    }
}

#[cfg(test)]
mod admission_controller_tests {
    use super::*;
    use crate::runtime_ledger::{self, RuntimeReceiptKind};

    #[test]
    fn meow_is_direct_and_skips_context_subsystems() {
        let messages = vec![Message::user("say meow")];
        let admission = classify_turn_admission(&messages);
        assert_eq!(admission, TurnAdmission::Direct);
        assert!(!admission.use_memory());
        assert!(!admission.use_interlang());
        assert!(!admission.use_sidecar());
    }

    #[test]
    fn coding_turn_is_deep() {
        let messages = vec![Message::user(
            "fix the failing build in src/provider/mod.rs and run tests",
        )];
        let admission = classify_turn_admission(&messages);
        assert_eq!(admission, TurnAdmission::Deep);
        assert!(admission.use_memory());
        assert!(admission.use_interlang());
        assert!(admission.use_sidecar());
    }

    #[test]
    fn normal_chat_is_light() {
        let messages = vec![Message::user(
            "what are good ways to organize a small rust project?",
        )];
        let admission = classify_turn_admission(&messages);
        assert_eq!(admission, TurnAdmission::Light);
        assert!(admission.use_memory());
        assert!(admission.use_interlang());
        assert!(!admission.use_sidecar());
    }
}

#[cfg(test)]
mod walk_tests {
    use super::*;
    use crate::runtime_ledger::{self, RuntimeReceiptKind};

    #[test]
    fn walk_partitions_chars_by_kind() {
        let messages = vec![
            Message::user("hello world"),
            Message::tool_result(
                "call-1",
                "tool result body that's a bit longer than the user message",
                false,
            ),
            Message::assistant_text("brief reply"),
        ];
        let walk = walk_messages(&messages);
        assert_eq!(walk.text_chars, "hello world".len() + "brief reply".len());
        assert_eq!(
            walk.tool_result_chars,
            "tool result body that's a bit longer than the user message".len()
        );
        assert_eq!(walk.tool_use_chars, 0);
        assert_eq!(walk.contains_auto_exact, false);
        assert_eq!(walk.interlang_refs_chars, 0);
        assert!(!walk.top_blocks.is_empty());
        // Top block should be the tool_result (largest).
        assert_eq!(walk.top_blocks[0].kind, "tool_result");
    }

    #[test]
    fn walk_detects_auto_exact_marker() {
        let messages = vec![Message::user(
            "<system-reminder>\n<ctx_auto_exact id=\"ctx:abc\" />\n</system-reminder>",
        )];
        let walk = walk_messages(&messages);
        assert!(walk.contains_auto_exact);
    }

    #[test]
    fn walk_counts_interlang_refs() {
        let messages = vec![
            Message::user("just text"),
            Message::tool_result("c1", "<ctx k=\"old-tool-result\" id=\"ctx:abc\" />", false),
        ];
        let walk = walk_messages(&messages);
        assert_eq!(walk.interlang_refs_blocks, 1);
        assert!(walk.interlang_refs_chars > 0);
    }

    #[test]
    fn walk_top_blocks_keeps_three_largest() {
        let mut messages = Vec::new();
        for size in [10usize, 50, 100, 200, 30, 75] {
            messages.push(Message::user(&"x".repeat(size)));
        }
        let walk = walk_messages(&messages);
        assert_eq!(walk.top_blocks.len(), 3);
        // Sorted descending by chars
        assert!(walk.top_blocks[0].chars >= walk.top_blocks[1].chars);
        assert!(walk.top_blocks[1].chars >= walk.top_blocks[2].chars);
        assert_eq!(walk.top_blocks[0].chars, 200);
    }

    #[test]
    fn latest_is_tool_result_only_detects_continuation() {
        let mut messages = vec![Message::user("fix the bug in src/foo.rs")];
        // Assistant emits a tool_use; user then sends only a tool_result.
        messages.push(Message::assistant_text("running grep"));
        messages.push(Message::tool_result("call-1", "match found", false));
        assert!(latest_is_tool_result_only(&messages));

        // Followed by a fresh user text → no longer a continuation.
        messages.push(Message::user("now also fix bar.rs"));
        assert!(!latest_is_tool_result_only(&messages));
    }

    #[test]
    fn continuation_admission_inherits_subsystem_use() {
        let cont = TurnAdmission::Continuation;
        assert!(cont.use_memory());
        assert!(cont.use_interlang());
        assert!(cont.use_sidecar());
    }
}
