#![cfg_attr(test, allow(clippy::await_holding_lock))]

use super::{Tool, ToolContext, ToolOutput};
use crate::plan::PlanItem;
use crate::protocol::{
    AgentInfo, AgentStatusSnapshot, AwaitedMemberStatus, CommDeliveryMode, ContextEntry,
    HistoryMessage, PlanGraphStatus, Request, ServerEvent, SwarmChannelInfo, ToolCallSummary,
};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

const REQUEST_ID: u64 = 1;

mod transport;
use transport::{send_request, send_request_with_timeout};

fn fresh_spawn_request_nonce(ctx: &ToolContext) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{}-{}-{}", ctx.session_id, ctx.message_id, now_ms)
}

fn check_error(response: &ServerEvent) -> Option<&str> {
    if let ServerEvent::Error { message, .. } = response {
        Some(message)
    } else {
        None
    }
}

fn ensure_success(response: &ServerEvent) -> Result<()> {
    if let Some(message) = check_error(response) {
        Err(anyhow::anyhow!(message.to_string()))
    } else {
        Ok(())
    }
}

async fn fetch_plan_status(session_id: &str) -> Result<PlanGraphStatus> {
    let request = Request::CommPlanStatus {
        id: REQUEST_ID,
        session_id: session_id.to_string(),
    };
    match send_request(request).await {
        Ok(ServerEvent::CommPlanStatusResponse { summary, .. }) => Ok(summary),
        Ok(response) => {
            ensure_success(&response)?;
            Err(anyhow::anyhow!("No plan status returned."))
        }
        Err(e) => Err(anyhow::anyhow!("Failed to get plan status: {}", e)),
    }
}

fn format_plan_followup(summary: &PlanGraphStatus) -> String {
    let mut parts = Vec::new();
    parts.push(format!("active={}", summary.active_ids.len()));
    if !summary.next_ready_ids.is_empty() {
        parts.push(format!("next={}", summary.next_ready_ids.join(", ")));
    }
    if !summary.newly_ready_ids.is_empty() {
        parts.push(format!(
            "newly_ready={}",
            summary.newly_ready_ids.join(", ")
        ));
    }
    parts.join(" · ")
}

fn auto_assignment_needs_spawn(response: &ServerEvent) -> bool {
    check_error(response).is_some_and(|message| {
        message.contains(
            "No ready or completed swarm agents are available for automatic task assignment",
        )
    })
}

async fn spawn_assignment_session(ctx: &ToolContext, params: &CommunicateInput) -> Result<String> {
    let spawn_request = Request::CommSpawn {
        id: REQUEST_ID,
        session_id: ctx.session_id.clone(),
        working_dir: params.working_dir.clone(),
        initial_message: None,
        request_nonce: Some(fresh_spawn_request_nonce(ctx)),
    };

    match send_request(spawn_request).await {
        Ok(ServerEvent::CommSpawnResponse { new_session_id, .. }) if !new_session_id.is_empty() => {
            Ok(new_session_id)
        }
        Ok(spawn_response) => {
            ensure_success(&spawn_response)?;
            Err(anyhow::anyhow!(
                "Spawn succeeded but new session ID was not returned."
            ))
        }
        Err(e) => Err(anyhow::anyhow!(
            "Failed to spawn agent for task assignment: {}",
            e
        )),
    }
}

async fn assign_task_to_session(
    ctx: &ToolContext,
    params: &CommunicateInput,
    target_session: String,
    spawned_suffix: &str,
) -> Result<ToolOutput> {
    let retry_request = Request::CommAssignTask {
        id: REQUEST_ID,
        session_id: ctx.session_id.clone(),
        target_session: Some(target_session.clone()),
        task_id: params.task_id.clone(),
        message: params.message.clone(),
    };

    match send_request(retry_request).await {
        Ok(ServerEvent::CommAssignTaskResponse { task_id, .. }) => Ok(ToolOutput::new(format!(
            "Task '{}' assigned to {}{}",
            task_id, target_session, spawned_suffix
        ))),
        Ok(retry_response) => {
            ensure_success(&retry_response)?;
            Ok(ToolOutput::new(format!(
                "Assigned next runnable task to {}{}",
                target_session, spawned_suffix
            )))
        }
        Err(e) => Err(anyhow::anyhow!(
            "Failed to assign task after selecting {}: {}",
            target_session,
            e
        )),
    }
}

fn format_context_entries(entries: &[ContextEntry]) -> ToolOutput {
    if entries.is_empty() {
        ToolOutput::new("No shared context found.")
    } else {
        let mut output = String::from("Shared context from other agents:\n\n");
        for entry in entries {
            let from = entry.from_name.as_deref().unwrap_or(&entry.from_session);
            output.push_str(&format!(
                "  {} (from {}): {}\n",
                entry.key, from, entry.value
            ));
        }
        ToolOutput::new(output)
    }
}

