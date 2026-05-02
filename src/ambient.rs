use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub mod runner;
pub mod scheduler;

use crate::config::config;
use crate::storage;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Context passed from the ambient runner to a visible TUI cycle.
/// Saved to `~/.kcode/ambient/visible_cycle.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisibleCycleContext {
    pub system_prompt: String,
    pub initial_message: String,
}

impl VisibleCycleContext {
    pub fn context_path() -> Result<PathBuf> {
        Ok(storage::kcode_dir()?
            .join("ambient")
            .join("visible_cycle.json"))
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::context_path()?;
        if let Some(parent) = path.parent() {
            storage::ensure_dir(parent)?;
        }
        storage::write_json(&path, self)
    }

    pub fn load() -> Result<Self> {
        let path = Self::context_path()?;
        storage::read_json(&path)
    }

    pub fn result_path() -> Result<PathBuf> {
        Ok(storage::kcode_dir()?
            .join("ambient")
            .join("cycle_result.json"))
    }
}

/// Ambient mode status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum AmbientStatus {
    #[default]
    Idle,
    Running {
        detail: String,
    },
    Scheduled {
        next_wake: DateTime<Utc>,
    },
    Paused {
        reason: String,
    },
    Disabled,
}

/// Priority for scheduled items
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Low,
    Normal,
    High,
}

/// Where a scheduled task should be delivered when it becomes due.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleTarget {
    /// Wake the ambient agent and hand it the queued task.
    #[default]
    Ambient,
    /// Deliver the reminder back into a specific interactive session.
    Session { session_id: String },
    /// Spawn a single new session derived from the originating session.
    Spawn { parent_session_id: String },
}

impl ScheduleTarget {
    pub fn is_direct_delivery(&self) -> bool {
        matches!(self, Self::Session { .. } | Self::Spawn { .. })
    }
}

/// A scheduled ambient task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledItem {
    pub id: String,
    pub scheduled_for: DateTime<Utc>,
    pub context: String,
    pub priority: Priority,
    #[serde(default)]
    pub target: ScheduleTarget,
    pub created_by_session: String,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relevant_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_context: Option<String>,
}

/// Persistent ambient state
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AmbientState {
    pub status: AmbientStatus,
    pub last_run: Option<DateTime<Utc>>,
    pub last_summary: Option<String>,
    pub last_compactions: Option<u32>,
    pub last_memories_modified: Option<u32>,
    pub total_cycles: u64,
}

