//! Context-compiler scaffold.
//!
//! This module is the home for the typed compile-pass model described in the
//! Neura upstream-payload-architecture roadmap. The pipeline is intentionally
//! additive: existing turn-loop behaviour is preserved unchanged when the
//! compiler runs in *shadow* mode (the default). Only when
//! `NEURA_CONTEXT_COMPILER=v2` does the compiler become the source of truth
//! for routing decisions; every change is gated behind feature flags so the
//! v1 path remains a clean fallback.
//!
//! Phases currently scaffolded here:
//!   * Phase 2 — `AdmissionTier` resolution + `ResourceBudget` allocation
//!     (used in shadow telemetry; not yet enforced).
//!   * Phase 2 — tier-aware tool shrinking (shadow recommendation only;
//!     filter_tool_definitions_for_messages keeps its current semantics).
//!   * Phase 3 — `MemoryPlan` (Inject | Anchor | Skip), feeding the anchor
//!     mode env flag.
//!
//! Future work (Phase 4) will lift these passes from "compute and log"
//! into a fully typed pipeline that produces a `ProviderPayload`.

use crate::agent::turn_loops::TurnAdmission;
use crate::message::Message;

/// Resource budgets per admission tier. Computed once per turn and fed into
/// telemetry. When `NEURA_ADMISSION_BUDGETS=enforce` is set, downstream passes
/// will treat these as hard limits; otherwise they are advisory ("shadow"
/// mode) and only logged.
#[derive(Debug, Clone, Copy)]
pub struct ResourceBudget {
    pub admission: TurnAdmission,
    /// Approximate char budget for the message history sent upstream. The
    /// compiler attempts to fit history (after interlang compaction) within
    /// this budget; over-budget content is summarised to anchor refs.
    pub history_chars: usize,
    /// Number of tokens permitted for memory injection (full text). Above
    /// this, memory falls back to anchor mode.
    pub memory_tokens: usize,
    /// Recommended cap on the number of tools sent upstream. Direct turns
    /// require almost no tools; Deep turns need the full filtered set.
    pub tool_count: usize,
    /// Char budget for the dynamic system-prompt portion (env context,
    /// optional skill, interlang decoder). Static system prompt is excluded
    /// because it is cached.
    pub dynamic_chars: usize,
}

impl ResourceBudget {
    /// Compute the per-tier budget. Numbers are intentionally generous on
    /// Deep tier so coding-flow turns stay capability-preserving; tightened
    /// on Light/Direct.
    pub fn for_admission(admission: TurnAdmission) -> Self {
        match admission {
            TurnAdmission::Direct => Self {
                admission,
                history_chars: 4_000,
                memory_tokens: 0,
                tool_count: 3,
                dynamic_chars: 800,
            },
            TurnAdmission::Light => Self {
                admission,
                history_chars: 12_000,
                memory_tokens: 300,
                tool_count: 8,
                dynamic_chars: 1_200,
            },
            TurnAdmission::Deep | TurnAdmission::Continuation => Self {
                admission,
                history_chars: 48_000,
                memory_tokens: 900,
                tool_count: 12,
                dynamic_chars: 2_000,
            },
        }
    }
}

/// Memory injection plan derived from the pending memory payload, the latest
/// user intent, and the admission tier.
#[derive(Debug, Clone)]
pub enum MemoryPlan {
    /// Inject the full memory prompt as-is (legacy behaviour).
    Inject {
        prompt: String,
        count: usize,
        memory_ids: Vec<String>,
    },
    /// Surface a tiny "memory available, ask via .mem_get" anchor instead of
    /// the full prompt. Useful for Light/Continuation turns where memory
    /// would otherwise burn ~900 tokens that may not be relevant.
    Anchor {
        count: usize,
        memory_ids: Vec<String>,
    },
    /// Drop memory for this turn entirely (e.g. Direct turn or low correlation).
    Skip,
}

impl MemoryPlan {
    pub fn anchor_text(count: usize) -> String {
        format!(
            "<system-reminder>\n<mem-anchor count=\"{}\" via=\".mem_get\" />\nMemory available; call `.mem_get reason=<why>` to retrieve relevant entries.\n</system-reminder>",
            count
        )
    }