fn duplicate_friendly_names<'a>(
    names: impl IntoIterator<Item = Option<&'a str>>,
) -> std::collections::HashSet<&'a str> {
    let mut counts = std::collections::HashMap::<&'a str, usize>::new();
    for name in names.into_iter().flatten() {
        *counts.entry(name).or_default() += 1;
    }
    counts
        .into_iter()
        .filter_map(|(name, count)| (count > 1).then_some(name))
        .collect()
}

fn session_display_suffix(session_id: &str) -> &str {
    let suffix = session_id.rsplit('_').next().unwrap_or(session_id);
    if suffix.len() > 6 {
        &suffix[suffix.len() - 6..]
    } else {
        suffix
    }
}

fn display_friendly_name(
    friendly_name: Option<&str>,
    session_id: &str,
    duplicate_names: &std::collections::HashSet<&str>,
) -> String {
    match friendly_name {
        Some(name) if duplicate_names.contains(name) => {
            format!("{} [{}]", name, session_display_suffix(session_id))
        }
        Some(name) => name.to_string(),
        None => session_id.to_string(),
    }
}

fn format_members(ctx: &ToolContext, members: &[AgentInfo]) -> ToolOutput {
    if members.is_empty() {
        ToolOutput::new("No other agents in this codebase.")
    } else {
        let duplicate_names =
            duplicate_friendly_names(members.iter().map(|member| member.friendly_name.as_deref()));
        let mut output = String::from("Agents in this codebase:\n\n");
        for member in members {
            let name = display_friendly_name(
                member.friendly_name.as_deref(),
                &member.session_id,
                &duplicate_names,
            );
            let session = &member.session_id;
            let role = member.role.as_deref().unwrap_or("agent");
            let files = member.files_touched.join(", ");
            let status = member.status.as_deref().unwrap_or("unknown");
            let is_me = session == &ctx.session_id;
            let role_label = if role != "agent" {
                format!(" [{}]", role)
            } else {
                String::new()
            };
            let mut extra_meta = Vec::new();
            if member.is_headless == Some(true) {
                extra_meta.push("headless".to_string());
            }
            if let Some(attachments) = member.live_attachments {
                extra_meta.push(format!("attachments={attachments}"));
            }
            if let Some(age_secs) = member.status_age_secs {
                extra_meta.push(format!("status_age={}s", age_secs));
            }
            let meta_suffix = if extra_meta.is_empty() {
                String::new()
            } else {
                format!("\n    Meta: {}", extra_meta.join(" · "))
            };
            output.push_str(&format!(
                "  {}{} ({})\n    Status: {}{}{}{}\n",
                name,
                role_label,
                if is_me { "you" } else { session },
                status,
                member
                    .detail
                    .as_deref()
                    .map(|detail| format!(" — {}", detail))
                    .unwrap_or_default(),
                if files.is_empty() {
                    String::new()
                } else {
                    format!("\n    Files: {}", files)
                },
                meta_suffix
            ));
        }
        ToolOutput::new(output)
    }
}

fn format_tool_summary(target: &str, calls: &[ToolCallSummary]) -> ToolOutput {
    if calls.is_empty() {
        ToolOutput::new(format!("No tool calls found for {}", target))
    } else {
        let call_count = calls.len();
        let mut output = format!(
            "Tool call summary for {} ({} call{}):\n\n",
            target,
            call_count,
            if call_count == 1 { "" } else { "s" }
        );
        for call in calls {
            output.push_str(&format!("  {} — {}\n", call.tool_name, call.brief_output));
        }
        ToolOutput::new(output)
    }
}