/// Result from an ambient cycle
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmbientCycleResult {
    pub summary: String,
    pub memories_modified: u32,
    pub compactions: u32,
    pub proactive_work: Option<String>,
    pub next_schedule: Option<ScheduleRequest>,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub status: CycleStatus,
    /// Full conversation transcript (markdown) for email notifications
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CycleStatus {
    Complete,
    Interrupted,
    Incomplete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleRequest {
    pub wake_in_minutes: Option<u32>,
    pub wake_at: Option<DateTime<Utc>>,
    pub context: String,
    pub priority: Priority,
    #[serde(default)]
    pub target: ScheduleTarget,
    #[serde(default)]
    pub created_by_session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relevant_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_context: Option<String>,
}

// ---------------------------------------------------------------------------
// Storage paths
// ---------------------------------------------------------------------------

fn ambient_dir() -> Result<PathBuf> {
    let dir = storage::kcode_dir()?.join("ambient");
    storage::ensure_dir(&dir)?;
    Ok(dir)
}

fn state_path() -> Result<PathBuf> {
    Ok(ambient_dir()?.join("state.json"))
}

fn queue_path() -> Result<PathBuf> {
    Ok(ambient_dir()?.join("queue.json"))
}

fn lock_path() -> Result<PathBuf> {
    Ok(ambient_dir()?.join("ambient.lock"))
}

fn transcripts_dir() -> Result<PathBuf> {
    let dir = ambient_dir()?.join("transcripts");
    storage::ensure_dir(&dir)?;
    Ok(dir)
}

// ---------------------------------------------------------------------------
// User Directives (from email replies)
// ---------------------------------------------------------------------------

/// A user directive received via email reply to an ambient cycle notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserDirective {
    pub id: String,
    pub text: String,
    pub received_at: DateTime<Utc>,
    pub in_reply_to_cycle: String,
    pub consumed: bool,
}

fn directives_path() -> Result<PathBuf> {
    Ok(ambient_dir()?.join("directives.json"))
}

pub fn load_directives() -> Vec<UserDirective> {
    directives_path()
        .ok()
        .and_then(|p| {
            if p.exists() {
                storage::read_json(&p).ok()
            } else {
                None
            }
        })
        .unwrap_or_default()
}

fn save_directives(directives: &[UserDirective]) -> Result<()> {
    storage::write_json(&directives_path()?, directives)
}

/// Store a new directive from an email reply.
pub fn add_directive(text: String, in_reply_to: String) -> Result<()> {
    let mut directives = load_directives();
    directives.push(UserDirective {
        id: format!("dir_{:08x}", rand::random::<u32>()),
        text,
        received_at: Utc::now(),
        in_reply_to_cycle: in_reply_to,
        consumed: false,
    });
    save_directives(&directives)
}

/// Take all unconsumed directives, marking them as consumed.
pub fn take_pending_directives() -> Vec<UserDirective> {
    let mut all = load_directives();
    let pending: Vec<_> = all.iter().filter(|d| !d.consumed).cloned().collect();
    if pending.is_empty() {
        return pending;
    }
    for d in &mut all {
        if !d.consumed {
            d.consumed = true;
        }
    }
    let _ = save_directives(&all);
    pending
}

/// Check if there are any unconsumed directives.
pub fn has_pending_directives() -> bool {
    load_directives().iter().any(|d| !d.consumed)
}

// ---------------------------------------------------------------------------
// AmbientState persistence
// ---------------------------------------------------------------------------

impl AmbientState {
    pub fn load() -> Result<Self> {
        let path = state_path()?;
        if path.exists() {
            storage::read_json(&path)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self) -> Result<()> {
        storage::write_json(&state_path()?, self)
    }

    pub fn record_cycle(&mut self, result: &AmbientCycleResult) {
        self.last_run = Some(result.ended_at);
        self.last_summary = Some(result.summary.clone());
        self.last_compactions = Some(result.compactions);
        self.last_memories_modified = Some(result.memories_modified);
        self.total_cycles += 1;

        match result.status {
            CycleStatus::Complete => {
                if let Some(ref req) = result.next_schedule {
                    let next = req.wake_at.unwrap_or_else(|| {
                        Utc::now()
                            + chrono::Duration::minutes(req.wake_in_minutes.unwrap_or(30) as i64)
                    });
                    self.status = AmbientStatus::Scheduled { next_wake: next };
                } else {
                    self.status = AmbientStatus::Idle;
                }
            }
            CycleStatus::Interrupted | CycleStatus::Incomplete => {
                self.status = AmbientStatus::Idle;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ScheduledQueue
// ---------------------------------------------------------------------------

pub struct ScheduledQueue {
    items: Vec<ScheduledItem>,
    path: PathBuf,
}

impl ScheduledQueue {
    pub fn load(path: PathBuf) -> Self {
        let items: Vec<ScheduledItem> = if path.exists() {
            storage::read_json(&path).unwrap_or_default()
        } else {
            Vec::new()
        };
        Self { items, path }
    }

    pub fn save(&self) -> Result<()> {
        storage::write_json(&self.path, &self.items)
    }

    pub fn push(&mut self, item: ScheduledItem) {
        self.items.push(item);
        let _ = self.save();
    }

    /// Pop items whose `scheduled_for` is in the past, sorted by priority
    /// (highest first) then by time (earliest first).
    pub fn pop_ready(&mut self) -> Vec<ScheduledItem> {
        let now = Utc::now();
        let (ready, remaining): (Vec<_>, Vec<_>) =
            self.items.drain(..).partition(|i| i.scheduled_for <= now);

        self.items = remaining;

        let mut ready = ready;
        // Sort: highest priority first, then earliest scheduled_for
        ready.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.scheduled_for.cmp(&b.scheduled_for))
        });

        if !ready.is_empty() {
            let _ = self.save();
        }

        ready
    }

    /// Remove and return ready items targeted at a specific direct-delivery session,
    /// leaving ambient-targeted queue items intact for the ambient agent to process.
    pub fn take_ready_direct_items(&mut self) -> Vec<ScheduledItem> {
        let now = Utc::now();
        let mut ready_direct = Vec::new();
        let mut remaining = Vec::with_capacity(self.items.len());

        for item in self.items.drain(..) {
            let is_ready = item.scheduled_for <= now;
            let is_direct_target = item.target.is_direct_delivery();
            if is_ready && is_direct_target {
                ready_direct.push(item);
            } else {
                remaining.push(item);
            }
        }

        self.items = remaining;

        if !ready_direct.is_empty() {
            let _ = self.save();
        }

        ready_direct.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.scheduled_for.cmp(&b.scheduled_for))
        });

        ready_direct
    }

    pub fn peek_next(&self) -> Option<&ScheduledItem> {
        self.items.iter().min_by_key(|i| i.scheduled_for)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn items(&self) -> &[ScheduledItem] {
        &self.items
    }
}

// ---------------------------------------------------------------------------
// AmbientLock  (single-instance guard)
// ---------------------------------------------------------------------------

pub struct AmbientLock {
    lock_path: PathBuf,
}

impl AmbientLock {
    /// Try to acquire the ambient lock.
    /// Returns `Ok(Some(lock))` if acquired, `Ok(None)` if another instance
    /// already holds it, or `Err` on I/O failure.
    pub fn try_acquire() -> Result<Option<Self>> {
        let path = lock_path()?;

        // Check existing lock
        if path.exists() {
            if let Ok(contents) = std::fs::read_to_string(&path)
                && let Ok(pid) = contents.trim().parse::<u32>()
                && is_pid_alive(pid)
            {
                return Ok(None); // Another instance is running
            }
            let _ = std::fs::remove_file(&path);
        }

        // Write our PID
        let pid = std::process::id();
        if let Some(parent) = path.parent() {
            storage::ensure_dir(parent)?;
        }
        std::fs::write(&path, pid.to_string())?;

        Ok(Some(Self { lock_path: path }))
    }

    pub fn release(self) -> Result<()> {
        let _ = std::fs::remove_file(&self.lock_path);
        // Drop runs, but we already cleaned up
        std::mem::forget(self);
        Ok(())
    }
}

impl Drop for AmbientLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

fn is_pid_alive(pid: u32) -> bool {
    crate::platform::is_process_running(pid)
}

// ---------------------------------------------------------------------------
// AmbientManager
// ---------------------------------------------------------------------------

pub struct AmbientManager {
    state: AmbientState,
    queue: ScheduledQueue,
}

impl AmbientManager {
    pub fn new() -> Result<Self> {
        // Ensure storage layout exists
        let _ = ambient_dir()?;
        let _ = transcripts_dir()?;

        let state = AmbientState::load()?;
        let queue = ScheduledQueue::load(queue_path()?);

        Ok(Self { state, queue })
    }

    pub fn is_enabled() -> bool {
        config().ambient.enabled
    }

    /// Check whether it's time to run a cycle based on current state and queue.
    pub fn should_run(&self) -> bool {
        if !Self::is_enabled() {
            return false;
        }

        match &self.state.status {
            AmbientStatus::Disabled | AmbientStatus::Paused { .. } => false,
            AmbientStatus::Running { .. } => false, // already running
            AmbientStatus::Idle => true,
            AmbientStatus::Scheduled { next_wake } => Utc::now() >= *next_wake,
        }
    }

    pub fn record_cycle_result(&mut self, result: AmbientCycleResult) -> Result<()> {
        self.state.record_cycle(&result);
        self.state.save()?;

        // If the cycle produced a schedule request, enqueue it
        if let Some(ref req) = result.next_schedule {
            self.schedule(req.clone())?;
        }

        Ok(())
    }

    /// Remove and return all ready scheduled items.
    pub fn take_ready_items(&mut self) -> Vec<ScheduledItem> {
        self.queue.pop_ready()
    }

    /// Remove and return only ready items targeted at direct delivery into a
    /// specific resumed or spawned session.
    pub fn take_ready_direct_items(&mut self) -> Vec<ScheduledItem> {
        self.queue.take_ready_direct_items()
    }

    /// Add a schedule request to the queue. Returns the item ID.
    pub fn schedule(&mut self, request: ScheduleRequest) -> Result<String> {
        let id = format!("sched_{:08x}", rand::random::<u32>());
        let scheduled_for = request.wake_at.unwrap_or_else(|| {
            Utc::now() + chrono::Duration::minutes(request.wake_in_minutes.unwrap_or(30) as i64)
        });

        let item = ScheduledItem {
            id: id.clone(),
            scheduled_for,
            context: request.context,
            priority: request.priority,
            target: request.target,
            created_by_session: request.created_by_session,
            created_at: Utc::now(),
            working_dir: request.working_dir,
            task_description: request.task_description,
            relevant_files: request.relevant_files,
            git_branch: request.git_branch,
            additional_context: request.additional_context,
        };

        self.queue.push(item);
        Ok(id)
    }

    pub fn state(&self) -> &AmbientState {
        &self.state
    }

    pub fn queue(&self) -> &ScheduledQueue {
        &self.queue
    }
}

// ---------------------------------------------------------------------------
// Ambient System Prompt Builder
// ---------------------------------------------------------------------------

/// Health stats for the memory graph, used in the ambient system prompt.
#[derive(Debug, Clone, Default)]
pub struct MemoryGraphHealth {
    pub total: usize,
    pub active: usize,
    pub inactive: usize,
    pub low_confidence: usize,
    pub contradictions: usize,
    pub missing_embeddings: usize,
    pub duplicate_candidates: usize,
    pub last_consolidation: Option<DateTime<Utc>>,
}

/// Summary of a recent session for the ambient prompt.
#[derive(Debug, Clone)]
pub struct RecentSessionInfo {
    pub id: String,
    pub status: String,
    pub topic: Option<String>,
    pub duration_secs: i64,
    pub extraction_status: String,
}

/// Resource budget info for the ambient prompt.
#[derive(Debug, Clone, Default)]
pub struct ResourceBudget {
    pub provider: String,
    pub tokens_remaining_desc: String,
    pub window_resets_desc: String,
    pub user_usage_rate_desc: String,
    pub cycle_budget_desc: String,
}

/// Gather memory graph health stats from the MemoryManager.
pub fn gather_memory_graph_health(
    memory_manager: &crate::memory::MemoryManager,
) -> MemoryGraphHealth {
    let mut health = MemoryGraphHealth::default();

    // Accumulate stats from project + global graphs
    for graph in [
        memory_manager.load_project_graph(),
        memory_manager.load_global_graph(),
    ]
    .into_iter()
    .flatten()
    {
        let active_count = graph.memories.values().filter(|m| m.active).count();
        let inactive_count = graph.memories.values().filter(|m| !m.active).count();
        health.total += graph.memories.len();
        health.active += active_count;
        health.inactive += inactive_count;

        // Low confidence: effective confidence < 0.1
        health.low_confidence += graph
            .memories
            .values()
            .filter(|m| m.active && m.effective_confidence() < 0.1)
            .count();

        // Missing embeddings
        health.missing_embeddings += graph
            .memories
            .values()
            .filter(|m| m.active && m.embedding.is_none())
            .count();

        // Count contradiction edges
        for edges in graph.edges.values() {
            for edge in edges {
                if matches!(edge.kind, crate::memory_graph::EdgeKind::Contradicts) {
                    health.contradictions += 1;
                }
            }
        }

        // Use last_cluster_update as a proxy for last consolidation
        if let Some(ts) = graph.metadata.last_cluster_update {
            match health.last_consolidation {
                Some(existing) if ts > existing => health.last_consolidation = Some(ts),
                None => health.last_consolidation = Some(ts),
                _ => {}
            }
        }
    }

    // Contradicts edges are bidirectional, so divide by 2
    health.contradictions /= 2;

    // Duplicate candidates would require embedding similarity scan;
    // placeholder for now — ambient agent will discover them during its cycle.
    health.duplicate_candidates = 0;

    health
}

/// Gather feedback memories relevant to ambient mode.
///
/// Pulls from two sources:
/// 1. Recent ambient transcripts (summaries of past cycles)
/// 2. Memory graph entries tagged "ambient" or "system"
///
/// Returns formatted strings for inclusion in the ambient system prompt.
pub fn gather_feedback_memories(memory_manager: &crate::memory::MemoryManager) -> Vec<String> {
    let mut feedback = Vec::new();

    // --- Source 1: Recent ambient transcripts ---
    let transcripts_dir = match crate::storage::kcode_dir() {
        Ok(d) => d.join("ambient").join("transcripts"),
        Err(_) => return feedback,
    };

    if transcripts_dir.exists()
        && let Ok(dir) = std::fs::read_dir(&transcripts_dir)
    {
        let mut files: Vec<_> = dir.flatten().collect();
        // Sort by filename descending (most recent first)
        files.sort_by_key(|entry| std::cmp::Reverse(entry.file_name()));
        // Only look at the last 5 transcripts
        files.truncate(5);

        for entry in files {
            if let Ok(content) = std::fs::read_to_string(entry.path())
                && let Ok(transcript) =
                    serde_json::from_str::<crate::safety::AmbientTranscript>(&content)
            {
                let status = format!("{:?}", transcript.status);
                let summary = transcript.summary.as_deref().unwrap_or("no summary");
                let age = format_duration_rough(Utc::now() - transcript.started_at);
                feedback.push(format!(
                    "Past cycle ({} ago, {}): {} memories modified, {} compactions — {}",
                    age,
                    status.to_lowercase(),
                    transcript.memories_modified,
                    transcript.compactions,
                    summary,
                ));
            }
        }
    }

    // --- Source 2: Memory graph entries tagged "ambient" or "system" ---
    for graph in [
        memory_manager.load_project_graph(),
        memory_manager.load_global_graph(),
    ]
    .into_iter()
    .flatten()
    {
        for memory in graph.memories.values() {
            if !memory.active {
                continue;
            }
            let has_ambient_tag = memory.tags.iter().any(|t| t == "ambient" || t == "system");
            if has_ambient_tag {
                feedback.push(format!("Memory [{}]: {}", memory.id, memory.content));
            }
        }
    }

    feedback
}

/// Gather recent sessions since a given timestamp.
pub fn gather_recent_sessions(since: Option<DateTime<Utc>>) -> Vec<RecentSessionInfo> {
    let sessions_dir = match crate::storage::kcode_dir() {
        Ok(d) => d.join("sessions"),
        Err(_) => return Vec::new(),
    };
    if !sessions_dir.exists() {
        return Vec::new();
    }

    let cutoff = since.unwrap_or_else(|| Utc::now() - chrono::Duration::hours(24));

    let mut recent = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&sessions_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false)
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                && let Ok(session) = crate::session::Session::load(stem)
            {
                // Skip debug sessions
                if session.is_debug {
                    continue;
                }
                // Only include sessions updated after cutoff
                if session.updated_at < cutoff {
                    continue;
                }
                let duration = (session.updated_at - session.created_at)
                    .num_seconds()
                    .max(0);
                let extraction = if session.messages.is_empty() {
                    "no messages"
                } else {
                    // Heuristic: if session closed normally, assume extracted
                    match &session.status {
                        crate::session::SessionStatus::Closed => "extracted",
                        crate::session::SessionStatus::Crashed { .. } => "missed",
                        crate::session::SessionStatus::Active => "in progress",
                        _ => "unknown",
                    }
                };
                recent.push(RecentSessionInfo {
                    id: session.id.clone(),
                    status: session.status.display().to_string(),
                    topic: session.title.clone(),
                    duration_secs: duration,
                    extraction_status: extraction.to_string(),
                });
            }
        }
    }

    // Sort by most recent first (we don't have created_at easily, sort by id which embeds timestamp)
    recent.sort_by(|a, b| b.id.cmp(&a.id));
    recent.truncate(20); // Cap at 20 to keep prompt reasonable
    recent
}

