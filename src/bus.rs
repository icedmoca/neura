use crate::message::ToolCall;
use crate::side_panel::SidePanelSnapshot;
use crate::todo::TodoItem;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToolStatus {
    Running,
    Completed,
    Error,
}

impl ToolStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolStatus::Running => "running",
            ToolStatus::Completed => "completed",
            ToolStatus::Error => "error",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolEvent {
    pub session_id: String,
    pub message_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub status: ToolStatus,
    pub title: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TodoEvent {
    pub session_id: String,
    pub todos: Vec<TodoItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolSummaryState {
    pub status: String,
    pub title: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolSummary {
    pub id: String,
    pub tool: String,
    pub state: ToolSummaryState,
}

/// Status update from a subagent (used by Task tool)
#[derive(Clone, Debug)]
pub struct SubagentStatus {
    pub session_id: String,
    pub status: String, // e.g., "calling API", "running grep", "streaming"
    pub model: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ManualToolCompleted {
    pub session_id: String,
    pub tool_call: ToolCall,
    pub output: String,
    pub is_error: bool,
    pub title: Option<String>,
    pub duration_ms: u64,
}

/// Progress update from a running batch tool call
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchSubcallState {
    Running,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BatchSubcallProgress {
    pub index: usize,
    pub tool_call: crate::message::ToolCall,
    pub state: BatchSubcallState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BatchProgress {
    pub session_id: String,
    /// Parent tool_call_id of the batch call
    pub tool_call_id: String,
    /// Total number of sub-calls in this batch
    pub total: usize,
    /// Number of sub-calls that have completed (success or error)
    pub completed: usize,
    /// Name of the sub-call that just completed
    pub last_completed: Option<String>,
    /// Sub-calls that are currently still running
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub running: Vec<ToolCall>,
    /// Ordered per-subcall progress state for richer UI rendering
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subcalls: Vec<BatchSubcallProgress>,
}

/// Type of file operation for swarm awareness
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum FileOp {
    Read,
    Write,
    Edit,
}

impl FileOp {
    pub fn as_str(&self) -> &'static str {
        match self {
            FileOp::Read => "read",
            FileOp::Write => "wrote",
            FileOp::Edit => "edited",
        }
    }

    pub fn is_modification(&self) -> bool {
        matches!(self, FileOp::Write | FileOp::Edit)
    }
}

/// File touch event for swarm coordination
#[derive(Clone, Debug)]
pub struct FileTouch {
    pub session_id: String,
    pub path: PathBuf,
    pub op: FileOp,
    /// Human-readable summary like "edited lines 45-60" or "read 200 lines"
    pub summary: Option<String>,
    /// Optional compact preview of what changed. Keep this short and already truncated.
    pub detail: Option<String>,
}

/// Status of a background task
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum BackgroundTaskStatus {
    Running,
    Completed,
    Superseded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskProgressKind {
    Determinate,
    Indeterminate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskProgressSource {
    Reported,
    ParsedOutput,
    Heuristic,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackgroundTaskProgress {
    pub kind: BackgroundTaskProgressKind,
    pub percent: Option<f32>,
    pub message: Option<String>,
    pub current: Option<u64>,
    pub total: Option<u64>,
    pub unit: Option<String>,
    pub eta_seconds: Option<u64>,
    pub updated_at: String,
    pub source: BackgroundTaskProgressSource,
}

impl BackgroundTaskProgress {
    pub fn normalize(mut self) -> Self {
        if let (Some(current), Some(total)) = (self.current, self.total)
            && total > 0
            && self.percent.is_none()
        {
            let computed = (current as f64 / total as f64) * 100.0;
            self.percent = Some(((computed * 100.0).round() / 100.0) as f32);
        }

        self.percent = self
            .percent
            .map(|percent| ((percent.clamp(0.0, 100.0) * 100.0).round()) / 100.0);

        if matches!(self.kind, BackgroundTaskProgressKind::Indeterminate)
            && (self.percent.is_some()
                || matches!((self.current, self.total), (_, Some(total)) if total > 0))
        {
            self.kind = BackgroundTaskProgressKind::Determinate;
        }

        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackgroundTaskProgressEvent {
    pub task_id: String,
    pub tool_name: String,
    pub display_name: Option<String>,
    pub session_id: String,
    pub progress: BackgroundTaskProgress,
}

/// Event sent when a background task completes
#[derive(Debug, Clone)]
pub struct BackgroundTaskCompleted {
    pub task_id: String,
    pub tool_name: String,
    pub display_name: Option<String>,
    pub session_id: String,
    pub status: BackgroundTaskStatus,
    pub exit_code: Option<i32>,
    pub output_preview: String,
    pub output_file: PathBuf,
    pub duration_secs: f64,
    pub notify: bool,
    pub wake: bool,
}

#[derive(Clone, Debug)]
pub struct LoginCompleted {
    pub provider: String,
    pub success: bool,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct InputShellCompleted {
    pub session_id: String,
    pub result: crate::message::InputShellResult,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SidePanelUpdated {
    pub session_id: String,
    pub snapshot: SidePanelSnapshot,
}

#[derive(Clone, Debug)]
pub enum UpdateStatus {
    Checking,
    Available { current: String, latest: String },
    Downloading { version: String },
    Installed { version: String },
    UpToDate,
    Error(String),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClientMaintenanceAction {
    Update,
    Rebuild,
}

impl ClientMaintenanceAction {
    pub fn noun(&self) -> &'static str {
        match self {
            Self::Update => "update",
            Self::Rebuild => "rebuild",
        }
    }

    pub fn title(&self) -> &'static str {
        match self {
            Self::Update => "Update",
            Self::Rebuild => "Rebuild",
        }
    }
}

#[derive(Clone, Debug)]
pub enum SessionUpdateStatus {
    Status {
        session_id: String,
        action: ClientMaintenanceAction,
        message: String,
    },
    NoUpdate {
        session_id: String,
        current: String,
    },
    ReadyToReload {
        session_id: String,
        action: ClientMaintenanceAction,
        version: String,
    },
    Error {
        session_id: String,
        action: ClientMaintenanceAction,
        message: String,
    },
}

#[derive(Clone, Debug)]
pub enum BusEvent {
    ToolUpdated(ToolEvent),
    TodoUpdated(TodoEvent),
    SubagentStatus(SubagentStatus),
    ManualToolCompleted(ManualToolCompleted),
    BatchProgress(BatchProgress),
    /// File was touched by an agent (for swarm conflict detection)
    FileTouch(FileTouch),
    /// Background task completed
    BackgroundTaskCompleted(BackgroundTaskCompleted),
    /// Background task reported progress
    BackgroundTaskProgress(BackgroundTaskProgressEvent),
    /// Usage report fetched from providers
    UsageReport(Vec<crate::usage::ProviderUsage>),
    /// Progressive usage report update while providers are still loading
    UsageReportProgress(crate::usage::ProviderUsageProgress),
    /// OAuth/login flow completed in the background
    LoginCompleted(LoginCompleted),
    /// Local `!cmd` shell command completed from the input line
    InputShellCompleted(InputShellCompleted),
    /// Update check status from background thread
    UpdateStatus(UpdateStatus),
    /// Interactive client update status for a specific session
    SessionUpdateStatus(SessionUpdateStatus),
    /// External dictation command completed with transcript text
    DictationCompleted {
        dictation_id: String,
        session_id: Option<String>,
        text: String,
        mode: crate::protocol::TranscriptMode,
    },
    /// External dictation command failed
    DictationFailed {
        dictation_id: String,
        session_id: Option<String>,
        message: String,
    },
    /// Background compaction task finished (check_and_apply should be called)
    CompactionFinished,
    /// Provider's available models list may have changed
    ModelsUpdated,
    /// Side panel pages were updated for a session
    SidePanelUpdated(SidePanelUpdated),
}

pub struct Bus {
    sender: broadcast::Sender<BusEvent>,
}

const MODELS_UPDATED_DEBOUNCE: Duration = Duration::from_millis(750);

#[derive(Default)]
struct ModelsUpdatedPublishState {
    last_published_at: Option<Instant>,
    publish_pending: bool,
}

fn models_updated_publish_state() -> &'static Mutex<ModelsUpdatedPublishState> {
    static STATE: OnceLock<Mutex<ModelsUpdatedPublishState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(ModelsUpdatedPublishState::default()))
}

#[cfg(test)]
pub(crate) fn reset_models_updated_publish_state_for_tests() {
    let mut state = models_updated_publish_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *state = ModelsUpdatedPublishState::default();
}

impl Bus {
    pub fn global() -> &'static Bus {
        static INSTANCE: OnceLock<Bus> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            let (sender, _) = broadcast::channel(256);
            Bus { sender }
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BusEvent> {
        self.sender.subscribe()
    }

    pub fn publish(&self, event: BusEvent) {
        let _ = self.sender.send(event);
    }

    pub fn publish_models_updated(&self) {
        let delay = {
            let now = Instant::now();
            let mut state = models_updated_publish_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match state.last_published_at {
                None => {
                    state.last_published_at = Some(now);
                    None
                }
                Some(last) => {
                    let elapsed = now.saturating_duration_since(last);
                    if elapsed >= MODELS_UPDATED_DEBOUNCE {
                        state.last_published_at = Some(now);
                        None
                    } else if state.publish_pending {
                        return;
                    } else {
                        state.publish_pending = true;
                        Some(MODELS_UPDATED_DEBOUNCE - elapsed)
                    }
                }
            }
        };

        if let Some(delay) = delay {
            let Ok(handle) = tokio::runtime::Handle::try_current() else {
                let mut state = models_updated_publish_state()
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.publish_pending = false;
                state.last_published_at = Some(Instant::now());
                drop(state);
                self.publish(BusEvent::ModelsUpdated);
                return;
            };
            handle.spawn(async move {
                tokio::time::sleep(delay).await;
                let mut state = models_updated_publish_state()
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.publish_pending = false;
                state.last_published_at = Some(Instant::now());
                drop(state);
                Bus::global().publish(BusEvent::ModelsUpdated);
            });
            return;
        }

        self.publish(BusEvent::ModelsUpdated);
    }
}

#[cfg(test)]
mod tests {
    use super::{Bus, BusEvent, reset_models_updated_publish_state_for_tests};
    use tokio::time::{Duration, timeout};

    #[tokio::test]
    async fn models_updated_publishes_are_coalesced() {
        let mut rx = Bus::global().subscribe();
        while rx.try_recv().is_ok() {}

        reset_models_updated_publish_state_for_tests();

        Bus::global().publish_models_updated();
        Bus::global().publish_models_updated();
        Bus::global().publish_models_updated();

        match timeout(Duration::from_secs(1), rx.recv()).await {
            Ok(Ok(BusEvent::ModelsUpdated)) => {}
            other => panic!("expected immediate ModelsUpdated event, got {other:?}"),
        }

        match timeout(Duration::from_secs(2), rx.recv()).await {
            Ok(Ok(BusEvent::ModelsUpdated)) => {}
            other => panic!("expected coalesced delayed ModelsUpdated event, got {other:?}"),
        }

        assert!(
            timeout(Duration::from_millis(300), rx.recv())
                .await
                .is_err()
        );
    }
}