fn format_status_snapshot(snapshot: &AgentStatusSnapshot) -> ToolOutput {
    let target = snapshot
        .friendly_name
        .as_deref()
        .unwrap_or(&snapshot.session_id);
    let status = snapshot.status.as_deref().unwrap_or("unknown");
    let mut output = format!(
        "Status snapshot for {} ({})\n\n",
        target, snapshot.session_id
    );
    output.push_str(&format!("  Lifecycle: {}", status));
    if let Some(detail) = snapshot.detail.as_deref() {
        output.push_str(&format!(" — {}", detail));
    }
    output.push('\n');

    let activity = snapshot
        .activity
        .as_ref()
        .map(|activity| match activity.current_tool_name.as_deref() {
            Some(tool_name) => format!("busy ({tool_name})"),
            None if activity.is_processing => "busy".to_string(),
            _ => "idle".to_string(),
        })
        .unwrap_or_else(|| "idle".to_string());
    output.push_str(&format!("  Activity: {}\n", activity));

    if let Some(role) = snapshot.role.as_deref() {
        output.push_str(&format!("  Role: {}\n", role));
    }
    if let Some(swarm_id) = snapshot.swarm_id.as_deref() {
        output.push_str(&format!("  Swarm: {}\n", swarm_id));
    }

    let mut meta = Vec::new();
    if snapshot.is_headless == Some(true) {
        meta.push("headless".to_string());
    }
    if let Some(attachments) = snapshot.live_attachments {
        meta.push(format!("attachments={attachments}"));
    }
    if let Some(age_secs) = snapshot.status_age_secs {
        meta.push(format!("status_age={}s", age_secs));
    }
    if let Some(age_secs) = snapshot.joined_age_secs {
        meta.push(format!("joined={}s", age_secs));
    }
    if !meta.is_empty() {
        output.push_str(&format!("  Meta: {}\n", meta.join(" · ")));
    }

    if snapshot.provider_name.is_some() || snapshot.provider_model.is_some() {
        let provider = snapshot.provider_name.as_deref().unwrap_or("unknown");
        let model = snapshot.provider_model.as_deref().unwrap_or("unknown");
        output.push_str(&format!("  Provider: {} / {}\n", provider, model));
    }

    if snapshot.files_touched.is_empty() {
        output.push_str("  Files: (none)\n");
    } else {
        output.push_str(&format!("  Files: {}\n", snapshot.files_touched.join(", ")));
    }

    ToolOutput::new(output)
}

fn format_plan_status(summary: &PlanGraphStatus) -> ToolOutput {
    let swarm_id = summary.swarm_id.as_deref().unwrap_or("unknown");
    let mut output = format!(
        "Plan status for swarm {}\n\n  Version: {}\n  Items: {}\n",
        swarm_id, summary.version, summary.item_count
    );

    output.push_str(&format!(
        "  Ready: {}\n",
        if summary.ready_ids.is_empty() {
            "(none)".to_string()
        } else {
            summary.ready_ids.join(", ")
        }
    ));
    output.push_str(&format!(
        "  Next up: {}\n",
        if summary.next_ready_ids.is_empty() {
            "(none)".to_string()
        } else {
            summary.next_ready_ids.join(", ")
        }
    ));
    if !summary.newly_ready_ids.is_empty() {
        output.push_str(&format!(
            "  Newly ready: {}\n",
            summary.newly_ready_ids.join(", ")
        ));
    }
    if !summary.blocked_ids.is_empty() {
        output.push_str(&format!("  Blocked: {}\n", summary.blocked_ids.join(", ")));
    }
    if !summary.active_ids.is_empty() {
        output.push_str(&format!("  Active: {}\n", summary.active_ids.join(", ")));
    }
    if !summary.completed_ids.is_empty() {
        output.push_str(&format!(
            "  Completed: {}\n",
            summary.completed_ids.join(", ")
        ));
    }
    if !summary.cycle_ids.is_empty() {
        output.push_str(&format!("  Cycles: {}\n", summary.cycle_ids.join(", ")));
    }
    if !summary.unresolved_dependency_ids.is_empty() {
        output.push_str(&format!(
            "  Missing deps: {}\n",
            summary.unresolved_dependency_ids.join(", ")
        ));
    }

    ToolOutput::new(output)
}

fn format_context_history(target: &str, messages: &[HistoryMessage]) -> ToolOutput {
    if messages.is_empty() {
        ToolOutput::new(format!("No conversation history for {}", target))
    } else {
        let mut output = format!(
            "Conversation context for {} ({} messages):\n\n",
            target,
            messages.len()
        );
        for msg in messages {
            let truncated = if msg.content.len() > 500 {
                format!("{}...", &msg.content[..500])
            } else {
                msg.content.clone()
            };
            output.push_str(&format!("[{}] {}\n\n", msg.role, truncated));
        }
        ToolOutput::new(output)
    }
}

fn format_awaited_members(
    completed: bool,
    summary: &str,
    members: &[AwaitedMemberStatus],
) -> ToolOutput {
    let mut output = if completed {
        format!("All members done. {}\n", summary)
    } else {
        format!("Await incomplete. {}\n", summary)
    };

    if !members.is_empty() {
        let duplicate_names =
            duplicate_friendly_names(members.iter().map(|member| member.friendly_name.as_deref()));
        output.push_str("\nMember statuses:\n");
        for member in members {
            let name = display_friendly_name(
                member.friendly_name.as_deref(),
                &member.session_id,
                &duplicate_names,
            );
            let icon = if member.done { "✓" } else { "✗" };
            output.push_str(&format!("  {} {} ({})\n", icon, name, member.status));
        }
    }

    ToolOutput::new(output)
}

