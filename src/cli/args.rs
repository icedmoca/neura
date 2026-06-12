use clap::{Parser, Subcommand, ValueEnum};

use super::provider_init::ProviderChoice;

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum TranscriptModeArg {
    Insert,
    Append,
    Replace,
    Send,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum GoogleAccessTierArg {
    Full,
    Readonly,
}

#[derive(Parser, Debug)]
#[command(name = "kcode")]
#[command(version = env!("KCODE_VERSION"))]
#[command(about = "J-Code: A coding agent using Claude Max or ChatGPT Pro subscriptions")]
pub(crate) struct Args {
    /// Provider to use (kcode, claude, openai, openrouter, azure, opencode, opencode-go, zai, 302ai, baseten, cortecs, deepseek, firmware, huggingface, moonshotai, nebius, scaleway, stackit, groq, mistral, perplexity, togetherai, deepinfra, xai, lmstudio, ollama, chutes, cerebras, alibaba-coding-plan, openai-compatible, cursor, copilot, gemini, antigravity, google, or auto-detect)
    #[arg(short, long, default_value = "auto", global = true)]
    pub(crate) provider: ProviderChoice,

    /// Working directory
    #[arg(short = 'C', long, global = true)]
    pub(crate) cwd: Option<String>,

    /// Skip the automatic update check
    #[arg(long, global = true)]
    pub(crate) no_update: bool,

    /// Auto-update when new version is available (default: true for release builds)
    #[arg(long, global = true, default_value = "true")]
    pub(crate) auto_update: bool,

    /// Log tool inputs/outputs and token usage to stderr
    #[arg(long, global = true)]
    pub(crate) trace: bool,

    /// Suppress non-error CLI/status output for scripting and wrappers
    #[arg(long, global = true)]
    pub(crate) quiet: bool,

    /// Resume a session by ID, or list sessions if no ID provided
    #[arg(long, global = true, num_args = 0..=1, default_missing_value = "")]
    pub(crate) resume: Option<String>,

    /// Internal: launched as a freshly spawned window, so skip heavy local resume bootstrap.
    #[arg(long, global = true, hide = true)]
    pub(crate) fresh_spawn: bool,

    /// DEPRECATED: Run standalone TUI without connecting to server.
    /// The default mode is now always client/server (even for self-dev).
    /// Standalone mode is missing features like graceful cancel with partial
    /// content preservation on the server side. Will be removed in a future version.
    #[arg(long, global = true, hide = true)]
    #[deprecated = "Use default client/server mode instead"]
    pub(crate) standalone: bool,

    /// Disable auto-detection of kcode repository and self-dev mode
    #[arg(long, global = true)]
    pub(crate) no_selfdev: bool,

    /// Custom socket path for server/client communication
    #[arg(long, global = true)]
    pub(crate) socket: Option<String>,

    /// Enable debug socket (broadcasts all TUI state changes)
    #[arg(long, global = true)]
    pub(crate) debug_socket: bool,

    /// Model to use (e.g., claude-opus-4-6, gpt-5.5)
    #[arg(short, long, global = true)]
    pub(crate) model: Option<String>,

    /// Named provider profile from [providers.<name>] in config.toml.
    /// Implies --provider openai-compatible for OpenAI-compatible profiles.
    #[arg(long, global = true)]
    pub(crate) provider_profile: Option<String>,

    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Command {
    /// Start the agent server (background daemon)
    Serve,

    /// Connect to a running server
    Connect,

    /// Run a single message and exit
    Run {
        /// Emit a machine-readable JSON result instead of streaming text
        #[arg(long, conflicts_with = "ndjson")]
        json: bool,

        /// Emit newline-delimited JSON events while the response streams
        #[arg(long, conflicts_with = "json")]
        ndjson: bool,

        /// The message to send
        message: String,
    },

    /// Login to a provider via OAuth
    Login {
        /// Account label for multi-account support (stored labels are auto-numbered)
        #[arg(long, short = 'a')]
        account: Option<String>,

        /// Do not try to open a browser locally. Useful over SSH or on headless machines.
        #[arg(long, alias = "headless")]
        no_browser: bool,

        /// Print a script-friendly auth URL and persist temporary login state for later completion.
        #[arg(long, conflicts_with_all = ["callback_url", "auth_code"])]
        print_auth_url: bool,

        /// Complete a previously printed auth flow using a full callback URL or query string.
        #[arg(long, conflicts_with = "auth_code")]
        callback_url: Option<String>,

        /// Complete a previously printed auth flow using a provider-issued authorization code.
        #[arg(long, conflicts_with = "callback_url")]
        auth_code: Option<String>,

        /// Emit machine-readable JSON for script-friendly login flows.
        #[arg(long)]
        json: bool,

        /// Resume a pending scriptable login flow that does not require callback/code input.
        #[arg(long, conflicts_with_all = ["print_auth_url", "callback_url", "auth_code"])]
        complete: bool,

        /// Gmail/Google access tier for non-interactive flows. Defaults to full.
        #[arg(long, value_enum)]
        google_access_tier: Option<GoogleAccessTierArg>,
    },

    /// Run in simple REPL mode (no TUI)
    Repl,

    /// Update kcode to the latest version
    Update,

    /// Show build/version information in human or JSON form
    Version {
        /// Emit JSON instead of plain text
        #[arg(long)]
        json: bool,
    },

    /// Show usage limits for connected providers
    Usage {
        /// Emit JSON instead of plain text
        #[arg(long)]
        json: bool,
    },

    /// Self-development mode: run as a canary session on the shared server
    #[command(alias = "selfdev")]
    SelfDev {
        /// Build and test a new canary version before launching
        #[arg(long)]
        build: bool,
    },

    /// Debug socket CLI - interact with running kcode server
    Debug {
        /// Debug command to run (list, start, sessions, create_session, message, tool, state, history, etc.)
        #[arg(default_value = "help")]
        command: String,

        /// Optional argument for the command
        #[arg(default_value = "")]
        arg: String,

        /// Target a specific session by ID
        #[arg(short = 'S', long)]
        session: Option<String>,

        /// Connect to specific server socket path
        #[arg(short = 's', long)]
        socket: Option<String>,

        /// Wait for response to complete (for message command)
        #[arg(short, long)]
        wait: bool,
    },

    /// Authentication status and validation helpers
    #[command(subcommand)]
    Auth(AuthCommand),

    /// Provider discovery and selection helpers
    #[command(subcommand)]
    Provider(ProviderCommand),

    /// Inspect dynamic latent operational recurrence state
    #[command(subcommand, name = "kcode-latent")]
    Latent(LatentCommand),

    /// Run evidence-ranked bounded self-improvement commands
    #[command(subcommand, name = "kcode-self-improve")]
    SelfImprove(SelfImproveCommand),

    /// Memory management commands
    #[command(subcommand)]
    Memory(MemoryCommand),

    /// Ambient mode management
    #[command(subcommand)]
    Ambient(AmbientCommand),

    /// Generate a pairing code for iOS/web client
    Pair {
        /// List paired devices instead of generating a code
        #[arg(long)]
        list: bool,

        /// Revoke a paired device by name or ID
        #[arg(long)]
        revoke: Option<String>,
    },

    /// Review and respond to pending ambient permission requests
    Permissions,

    /// Inject externally transcribed text into the active Kcode TUI
    Transcript {
        /// Transcript text. If omitted, reads from stdin.
        text: Option<String>,

        /// How to apply the transcript inside Kcode
        #[arg(long, value_enum, default_value = "send")]
        mode: TranscriptModeArg,

        /// Target a specific live session instead of the active TUI
        #[arg(short = 'S', long)]
        session: Option<String>,
    },

    /// Run configured dictation: send to last-focused kcode client or type raw text
    Dictate {
        /// Type the transcript into the focused app instead of sending to kcode
        #[arg(long)]
        r#type: bool,
    },

    /// Set up a global hotkey (Alt+;) to launch kcode
    SetupHotkey {
        /// Internal: run as the macOS hotkey listener process.
        #[arg(long, hide = true)]
        listen_macos_hotkey: bool,
    },

    /// Install a launcher so kcode appears in your app launcher
    SetupLauncher,

    /// Browser automation setup and status
    Browser {
        /// Action (setup, status)
        #[arg(default_value = "setup")]
        action: String,
    },

    /// Replay a saved session in the TUI
    Replay {
        /// Session ID, name, or path to session JSON file
        session: String,

        /// Replay related swarm sessions together in a synchronized multi-pane view
        #[arg(long)]
        swarm: bool,

        /// Export timeline as JSON instead of playing
        #[arg(long)]
        export: bool,

        /// Playback speed multiplier (default: 1.0)
        #[arg(long, default_value = "1.0")]
        speed: f64,

        /// Path to an edited timeline JSON file (overrides session timing)
        #[arg(long)]
        timeline: Option<String>,

        /// Auto-edit timeline: compress tool call wait times and gaps between prompts
        #[arg(long)]
        auto_edit: bool,

        /// Export as video file (auto-generates name if no path given)
        #[arg(long, default_missing_value = "auto", num_args = 0..=1)]
        video: Option<String>,

        /// Video width in columns (default: 120)
        #[arg(long, default_value = "120")]
        cols: u16,

        /// Video height in rows (default: 40)
        #[arg(long, default_value = "40")]
        rows: u16,

        /// Video frames per second (default: 60)
        #[arg(long, default_value = "60")]
        fps: u32,

        /// Force centered layout (overrides config)
        #[arg(long, conflicts_with = "no_centered")]
        centered: bool,

        /// Force left-aligned (non-centered) layout (overrides config)
        #[arg(long, conflicts_with = "centered")]
        no_centered: bool,
    },

    /// Model management commands
    #[command(subcommand)]
    Model(ModelCommand),

    /// Test authentication end-to-end: login (optional), credential probe, refresh, and provider smoke
    AuthTest {
        /// Run the provider login flow before validation (interactive/browser-based)
        #[arg(long)]
        login: bool,

        /// Test all currently configured supported auth providers instead of just --provider
        #[arg(long)]
        all_configured: bool,

        /// Skip the provider runtime smoke prompt
        #[arg(long)]
        no_smoke: bool,

        /// Skip the tool-enabled runtime smoke prompt (the same request path used during normal chat)
        #[arg(long)]
        no_tool_smoke: bool,

        /// Custom smoke prompt (default asks for AUTH_TEST_OK)
        #[arg(long)]
        prompt: Option<String>,

        /// Emit JSON report instead of human-readable output
        #[arg(long)]
        json: bool,

        /// Write the full auth-test report JSON to a file
        #[arg(long)]
        output: Option<String>,
    },

    /// Save or restore the current set of open kcode windows across a system reboot
    Restart {
        #[command(subcommand)]
        action: RestartCommand,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum RestartCommand {
    /// Save a reboot snapshot of currently active kcode windows
    Save {
        /// Restore this reboot snapshot automatically the next time plain `kcode` starts
        #[arg(long)]
        auto_restore: bool,
    },
    /// Restore the most recently saved reboot snapshot
    Restore,
    /// Show the currently saved reboot snapshot
    Status,
    /// Remove the currently saved reboot snapshot
    Clear,
}

#[derive(Subcommand, Debug)]
pub(crate) enum ModelCommand {
    /// List model names you can pass to -m/--model
    List {
        /// Emit JSON instead of plain text
        #[arg(long)]
        json: bool,

        /// Show provider/selection summary before the list
        #[arg(long)]
        verbose: bool,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum ProviderCommand {
    /// List provider IDs you can pass to -p/--provider
    List {
        /// Emit JSON instead of plain text
        #[arg(long)]
        json: bool,
    },

    /// Show the currently requested and resolved provider selection
    Current {
        /// Emit JSON instead of plain text
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum AuthCommand {
    /// Show configured authentication status for model/tool providers
    Status {
        /// Emit JSON instead of plain text
        #[arg(long)]
        json: bool,
    },
    /// Diagnose provider auth issues and suggest next steps
    Doctor {
        /// Optional provider id or alias to focus diagnosis on one provider
        #[arg(id = "auth_provider", value_name = "PROVIDER")]
        provider: Option<String>,

        /// Run live post-login validation for configured providers during diagnosis
        #[arg(long)]
        validate: bool,

        /// Emit JSON instead of plain text
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum AmbientCommand {
    /// Show ambient mode status
    Status,
    /// Show recent ambient activity log
    Log,
    /// Manually trigger an ambient cycle
    Trigger,
    /// Stop ambient mode
    Stop,
    /// Run an ambient cycle in a visible TUI (internal, spawned by the ambient runner)
    #[command(hide = true)]
    RunVisible,
}

#[derive(Subcommand, Debug)]
pub(crate) enum MemoryCommand {
    /// List all stored memories
    List {
        /// Filter by scope (project, global, all)
        #[arg(short, long, default_value = "all")]
        scope: String,

        /// Filter by tag
        #[arg(short, long)]
        tag: Option<String>,
    },

    /// Search memories by query
    Search {
        /// Search query
        query: String,

        /// Use semantic search (embedding-based) instead of keyword
        #[arg(short, long)]
        semantic: bool,
    },

    /// Export memories to a JSON file
    Export {
        /// Output file path
        output: String,

        /// Export scope (project, global, all)
        #[arg(short, long, default_value = "all")]
        scope: String,
    },

    /// Import memories from a JSON file
    Import {
        /// Input file path
        input: String,

        /// Import scope (project, global)
        #[arg(short, long, default_value = "project")]
        scope: String,

        /// Overwrite existing memories with same ID
        #[arg(long)]
        overwrite: bool,
    },

    /// Show memory statistics
    Stats,

    /// Clear test memory storage (used by debug sessions)
    ClearTest,

    /// Ensure the Kcode GGUF local model server is running
    SidecarEnsure {
        /// Emit JSON instead of plain text
        #[arg(long)]
        json: bool,
    },

    /// Run deterministic memory retrieval evaluation
    Eval {
        /// Emit JSON instead of plain text
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum SelfImproveCommand {
    /// Run bounded autonomous self-improvement cycle
    Run {
        #[arg(long, default_value_t = 1)]
        iterations: usize,
        #[arg(long, default_value_t = true)]
        dry_run: bool,
        #[arg(long, default_value_t = false)]
        allow_mutation: bool,
    },

    /// Synthesize evidence-ranked self-improvement task queue
    Tasks,

    /// Write evidence-ranked self-improvement task report
    TaskReport {
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },

    /// Evaluate tiny patch gate for the highest-ranked task
    TinyPatchGate {
        #[arg(long, default_value_t = true)]
        dry_run: bool,
        #[arg(long, default_value_t = false)]
        allow_mutation: bool,
    },

    /// Write autonomous self-improvement cycle report
    Report {
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum LatentCommand {
    /// Show latent recurrence status as JSON
    Status,
    /// Show the current latent vector
    Vector,
    /// Observe an operational event and update recurrence state
    Observe {
        kind: String,
        outcome: String,
        #[arg(long = "tag")]
        tag: Vec<String>,
        #[arg(long)]
        tool: Option<String>,
        #[arg(long = "latent-provider", id = "latent_provider")]
        provider: Option<String>,
        #[arg(long, default_value_t = 1.0)]
        weight: f32,
    },
    /// Translate an event into invariant matches
    Translate {
        kind: String,
        outcome: String,
        #[arg(long = "tag")]
        tag: Vec<String>,
    },
    /// Print drift from the previous latent vector
    Drift,
    /// Remap the vector metadata to a target schema version
    Remap { schema_version: u32 },
    /// Print invariant translation rules
    Invariants,
    /// Print temporal provenance records
    Provenance,
    /// Print temporal latent memory entries
    Temporal,
    /// Preview event influence without mutating state
    Influence {
        kind: String,
        outcome: String,
        #[arg(long = "tag")]
        tag: Vec<String>,
    },
    /// Render a Markdown latent recurrence report
    Report {
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },
    /// Learn from an operational event without hiding state in the model
    Learn {
        kind: String,
        outcome: String,
        #[arg(long = "tag")]
        tag: Vec<String>,
        #[arg(long)]
        tool: Option<String>,
        #[arg(long, default_value_t = 1.0)]
        weight: f32,
    },
    /// Print learned latent vectors
    LearnedVectors,
    /// Print learned latent attractors
    Attractors,
    /// Compare baseline event score against alternate tags
    Counterfactual {
        kind: String,
        outcome: String,
        #[arg(long = "tag")]
        tag: Vec<String>,
        #[arg(long = "alternate-tag")]
        alternate_tag: Vec<String>,
    },
    /// Print doctrine bindings learned from invariants
    Doctrine,
    /// Print immune responses to rejected samples
    Immune,
    /// Print latent topology edges
    Topology,
    /// Print convergence metrics
    Convergence,
    /// Render adaptive latent evolution report
    EvolutionReport {
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },
    /// Ingest a runtime learning sample into the persistent background queue
    Ingest {
        kind: String,
        outcome: String,
        #[arg(long = "tag")]
        tag: Vec<String>,
        #[arg(long)]
        tool: Option<String>,
        #[arg(long, default_value = "manual")]
        source: String,
    },
    /// Run one bounded background learning cycle now
    LearnNow {
        #[arg(long, default_value_t = 32)]
        limit: usize,
    },
    /// Show background learning status
    BackgroundStatus,
    /// Show persisted runtime learning samples
    Samples,
    /// Summarize sampled operational outcomes
    Outcomes,
    /// Summarize learned doctrine/topology state
    Doctrines,
    /// Pause background learning consumption
    Pause,
    /// Resume background learning consumption
    Resume,
    /// Show live operational fabric status
    FabricStatus,
    /// Show live operational fabric event log
    FabricEvents,
    /// Render live operational fabric markdown report
    FabricReport {
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },
    /// Pause live fabric event ingestion
    FabricPause,
    /// Resume live fabric event ingestion
    FabricResume,
    /// Emit a live fabric system ping for validation
    FabricPing,
    /// Show latent memory bank status
    LatentMemoryStatus,
    /// Print ctx-style latent memory blocks
    LatentMemoryBlocks,
    /// Render latent memory bank report
    LatentMemoryReport {
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },
    /// Show closed-loop latent memory usefulness attribution
    LatentMemoryUsefulness,
    /// Show operational policy synthesis status
    PolicyStatus,
    /// Show synthesized policy rules
    PolicyRules,
    /// Ask the gated operational policy for a decision
    PolicyDecide { domain: String, target: String },
    /// Show policy influence audit log
    PolicyAudit,
    /// Render operational policy influence report
    PolicyReport {
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },
    /// Show active and API-only policy domains
    PolicyDomains,
    /// Render policy outcome credit report
    PolicyCreditReport {
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },
    /// Assign an explicit outcome to a policy audit id
    PolicyCreditAssign { audit_id: String, outcome: String },
    /// Run shadow policy simulation over recent events/samples
    PolicySimulate {
        #[arg(long, default_value_t = 200)]
        limit: usize,
    },
    /// Run full closed-loop operational self-eval suite
    EvalRun,

    /// Write the operational self-eval markdown report
    EvalReport {
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },

    /// Enforce the operational self-eval promotion gate
    EvalGate,

    /// Run adversarial operational eval suite
    AdversarialEvalRun,

    /// Write adversarial operational eval markdown report
    AdversarialEvalReport {
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },

    /// Enforce adversarial promotion hardening gate
    AdversarialEvalGate,

    /// Run bounded autonomous internal testing and self-improvement scheduler
    SelfImproveRun {
        #[arg(long, default_value_t = 1)]
        iterations: usize,
        #[arg(long, default_value_t = true)]
        dry_run: bool,
        #[arg(long, default_value_t = false)]
        allow_mutation: bool,
    },

    /// Write autonomous self-improvement markdown report
    SelfImproveReport {
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },

    /// Synthesize evidence-ranked self-improvement task queue
    SelfImproveTasks,

    /// Write evidence-ranked self-improvement task report
    SelfImproveTaskReport {
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },

    /// Evaluate tiny patch gate for highest-ranked self-improvement task
    SelfImproveTinyPatchGate {
        #[arg(long, default_value_t = true)]
        dry_run: bool,
        #[arg(long, default_value_t = false)]
        allow_mutation: bool,
    },

    /// Verify cognition evidence ledger hash chain
    EvidenceLedgerVerify,

    /// Write cognition evidence ledger markdown report
    EvidenceLedgerReport {
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },

    /// Query cognition evidence ledger blocks
    EvidenceLedgerQuery {
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        subject: Option<String>,
        #[arg(long)]
        subsystem: Option<String>,
        #[arg(long, default_value_t = 25)]
        limit: usize,
    },

    /// Explain a cognition evidence block by index or hash prefix
    EvidenceLedgerExplain { target: String },

    /// Replay ledger-backed decisions without future leakage
    EvidenceReplayRun {
        #[arg(long, default_value_t = 25)]
        limit: usize,
        #[arg(long)]
        max_index: Option<u64>,
        #[arg(long)]
        subject: Option<String>,
        #[arg(long, default_value_t = true)]
        alternatives: bool,
    },

    /// Write evidence replay markdown report
    EvidenceReplayReport {
        #[arg(long)]
        output: Option<std::path::PathBuf>,
        #[arg(long, default_value_t = 25)]
        limit: usize,
    },

    /// Explain replay context for a ledger block
    EvidenceReplayExplain { target: String },

    /// Propose a replay-gated dry-run patch for a ranked task
    PatchPropose {
        #[arg(long, default_value = "top")]
        task: String,
    },

    /// Produce dry-run patch text for a proposal candidate
    PatchDryRun {
        #[arg(long, default_value = "top")]
        task: String,
    },

    /// Validate a replay-gated patch proposal
    PatchValidate {
        #[arg(long, default_value = "top")]
        task: String,
    },

    /// Score a patch proposal with replay delta
    PatchReplayScore {
        #[arg(long, default_value = "top")]
        task: String,
    },

    /// Run the patch promotion gate
    PatchPromoteGate {
        #[arg(long, default_value = "top")]
        task: String,
    },

    /// Write replay-gated patch proposal report
    PatchReport {
        #[arg(long)]
        output: Option<std::path::PathBuf>,
        #[arg(long, default_value_t = false)]
        validate: bool,
    },

    /// Run the replay-scored self-improvement patch pipeline
    PatchPipelineRun {
        #[arg(long, default_value = "top")]
        task: String,
        #[arg(long, default_value_t = false)]
        validate: bool,
    },

    /// Write replay-scored self-improvement patch pipeline report
    PatchPipelineReport {
        #[arg(long)]
        output: Option<std::path::PathBuf>,
        #[arg(long, default_value = "top")]
        task: String,
        #[arg(long, default_value_t = false)]
        validate: bool,
    },

    /// Render policy shadow simulation report
    PolicyShadowReport {
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },
    /// Promote policies that win in shadow simulation
    PolicyPromoteSafe,
    /// Demote policies that lose in shadow simulation
    PolicyDemoteBad,
}

#[cfg(test)]
mod tests;