/// Build the dynamic system prompt for an ambient cycle.
///
/// Populates the template from AMBIENT_MODE.md with real data from the
/// current state, queue, memory graph, sessions, and resource budget.
pub fn build_ambient_system_prompt(
    state: &AmbientState,
    queue: &[ScheduledItem],
    graph_health: &MemoryGraphHealth,
    recent_sessions: &[RecentSessionInfo],
    feedback_memories: &[String],
    budget: &ResourceBudget,
    active_user_sessions: usize,
) -> String {
    let mut prompt = String::with_capacity(4096);

    prompt.push_str(
        "You are the ambient agent for kcode. You operate autonomously without \
         user prompting. Your job is to maintain and improve the user's \
         development environment.\n\n",
    );

    // --- Current State ---
    prompt.push_str("## Current State\n");
    if let Some(last_run) = state.last_run {
        let ago = Utc::now() - last_run;
        let ago_str = format_duration_rough(ago);
        prompt.push_str(&format!(
            "- Last ambient cycle: {} ({} ago)\n",
            last_run.format("%Y-%m-%d %H:%M UTC"),
            ago_str,
        ));
    } else {
        prompt.push_str("- Last ambient cycle: never (first run)\n");
    }
    if active_user_sessions > 0 {
        prompt.push_str(&format!(
            "- Active user sessions: {}\n",
            active_user_sessions
        ));
    } else {
        prompt.push_str("- Active user sessions: none\n");
    }
    prompt.push_str(&format!(
        "- Total cycles completed: {}\n",
        state.total_cycles
    ));
    prompt.push('\n');

    // --- Scheduled Queue ---
    prompt.push_str("## Scheduled Queue\n");
    if queue.is_empty() {
        prompt.push_str("Empty -- do general ambient work.\n");
    } else {
        for item in queue {
            let age = Utc::now() - item.created_at;
            let priority = match item.priority {
                Priority::Low => "low",
                Priority::Normal => "normal",
                Priority::High => "HIGH",
            };
            prompt.push_str(&format!(
                "- [{}] {} (scheduled {} ago, priority: {})\n",
                item.id,
                item.context,
                format_duration_rough(age),
                priority,
            ));
            match &item.target {
                ScheduleTarget::Ambient => {}
                ScheduleTarget::Session { session_id } => {
                    prompt.push_str(&format!("  Target session: {}\n", session_id));
                }
                ScheduleTarget::Spawn { parent_session_id } => {
                    prompt.push_str(&format!("  Spawn from session: {}\n", parent_session_id));
                }
            }
            if let Some(ref dir) = item.working_dir {
                prompt.push_str(&format!("  Working dir: {}\n", dir));
            }
            if let Some(ref desc) = item.task_description {
                prompt.push_str(&format!("  Details: {}\n", desc));
            }
            if !item.relevant_files.is_empty() {
                prompt.push_str(&format!("  Files: {}\n", item.relevant_files.join(", ")));
            }
            if let Some(ref branch) = item.git_branch {
                prompt.push_str(&format!("  Branch: {}\n", branch));
            }
            if let Some(ref ctx) = item.additional_context {
                for line in ctx.lines() {
                    prompt.push_str(&format!("  {}\n", line));
                }
            }
        }
    }
    prompt.push('\n');

    // --- Recent Sessions ---
    prompt.push_str("## Recent Sessions (since last cycle)\n");
    if recent_sessions.is_empty() {
        prompt.push_str("No sessions since last cycle.\n");
    } else {
        for s in recent_sessions {
            let topic = s.topic.as_deref().unwrap_or("(no title)");
            let dur = format_duration_rough(chrono::Duration::seconds(s.duration_secs));
            prompt.push_str(&format!(
                "- {} | {} | {} | {} | extraction: {}\n",
                s.id, s.status, dur, topic, s.extraction_status,
            ));
        }
    }
    prompt.push('\n');

    // --- Memory Graph Health ---
    prompt.push_str("## Memory Graph Health\n");
    prompt.push_str(&format!(
        "- Total memories: {} ({} active, {} inactive)\n",
        graph_health.total, graph_health.active, graph_health.inactive,
    ));
    prompt.push_str(&format!(
        "- Memories with confidence < 0.1: {}\n",
        graph_health.low_confidence,
    ));
    prompt.push_str(&format!(
        "- Unresolved contradictions: {}\n",
        graph_health.contradictions,
    ));
    prompt.push_str(&format!(
        "- Memories without embeddings: {}\n",
        graph_health.missing_embeddings,
    ));
    if graph_health.duplicate_candidates > 0 {
        prompt.push_str(&format!(
            "- Duplicate candidates (similarity > 0.95): {}\n",
            graph_health.duplicate_candidates,
        ));
    } else {
        prompt.push_str("- Duplicate candidates: run embedding scan to detect\n");
    }
    if let Some(ts) = graph_health.last_consolidation {
        let ago = format_duration_rough(Utc::now() - ts);
        prompt.push_str(&format!("- Last consolidation: {} ago\n", ago));
    } else {
        prompt.push_str("- Last consolidation: never\n");
    }
    prompt.push('\n');

    // --- User Feedback History ---
    prompt.push_str("## User Feedback History\n");
    if feedback_memories.is_empty() {
        prompt.push_str("No feedback memories found about ambient mode yet.\n");
    } else {
        for mem in feedback_memories {
            prompt.push_str(&format!("- {}\n", mem));
        }
    }
    prompt.push('\n');

    // --- Resource Budget ---
    prompt.push_str("## Resource Budget\n");
    prompt.push_str(&format!("- Provider: {}\n", budget.provider));
    prompt.push_str(&format!(
        "- Tokens remaining in window: {}\n",
        budget.tokens_remaining_desc,
    ));
    prompt.push_str(&format!("- Window resets: {}\n", budget.window_resets_desc));
    prompt.push_str(&format!(
        "- User usage rate: {}\n",
        budget.user_usage_rate_desc,
    ));
    prompt.push_str(&format!(
        "- Budget for this cycle: {}\n",
        budget.cycle_budget_desc,
    ));
    prompt.push('\n');

    // --- User Directives (from email/Telegram replies) ---
    let pending_directives = take_pending_directives();
    if !pending_directives.is_empty() {
        prompt.push_str("## User Directives (from replies)\n");
        prompt.push_str(
            "The user replied to ambient notifications with these instructions. \
             Address them as your **top priority** this cycle.\n\n",
        );
        for dir in &pending_directives {
            let ago = format_duration_rough(Utc::now() - dir.received_at);
            prompt.push_str(&format!(
                "- [reply to cycle {}] ({} ago): {}\n",
                dir.in_reply_to_cycle, ago, dir.text,
            ));
        }
        prompt.push('\n');
    }

    // --- Instructions ---
    prompt.push_str(
        "## Instructions\n\n\
         Start by using the todos tool to plan what you'll do this cycle.\n\n\
         Priority order:\n\
         1. Execute any scheduled queue items first.\n\
         2. Garden the memory graph -- consolidate duplicates, resolve \
            contradictions, prune dead memories, verify stale facts, \
            extract from missed sessions.\n\
         3. Scout for proactive work (only if enabled and past cold start) -- \
            look at recent sessions and git history to identify useful work \
            the user would appreciate.\n\n\
         For gardening: focus on highest-value maintenance first. Duplicates \
         and contradictions before pruning. Verify stale facts only if you \
         have budget left.\n\n\
         For proactive work: be conservative. A bad surprise is worse than \
         no surprise. Check the user feedback memories -- if they've rejected \
         similar work before, don't do it. Code changes must go on a worktree \
         branch with a PR via request_permission.\n\n\
         Every request_permission call must be reviewer-ready. Include:\n\
         - description: concise summary of what you are about to do\n\
         - rationale: why approval is needed right now\n\
         - context.summary: what you are working on in this cycle\n\
         - context.why_permission_needed: explicit justification for permission\n\
         - context.planned_steps, context.files, context.commands (if known)\n\
         - context.risks and context.rollback_plan (if relevant)\n\n\
         Good sources for scouting proactive work:\n\
         - Todoist (via MCP) — check for relevant tasks and deadlines\n\
         - Canvas (via MCP) — check for upcoming assignments or deadlines\n\
         - Git history — recent commits, open branches, stale PRs\n\
         - Session history — patterns in what the user works on\n\n\
         When done, you MUST call end_ambient_cycle with a summary of \
         everything you did, including compaction count. Always schedule \
         your next wake time with context for what you plan to do next.\n\n\
         ## Messaging Check-ins\n\n\
         You have a `send_message` tool. Use it to keep the user informed \
         about what you're doing. Send a brief message when you start a cycle \
         and when you finish significant work. Keep messages short and useful — \
         the user should be able to glance at their messages and know what's happening \
         without opening kcode. You can optionally target a specific channel \
         (e.g. telegram, discord) or omit channel to send to all.\n",
    );

    prompt
}