fn default_await_target_statuses() -> Vec<String> {
    vec![
        "ready".to_string(),
        "completed".to_string(),
        "stopped".to_string(),
        "failed".to_string(),
    ]
}

fn format_channels(channels: &[SwarmChannelInfo]) -> ToolOutput {
    if channels.is_empty() {
        ToolOutput::new("No swarm channels found.")
    } else {
        let mut output = String::from("Swarm channels:\n\n");
        for channel in channels {
            output.push_str(&format!(
                "  #{} — {} subscriber{}\n",
                channel.channel,
                channel.member_count,
                if channel.member_count == 1 { "" } else { "s" }
            ));
        }
        ToolOutput::new(output)
    }
}

pub struct CommunicateTool;

impl CommunicateTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct CommunicateInput {
    action: String,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    to_session: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    proposer_session: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    target_session: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    initial_message: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    spawn_if_needed: Option<bool>,
    #[serde(default)]
    prefer_spawn: Option<bool>,
    #[serde(default)]
    plan_items: Option<Vec<PlanItem>>,
    #[serde(default)]
    target_status: Option<Vec<String>>,
    #[serde(default)]
    session_ids: Option<Vec<String>>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    timeout_minutes: Option<u64>,
    #[serde(default)]
    wake: Option<bool>,
    #[serde(default)]
    delivery: Option<CommDeliveryMode>,
    #[serde(default)]
    concurrency_limit: Option<usize>,
}

impl CommunicateInput {
    fn spawn_initial_message(&self) -> Option<String> {
        self.initial_message.clone().or_else(|| self.prompt.clone())
    }
}

#[async_trait]
impl Tool for CommunicateTool {
    fn name(&self) -> &str {
        "swarm"
    }

