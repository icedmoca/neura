use super::*;
use crate::runtime_ledger::{self, RuntimeReceiptKind};

impl Agent {
    /// Run a single turn with the given user message
    pub async fn run_once(&mut self, user_message: &str) -> Result<()> {
        crate::live_operational_fabric::emit_user_message("agent.run_once", user_message);
        crate::live_operational_fabric::emit_memory_bridge("agent.run_once", user_message.len());
        let _ = crate::directive_memory::ingest_text("agent.run_once", user_message);
        self.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: user_message.to_string(),
                cache_control: None,
            }],
        );
        self.session.save()?;
        if trace_enabled() {
            eprintln!("[trace] session_id {}", self.session.id);
        }
        let _ = self.run_turn(true).await?;
        Ok(())
    }

    pub async fn run_once_capture(&mut self, user_message: &str) -> Result<String> {
        crate::live_operational_fabric::emit_user_message("agent.run_once_capture", user_message);
        crate::live_operational_fabric::emit_memory_bridge(
            "agent.run_once_capture",
            user_message.len(),
        );
        let _ = crate::directive_memory::ingest_text("agent.run_once_capture", user_message);
        self.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: user_message.to_string(),
                cache_control: None,
            }],
        );
        self.session.save()?;
        if trace_enabled() {
            eprintln!("[trace] session_id {}", self.session.id);
        }
        self.run_turn(false).await
    }

    /// Run a single message with events streamed to a broadcast channel (for server mode)
    pub async fn run_once_streaming(
        &mut self,
        user_message: &str,
        event_tx: broadcast::Sender<ServerEvent>,
    ) -> Result<()> {
        crate::live_operational_fabric::emit_user_message("agent.streaming", user_message);
        crate::live_operational_fabric::emit_memory_bridge("agent.streaming", user_message.len());
        let _ = crate::directive_memory::ingest_text("agent.streaming", user_message);
        // Inject any pending notifications before the user message
        let alerts = self.take_alerts();
        if !alerts.is_empty() {
            let alert_text = format!(
                "[NOTIFICATION]\nYou received {} notification(s) from other agents working in this codebase:\n\n{}\n\nUse the communicate tool (actions: list, read, message/broadcast, dm, channel, share) to coordinate with other agents.",
                alerts.len(),
                alerts.join("\n\n---\n\n")
            );
            self.add_message(
                Role::User,
                vec![ContentBlock::Text {
                    text: alert_text,
                    cache_control: None,
                }],
            );
        }

        self.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: user_message.to_string(),
                cache_control: None,
            }],
        );
        self.session.save()?;
        self.run_turn_streaming(event_tx).await
    }

    /// Run one conversation turn with streaming events via mpsc channel (per-client)
    pub async fn run_once_streaming_mpsc(
        &mut self,
        user_message: &str,
        images: Vec<(String, String)>,
        system_reminder: Option<String>,
        event_tx: mpsc::UnboundedSender<ServerEvent>,
    ) -> Result<()> {
        crate::live_operational_fabric::emit_user_message("agent.streaming_mpsc", user_message);
        crate::live_operational_fabric::emit_memory_bridge(
            "agent.streaming_mpsc",
            user_message.len(),
        );
        let _ = crate::directive_memory::ingest_text("agent.streaming", user_message);
        // Inject any pending notifications before the user message
        let alerts = self.take_alerts();
        if !alerts.is_empty() {
            let alert_text = format!(
                "[NOTIFICATION]\nYou received {} notification(s) from other agents working in this codebase:\n\n{}\n\nUse the communicate tool (actions: list, read, message/broadcast, dm, channel, share) to coordinate with other agents.",
                alerts.len(),
                alerts.join("\n\n---\n\n")
            );
            self.add_message(
                Role::User,
                vec![ContentBlock::Text {
                    text: alert_text,
                    cache_control: None,
                }],
            );
        }

        self.current_turn_system_reminder =
            system_reminder.filter(|value| !value.trim().is_empty());

        let mut blocks: Vec<ContentBlock> = images
            .into_iter()
            .map(|(media_type, data)| ContentBlock::Image { media_type, data })
            .collect();
        blocks.push(ContentBlock::Text {
            text: user_message.to_string(),
            cache_control: None,
        });

        if blocks.len() > 1 {
            crate::logging::info(&format!(
                "Agent received message with {} image(s)",
                blocks.len() - 1
            ));
        }

        self.add_message(Role::User, blocks);
        let subtext_messages: Vec<_> = self
            .session
            .messages
            .iter()
            .map(crate::session::StoredMessage::to_message)
            .collect();
        crate::agent::subtext_observer::spawn_subtext_observer_for_turn(
            self.session.id.clone(),
            &subtext_messages,
            Some(event_tx.clone()),
        );
        crate::telemetry::record_turn();
        self.session.save()?;
        let result = self.run_turn_streaming_mpsc(event_tx).await;
        self.current_turn_system_reminder = None;
        result
    }

    /// Clear conversation history
    pub fn clear(&mut self) {
        let preserve_canary = self.session.is_canary;
        let preserve_testing_build = self.session.testing_build.clone();
        let preserve_debug = self.session.is_debug;
        let preserve_working_dir = self.session.working_dir.clone();

        self.session.mark_closed();
        self.persist_session_best_effort("pre-clear session close state");

        let mut new_session = Session::create(None, None);
        new_session.mark_active();
        new_session.model = Some(self.provider.model());
        new_session.is_canary = preserve_canary;
        new_session.testing_build = preserve_testing_build;
        new_session.is_debug = preserve_debug;
        new_session.working_dir = preserve_working_dir;

        self.session = new_session;
        self.reset_runtime_state_for_session_change();
        self.provider_session_id = None;
        self.seed_compaction_from_session();
    }

    /// Clear provider session so the next turn sends full context.
    pub fn reset_provider_session(&mut self) {
        self.provider_session_id = None;
        self.session.provider_session_id = None;
        self.persist_session_best_effort("provider session reset");
    }

    /// Unlock the tool list so the next API request picks up any new tools.
    /// Called after MCP reload or when the user explicitly wants new tools.
    pub fn unlock_tools(&mut self) {
        if self.locked_tools.is_some() {
            logging::info("Tool list unlocked — next request will pick up current tools");
            self.locked_tools = None;
            self.locked_tools_chars = None;
            self.locked_tools_token_estimate = None;
            self.cache_tracker.reset();
        }
    }

    /// Unlock tools if a tool execution may have changed the registry
    /// (e.g., mcp connect/disconnect/reload)
    pub(super) fn unlock_tools_if_needed(&mut self, tool_name: &str) {
        if tool_name == "mcp" {
            self.unlock_tools();
        }
    }

    pub fn is_canary(&self) -> bool {
        self.session.is_canary
    }

    pub fn is_debug(&self) -> bool {
        self.session.is_debug
    }

    pub fn set_canary(&mut self, build_hash: &str) {
        self.session.set_canary(build_hash);
        if let Err(err) = self.session.save() {
            logging::error(&format!("Failed to persist canary session state: {}", err));
        }
    }

    /// Mark this session as a debug/test session
    /// Set a custom system prompt override (used by ambient mode).
    /// When set, this replaces the normal system prompt entirely.
    pub fn set_system_prompt(&mut self, prompt: &str) {
        self.system_prompt_override = Some(prompt.to_string());
    }

    pub fn set_debug(&mut self, is_debug: bool) {
        self.session.set_debug(is_debug);
        if let Err(err) = self.session.save() {
            logging::error(&format!("Failed to persist debug session state: {}", err));
        }
    }

    /// Enable or disable memory features for this session.
    pub fn set_memory_enabled(&mut self, enabled: bool) {
        self.memory_enabled = enabled;
        if !enabled {
            crate::memory::clear_pending_memory(&self.session.id);
        }
    }

    /// Check whether memory features are enabled for this session.
    pub fn memory_enabled(&self) -> bool {
        self.memory_enabled
    }

    /// Set the stdin request channel for interactive stdin forwarding
    pub fn set_stdin_request_tx(
        &mut self,
        tx: tokio::sync::mpsc::UnboundedSender<crate::tool::StdinInputRequest>,
    ) {
        self.stdin_request_tx = Some(tx);
    }

    pub(super) async fn tool_definitions(&mut self) -> Vec<ToolDefinition> {
        if self.session.is_canary {
            self.registry.register_selfdev_tools().await;
        }

        // Return locked tools if available (prevents cache invalidation from
        // MCP tools arriving asynchronously after the first API request)
        if let Some(ref locked) = self.locked_tools {
            return locked.clone();
        }

        let mut tools = self.registry.definitions(self.allowed_tools.as_ref()).await;
        if !self.session.is_canary {
            tools.retain(|tool| tool.name != "selfdev");
        }
        tools = filter_tool_definitions_for_messages(tools, &self.session.messages);

        // Lock the tool list on first call to prevent cache invalidation
        // when MCP tools arrive asynchronously mid-session
        logging::info(&format!(
            "Locking tool list at {} tools for cache stability",
            tools.len()
        ));
        // Cache aggregate prompt-chars + token estimate alongside the lock
        // so per-turn telemetry / prefix accounting can read these without
        // re-serializing the tool list. Single computation on lock; cleared
        // alongside the lock on unlock.
        let chars = ToolDefinition::aggregate_prompt_chars(&tools);
        let tokens = ToolDefinition::aggregate_prompt_token_estimate(&tools);
        self.locked_tools_chars = Some(chars);
        self.locked_tools_token_estimate = Some(tokens);
        self.locked_tools = Some(tools.clone());
        tools
    }

    /// Cached aggregate char count of the locked tool list (or `None` if not
    /// yet locked or already cleared). Consumers should fall back to
    /// `ToolDefinition::aggregate_prompt_chars(tools)` when this returns None.
    pub(crate) fn locked_tools_chars(&self) -> Option<usize> {
        self.locked_tools_chars
    }

    /// Cached aggregate prompt-token estimate of the locked tool list.
    pub(crate) fn locked_tools_token_estimate(&self) -> Option<usize> {
        self.locked_tools_token_estimate
    }

    pub async fn tool_names(&self) -> Vec<String> {
        self.registry.tool_names().await
    }

    /// Get full tool definitions for debug introspection (bypasses lock)
    pub async fn tool_definitions_for_debug(&self) -> Vec<crate::message::ToolDefinition> {
        if self.session.is_canary {
            self.registry.register_selfdev_tools().await;
        }
        let mut tools = self.registry.definitions(self.allowed_tools.as_ref()).await;
        if !self.session.is_canary {
            tools.retain(|tool| tool.name != "selfdev");
        }
        tools
    }

    pub async fn execute_tool(
        &self,
        name: &str,
        input: serde_json::Value,
    ) -> Result<crate::tool::ToolOutput> {
        self.validate_tool_allowed(name)?;

        let call_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| format!("debug-{}", d.as_millis()))
            .unwrap_or_else(|_| "debug".to_string());
        let ctx = ToolContext {
            session_id: self.session.id.clone(),
            message_id: self.session.id.clone(),
            tool_call_id: call_id,
            working_dir: self.working_dir().map(PathBuf::from),
            stdin_request_tx: self.stdin_request_tx.clone(),
            graceful_shutdown_signal: Some(self.graceful_shutdown.clone()),
            execution_mode: ToolExecutionMode::Direct,
        };
        if runtime_ledger::enabled() {
            runtime_ledger::append_receipt_best_effort(
                RuntimeReceiptKind::ToolCall,
                "start",
                serde_json::json!({
                    "tool": name,
                    "session_id": self.session.id,
                }),
            );
        }
        let result = self.registry.execute(name, input, ctx).await;
        if runtime_ledger::enabled() {
            runtime_ledger::append_receipt_best_effort(
                RuntimeReceiptKind::ToolCall,
                "finish",
                serde_json::json!({
                    "tool": name,
                    "ok": result.is_ok(),
                    "session_id": self.session.id,
                }),
            );
        }
        result
    }

    pub fn add_manual_tool_use(
        &mut self,
        tool_call_id: String,
        tool_name: String,
        input: serde_json::Value,
    ) -> Result<String> {
        let message_id = self.add_message(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: tool_call_id,
                name: tool_name,
                input,
            }],
        );
        self.session.save()?;
        Ok(message_id)
    }

    pub fn add_manual_tool_result(
        &mut self,
        tool_call_id: String,
        output: crate::tool::ToolOutput,
        duration_ms: u64,
    ) -> Result<()> {
        let blocks = tool_output_to_content_blocks(tool_call_id, output);
        self.add_message_with_duration(Role::User, blocks, Some(duration_ms));
        self.session.save()?;
        Ok(())
    }

    pub fn add_manual_tool_error(
        &mut self,
        tool_call_id: String,
        error: String,
        duration_ms: u64,
    ) -> Result<()> {
        self.add_message_with_duration(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: tool_call_id,
                content: error,
                is_error: Some(true),
            }],
            Some(duration_ms),
        );
        self.session.save()?;
        Ok(())
    }

    pub(super) fn validate_tool_allowed(&self, name: &str) -> Result<()> {
        if let Some(allowed) = self.allowed_tools.as_ref()
            && !allowed.contains(name)
        {
            return Err(anyhow::anyhow!("Tool '{}' is not allowed", name));
        }
        Ok(())
    }

    /// Restore a session by ID (loads from disk)
    pub fn restore_session(&mut self, session_id: &str) -> Result<SessionStatus> {
        let restore_start = Instant::now();
        let load_start = Instant::now();
        let session = Session::load(session_id)?;
        let load_ms = load_start.elapsed().as_millis();
        logging::info(&format!(
            "Restoring session '{}' with {} messages, provider_session_id: {:?}, status: {}",
            session_id,
            session.messages.len(),
            session.provider_session_id,
            session.status.display()
        ));
        let previous_status = session.status.clone();

        let assign_start = Instant::now();
        // Restore provider_session_id for Claude CLI session resume
        self.provider_session_id = session.provider_session_id.clone();
        self.session = session;
        let assign_ms = assign_start.elapsed().as_millis();

        let reset_start = Instant::now();
        self.reset_runtime_state_for_session_change();
        let restored_soft_interrupts = self.restore_persisted_soft_interrupts();
        let reset_ms = reset_start.elapsed().as_millis();

        let model_start = Instant::now();
        if let Some(model) = self.session.model.clone() {
            if let Err(e) = self.provider.set_model(&model) {
                logging::error(&format!(
                    "Failed to restore session model '{}': {}",
                    model, e
                ));
            }
        } else {
            self.session.model = Some(self.provider.model());
        }
        let model_ms = model_start.elapsed().as_millis();

        let mark_active_start = Instant::now();
        self.session.mark_active();
        let mark_active_ms = mark_active_start.elapsed().as_millis();
        self.sync_memory_dedup_state_from_session();

        logging::info(&format!(
            "restore_session: loaded session {} with {} messages, calling seed_compaction",
            session_id,
            self.session.messages.len()
        ));
        let compaction_start = Instant::now();
        self.seed_compaction_from_session();
        let compaction_ms = compaction_start.elapsed().as_millis();

        let env_snapshot_start = Instant::now();
        self.log_env_snapshot("resume");
        let env_snapshot_ms = env_snapshot_start.elapsed().as_millis();

        let save_start = Instant::now();
        if let Err(err) = self.session.save() {
            logging::error(&format!(
                "Failed to persist resumed session state for {}: {}",
                session_id, err
            ));
        }
        let save_ms = save_start.elapsed().as_millis();

        logging::info(&format!(
            "[TIMING] restore_session: session={}, messages={}, restored_soft_interrupts={}, load={}ms, assign={}ms, reset={}ms, model={}ms, mark_active={}ms, compaction={}ms, env_snapshot={}ms, save={}ms, total={}ms",
            session_id,
            self.session.messages.len(),
            restored_soft_interrupts,
            load_ms,
            assign_ms,
            reset_ms,
            model_ms,
            mark_active_ms,
            compaction_ms,
            env_snapshot_ms,
            save_ms,
            restore_start.elapsed().as_millis(),
        ));
        logging::info(&format!(
            "Session restored: {} messages in session",
            self.session.messages.len()
        ));
        Ok(previous_status)
    }

    /// Get conversation history for sync
    pub fn get_history(&self) -> Vec<HistoryMessage> {
        crate::session::render_messages(&self.session)
            .into_iter()
            .map(|msg| HistoryMessage {
                role: msg.role,
                content: msg.content,
                tool_calls: if msg.tool_calls.is_empty() {
                    None
                } else {
                    Some(msg.tool_calls)
                },
                tool_data: msg.tool_data,
            })
            .collect()
    }

    pub fn get_history_and_rendered_images(
        &self,
    ) -> (Vec<HistoryMessage>, Vec<crate::session::RenderedImage>) {
        let (messages, images) = crate::session::render_messages_and_images(&self.session);
        let history = messages
            .into_iter()
            .map(|msg| HistoryMessage {
                role: msg.role,
                content: msg.content,
                tool_calls: if msg.tool_calls.is_empty() {
                    None
                } else {
                    Some(msg.tool_calls)
                },
                tool_data: msg.tool_data,
            })
            .collect();
        (history, images)
    }

    pub fn get_tool_call_summaries(&self, limit: usize) -> Vec<crate::protocol::ToolCallSummary> {
        crate::session::summarize_tool_calls(&self.session, limit)
    }

    /// Start an interactive REPL
    pub async fn repl(&mut self) -> Result<()> {
        println!("J-Code - Coding Agent");
        println!("Type your message, or 'quit' to exit.");

        // Show available skills
        let skills = self.current_skills_snapshot();
        let skill_list = skills.list();
        if !skill_list.is_empty() {
            println!(
                "Available skills: {}",
                skill_list
                    .iter()
                    .map(|s| format!("/{}", s.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        println!();

        loop {
            print!("> ");
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;

            let input = input.trim();
            if input.is_empty() {
                continue;
            }

            if input == "quit" || input == "exit" {
                break;
            }

            if input == "clear" {
                self.clear();
                println!("Conversation cleared.");
                continue;
            }

            // Check for skill invocation
            if let Some(skill_name) = SkillRegistry::parse_invocation(input) {
                if let Some(skill) = skills.get(skill_name) {
                    println!("Activating skill: {}", skill.name);
                    println!("{}\n", skill.description);
                    self.active_skill = Some(skill_name.to_string());
                    continue;
                } else if skill_name == "neuraui" {
                    println!("{}", crate::neura_ui::launch());
                    continue;
                } else {
                    println!("Unknown skill: /{}", skill_name);
                    println!(
                        "Available: {}",
                        skills
                            .list()
                            .iter()
                            .map(|s| format!("/{}", s.name))
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    continue;
                }
            }

            if let Err(e) = self.run_once(input).await {
                eprintln!("\nError: {}\n", e);
            }

            println!();
        }

        // Extract memories from session before exiting
        self.extract_session_memories().await;

        Ok(())
    }

    /// Extract memories from the session transcript
    /// Returns the number of memories extracted, or 0 if none/skipped
    pub async fn extract_session_memories(&self) -> usize {
        if !self.memory_enabled {
            return 0;
        }

        // Need at least 4 messages for meaningful extraction
        if self.session.messages.len() < 4 {
            return 0;
        }

        logging::info(&format!(
            "Extracting memories from {} messages",
            self.session.messages.len()
        ));

        // Build transcript
        let mut transcript = String::new();
        for msg in &self.session.messages {
            let role = match msg.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
            };
            transcript.push_str(&format!("**{}:**\n", role));
            for block in &msg.content {
                match block {
                    ContentBlock::Text { text, .. } => {
                        transcript.push_str(text);
                        transcript.push('\n');
                    }
                    ContentBlock::ToolUse { name, .. } => {
                        transcript.push_str(&format!("[Used tool: {}]\n", name));
                    }
                    ContentBlock::ToolResult { content, .. } => {
                        let preview = if content.len() > 200 {
                            format!("{}...", crate::util::truncate_str(content, 200))
                        } else {
                            content.clone()
                        };
                        transcript.push_str(&format!("[Result: {}]\n", preview));
                    }
                    ContentBlock::Reasoning { .. } => {}
                    ContentBlock::Image { .. } => {
                        transcript.push_str("[Image]\n");
                    }
                    ContentBlock::OpenAICompaction { .. } => {
                        transcript.push_str("[OpenAI native compaction]\n");
                    }
                }
            }
            transcript.push('\n');
        }

        if !crate::memory::memory_sidecar_enabled() {
            logging::info("Memory extraction skipped: memory sidecar disabled");
            return 0;
        }

        // Extract using sidecar
        let sidecar = crate::sidecar::Sidecar::new();
        match sidecar.extract_memories(&transcript).await {
            Ok(extracted) if !extracted.is_empty() => {
                let manager = self
                    .session
                    .working_dir
                    .as_deref()
                    .map(|dir| crate::memory::MemoryManager::new().with_project_dir(dir))
                    .unwrap_or_default();
                let mut stored_count = 0;

                for memory in &extracted {
                    let category = crate::memory::MemoryCategory::from_extracted(&memory.category);

                    let trust = match memory.trust.as_str() {
                        "high" => crate::memory::TrustLevel::High,
                        "low" => crate::memory::TrustLevel::Low,
                        _ => crate::memory::TrustLevel::Medium,
                    };

                    let entry = crate::memory::MemoryEntry::new(category, &memory.content)
                        .with_source(&self.session.id)
                        .with_trust(trust);

                    if manager.remember_project(entry).is_ok() {
                        stored_count += 1;
                    }
                }

                if stored_count > 0 {
                    logging::info(&format!("Extracted {} memories from session", stored_count));
                }
                stored_count
            }
            Ok(_) => 0,
            Err(e) => {
                logging::info(&format!("Memory extraction skipped: {}", e));
                0
            }
        }
    }
}
fn filter_tool_definitions_for_messages(
    defs: Vec<ToolDefinition>,
    messages: &[crate::session::StoredMessage],
) -> Vec<ToolDefinition> {
    if !dynamic_tool_filter_enabled() {
        return defs;
    }
    let latest = latest_real_user_text(messages);
    if latest.is_empty() {
        return defs;
    }
    let wanted = classify_tools(&latest);
    let fallback = fallback_tool_catalog(&defs, &wanted);
    let mut filtered = Vec::new();
    for def in defs {
        if is_always_on_tool(&def.name) || wanted.iter().any(|name| name == &def.name) {
            filtered.push(def);
        }
    }
    if let Some(catalog) = fallback {
        filtered.push(catalog);
    }
    filtered
}

fn dynamic_tool_filter_enabled() -> bool {
    std::env::var("NEURA_DYNAMIC_TOOL_FILTER")
        .map(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
        .unwrap_or(true)
}

fn latest_real_user_text(messages: &[crate::session::StoredMessage]) -> String {
    messages
        .iter()
        .rev()
        .find_map(|message| {
            if !matches!(message.role, Role::User) {
                return None;
            }
            let text = message.content.iter().find_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })?;
            let trimmed = text.trim();
            if trimmed.is_empty() || trimmed.starts_with("<system-reminder>") {
                None
            } else {
                Some(trimmed.to_ascii_lowercase())
            }
        })
        .unwrap_or_default()
}

fn is_always_on_tool(name: &str) -> bool {
    // Keep the floor intentionally tiny.  Most turns do not need the full tool
    // catalog, and every schema sent to the provider is recurring prompt
    // overhead.  Intent classification below expands this set when a request
    // actually implies repo/file work, background execution, browsing, memory,
    // or coordination.
    matches!(name, "bash" | "read" | "tool_expand")
}

fn add_tool(names: &mut Vec<String>, name: &str) {
    if !names.iter().any(|existing| existing == name) {
        names.push(name.to_string());
    }
}

fn classify_tools(latest: &str) -> Vec<String> {
    let mut wanted = Vec::new();

    let file_or_repo_work = contains_any(
        latest,
        &[
            "repo",
            "code",
            "file",
            "readme",
            "docs",
            ".md",
            ".rs",
            ".py",
            "src/",
            "fix",
            "bug",
            "implement",
            "refactor",
            "patch",
            "edit",
            "write",
            "commit",
            "push",
            "test",
            "build",
            "benchmark",
            "grep",
            "search the code",
            "find usage",
            "symbol",
        ],
    );
    if file_or_repo_work {
        // read2.txt's recommended stack is: text search -> syntax-aware narrow
        // region -> patch -> formatter/tests.  agentgrep is the primary low-token
        // narrowing tool; patch/edit tools are only exposed when repository or
        // file work is likely.
        add_tool(&mut wanted, "agentgrep");
        add_tool(&mut wanted, "grep");
        add_tool(&mut wanted, "glob");
        add_tool(&mut wanted, "ls");
        add_tool(&mut wanted, "apply_patch");
        add_tool(&mut wanted, "edit");
        add_tool(&mut wanted, "multiedit");
        add_tool(&mut wanted, "write");
    }

    if contains_any(
        latest,
        &["parallel", "batch", "multiple files", "many files"],
    ) {
        add_tool(&mut wanted, "batch");
    }
    if contains_any(
        latest,
        &["background", "long", "tail", "wait", "progress", "timeout"],
    ) {
        add_tool(&mut wanted, "bg");
    }
    if contains_any(
        latest,
        &["weather", "website", "web", "search", "http", "url"],
    ) {
        add_tool(&mut wanted, "websearch");
        add_tool(&mut wanted, "webfetch");
    }
    if contains_any(
        latest,
        &["browser", "click", "page", "login", "screenshot", "ui"],
    ) {
        add_tool(&mut wanted, "browser");
        add_tool(&mut wanted, "mouse");
    }
    if contains_any(latest, &["email", "gmail", "inbox"]) {
        add_tool(&mut wanted, "gmail");
    }
    if contains_any(latest, &["remember", "memory", "recall", "preference"]) {
        add_tool(&mut wanted, "memory");
    }
    if contains_any(latest, &["todo", "roadmap", "checklist"]) {
        add_tool(&mut wanted, "todo");
    }
    if contains_any(latest, &["goal", "schedule", "remind", "later"]) {
        add_tool(&mut wanted, "goal");
        add_tool(&mut wanted, "schedule");
    }
    if contains_any(
        latest,
        &["subagent", "swarm", "delegate", "parallel agents"],
    ) {
        add_tool(&mut wanted, "subagent");
        add_tool(&mut wanted, "swarm");
    }
    wanted
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn fallback_tool_catalog(defs: &[ToolDefinition], wanted: &[String]) -> Option<ToolDefinition> {
    let hidden: Vec<String> = defs
        .iter()
        .filter(|def| !is_always_on_tool(&def.name) && !wanted.iter().any(|name| name == &def.name))
        .map(|def| def.name.clone())
        .collect();
    if hidden.is_empty() {
        return None;
    }
    Some(ToolDefinition {
        name: "tool_expand".to_string(),
        description: format!(
            "Request hidden tools if needed. Available: {}. Prefer current tools unless missing capability.",
            hidden.join(",")
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "tools": {"type": "array", "items": {"type": "string"}},
                "reason": {"type": "string"}
            },
            "required": ["tools", "reason"]
        }),
    })
}

#[cfg(test)]
mod dynamic_tool_filter_tests {
    use super::*;
    use crate::runtime_ledger::{self, RuntimeReceiptKind};

    fn def(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: name.to_string(),
            input_schema: serde_json::json!({"type":"object"}),
        }
    }

    #[test]
    fn weather_turn_keeps_web_tools_and_expand_fallback() {
        let defs = vec![
            def("bash"),
            def("agentgrep"),
            def("websearch"),
            def("webfetch"),
            def("gmail"),
            def("browser"),
        ];
        let messages = vec![crate::session::StoredMessage {
            id: "m1".to_string(),
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "what is the weather today?".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            display_role: None,
            tool_duration_ms: None,
            token_usage: None,
        }];

        let filtered = filter_tool_definitions_for_messages(defs, &messages);
        let names: Vec<_> = filtered.iter().map(|def| def.name.as_str()).collect();
        assert!(names.contains(&"bash"));
        assert!(!names.contains(&"agentgrep"));
        assert!(names.contains(&"websearch"));
        assert!(names.contains(&"webfetch"));
        assert!(names.contains(&"tool_expand"));
        assert!(!names.contains(&"gmail"));
    }

    #[test]
    fn direct_answer_turn_keeps_only_core_tools() {
        let defs = vec![def("bash"), def("read"), def("websearch"), def("gmail")];
        let messages = vec![crate::session::StoredMessage {
            id: "m1".to_string(),
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "what time is it in arizona?".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            display_role: None,
            tool_duration_ms: None,
            token_usage: None,
        }];

        let filtered = filter_tool_definitions_for_messages(defs, &messages);
        let names: Vec<_> = filtered.iter().map(|def| def.name.as_str()).collect();
        assert_eq!(names, vec!["bash", "read", "tool_expand"]);
    }

    #[test]
    fn coding_turn_expands_search_and_edit_tools() {
        let defs = vec![
            def("bash"),
            def("read"),
            def("agentgrep"),
            def("glob"),
            def("apply_patch"),
            def("edit"),
            def("websearch"),
        ];
        let messages = vec![crate::session::StoredMessage {
            id: "m1".to_string(),
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "fix the bug in src/provider.rs and commit it".to_string(),
                cache_control: None,
            }],
            timestamp: None,
            display_role: None,
            tool_duration_ms: None,
            token_usage: None,
        }];

        let filtered = filter_tool_definitions_for_messages(defs, &messages);
        let names: Vec<_> = filtered.iter().map(|def| def.name.as_str()).collect();
        assert!(names.contains(&"agentgrep"));
        assert!(names.contains(&"glob"));
        assert!(names.contains(&"apply_patch"));
        assert!(names.contains(&"edit"));
        assert!(!names.contains(&"websearch"));
    }
}