pub fn format_scheduled_session_message(item: &ScheduledItem) -> String {
    let mut lines = vec![
        "[Scheduled task]".to_string(),
        "A scheduled task for this session is now due.".to_string(),
        String::new(),
        format!(
            "Task: {}",
            item.task_description.as_deref().unwrap_or(&item.context)
        ),
    ];

    if let Some(ref dir) = item.working_dir {
        lines.push(format!("Working directory: {}", dir));
    }
    if !item.relevant_files.is_empty() {
        lines.push(format!(
            "Relevant files: {}",
            item.relevant_files.join(", ")
        ));
    }
    if let Some(ref branch) = item.git_branch {
        lines.push(format!("Branch: {}", branch));
    }
    if let Some(ref ctx) = item.additional_context {
        lines.push(String::new());
        lines.push(ctx.clone());
    }

    lines.join("\n")
}

/// Format a chrono::Duration into a rough human-readable string.
fn format_duration_rough(d: chrono::Duration) -> String {
    let secs = d.num_seconds().max(0);
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m > 0 {
            format!("{}h {}m", h, m)
        } else {
            format!("{}h", h)
        }
    } else {
        let days = secs / 86400;
        format!("{}d", days)
    }
}

/// Format a number of minutes into a human-friendly string.
/// E.g. 5 → "5m", 90 → "1h 30m", 370 → "6h 10m", 1500 → "1d 1h"
pub fn format_minutes_human(mins: u32) -> String {
    if mins < 60 {
        format!("{}m", mins)
    } else if mins < 1440 {
        let h = mins / 60;
        let m = mins % 60;
        if m > 0 {
            format!("{}h {}m", h, m)
        } else {
            format!("{}h", h)
        }
    } else {
        let d = mins / 1440;
        let h = (mins % 1440) / 60;
        if h > 0 {
            format!("{}d {}h", d, h)
        } else {
            format!("{}d", d)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "ambient_tests.rs"]
mod ambient_tests;