    pub fn anchor_chars(count: usize) -> usize {
        Self::anchor_text(count).len()
    }
}

/// Decide which memory plan to use for the upcoming turn. Falls back to
/// `Inject` (legacy behaviour) unless `NEURA_MEMORY_ANCHOR` is set.
pub fn plan_memory(
    admission: TurnAdmission,
    pending_prompt: Option<&str>,
    pending_count: usize,
    pending_ids: &[String],
    latest_user_text: Option<&str>,
) -> MemoryPlan {
    let Some(prompt) = pending_prompt else {
        return MemoryPlan::Skip;
    };
    if matches!(admission, TurnAdmission::Direct) {
        return MemoryPlan::Skip;
    }
    if !memory_anchor_enabled() {
        return MemoryPlan::Inject {
            prompt: prompt.to_string(),
            count: pending_count,
            memory_ids: pending_ids.to_vec(),
        };
    }
    // Anchor mode is only safe when the user turn doesn't *require* full
    // memory text (e.g. they asked "what did I prefer about X?"). Heuristic:
    // if the latest user text mentions memory/preferences/recall/remember,
    // inject in full so the model sees the entries directly.
    let mentions_memory_explicitly = latest_user_text
        .map(|text| {
            let lower = text.to_ascii_lowercase();
            lower.contains("remember")
                || lower.contains("recall")
                || lower.contains("memory")
                || lower.contains("prefer")
                || lower.contains("preference")
        })
        .unwrap_or(false);
    if mentions_memory_explicitly {
        return MemoryPlan::Inject {
            prompt: prompt.to_string(),
            count: pending_count,
            memory_ids: pending_ids.to_vec(),
        };
    }
    MemoryPlan::Anchor {
        count: pending_count,
        memory_ids: pending_ids.to_vec(),
    }
}

/// Tool list shrinking recommendation for telemetry (shadow only). Compares
/// the actual tool list to the per-tier ideal so we can measure the savings
/// before enforcing.
#[derive(Debug, Clone)]
pub struct ToolPlan {
    pub admission: TurnAdmission,
    pub recommended_count: usize,
    pub actual_count: usize,
    pub recommended_names: Vec<String>,
}

