use std::time::Instant;

/// Represents current memory system activity.
#[derive(Debug, Clone)]
pub struct MemoryActivity {
    /// Current state of the memory system.
    pub state: MemoryState,
    /// When the current state was entered, used for elapsed time display and staleness detection.
    pub state_since: Instant,
    /// Pipeline progress for the per-turn search, verify, inject, maintain flow.
    pub pipeline: Option<PipelineState>,
    /// Recent events, most recent first.
    pub recent_events: Vec<MemoryEvent>,
}

impl MemoryActivity {
    pub fn is_processing(&self) -> bool {
        !matches!(self.state, MemoryState::Idle)
            || self
                .pipeline
                .as_ref()
                .map(PipelineState::has_running_step)
                .unwrap_or(false)
    }
}

/// Status of a single pipeline step.
#[derive(Debug, Clone, PartialEq)]
pub enum StepStatus {
    Pending,
    Running,
    Done,
    Error,
    Skipped,
}

/// Result data for a completed pipeline step.
#[derive(Debug, Clone)]
pub struct StepResult {
    pub summary: String,
    pub latency_ms: u64,
}

/// Tracks the 4-step per-turn memory pipeline: search, verify, inject, maintain.
#[derive(Debug, Clone)]
pub struct PipelineState {
    pub search: StepStatus,
    pub search_result: Option<StepResult>,
    pub verify: StepStatus,
    pub verify_result: Option<StepResult>,
    pub verify_progress: Option<(usize, usize)>,
    pub inject: StepStatus,
    pub inject_result: Option<StepResult>,
    pub maintain: StepStatus,
    pub maintain_result: Option<StepResult>,
    pub started_at: Instant,
}

impl PipelineState {
    pub fn new() -> Self {
        Self {
            search: StepStatus::Pending,
            search_result: None,
            verify: StepStatus::Pending,
            verify_result: None,
            verify_progress: None,
            inject: StepStatus::Pending,
            inject_result: None,
            maintain: StepStatus::Pending,
            maintain_result: None,
            started_at: Instant::now(),
        }
    }

    pub fn is_complete(&self) -> bool {
        matches!(
            (&self.search, &self.verify, &self.inject, &self.maintain),
            (
                StepStatus::Done | StepStatus::Error | StepStatus::Skipped,
                StepStatus::Done | StepStatus::Error | StepStatus::Skipped,
                StepStatus::Done | StepStatus::Error | StepStatus::Skipped,
                StepStatus::Done | StepStatus::Error | StepStatus::Skipped,
            )
        )
    }

    pub fn has_running_step(&self) -> bool {
        matches!(self.search, StepStatus::Running)
            || matches!(self.verify, StepStatus::Running)
            || matches!(self.inject, StepStatus::Running)
            || matches!(self.maintain, StepStatus::Running)
    }
}

impl Default for PipelineState {
    fn default() -> Self {
        Self::new()
    }
}

/// State of the memory sidecar.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum MemoryState {
    /// Idle, no activity.
    #[default]
    Idle,
    /// Running embedding search.
    Embedding,
    /// Sidecar checking relevance.
    SidecarChecking { count: usize },
    /// Found relevant memories.
    FoundRelevant { count: usize },
    /// Extracting memories from conversation.
    Extracting { reason: String },
    /// Background maintenance or gardening of the memory graph.
    Maintaining { phase: String },
    /// Agent is actively using a memory tool.
    ToolAction { action: String, detail: String },
}

/// A memory system event.
#[derive(Debug, Clone)]
pub struct MemoryEvent {
    /// Type of event.
    pub kind: MemoryEventKind,
    /// When it happened.
    pub timestamp: Instant,
    /// Optional details.
    pub detail: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InjectedMemoryItem {
    pub section: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub enum MemoryEventKind {
    /// Embedding search started.
    EmbeddingStarted,
    /// Embedding search completed.
    EmbeddingComplete { latency_ms: u64, hits: usize },
    /// Sidecar started checking.
    SidecarStarted,
    /// Sidecar found memory relevant.
    SidecarRelevant { memory_preview: String },
    /// Sidecar found memory not relevant.
    SidecarNotRelevant,
    /// Sidecar call completed with latency.
    SidecarComplete { latency_ms: u64 },
    /// Memory was surfaced to main agent.
    MemorySurfaced { memory_preview: String },
    /// Memory payload was injected into model context.
    MemoryInjected {
        count: usize,
        prompt_chars: usize,
        age_ms: u64,
        preview: String,
        items: Vec<InjectedMemoryItem>,
    },
    /// Background maintenance started.
    MaintenanceStarted { verified: usize, rejected: usize },
    /// Background maintenance discovered or strengthened links.
    MaintenanceLinked { links: usize },
    /// Background maintenance adjusted confidence.
    MaintenanceConfidence { boosted: usize, decayed: usize },
    /// Background maintenance refined clusters.
    MaintenanceCluster { clusters: usize, members: usize },
    /// Background maintenance inferred or applied a shared tag.
    MaintenanceTagInferred { tag: String, applied: usize },
    /// Background maintenance detected a gap.
    MaintenanceGap { candidates: usize },
    /// Background maintenance completed.
    MaintenanceComplete { latency_ms: u64 },
    /// Extraction started.
    ExtractionStarted { reason: String },
    /// Extraction completed.
    ExtractionComplete { count: usize },
    /// Error occurred.
    Error { message: String },
    /// Agent stored a memory via tool.
    ToolRemembered {
        content: String,
        scope: String,
        category: String,
    },
    /// Agent recalled or searched memories via tool.
    ToolRecalled { query: String, count: usize },
    /// Agent forgot a memory via tool.
    ToolForgot { id: String },
    /// Agent tagged a memory via tool.
    ToolTagged { id: String, tags: String },
    /// Agent linked memories via tool.
    ToolLinked { from: String, to: String },
    /// Agent listed memories via tool.
    ToolListed { count: usize },
}