    fn description(&self) -> &str {
        "Coordinate agents. For spawn, prefer providing a prompt so the new agent starts with a concrete task instead of idling."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "intent": super::intent_schema_property(),
                "action": {
                    "type": "string",
                    "enum": ["share", "share_append", "read", "message", "broadcast", "dm", "channel", "list", "list_channels", "channel_members",
                             "propose_plan", "approve_plan", "reject_plan", "spawn", "stop", "assign_role",
                             "status", "plan_status", "summary", "read_context", "resync_plan", "assign_task", "assign_next", "fill_slots",
                             "start", "wake", "resume", "retry", "reassign", "replace", "salvage",
                             "subscribe_channel", "unsubscribe_channel", "await_members"],
                    "description": "Action. For spawn, prefer including prompt with the initial task so the new agent starts useful work immediately."
                },
                "key": {
                    "type": "string"
                },
                "value": {
                    "type": "string"
                },
                "message": {
                    "type": "string"
                },
                "to_session": {
                    "type": "string",
                    "description": "DM target. Accepts an exact session ID or a unique friendly name within the swarm. If a friendly name is ambiguous, run swarm list and use the exact session ID."
                },
                "channel": { "type": "string" },
                "proposer_session": { "type": "string" },
                "reason": { "type": "string" },
                "target_session": { "type": "string" },
                "role": {
                    "type": "string",
                    "enum": ["agent", "coordinator", "worktree_manager"]
                },
                "working_dir": {
                    "type": "string",
                    "description": "Optional working directory for spawn."
                },
                "prompt": {
                    "type": "string",
                    "description": "Preferred for spawn. Initial task/instructions for the new agent. Spawning without prompt usually creates an idle agent that needs follow-up assignment."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional max items for summary-style reads."
                },
                "task_id": {
                    "type": "string",
                    "description": "Optional plan task ID. If omitted for assign_task, the coordinator assigns the next runnable unassigned task."
                },
                "spawn_if_needed": {
                    "type": "boolean",
                    "description": "For assign_task without an explicit target_session: if no reusable agent is available, spawn a fresh agent and retry the assignment automatically."
                },
                "prefer_spawn": {
                    "type": "boolean",
                    "description": "For assign_task without an explicit target_session: prefer a fresh spawned agent even if reusable workers are available."
                },
                "session_ids": {
                    "type": "array",
                    "items": {"type": "string"}
                },
                "mode": {
                    "type": "string",
                    "enum": ["all", "any"],
                    "description": "For await_members: wait for all targeted members or wake when any targeted member matches."
                },
                "target_status": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional completion statuses for await_members. Defaults to ready/completed/stopped/failed."
                },
                "timeout_minutes": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional timeout for await_members."
                },
                "concurrency_limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "For fill_slots: desired maximum number of active swarm tasks."
                },
                "wake": {
                    "type": "boolean",
                    "description": "Optional wake hint for messages."
                },
                "delivery": {
                    "type": "string",
                    "enum": ["notify", "interrupt", "wake"],
                    "description": "Optional delivery mode for dm/channel messaging."
                },
                "plan_items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "additionalProperties": true
                    }
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: CommunicateInput = serde_json::from_value(input)?;

        match params.action.as_str() {
            "share" | "share_append" => {
                let key = params
                    .key
                    .ok_or_else(|| anyhow::anyhow!("'key' is required for share action"))?;
                let value = params
                    .value
                    .ok_or_else(|| anyhow::anyhow!("'value' is required for share action"))?;

                let request = Request::CommShare {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    key: key.clone(),
                    value: value.clone(),
                    append: params.action == "share_append",
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        let verb = if params.action == "share_append" {
                            "Appended shared context"
                        } else {
                            "Shared with other agents"
                        };
                        Ok(ToolOutput::new(format!("{}: {} = {}", verb, key, value)))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to share: {}", e)),
                }
            }

            "read" => {
                let request = Request::CommRead {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    key: params.key.clone(),
                };

                match send_request(request).await {
                    Ok(ServerEvent::CommContext { entries, .. }) => {
                        Ok(format_context_entries(&entries))
                    }
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new("No shared context found."))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to read shared context: {}", e)),
                }
            }

            "message" | "broadcast" => {
                let message = params
                    .message
                    .ok_or_else(|| anyhow::anyhow!("'message' is required for message action"))?;

                let request = Request::CommMessage {
                    id: REQUEST_ID,
                    from_session: ctx.session_id.clone(),
                    message: message.clone(),
                    to_session: None,
                    channel: None,
                    wake: params.wake,
                    delivery: None,
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!(
                            "Message sent to other agents: {}",
                            message
                        )))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to send message: {}", e)),
                }
            }

            "dm" => {
                let message = params
                    .message
                    .ok_or_else(|| anyhow::anyhow!("'message' is required for dm action"))?;
                let to_session = params
                    .to_session
                    .ok_or_else(|| anyhow::anyhow!("'to_session' is required for dm action"))?;

                let request = Request::CommMessage {
                    id: REQUEST_ID,
                    from_session: ctx.session_id.clone(),
                    message: message.clone(),
                    to_session: Some(to_session.clone()),
                    channel: None,
                    delivery: params.delivery,
                    wake: params.wake,
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!(
                            "Direct message sent to {}: {}",
                            to_session, message
                        )))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to send DM: {}", e)),
                }
            }

            "channel" => {
                let message = params
                    .message
                    .ok_or_else(|| anyhow::anyhow!("'message' is required for channel action"))?;
                let channel = params
                    .channel
                    .ok_or_else(|| anyhow::anyhow!("'channel' is required for channel action"))?;

                let request = Request::CommMessage {
                    id: REQUEST_ID,
                    from_session: ctx.session_id.clone(),
                    message: message.clone(),
                    to_session: None,
                    channel: Some(channel.clone()),
                    delivery: params.delivery,
                    wake: params.wake,
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!(
                            "Channel message sent to #{}: {}",
                            channel, message
                        )))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to send channel message: {}", e)),
                }
            }

            "list" => {
                let request = Request::CommList {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                };

                match send_request(request).await {
                    Ok(ServerEvent::CommMembers { members, .. }) => {
                        Ok(format_members(&ctx, &members))
                    }
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new("No agents found."))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to list agents: {}", e)),
                }
            }

            "list_channels" => {
                let request = Request::CommListChannels {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                };

                match send_request(request).await {
                    Ok(ServerEvent::CommChannels { channels, .. }) => {
                        Ok(format_channels(&channels))
                    }
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new("No channels found."))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to list channels: {}", e)),
                }
            }

            "channel_members" => {
                let channel = params.channel.ok_or_else(|| {
                    anyhow::anyhow!("'channel' is required for channel_members action")
                })?;
                let request = Request::CommChannelMembers {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    channel: channel.clone(),
                };

                match send_request(request).await {
                    Ok(ServerEvent::CommMembers { members, .. }) => {
                        let mut output = format!("Members subscribed to #{}:\n\n", channel);
                        if members.is_empty() {
                            output.push_str("  (none)\n");
                        } else {
                            for member in members {
                                let name = member.friendly_name.unwrap_or(member.session_id);
                                let status = member.status.unwrap_or_else(|| "unknown".to_string());
                                output.push_str(&format!("  {} ({})\n", name, status));
                            }
                        }
                        Ok(ToolOutput::new(output))
                    }
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new("No channel members found."))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to list channel members: {}", e)),
                }
            }

            "propose_plan" => {
                let items = params.plan_items.ok_or_else(|| {
                    anyhow::anyhow!("'plan_items' is required for propose_plan action")
                })?;
                if items.is_empty() {
                    return Err(anyhow::anyhow!(
                        "'plan_items' must include at least one item"
                    ));
                }
                let item_count = items.len() as u64;

                let request = Request::CommProposePlan {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    items,
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!(
                            "Plan proposal submitted ({} items).",
                            item_count
                        )))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to propose plan: {}", e)),
                }
            }

            "approve_plan" => {
                let proposer = params.proposer_session.ok_or_else(|| {
                    anyhow::anyhow!("'proposer_session' is required for approve_plan action")
                })?;

                let request = Request::CommApprovePlan {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    proposer_session: proposer.clone(),
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!(
                            "Approved plan proposal from {}",
                            proposer
                        )))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to approve plan: {}", e)),
                }
            }

            "reject_plan" => {
                let proposer = params.proposer_session.ok_or_else(|| {
                    anyhow::anyhow!("'proposer_session' is required for reject_plan action")
                })?;
                let reason = params.reason.clone();

                let request = Request::CommRejectPlan {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    proposer_session: proposer.clone(),
                    reason: reason.clone(),
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        let reason_msg = reason
                            .as_ref()
                            .map(|r| format!(" (reason: {})", r))
                            .unwrap_or_default();
                        Ok(ToolOutput::new(format!(
                            "Rejected plan proposal from {}{}",
                            proposer, reason_msg
                        )))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to reject plan: {}", e)),
                }
            }

            "spawn" => {
                let request = Request::CommSpawn {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    working_dir: params.working_dir.clone(),
                    initial_message: params.spawn_initial_message(),
                    request_nonce: None,
                };

                match send_request(request).await {
                    Ok(ServerEvent::CommSpawnResponse { new_session_id, .. })
                        if !new_session_id.is_empty() =>
                    {
                        Ok(ToolOutput::new(format!(
                            "Spawned new agent: {}",
                            new_session_id
                        )))
                    }
                    Ok(response) => {
                        ensure_success(&response)?;
                        Err(anyhow::anyhow!(
                            "Spawn succeeded but new session ID was not returned."
                        ))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to spawn agent: {}", e)),
                }
            }

            "stop" => {
                let target = params.target_session.ok_or_else(|| {
                    anyhow::anyhow!("'target_session' is required for stop action")
                })?;

                let request = Request::CommStop {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    target_session: target.clone(),
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!("Stopped agent: {}", target)))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to stop agent: {}", e)),
                }
            }

            "assign_role" => {
                let target_raw = params.target_session.ok_or_else(|| {
                    anyhow::anyhow!("'target_session' is required for assign_role action")
                })?;
                let role = params
                    .role
                    .ok_or_else(|| anyhow::anyhow!("'role' is required for assign_role action"))?;

                // Resolve "current" to the caller's own session ID
                let target = if target_raw == "current" {
                    ctx.session_id.clone()
                } else {
                    target_raw
                };

                let request = Request::CommAssignRole {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    target_session: target.clone(),
                    role: role.clone(),
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!(
                            "Assigned role '{}' to {}",
                            role, target
                        )))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to assign role: {}", e)),
                }
            }

            "status" => {
                let target = params.target_session.ok_or_else(|| {
                    anyhow::anyhow!("'target_session' is required for status action")
                })?;

                let request = Request::CommStatus {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    target_session: target.clone(),
                };

                match send_request(request).await {
                    Ok(ServerEvent::CommStatusResponse { snapshot, .. }) => {
                        Ok(format_status_snapshot(&snapshot))
                    }
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new("No status snapshot returned."))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to get status snapshot: {}", e)),
                }
            }

            "plan_status" => {
                let summary = fetch_plan_status(&ctx.session_id).await?;
                Ok(format_plan_status(&summary))
            }

            "summary" => {
                let target = params.target_session.ok_or_else(|| {
                    anyhow::anyhow!("'target_session' is required for summary action")
                })?;

                let request = Request::CommSummary {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    target_session: target.clone(),
                    limit: params.limit,
                };

                match send_request(request).await {
                    Ok(ServerEvent::CommSummaryResponse { tool_calls, .. }) => {
                        Ok(format_tool_summary(&target, &tool_calls))
                    }
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new("No tool call data returned."))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to get summary: {}", e)),
                }
            }

            "read_context" => {
                let target = params.target_session.ok_or_else(|| {
                    anyhow::anyhow!("'target_session' is required for read_context action")
                })?;

                let request = Request::CommReadContext {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    target_session: target.clone(),
                };

                match send_request(request).await {
                    Ok(ServerEvent::CommContextHistory { messages, .. }) => {
                        Ok(format_context_history(&target, &messages))
                    }
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new("No context data returned."))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to read context: {}", e)),
                }
            }

            "resync_plan" => {
                let request = Request::CommResyncPlan {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new("Swarm plan re-synced to your session."))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to resync plan: {}", e)),
                }
            }

            "assign_task" => {
                let target = params
                    .target_session
                    .clone()
                    .unwrap_or_else(|| "next available agent".to_string());
                let spawn_if_needed = params.spawn_if_needed.unwrap_or(false);
                let prefer_spawn = params.prefer_spawn.unwrap_or(false);

                if prefer_spawn && params.target_session.is_none() {
                    let spawned_session = spawn_assignment_session(&ctx, &params).await?;
                    return assign_task_to_session(
                        &ctx,
                        &params,
                        spawned_session,
                        " (spawned by planner preference)",
                    )
                    .await;
                }

                let request = Request::CommAssignTask {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    target_session: params.target_session.clone(),
                    task_id: params.task_id.clone(),
                    message: params.message.clone(),
                };

                match send_request(request).await {
                    Ok(ServerEvent::CommAssignTaskResponse {
                        task_id,
                        target_session,
                        ..
                    }) => {
                        let mut output =
                            format!("Task '{}' assigned to {}", task_id, target_session);
                        if let Ok(summary) = fetch_plan_status(&ctx.session_id).await {
                            output.push_str(&format!("\n{}", format_plan_followup(&summary)));
                        }
                        Ok(ToolOutput::new(output))
                    }
                    Ok(response)
                        if spawn_if_needed
                            && params.target_session.is_none()
                            && auto_assignment_needs_spawn(&response) =>
                    {
                        let spawned_session = spawn_assignment_session(&ctx, &params).await?;
                        assign_task_to_session(
                            &ctx,
                            &params,
                            spawned_session,
                            " (spawned automatically)",
                        )
                        .await
                    }
                    Ok(response) => {
                        ensure_success(&response)?;
                        let msg = params.task_id.as_deref().map_or_else(
                            || format!("Assigned next runnable task to {}", target),
                            |task_id| format!("Task '{}' assigned to {}", task_id, target),
                        );
                        Ok(ToolOutput::new(msg))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to assign task: {}", e)),
                }
            }

            "assign_next" => {
                let target = params
                    .target_session
                    .clone()
                    .unwrap_or_else(|| "next available agent".to_string());

                let request = Request::CommAssignNext {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    target_session: params.target_session.clone(),
                    prefer_spawn: params.prefer_spawn,
                    spawn_if_needed: params.spawn_if_needed,
                    message: params.message.clone(),
                };

                match send_request(request).await {
                    Ok(ServerEvent::CommAssignTaskResponse {
                        task_id,
                        target_session,
                        ..
                    }) => Ok(ToolOutput::new(format!(
                        "Task '{}' assigned to {}",
                        task_id, target_session
                    ))),
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!(
                            "Assigned next runnable task to {}",
                            target
                        )))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to assign next task: {}", e)),
                }
            }

            "fill_slots" => {
                let concurrency_limit = params.concurrency_limit.ok_or_else(|| {
                    anyhow::anyhow!("'concurrency_limit' is required for fill_slots action")
                })?;

                let summary = fetch_plan_status(&ctx.session_id).await?;

                let active_count = summary.active_ids.len();
                if active_count >= concurrency_limit {
                    return Ok(ToolOutput::new(format!(
                        "Window already full: {} active task(s) >= limit {}",
                        active_count, concurrency_limit
                    )));
                }

                let mut assignments = Vec::new();
                let available_slots = concurrency_limit.saturating_sub(active_count);
                for _ in 0..available_slots {
                    let request = Request::CommAssignNext {
                        id: REQUEST_ID,
                        session_id: ctx.session_id.clone(),
                        target_session: params.target_session.clone(),
                        prefer_spawn: params.prefer_spawn,
                        spawn_if_needed: params.spawn_if_needed,
                        message: params.message.clone(),
                    };

                    match send_request(request).await {
                        Ok(ServerEvent::CommAssignTaskResponse {
                            task_id,
                            target_session,
                            ..
                        }) => assignments.push(format!("{} -> {}", task_id, target_session)),
                        Ok(ServerEvent::Error { message, .. })
                            if message.contains("No runnable unassigned tasks")
                                || message.contains("No ready or completed swarm agents") =>
                        {
                            break;
                        }
                        Ok(response) => {
                            ensure_success(&response)?;
                        }
                        Err(e) => {
                            return Err(anyhow::anyhow!("Failed to fill slots: {}", e));
                        }
                    }
                }

                if assignments.is_empty() {
                    Ok(ToolOutput::new(format!(
                        "No assignments made. Active: {}, limit: {}",
                        active_count, concurrency_limit
                    )))
                } else {
                    let mut output = format!(
                        "Filled {} slot(s):\n{}",
                        assignments.len(),
                        assignments
                            .into_iter()
                            .map(|line| format!("- {}", line))
                            .collect::<Vec<_>>()
                            .join("\n")
                    );
                    if let Ok(summary) = fetch_plan_status(&ctx.session_id).await {
                        output.push_str(&format!("\n{}", format_plan_followup(&summary)));
                    }
                    Ok(ToolOutput::new(output))
                }
            }

            "start" | "wake" | "resume" | "retry" | "reassign" | "replace" | "salvage" => {
                let task_id = params.task_id.ok_or_else(|| {
                    anyhow::anyhow!("'task_id' is required for {} action", params.action)
                })?;
                if matches!(params.action.as_str(), "reassign" | "replace" | "salvage")
                    && params.target_session.is_none()
                {
                    return Err(anyhow::anyhow!(
                        "'target_session' is required for {} action",
                        params.action
                    ));
                }

                let request = Request::CommTaskControl {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    action: params.action.clone(),
                    task_id: task_id.clone(),
                    target_session: params.target_session.clone(),
                    message: params.message.clone(),
                };

                match send_request(request).await {
                    Ok(ServerEvent::CommTaskControlResponse {
                        task_id,
                        action,
                        target_session,
                        status,
                        summary,
                        ..
                    }) => {
                        let mut output = format!("Task '{}' {}", task_id, action);
                        if let Some(target_session) = target_session {
                            output.push_str(&format!(" -> {}", target_session));
                        }
                        output.push_str(&format!("\nStatus: {}", status));
                        if !summary.next_ready_ids.is_empty() {
                            output.push_str(&format!(
                                "\nNext ready: {}",
                                summary.next_ready_ids.join(", ")
                            ));
                        }
                        if !summary.newly_ready_ids.is_empty() {
                            output.push_str(&format!(
                                "\nNewly ready: {}",
                                summary.newly_ready_ids.join(", ")
                            ));
                        }
                        Ok(ToolOutput::new(output))
                    }
                    Ok(response) => {
                        ensure_success(&response)?;
                        let target_suffix = params
                            .target_session
                            .as_deref()
                            .map(|target| format!(" -> {}", target))
                            .unwrap_or_default();
                        Ok(ToolOutput::new(format!(
                            "Task '{}' {}{}",
                            task_id, params.action, target_suffix
                        )))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to {} task: {}", params.action, e)),
                }
            }

            "subscribe_channel" => {
                let channel = params.channel.ok_or_else(|| {
                    anyhow::anyhow!("'channel' is required for subscribe_channel action")
                })?;

                let request = Request::CommSubscribeChannel {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    channel: channel.clone(),
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!("Subscribed to #{}", channel)))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to subscribe: {}", e)),
                }
            }

            "unsubscribe_channel" => {
                let channel = params.channel.ok_or_else(|| {
                    anyhow::anyhow!("'channel' is required for unsubscribe_channel action")
                })?;

                let request = Request::CommUnsubscribeChannel {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    channel: channel.clone(),
                };

                match send_request(request).await {
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new(format!("Unsubscribed from #{}", channel)))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to unsubscribe: {}", e)),
                }
            }

            "await_members" => {
                let target_status = params
                    .target_status
                    .unwrap_or_else(default_await_target_statuses);
                let session_ids = params.session_ids.unwrap_or_default();
                let timeout_minutes = params.timeout_minutes.unwrap_or(60);
                let timeout_secs = timeout_minutes * 60;

                let request = Request::CommAwaitMembers {
                    id: REQUEST_ID,
                    session_id: ctx.session_id.clone(),
                    target_status,
                    session_ids,
                    mode: params.mode.clone(),
                    timeout_secs: Some(timeout_secs),
                };

                let socket_timeout = std::time::Duration::from_secs(timeout_secs + 30);

                match send_request_with_timeout(request, Some(socket_timeout)).await {
                    Ok(ServerEvent::CommAwaitMembersResponse {
                        completed,
                        members,
                        summary,
                        ..
                    }) => Ok(format_awaited_members(completed, &summary, &members)),
                    Ok(response) => {
                        ensure_success(&response)?;
                        Ok(ToolOutput::new("Await completed."))
                    }
                    Err(e) => Err(anyhow::anyhow!("Failed to await members: {}", e)),
                }
            }

            _ => Err(anyhow::anyhow!(
                "Unknown action '{}'. Valid actions: share, share_append, read, message, broadcast, dm, channel, list, list_channels, channel_members, \
                 propose_plan, approve_plan, reject_plan, spawn, stop, assign_role, status, plan_status, summary, read_context, \
                 resync_plan, assign_task, assign_next, fill_slots, start, wake, resume, retry, reassign, replace, salvage, subscribe_channel, unsubscribe_channel, await_members",
                params.action
            )),
        }
    }
}

#[cfg(test)]
#[path = "communicate_tests.rs"]
mod tests;