pub fn plan_tools(
    admission: TurnAdmission,
    actual_tool_names: &[String],
    classified_intent_names: &[String],
) -> ToolPlan {
    let always_on: &[&str] = &["bash", "read", "tool_expand"];
    let recommended_names: Vec<String> = match admission {
        TurnAdmission::Direct => vec!["tool_expand".to_string()],
        TurnAdmission::Light => always_on.iter().map(|s| (*s).to_string()).collect(),
        TurnAdmission::Deep | TurnAdmission::Continuation => {
            // Deep keeps the full intent-classified list plus always-on.
            let mut names: Vec<String> = always_on.iter().map(|s| (*s).to_string()).collect();
            for name in classified_intent_names {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
            names
        }
    };
    ToolPlan {
        admission,
        recommended_count: recommended_names.len(),
        actual_count: actual_tool_names.len(),
        recommended_names,
    }
}

pub fn admission_budgets_mode() -> AdmissionBudgetsMode {
    match std::env::var("NEURA_ADMISSION_BUDGETS")
        .ok()
        .as_deref()
        .map(|v| v.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("enforce") | Some("on") | Some("1") => AdmissionBudgetsMode::Enforce,
        Some("0") | Some("off") | Some("disabled") | Some("none") => AdmissionBudgetsMode::Off,
        _ => AdmissionBudgetsMode::Shadow,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionBudgetsMode {
    /// Compute budget but do not enforce; log only.
    Shadow,
    /// Compute and enforce budget caps.
    Enforce,
    /// Disable budget computation entirely (telemetry omitted).
    Off,
}

pub fn memory_anchor_enabled() -> bool {
    std::env::var("NEURA_MEMORY_ANCHOR")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// `NEURA_CONTEXT_COMPILER` modes:
/// * `off` / `v1` (default): compiler runs but only emits shadow telemetry;
///   the legacy v1 turn-loop logic owns every decision.
/// * `shadow`: alias for default; explicit shadow telemetry, no enforcement.
/// * `v2`: compiler-driven enforcement of tool admission, interlang budget,
///   memory plan, and history clamp. v1 is still available as a fallback when
///   the compiler reports low confidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompilerMode {
    V1,
    Shadow,
    V2,
}

pub fn compiler_mode() -> CompilerMode {
    match std::env::var("NEURA_CONTEXT_COMPILER")
        .ok()
        .as_deref()
        .map(|v| v.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("v2") | Some("2") => CompilerMode::V2,
        Some("shadow") | Some("log") => CompilerMode::Shadow,
        Some("v1") | Some("1") | Some("off") | Some("disabled") | Some("") => CompilerMode::V1,
        None => CompilerMode::V1,
        _ => CompilerMode::V1,
    }
}

/// Convenience boolean: true only in `v2` enforcement mode.
pub fn context_compiler_v2_enabled() -> bool {
    matches!(compiler_mode(), CompilerMode::V2)
}

/// `NEURA_LOCAL_PREROUTER` modes:
/// * `log` (default when NEURA_LOCAL_PREROUTER is set or unset): sidecar
///   pre-route classification is recorded but has no behavioural effect.
/// * `decide`: the pre-route classification is fed into compile_v2 and may
///   influence admission tier or memory plan when the model has high
///   confidence. Always degrades safely to `log` on sidecar error.
/// * `off`: skip the pre-route entirely (no sidecar build / log).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarPrerouteMode {
    Off,
    Log,
    Decide,
}

pub fn sidecar_preroute_mode() -> SidecarPrerouteMode {
    match std::env::var("NEURA_LOCAL_PREROUTER")
        .ok()
        .as_deref()
        .map(|v| v.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("decide") | Some("on") | Some("v2") => SidecarPrerouteMode::Decide,
        Some("off") | Some("0") | Some("false") | Some("disabled") => SidecarPrerouteMode::Off,
        _ => SidecarPrerouteMode::Log,
    }
}

/// Snapshot of the v2 admission decision for shadow logging. Always safe to
/// compute even when v1 is doing the actual work.
pub fn shadow_admission_snapshot(
    admission: TurnAdmission,
    actual_tool_names: &[String],
    classified_intent_names: &[String],
    history_chars: usize,
    memory_pending_prompt_chars: usize,
    latest_user_text: Option<&str>,
) -> serde_json::Value {
    let budget = ResourceBudget::for_admission(admission);
    let tool_plan = plan_tools(admission, actual_tool_names, classified_intent_names);
    // Lightweight memory plan classification: we don't have the actual prompt
    // here, so we replay the same Inject/Anchor/Skip decision logic on the
    // metadata alone.
    let plan_label = if memory_pending_prompt_chars == 0 {
        "skip"
    } else if matches!(admission, TurnAdmission::Direct) {
        "skip"
    } else if !memory_anchor_enabled() {
        "inject"
    } else if latest_user_text
        .map(|t| {
            let l = t.to_ascii_lowercase();
            l.contains("remember")
                || l.contains("recall")
                || l.contains("memory")
                || l.contains("prefer")
        })
        .unwrap_or(false)
    {
        "inject"
    } else {
        "anchor"
    };
    serde_json::json!({
        "admission": admission.label(),
        "budget": {
            "history_chars": budget.history_chars,
            "memory_tokens": budget.memory_tokens,
            "tool_count": budget.tool_count,
            "dynamic_chars": budget.dynamic_chars,
        },
        "history_observed_chars": history_chars,
        "history_over_budget": history_chars > budget.history_chars,
        "tool_plan": {
            "actual_count": tool_plan.actual_count,
            "recommended_count": tool_plan.recommended_count,
            "recommended_names": tool_plan.recommended_names,
            "would_shrink_by": tool_plan.actual_count.saturating_sub(tool_plan.recommended_count),
        },
        "memory_plan": plan_label,
        "memory_pending_prompt_chars": memory_pending_prompt_chars,
        "compiler_v2_enabled": context_compiler_v2_enabled(),
        "anchor_mode_enabled": memory_anchor_enabled(),
    })
}

/// Emit the shadow admission snapshot via the remote-telemetry turn-trace
/// sink. Cheap; disabled entirely when budgets mode is Off.
pub fn record_shadow_admission(
    admission: TurnAdmission,
    actual_tool_names: &[String],
    classified_intent_names: &[String],
    history_chars: usize,
    memory_pending_prompt_chars: usize,
    latest_user_text: Option<&str>,
) {
    if matches!(admission_budgets_mode(), AdmissionBudgetsMode::Off) {
        return;
    }
    let snapshot = shadow_admission_snapshot(
        admission,
        actual_tool_names,
        classified_intent_names,
        history_chars,
        memory_pending_prompt_chars,
        latest_user_text,
    );
    let mut record = serde_json::json!({
        "event": "shadow_admission",
    });
    if let Some(obj) = record.as_object_mut() {
        if let Some(snap_obj) = snapshot.as_object() {
            for (k, v) in snap_obj {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
    crate::provider::remote_telemetry::log_turn_trace(record);
}

// ============================================================================
// Phase 4 — Context Compiler v2 scaffold
// ============================================================================
//
// `compile_v2` is the *additive* typed-pipeline entry point. It runs each pass
// (intent → budget → memory plan → tool plan → cache plan → assembly) on the
// same inputs the legacy turn loop already builds, producing a `CompiledTurn`
// description. Today the scaffold returns a description; the legacy turn loop
// is *not* re-routed through this function. Wiring the scaffold to actually
// drive `run_turn` is gated behind `NEURA_CONTEXT_COMPILER=v2` and will be
// done in a follow-up patch.
//
// The compile passes are kept side-effect free where possible so the same
// `CompiledTurn` can be produced as a *dry-run* for telemetry / diff testing
// against the v1 path.

/// Inputs the v2 compiler needs. Borrows everything to avoid clones.
#[derive(Debug, Clone)]
pub struct CompileInputs<'a> {
    pub admission: TurnAdmission,
    pub messages: &'a [Message],
    pub tool_names: &'a [String],
    pub classified_intent_tool_names: &'a [String],
    pub system_static_chars: usize,
    pub system_dynamic_chars: usize,
    pub provider_context_window: Option<usize>,
    pub memory_pending: Option<MemoryPlanInput<'a>>,
    pub interlang_refs_chars: usize,
    pub history_chars_observed: usize,
    pub locked_tools_chars: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct MemoryPlanInput<'a> {
    pub prompt: &'a str,
    pub count: usize,
    pub memory_ids: &'a [String],
}

/// Output of `compile_v2`. Contains every routing decision plus enough
/// telemetry to diff against v1 in tests.
#[derive(Debug, Clone)]
pub struct CompiledTurn {
    pub admission: TurnAdmission,
    pub budget: ResourceBudget,
    pub memory_plan: MemoryPlan,
    pub tool_plan: ToolPlan,
    pub cache_plan: CachePlan,
    pub estimated_total_chars: usize,
    pub history_over_budget: bool,
}

#[derive(Debug, Clone)]
pub struct CachePlan {
    /// Whether the static system prompt should carry an ephemeral cache
    /// breakpoint. Always true (matches v1 `build_system_param_split`).
    pub cache_static_system: bool,
    /// Whether the dynamic part should be cached (always false; it changes per
    /// turn).
    pub cache_dynamic_system: bool,
    /// Whether the tool list should carry an ephemeral cache breakpoint.
    /// True when tools count >= 1 (matches v1).
    pub cache_tools: bool,
    /// Number of message-level cache breakpoints to place. Matches v1's two
    /// (READ on prev assistant, WRITE on newest assistant).
    pub message_breakpoints: usize,
}

impl CachePlan {
    pub fn for_admission(admission: TurnAdmission, tool_count: usize) -> Self {
        Self {
            cache_static_system: true,
            cache_dynamic_system: false,
            cache_tools: tool_count >= 1,
            message_breakpoints: if matches!(admission, TurnAdmission::Direct) {
                1
            } else {
                2
            },
        }
    }
}

/// Run the typed compile passes.
pub fn compile_v2(inputs: &CompileInputs<'_>) -> CompiledTurn {
    // Pass 1: admission resolution (already done by caller; we just record).
    let admission = inputs.admission;
    // Pass 2: per-tier budget.
    let budget = ResourceBudget::for_admission(admission);
    // Pass 3: memory plan.
    let memory_plan = match inputs.memory_pending.as_ref() {
        Some(pending) => plan_memory(
            admission,
            Some(pending.prompt),
            pending.count,
            pending.memory_ids,
            latest_user_text(inputs.messages).as_deref(),
        ),
        None => MemoryPlan::Skip,
    };
    // Pass 4: tool plan.
    let tool_plan = plan_tools(
        admission,
        inputs.tool_names,
        inputs.classified_intent_tool_names,
    );
    // Pass 5: cache plan.
    let cache_plan = CachePlan::for_admission(admission, inputs.tool_names.len());
    // Pass 6: rough total-chars estimate (component-level; matches the
    // accounting computed in the v1 turn loop).
    let memory_chars = match &memory_plan {
        MemoryPlan::Inject { prompt, .. } => prompt.len() + 30, // wrapper overhead
        MemoryPlan::Anchor { count, .. } => MemoryPlan::anchor_chars(*count),
        MemoryPlan::Skip => 0,
    };
    let tools_chars = inputs.locked_tools_chars.unwrap_or(0);
    let estimated_total_chars = inputs
        .system_static_chars
        .saturating_add(inputs.system_dynamic_chars)
        .saturating_add(inputs.history_chars_observed)
        .saturating_add(tools_chars)
        .saturating_add(memory_chars);
    let history_over_budget = inputs.history_chars_observed > budget.history_chars;
    CompiledTurn {
        admission,
        budget,
        memory_plan,
        tool_plan,
        cache_plan,
        estimated_total_chars,
        history_over_budget,
    }
}

// ============================================================================
// Phase 5 — TurnPlan: concrete actions applied by run_turn under v2 mode
// ============================================================================

/// Concrete per-turn execution plan. Built from `CompiledTurn` plus runtime
/// signals; consumed directly by the run loop. v1 ignores this struct; v2
/// applies it.
#[derive(Debug, Clone)]
pub struct TurnPlan {
    pub mode: CompilerMode,
    pub admission: TurnAdmission,
    pub budget: ResourceBudget,
    pub memory_plan: MemoryPlan,
    /// Filtered tool list to actually send upstream (subset of the locked
    /// tool list). When `None`, the run loop sends the full locked list
    /// (legacy behaviour).
    pub tool_subset: Option<Vec<String>>,
    /// Maximum number of interlang ref blocks to emit this turn. None means
    /// no cap (legacy behaviour).
    pub max_interlang_blocks: Option<usize>,
    /// Override for the recent-window byte budget. None means use whatever
    /// `NEURA_CONTEXT_DIET_RECENT_BYTES` says, falling back to the message-
    /// count floor.
    pub recent_window_bytes: Option<usize>,
    /// True when sidecar pre-route should run for this turn.
    pub run_sidecar_preroute: bool,
    /// True when context-diet should run; false to bypass entirely (Direct).
    pub run_interlang: bool,
    /// True when the run loop should consult pending memory for the turn.
    pub run_memory: bool,
    /// True when the run loop should clamp message history to the budget.
    /// Always opt-in: legacy v1 never clamps, only summarises via interlang.
    pub clamp_history: bool,
    /// Confidence the compiler has in this plan, [0.0, 1.0]. Below ~0.5 the
    /// run loop should fall back to v1 behaviour for safety.
    pub confidence: f32,
}

impl TurnPlan {
    /// Conservative legacy plan — caller does what v1 always did.
    pub fn legacy(admission: TurnAdmission) -> Self {
        Self {
            mode: CompilerMode::V1,
            admission,
            budget: ResourceBudget::for_admission(admission),
            memory_plan: MemoryPlan::Skip,
            tool_subset: None,
            max_interlang_blocks: None,
            recent_window_bytes: None,
            run_sidecar_preroute: admission.use_sidecar(),
            run_interlang: admission.use_interlang(),
            run_memory: admission.use_memory(),
            clamp_history: false,
            confidence: 1.0,
        }
    }

    /// Returns true when the run loop should apply this plan instead of
    /// falling back to v1. Always false outside V2 mode and below the
    /// confidence floor.
    pub fn should_apply(&self) -> bool {
        matches!(self.mode, CompilerMode::V2) && self.confidence >= 0.5
    }
}

/// Build a `TurnPlan` from the same inputs `compile_v2` consumes plus the
/// active mode. Always produces a sensible plan; the run loop decides whether
/// to apply it via `should_apply()`.
pub fn compile_turn_plan(inputs: &CompileInputs<'_>) -> TurnPlan {
    let mode = compiler_mode();
    let compiled = compile_v2(inputs);
    let admission = compiled.admission;
    let budget = compiled.budget;

    // Mode-specific concrete actions:
    let (
        tool_subset,
        max_interlang_blocks,
        recent_window_bytes,
        run_sidecar_preroute,
        run_interlang,
        run_memory,
        clamp_history,
        confidence,
    ) = match mode {
        CompilerMode::V1 | CompilerMode::Shadow => (
            None,
            None,
            None,
            admission.use_sidecar(),
            admission.use_interlang(),
            admission.use_memory(),
            false,
            1.0,
        ),
        CompilerMode::V2 => {
            let subset = if matches!(admission, TurnAdmission::Direct) {
                Some(direct_tool_subset(inputs.tool_names))
            } else if matches!(admission, TurnAdmission::Light) {
                Some(light_tool_subset(
                    inputs.tool_names,
                    inputs.classified_intent_tool_names,
                ))
            } else {
                // Deep / Continuation use the full filtered list (already
                // intent-shrunk by filter_tool_definitions_for_messages).
                None
            };
            let blocks_cap = match admission {
                TurnAdmission::Direct => Some(0),
                TurnAdmission::Light => Some(8),
                _ => None,
            };
            let recent_bytes = match admission {
                TurnAdmission::Direct => Some(2_000),
                TurnAdmission::Light => Some(8_000),
                _ => None,
            };
            // Confidence: drop a notch when classified admission disagrees
            // with what the latest user text suggests, so the run loop can
            // bail to v1. For the foreseeable future we only mark Direct
            // turns as high confidence (we never want to silently downgrade
            // a Deep coding turn).
            let confidence = match admission {
                TurnAdmission::Direct => 0.95,
                TurnAdmission::Light => 0.9,
                TurnAdmission::Deep | TurnAdmission::Continuation => 0.85,
            };
            (
                subset,
                blocks_cap,
                recent_bytes,
                admission.use_sidecar(),
                admission.use_interlang(),
                admission.use_memory(),
                matches!(admission, TurnAdmission::Direct),
                confidence,
            )
        }
    };
    TurnPlan {
        mode,
        admission,
        budget,
        memory_plan: compiled.memory_plan,
        tool_subset,
        max_interlang_blocks,
        recent_window_bytes,
        run_sidecar_preroute,
        run_interlang,
        run_memory,
        clamp_history,
        confidence,
    }
}

/// Tool subset for Direct turns: only `tool_expand` (or empty if not in
/// list). The model can request hidden tools by name via `tool_expand` if
/// needed.
fn direct_tool_subset(all_names: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for name in all_names {
        if name == "tool_expand" {
            out.push(name.clone());
        }
    }
    out
}

/// Tool subset for Light turns: minimal core ('bash', 'read', 'tool_expand')
/// plus any classified-intent tools. Skips Edit/Write/Apply_patch since Light
/// turns are typically Q&A.
fn light_tool_subset(all_names: &[String], classified: &[String]) -> Vec<String> {
    const CORE: &[&str] = &["bash", "read", "tool_expand"];
    let mut out = Vec::new();
    for name in all_names {
        if CORE.iter().any(|c| *c == name) || classified.iter().any(|c| c == name) {
            out.push(name.clone());
        }
    }
    out
}

/// Latest *non-system-reminder* user text from the message vector. Mirrors the
/// helper in `turn_loops`, exposed here so the compiler can stay self-contained.
pub fn latest_user_text(messages: &[Message]) -> Option<String> {
    messages.iter().rev().find_map(|message| {
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
            .join("\n");
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed.starts_with("<system-reminder>") {
            None
        } else {
            Some(text)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::turn_loops::TurnAdmission;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// Serialise tests that mutate process-wide env vars.
    fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        match LOCK.get_or_init(|| Mutex::new(())).lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    #[test]
    fn budgets_widen_with_admission_tier() {
        let direct = ResourceBudget::for_admission(TurnAdmission::Direct);
        let light = ResourceBudget::for_admission(TurnAdmission::Light);
        let deep = ResourceBudget::for_admission(TurnAdmission::Deep);
        assert!(direct.history_chars < light.history_chars);
        assert!(light.history_chars < deep.history_chars);
        assert_eq!(direct.memory_tokens, 0);
        assert!(deep.memory_tokens >= light.memory_tokens);
    }

    #[test]
    fn continuation_inherits_deep_budget() {
        let cont = ResourceBudget::for_admission(TurnAdmission::Continuation);
        let deep = ResourceBudget::for_admission(TurnAdmission::Deep);
        assert_eq!(cont.history_chars, deep.history_chars);
        assert_eq!(cont.memory_tokens, deep.memory_tokens);
    }

    #[test]
    fn plan_memory_skips_on_direct() {
        let plan = plan_memory(
            TurnAdmission::Direct,
            Some("memory body"),
            2,
            &["m1".to_string()],
            Some("meow"),
        );
        assert!(matches!(plan, MemoryPlan::Skip));
    }

    #[test]
    fn plan_memory_injects_when_anchor_disabled() {
        let _g = env_lock();
        unsafe {
            std::env::remove_var("NEURA_MEMORY_ANCHOR");
        }
        let plan = plan_memory(
            TurnAdmission::Light,
            Some("memory body"),
            1,
            &[],
            Some("how should I structure this code?"),
        );
        assert!(matches!(plan, MemoryPlan::Inject { .. }));
    }

    #[test]
    fn plan_memory_anchors_when_anchor_enabled_and_no_explicit_recall() {
        let _g = env_lock();
        unsafe {
            std::env::set_var("NEURA_MEMORY_ANCHOR", "1");
        }
        let plan = plan_memory(
            TurnAdmission::Light,
            Some("memory body"),
            3,
            &["m1".to_string()],
            Some("just an unrelated coding question about loops"),
        );
        assert!(matches!(plan, MemoryPlan::Anchor { count: 3, .. }));
        unsafe {
            std::env::remove_var("NEURA_MEMORY_ANCHOR");
        }
    }

    #[test]
    fn plan_memory_injects_when_user_explicitly_recalls() {
        let _g = env_lock();
        unsafe {
            std::env::set_var("NEURA_MEMORY_ANCHOR", "1");
        }
        let plan = plan_memory(
            TurnAdmission::Light,
            Some("memory body"),
            2,
            &[],
            Some("what was my preference about indentation?"),
        );
        assert!(matches!(plan, MemoryPlan::Inject { .. }));
        unsafe {
            std::env::remove_var("NEURA_MEMORY_ANCHOR");
        }
    }

    #[test]
    fn plan_tools_direct_is_minimal() {
        let plan = plan_tools(
            TurnAdmission::Direct,
            &["bash".into(), "read".into(), "edit".into()],
            &[],
        );
        assert_eq!(plan.recommended_names, vec!["tool_expand"]);
    }

    #[test]
    fn plan_tools_deep_keeps_intent_specific() {
        let plan = plan_tools(
            TurnAdmission::Deep,
            &[
                "bash".into(),
                "read".into(),
                "edit".into(),
                "agentgrep".into(),
            ],
            &["agentgrep".into(), "edit".into()],
        );
        assert!(plan.recommended_names.iter().any(|n| n == "agentgrep"));
        assert!(plan.recommended_names.iter().any(|n| n == "edit"));
        assert!(plan.recommended_names.iter().any(|n| n == "bash"));
    }

    #[test]
    fn compile_v2_shape_for_meow_turn() {
        let messages = vec![Message::user("meow")];
        let inputs = CompileInputs {
            admission: TurnAdmission::Direct,
            messages: &messages,
            tool_names: &["bash".into(), "read".into(), "tool_expand".into()],
            classified_intent_tool_names: &[],
            system_static_chars: 4_000,
            system_dynamic_chars: 200,
            provider_context_window: Some(200_000),
            memory_pending: None,
            interlang_refs_chars: 0,
            history_chars_observed: 4,
            locked_tools_chars: Some(1_500),
        };
        let compiled = compile_v2(&inputs);
        assert_eq!(compiled.admission, TurnAdmission::Direct);
        assert_eq!(compiled.budget.tool_count, 3);
        assert!(matches!(compiled.memory_plan, MemoryPlan::Skip));
        assert!(!compiled.history_over_budget);
        assert_eq!(compiled.cache_plan.message_breakpoints, 1);
    }

    #[test]
    fn compile_v2_shape_for_deep_turn_with_memory() {
        let messages = vec![Message::user(
            "fix the failing build in src/foo.rs and run tests",
        )];
        let memory_ids = vec!["m1".to_string()];
        let pending = MemoryPlanInput {
            prompt: "## Notes\n1. user prefers tabs",
            count: 1,
            memory_ids: &memory_ids,
        };
        let inputs = CompileInputs {
            admission: TurnAdmission::Deep,
            messages: &messages,
            tool_names: &[
                "bash".into(),
                "read".into(),
                "edit".into(),
                "agentgrep".into(),
                "tool_expand".into(),
            ],
            classified_intent_tool_names: &["agentgrep".into(), "edit".into()],
            system_static_chars: 4_000,
            system_dynamic_chars: 300,
            provider_context_window: Some(200_000),
            memory_pending: Some(pending),
            interlang_refs_chars: 0,
            history_chars_observed: 56,
            locked_tools_chars: Some(2_500),
        };
        let _g = env_lock();
        unsafe {
            std::env::remove_var("NEURA_MEMORY_ANCHOR");
        }
        let compiled = compile_v2(&inputs);
        assert_eq!(compiled.admission, TurnAdmission::Deep);
        assert!(matches!(compiled.memory_plan, MemoryPlan::Inject { .. }));
        assert_eq!(compiled.cache_plan.message_breakpoints, 2);
        assert!(!compiled.history_over_budget);
        // tool plan should include classified intent names
        assert!(
            compiled
                .tool_plan
                .recommended_names
                .iter()
                .any(|n| n == "agentgrep")
        );
    }

    #[test]
    fn compile_v2_anchors_memory_when_flag_set() {
        let messages = vec![Message::user("now do that other refactor")];
        let memory_ids = vec!["m1".to_string()];
        let pending = MemoryPlanInput {
            prompt: "## Notes\n1. ...",
            count: 1,
            memory_ids: &memory_ids,
        };
        let inputs = CompileInputs {
            admission: TurnAdmission::Light,
            messages: &messages,
            tool_names: &["bash".into(), "read".into(), "tool_expand".into()],
            classified_intent_tool_names: &[],
            system_static_chars: 4_000,
            system_dynamic_chars: 250,
            provider_context_window: Some(200_000),
            memory_pending: Some(pending),
            interlang_refs_chars: 0,
            history_chars_observed: 30,
            locked_tools_chars: Some(1_500),
        };
        let _g = env_lock();
        unsafe {
            std::env::set_var("NEURA_MEMORY_ANCHOR", "1");
        }
        let compiled = compile_v2(&inputs);
        assert!(matches!(compiled.memory_plan, MemoryPlan::Anchor { .. }));
        unsafe {
            std::env::remove_var("NEURA_MEMORY_ANCHOR");
        }
    }
}
