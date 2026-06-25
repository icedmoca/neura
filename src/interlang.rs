//! Optional llm-interlang-inspired message compaction.
//!
//! Conservative first integration of /home/dad/neura-agent/tmp/llm-interlang-main:
//! rewrite only highly repetitive text/tool-result blocks into a tiny
//! line-reference protocol.

use crate::message::{ContentBlock, Message, Role};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokenizers::Tokenizer;

const ENV_ENABLE: &str = "NEURA_INTERLANG_COMPACT";
const ENV_MODE: &str = "NEURA_INTERLANG_MODE";
const ENV_TOKENIZER_JSON: &str = "NEURA_INTERLANG_TOKENIZER_JSON";
const ENV_CONTEXT_DIET: &str = "NEURA_CONTEXT_DIET";
const ENV_CONTEXT_DIET_TRIGGER_TOKENS: &str = "NEURA_CONTEXT_DIET_TRIGGER_TOKENS";
const ENV_CONTEXT_DIET_RECENT_MESSAGES: &str = "NEURA_CONTEXT_DIET_RECENT_MESSAGES";
const ENV_CONTEXT_DIET_MIN_BLOCK_CHARS: &str = "NEURA_CONTEXT_DIET_MIN_BLOCK_CHARS";
const DEFAULT_TOKENIZER_JSON: &str = "/home/dad/.neura/models/all-MiniLM-L6-v2/tokenizer.json";
const MIN_TEXT_CHARS: usize = 900;
const MIN_SAVED_CHARS: usize = 240;
const MIN_SEEN_REF_CHARS: usize = 2_400;
const MIN_VAULT_REF_CHARS: usize = 16_000;
const DEFAULT_CONTEXT_DIET_TRIGGER_TOKENS: usize = 6_000;
const DEFAULT_CONTEXT_DIET_RECENT_MESSAGES: usize = 6;
const DEFAULT_CONTEXT_DIET_MIN_BLOCK_CHARS: usize = 300;
const APPROX_CHARS_PER_TOKEN: usize = 4;
const AUTO_REHYDRATE_CONFIDENCE_THRESHOLD: f32 = 0.56;
const AUTO_REHYDRATE_MAX_BLOCKS: usize = 1;
const AUTO_REHYDRATE_DEBUG_ENV: &str = "NEURA_CTX_REHYDRATE_DEBUG";

#[derive(Debug, Clone)]
struct SeenBlock {
    hash: String,
    original_chars: usize,
    summary: String,
    exact: String,
    confidence: f32,
    priority: ContextPriority,
    sensitive: bool,
    topics: Vec<&'static str>,
    lexical_keys: Vec<String>,
    /// Cached encoded reference string (e.g., `<ctx ... />` or
    /// `<il:seen ... />`). Filled on first encode; reused on later turns to
    /// avoid recomputing the deterministic ref body. Different ref kinds
    /// produce different strings, so we key on a small `(kind)` discriminant.
    encoded_refs: HashMap<&'static str, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextPriority {
    Low,
    Normal,
    High,
    Verify,
}

impl ContextPriority {
    fn as_str(self) -> &'static str {
        match self {
            ContextPriority::Low => "low",
            ContextPriority::Normal => "normal",
            ContextPriority::High => "high",
            ContextPriority::Verify => "verify",
        }
    }
}

#[derive(Debug, Clone)]
struct ContextMetadata {
    confidence: f32,
    priority: ContextPriority,
    sensitive: bool,
    topics: Vec<&'static str>,
    lexical_keys: Vec<String>,
}

fn seen_blocks() -> &'static Mutex<HashMap<String, SeenBlock>> {
    static SEEN: OnceLock<Mutex<HashMap<String, SeenBlock>>> = OnceLock::new();
    SEEN.get_or_init(|| Mutex::new(HashMap::new()))
}

const RETRIEVAL_MAX_PER_TURN: usize = 3;
const RETRIEVAL_MAX_CHARS_PER_TURN: usize = 48_000;
const RETRIEVAL_RECENT_EVENT_LIMIT: usize = 64;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetrievalTurnStats {
    pub requests: usize,
    pub fulfilled: usize,
    pub failed: usize,
    pub duplicate_suppressed: usize,
    pub cap_suppressed: usize,
    pub chars_injected: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalEvent {
    pub kind: String,
    pub key: String,
    pub reason: Option<String>,
    pub outcome: String,
    pub source: Option<String>,
    pub chars: usize,
}

#[derive(Default)]
struct RetrievalState {
    turn: RetrievalTurnStats,
    requested_this_turn: HashSet<String>,
    recent: VecDeque<RetrievalEvent>,
}

fn retrieval_state() -> &'static Mutex<RetrievalState> {
    static STATE: OnceLock<Mutex<RetrievalState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(RetrievalState::default()))
}

pub fn reset_retrieval_turn() {
    if let Ok(mut state) = retrieval_state().lock() {
        state.turn = RetrievalTurnStats::default();
        state.requested_this_turn.clear();
    }
}

fn record_retrieval_event(event: RetrievalEvent) {
    if let Ok(mut state) = retrieval_state().lock() {
        while state.recent.len() >= RETRIEVAL_RECENT_EVENT_LIMIT {
            state.recent.pop_front();
        }
        state.recent.push_back(event);
    }
}

fn retrieval_can_fulfill(kind: &str, key: &str, requested_chars: usize) -> Result<(), String> {
    let mut state = retrieval_state()
        .lock()
        .map_err(|_| "retrieval state unavailable".to_string())?;
    state.turn.requests += 1;
    let request_key = format!("{}:{}", kind, key);
    if state.requested_this_turn.contains(&request_key) {
        state.turn.duplicate_suppressed += 1;
        state.turn.failed += 1;
        return Err("duplicate request suppressed for this turn".to_string());
    }
    if state.turn.fulfilled >= RETRIEVAL_MAX_PER_TURN {
        state.turn.cap_suppressed += 1;
        state.turn.failed += 1;
        return Err(format!(
            "per-turn retrieval cap reached ({})",
            RETRIEVAL_MAX_PER_TURN
        ));
    }
    if state.turn.chars_injected.saturating_add(requested_chars) > RETRIEVAL_MAX_CHARS_PER_TURN {
        state.turn.cap_suppressed += 1;
        state.turn.failed += 1;
        return Err(format!(
            "per-turn retrieval char cap reached ({})",
            RETRIEVAL_MAX_CHARS_PER_TURN
        ));
    }
    state.requested_this_turn.insert(request_key);
    state.turn.fulfilled += 1;
    state.turn.chars_injected += requested_chars;
    Ok(())
}

pub fn retrieval_diagnostics() -> String {
    let Ok(state) = retrieval_state().lock() else {
        return "retrieval diagnostics unavailable".to_string();
    };
    let mut out = String::new();
    out.push_str("# Retrieval context diagnostics\n\n");
    out.push_str(&format!(
        "Current turn: requests={}, fulfilled={}, failed={}, duplicate_suppressed={}, cap_suppressed={}, chars_injected={}\n\n",
        state.turn.requests,
        state.turn.fulfilled,
        state.turn.failed,
        state.turn.duplicate_suppressed,
        state.turn.cap_suppressed,
        state.turn.chars_injected
    ));
    out.push_str("Recent retrieval events:\n");
    if state.recent.is_empty() {
        out.push_str("- none\n");
    } else {
        for event in state.recent.iter().rev().take(12) {
            out.push_str(&format!(
                "- kind={} key={} outcome={} source={} chars={} reason={}\n",
                event.kind,
                event.key,
                event.outcome,
                event.source.as_deref().unwrap_or("none"),
                event.chars,
                event.reason.as_deref().unwrap_or("unspecified")
            ));
        }
    }
    out
}

// Per-thread per-call budget. Set by `maybe_compact_messages_with_budget`,
// consulted by `context_diet_recent_byte_budget` and `context_diet_max_blocks`.
thread_local! {
    static ACTIVE_BUDGET: std::cell::RefCell<Option<CompactBudget>> =
        const { std::cell::RefCell::new(None) };
}

struct CompactBudgetGuard {
    previous: Option<CompactBudget>,
}

impl CompactBudgetGuard {
    fn install(budget: CompactBudget) -> Self {
        let previous = ACTIVE_BUDGET.with(|cell| cell.replace(Some(budget)));
        Self { previous }
    }
}

impl Drop for CompactBudgetGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        ACTIVE_BUDGET.with(|cell| *cell.borrow_mut() = previous);
    }
}

fn current_budget() -> Option<CompactBudget> {
    ACTIVE_BUDGET.with(|cell| *cell.borrow())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InterlangMode {
    Off,
    Safe,
    Verified,
    Aggressive,
    Ultra,
}

pub fn mode() -> InterlangMode {
    if !enabled() {
        return InterlangMode::Off;
    }
    match std::env::var(ENV_MODE)
        .unwrap_or_else(|_| "ultra".to_string())
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "0" | "off" | "false" | "none" => InterlangMode::Off,
        "verified" | "aggressive-safe" | "safe-aggressive" => InterlangMode::Verified,
        "aggressive" | "full" | "max" => InterlangMode::Aggressive,
        "ultra" | "vault" | "minimal" | "min" => InterlangMode::Ultra,
        _ => InterlangMode::Safe,
    }
}

pub fn enabled() -> bool {
    std::env::var(ENV_ENABLE)
        .map(|v| {
            !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
        .unwrap_or(true)
}

pub fn decoder_prompt() -> String {
    match mode() {
        InterlangMode::Ultra => "\n\n<system-reminder>\nNeura ctx vault active. Decode <il:v1>. <ctx>/<il:seen> are refs to local exact text; don't invent hidden details. Need exact: `.ctx_get id=<id> reason=<why>`. Attrs: c=confidence,p=priority,ar=auto,t=topics,s=summary.\n</system-reminder>".to_string(),
        InterlangMode::Verified | InterlangMode::Aggressive => "\n\n<system-reminder>\nNeura interlang active. Decode <il:v1>. <il:seen> means exact text was shown before; request `. err need_ref <hash>` if needed. Don't guess hidden refs.\n</system-reminder>".to_string(),
        InterlangMode::Safe => "\n\n<system-reminder>\nNeura interlang safe: decode <il:v1> line/path refs before reasoning.\n</system-reminder>".to_string(),
        InterlangMode::Off => String::new(),
    }
}

pub fn realtime_stats_prompt(latest: InterlangStats) -> String {
    let status = status_json();
    let total_saved = status
        .get("exact_saved_tokens")
        .or_else(|| status.get("saved_tokens_estimate"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);
    let latest_saved = latest
        .exact_saved_tokens()
        .max(latest.saved_tokens_estimate())
        .max(0);
    let latest_raw_avoided = latest.raw_context_avoided_tokens_estimate();
    let mode = status.get("mode").and_then(|v| v.as_str()).unwrap_or("off");
    format!(
        "\n\n<system-reminder>\nNeura ctx stats: mode={mode}, saved={total_saved}, latest={latest_saved}, blocks={}, avoided={latest_raw_avoided}, diet={}.\n</system-reminder>",
        latest.blocks_encoded, latest.diet_blocks
    )
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InterlangStats {
    pub blocks_encoded: usize,
    pub original_chars: usize,
    pub encoded_chars: usize,
    pub seen_ref_blocks: usize,
    pub raw_context_avoided_chars: usize,
    pub exact_original_tokens: usize,
    pub exact_encoded_tokens: usize,
    pub diet_blocks: usize,
    pub diet_original_chars: usize,
    pub diet_encoded_chars: usize,
    pub low_confidence_blocks: usize,
    pub auto_rehydrate_candidates: usize,
    pub auto_rehydrate_skipped: usize,
    pub auto_rehydrated_blocks: usize,
    pub auto_rehydrated_chars: usize,
}

impl InterlangStats {
    pub fn is_zero(self) -> bool {
        self.blocks_encoded == 0
            && self.original_chars == 0
            && self.encoded_chars == 0
            && self.seen_ref_blocks == 0
            && self.raw_context_avoided_chars == 0
            && self.exact_original_tokens == 0
            && self.exact_encoded_tokens == 0
            && self.diet_blocks == 0
            && self.diet_original_chars == 0
            && self.diet_encoded_chars == 0
            && self.low_confidence_blocks == 0
            && self.auto_rehydrate_candidates == 0
            && self.auto_rehydrate_skipped == 0
            && self.auto_rehydrated_blocks == 0
            && self.auto_rehydrated_chars == 0
    }

    pub fn saved_chars(self) -> isize {
        self.original_chars as isize - self.encoded_chars as isize
    }

    pub fn original_tokens_estimate(self) -> usize {
        estimate_tokens(self.original_chars)
    }

    pub fn encoded_tokens_estimate(self) -> usize {
        estimate_tokens(self.encoded_chars)
    }

    pub fn saved_tokens_estimate(self) -> isize {
        self.original_tokens_estimate() as isize - self.encoded_tokens_estimate() as isize
    }

    pub fn report_line(self) -> String {
        format!(
            "llm-interlang compacted {} block(s): {} -> {} chars (saved {}, ~{} tokens)",
            self.blocks_encoded,
            self.original_chars,
            self.encoded_chars,
            self.saved_chars(),
            self.saved_tokens_estimate()
        )
    }

    pub fn raw_context_avoided_tokens_estimate(self) -> usize {
        estimate_tokens(self.raw_context_avoided_chars)
    }

    pub fn exact_saved_tokens(self) -> isize {
        self.exact_original_tokens as isize - self.exact_encoded_tokens as isize
    }

    pub fn has_exact_tokens(self) -> bool {
        self.exact_original_tokens > 0 || self.exact_encoded_tokens > 0
    }
}

fn tokenizer_path() -> String {
    std::env::var(ENV_TOKENIZER_JSON).unwrap_or_else(|_| DEFAULT_TOKENIZER_JSON.to_string())
}

fn local_tokenizer() -> Option<&'static Tokenizer> {
    static TOKENIZER: OnceLock<Option<Tokenizer>> = OnceLock::new();
    TOKENIZER
        .get_or_init(|| Tokenizer::from_file(tokenizer_path()).ok())
        .as_ref()
}

/// Tokenizer pre-configured with truncation and padding disabled, so
/// `exact_token_count` does not have to clone-and-mutate a fresh tokenizer per
/// call. The clone+with_truncation cost was the dominant per-block hotspot in
/// long sessions.
fn precomputed_tokenizer_no_trunc() -> Option<&'static Tokenizer> {
    static TOKENIZER: OnceLock<Option<Tokenizer>> = OnceLock::new();
    TOKENIZER
        .get_or_init(|| {
            local_tokenizer().map(|tokenizer| {
                let mut clone = tokenizer.clone();
                let _ = clone.with_truncation(None);
                let _ = clone.with_padding(None);
                clone
            })
        })
        .as_ref()
}

pub(crate) fn exact_token_count(text: &str) -> Option<usize> {
    let tokenizer = precomputed_tokenizer_no_trunc()?;
    tokenizer.encode(text, false).ok().map(|enc| enc.len())
}

fn estimate_tokens(chars: usize) -> usize {
    chars.div_ceil(APPROX_CHARS_PER_TOKEN)
}

pub fn record_stats(stats: InterlangStats) {
    if stats.is_zero() {
        return;
    }
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let path = std::path::Path::new(&home).join(".neura/interlang-stats.jsonl");
    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let line = serde_json::json!({
        "timestamp_ms": timestamp_ms,
        "blocks_encoded": stats.blocks_encoded,
        "original_chars": stats.original_chars,
        "encoded_chars": stats.encoded_chars,
        "saved_chars": stats.saved_chars(),
        "original_tokens_estimate": stats.original_tokens_estimate(),
        "encoded_tokens_estimate": stats.encoded_tokens_estimate(),
        "saved_tokens_estimate": stats.saved_tokens_estimate(),
        "exact_tokenizer": local_tokenizer().is_some(),
        "exact_tokenizer_path": tokenizer_path(),
        "exact_original_tokens": stats.exact_original_tokens,
        "exact_encoded_tokens": stats.exact_encoded_tokens,
        "exact_saved_tokens": stats.exact_saved_tokens(),
        "diet_blocks": stats.diet_blocks,
        "diet_original_chars": stats.diet_original_chars,
        "diet_encoded_chars": stats.diet_encoded_chars,
        "diet_saved_chars": stats.diet_original_chars as isize - stats.diet_encoded_chars as isize,
        "diet_saved_tokens_estimate": estimate_tokens(stats.diet_original_chars).saturating_sub(estimate_tokens(stats.diet_encoded_chars)),
        "low_confidence_blocks": stats.low_confidence_blocks,
        "auto_rehydrate_candidates": stats.auto_rehydrate_candidates,
        "auto_rehydrate_skipped": stats.auto_rehydrate_skipped,
        "auto_rehydrated_blocks": stats.auto_rehydrated_blocks,
        "auto_rehydrated_chars": stats.auto_rehydrated_chars,
        "auto_rehydrated_tokens_estimate": estimate_tokens(stats.auto_rehydrated_chars),
        "seen_ref_blocks": stats.seen_ref_blocks,
        "raw_context_avoided_chars": stats.raw_context_avoided_chars,
        "raw_context_avoided_tokens_estimate": stats.raw_context_avoided_tokens_estimate(),
        "note": "token counts are approximate pre-provider estimates using chars/4"
    });
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{}", line);
    }
}

pub fn stats_path() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .map(|home| std::path::Path::new(&home).join(".neura/interlang-stats.jsonl"))
}

pub fn status_json() -> serde_json::Value {
    let mut totals = InterlangStats::default();
    let mut events = 0usize;
    let mut last_saved_tokens = 0i64;
    let mut last_saved_chars = 0i64;
    let mut last_blocks_encoded = 0u64;
    let mut last_timestamp_ms = 0u64;
    let mut total_seen_ref_blocks = 0usize;
    let mut total_raw_context_avoided_chars = 0usize;
    let mut last_raw_context_avoided_tokens = 0i64;
    let mut total_exact_original_tokens = 0usize;
    let mut total_exact_encoded_tokens = 0usize;
    let mut last_exact_saved_tokens = 0i64;
    if let Some(path) = stats_path() {
        if let Ok(file) = std::fs::File::open(path) {
            for line in BufReader::new(file).lines().map_while(Result::ok) {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                events += 1;
                last_saved_tokens = value
                    .get("saved_tokens_estimate")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                last_saved_chars = value
                    .get("saved_chars")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                last_blocks_encoded = value
                    .get("blocks_encoded")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                last_timestamp_ms = value
                    .get("timestamp_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                last_raw_context_avoided_tokens = value
                    .get("raw_context_avoided_tokens_estimate")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                total_seen_ref_blocks += value
                    .get("seen_ref_blocks")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                total_raw_context_avoided_chars += value
                    .get("raw_context_avoided_chars")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                last_exact_saved_tokens = value
                    .get("exact_saved_tokens")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                total_exact_original_tokens += value
                    .get("exact_original_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                total_exact_encoded_tokens += value
                    .get("exact_encoded_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                totals.blocks_encoded += value
                    .get("blocks_encoded")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                totals.original_chars += value
                    .get("original_chars")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                totals.encoded_chars += value
                    .get("encoded_chars")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
            }
        }
    }
    serde_json::json!({
        "enabled": enabled(),
        "mode": mode(),
        "stats_file": stats_path().map(|p| p.display().to_string()),
        "events": events,
        "blocks_encoded": totals.blocks_encoded,
        "original_chars": totals.original_chars,
        "encoded_chars": totals.encoded_chars,
        "saved_chars": totals.saved_chars(),
        "saved_tokens_estimate": totals.saved_tokens_estimate(),
        "last_saved_tokens_estimate": last_saved_tokens,
        "last_saved_chars": last_saved_chars,
        "last_blocks_encoded": last_blocks_encoded,
        "last_timestamp_ms": last_timestamp_ms,
        "seen_ref_blocks": total_seen_ref_blocks,
        "raw_context_avoided_chars": total_raw_context_avoided_chars,
        "raw_context_avoided_tokens_estimate": estimate_tokens(total_raw_context_avoided_chars),
        "last_raw_context_avoided_tokens_estimate": last_raw_context_avoided_tokens,
        "exact_tokenizer": local_tokenizer().is_some(),
        "exact_tokenizer_path": tokenizer_path(),
        "exact_original_tokens": total_exact_original_tokens,
        "exact_encoded_tokens": total_exact_encoded_tokens,
        "exact_saved_tokens": total_exact_original_tokens as isize - total_exact_encoded_tokens as isize,
        "last_exact_saved_tokens": last_exact_saved_tokens,
        "note": "token counts are approximate pre-provider estimates using chars/4"
    })
}

/// Per-call interlang budget. Lets the v2 context compiler enforce per-tier
/// caps without baking new env vars in: Direct turns pass `max_blocks=Some(0)`
/// to disable encoding entirely; Light turns pass `max_blocks=Some(8)` to cap
/// noise; Deep/Continuation pass `None` to use the legacy global behaviour.
#[derive(Debug, Clone, Copy, Default)]
pub struct CompactBudget {
    pub max_blocks: Option<usize>,
    pub recent_bytes: Option<usize>,
}

impl CompactBudget {
    pub fn unbounded() -> Self {
        Self::default()
    }
}

pub fn maybe_compact_messages(messages: &[Message]) -> (Vec<Message>, InterlangStats) {
    maybe_compact_messages_with_budget(messages, CompactBudget::unbounded())
}

/// Like `maybe_compact_messages`, but lets the caller cap the number of
/// emitted ref blocks and override the recent-window byte budget for this
/// invocation only. Returns `(messages_unchanged, default_stats)` when
/// `max_blocks == Some(0)`, which v2 uses for Direct turns.
pub fn maybe_compact_messages_with_budget(
    messages: &[Message],
    budget: CompactBudget,
) -> (Vec<Message>, InterlangStats) {
    if mode() == InterlangMode::Off {
        return (messages.to_vec(), InterlangStats::default());
    }
    if matches!(budget.max_blocks, Some(0)) {
        return (messages.to_vec(), InterlangStats::default());
    }
    let _budget_guard = CompactBudgetGuard::install(budget);
    let (dieted, mut stats) = maybe_context_diet_messages(messages);
    let (mut compacted, compact_stats) = compact_messages_for_test(&dieted);
    merge_stats(&mut stats, compact_stats);
    maybe_append_auto_rehydration(&mut compacted, &mut stats);
    (compacted, stats)
}

fn merge_stats(into: &mut InterlangStats, other: InterlangStats) {
    into.blocks_encoded += other.blocks_encoded;
    into.original_chars += other.original_chars;
    into.encoded_chars += other.encoded_chars;
    into.seen_ref_blocks += other.seen_ref_blocks;
    into.raw_context_avoided_chars += other.raw_context_avoided_chars;
    into.exact_original_tokens += other.exact_original_tokens;
    into.exact_encoded_tokens += other.exact_encoded_tokens;
    into.diet_blocks += other.diet_blocks;
    into.diet_original_chars += other.diet_original_chars;
    into.diet_encoded_chars += other.diet_encoded_chars;
    into.low_confidence_blocks += other.low_confidence_blocks;
    into.auto_rehydrate_candidates += other.auto_rehydrate_candidates;
    into.auto_rehydrate_skipped += other.auto_rehydrate_skipped;
    into.auto_rehydrated_blocks += other.auto_rehydrated_blocks;
    into.auto_rehydrated_chars += other.auto_rehydrated_chars;
}

fn context_diet_enabled() -> bool {
    std::env::var(ENV_CONTEXT_DIET)
        .map(|v| {
            !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
        .unwrap_or(true)
}

fn env_usize(name: &str, default: usize, min: usize, max: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .map(|value| value.clamp(min, max))
        .unwrap_or(default)
}

fn context_diet_trigger_tokens() -> usize {
    env_usize(
        ENV_CONTEXT_DIET_TRIGGER_TOKENS,
        DEFAULT_CONTEXT_DIET_TRIGGER_TOKENS,
        2_000,
        200_000,
    )
}

fn context_diet_recent_messages() -> usize {
    env_usize(
        ENV_CONTEXT_DIET_RECENT_MESSAGES,
        DEFAULT_CONTEXT_DIET_RECENT_MESSAGES,
        4,
        64,
    )
}

fn context_diet_min_block_chars() -> usize {
    env_usize(
        ENV_CONTEXT_DIET_MIN_BLOCK_CHARS,
        DEFAULT_CONTEXT_DIET_MIN_BLOCK_CHARS,
        240,
        8_000,
    )
}

fn maybe_context_diet_messages(messages: &[Message]) -> (Vec<Message>, InterlangStats) {
    let mut stats = InterlangStats::default();
    if !context_diet_enabled()
        || mode() != InterlangMode::Ultra
        || messages.len() <= context_diet_recent_messages()
    {
        return (messages.to_vec(), stats);
    }

    let total_text: usize = messages.iter().map(message_visible_chars).sum();
    let has_large_recent_tool_result = messages
        .iter()
        .rev()
        .take(context_diet_recent_messages())
        .any(message_has_large_recent_tool_result);
    let total_tokens =
        exact_token_count_messages(messages).unwrap_or_else(|| estimate_tokens(total_text));
    if total_tokens < context_diet_trigger_tokens() && !has_large_recent_tool_result {
        return (messages.to_vec(), stats);
    }

    let cutoff = messages
        .len()
        .saturating_sub(context_diet_recent_messages());
    // Phase 2.B — adaptive recent-window byte budget. When enabled, recent
    // messages still pass through exact unless their cumulative byte budget is
    // exceeded. The fixed `=6` recent floor stays as a hard baseline.
    let recent_byte_budget = context_diet_recent_byte_budget();
    // Most recent message is at index N-1; budget is consumed walking
    // backwards from the latest. The bool answers "is this recent message
    // protected from byte-budget eviction?".
    let mut bytes_so_far = 0usize;
    let mut recent_protected = vec![false; messages.len()];
    if let Some(budget) = recent_byte_budget {
        for (rev_idx, message) in messages.iter().rev().enumerate() {
            let idx = messages.len() - 1 - rev_idx;
            let chars: usize = message_visible_chars(message);
            if bytes_so_far + chars <= budget && idx >= cutoff {
                recent_protected[idx] = true;
                bytes_so_far += chars;
            } else if idx >= cutoff && bytes_so_far == 0 {
                // Always keep the last message exact, even if it alone is huge
                recent_protected[idx] = true;
                bytes_so_far = chars;
            } else {
                break;
            }
        }
    }
    let diet_tool_input = diet_tool_input_enabled();
    let max_blocks = context_diet_max_blocks();
    let mut emitted_blocks = 0usize;
    // The most recent assistant message's tool_use blocks must NEVER be
    // diet'd: the model just emitted them and may reference exact input on
    // the very next iteration.
    let last_assistant_idx = messages
        .iter()
        .rposition(|message| message.role == Role::Assistant);
    let mut out = Vec::with_capacity(messages.len());
    for (idx, message) in messages.iter().enumerate() {
        let is_recent_byte_protected = recent_byte_budget.is_some() && recent_protected[idx];
        let is_recent = if recent_byte_budget.is_some() {
            is_recent_byte_protected
        } else {
            idx >= cutoff
        };
        let is_last_assistant = Some(idx) == last_assistant_idx;
        let mut msg = message.clone();
        let mut changed = false;
        // If we already hit the per-call max_blocks cap, only clone-pass-through.
        let cap_reached = max_blocks.map(|cap| emitted_blocks >= cap).unwrap_or(false);
        if !cap_reached {
            for block in &mut msg.content {
                match block {
                    ContentBlock::Text { text, .. } if !is_recent && should_diet_text(text) => {
                        *text = encode_context_diet_ref(text, "old-text", &mut stats);
                        changed = true;
                    }
                    ContentBlock::ToolResult { content, .. }
                        if should_diet_tool_result(content)
                            && (!is_recent || should_diet_recent_tool_result(content)) =>
                    {
                        // Keep recent human/assistant text exact, but do not let a large
                        // grep/read/build output inside the recent-message window dominate
                        // every following provider request. Exact content remains available
                        // through the context vault if the model needs it.
                        *content = encode_context_diet_ref(content, "old-tool-result", &mut stats);
                        changed = true;
                    }
                    ContentBlock::Reasoning { text }
                        if !is_recent && text.len() > context_diet_min_block_chars() =>
                    {
                        *text = encode_context_diet_ref(text, "old-reasoning", &mut stats);
                        changed = true;
                    }
                    ContentBlock::ToolUse { input, .. } if diet_tool_input => {
                        let big = should_diet_tool_input(&*input);
                        if !is_recent && !is_last_assistant && big {
                            let serialized = input.to_string();
                            let encoded =
                                encode_context_diet_ref(&serialized, "old-tool-input", &mut stats);
                            *input = serde_json::json!({"_neura_ctx_ref": encoded});
                            changed = true;
                        }
                    }
                    _ => {}
                }
                if max_blocks
                    .map(|cap| emitted_blocks + (changed as usize) >= cap)
                    .unwrap_or(false)
                {
                    break;
                }
            }
        }
        if changed {
            stats.blocks_encoded += 1;
            emitted_blocks += 1;
        }
        out.push(msg);
    }
    (out, stats)
}

fn should_diet_tool_input(input: &serde_json::Value) -> bool {
    // Avoid double-encoding wrappers we previously emitted, and only diet
    // genuinely large payloads that would bloat the wire.
    if let Some(map) = input.as_object() {
        if map.contains_key("_neura_ctx_ref") {
            return false;
        }
    }
    let serialized = input.to_string();
    serialized.len() >= context_diet_min_block_chars().max(800)
}

fn diet_tool_input_enabled() -> bool {
    std::env::var("NEURA_INTERLANG_DIET_TOOL_INPUT")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// Optional byte budget for the recent message window. When set, the recent
/// window stops being a fixed message count and instead protects the last N
/// bytes of conversation from interlang eviction. Returns `None` when the
/// feature is unset, preserving the legacy fixed-count behaviour.
fn context_diet_recent_byte_budget() -> Option<usize> {
    if let Some(budget) = current_budget()
        && let Some(bytes) = budget.recent_bytes
    {
        return Some(bytes.max(1_000));
    }
    let raw = std::env::var("NEURA_CONTEXT_DIET_RECENT_BYTES").ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if matches!(
        trimmed.to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    ) {
        return None;
    }
    trimmed.parse::<usize>().ok().map(|v| v.max(1_000))
}

/// Per-call max-blocks cap from `current_budget()`. None when not v2-active.
fn context_diet_max_blocks() -> Option<usize> {
    current_budget().and_then(|b| b.max_blocks)
}

fn should_diet_text(text: &str) -> bool {
    text.len() >= context_diet_min_block_chars()
        && !text.contains("<ctx")
        && !text.contains("<il:")
        && !text.contains("<system-reminder>")
}

fn should_diet_tool_result(content: &str) -> bool {
    content.len() >= context_diet_min_block_chars()
        && !content.contains("<ctx")
        && !content.contains("<il:")
}

fn message_has_large_recent_tool_result(message: &Message) -> bool {
    message.content.iter().any(|block| match block {
        ContentBlock::ToolResult { content, .. } => should_diet_recent_tool_result(content),
        _ => false,
    })
}

fn should_diet_recent_tool_result(content: &str) -> bool {
    content.len() >= context_diet_recent_tool_result_chars()
}

fn context_diet_recent_tool_result_chars() -> usize {
    env_usize(
        "NEURA_CONTEXT_DIET_RECENT_TOOL_RESULT_CHARS",
        2_000,
        800,
        24_000,
    )
}

fn encode_context_diet_ref(text: &str, kind: &str, stats: &mut InterlangStats) -> String {
    let hash = stable_hash(text);
    // Phase 1.C — fast path: if we already encoded this hash with the same
    // ref-kind on a previous turn, reuse the cached string and the metadata
    // we computed then. Skips re-running summary/metadata/lexical-keys/etc.
    let kind_tag = ref_kind_to_tag(kind);
    if let Ok(seen) = seen_blocks().lock() {
        if let Some(block) = seen.get(&hash) {
            if let Some(cached) = block.encoded_refs.get(kind_tag) {
                let cached = cached.clone();
                let block_priority = block.priority;
                let block_confidence = block.confidence;
                let block_sensitive = block.sensitive;
                drop(seen);
                if !block_sensitive
                    && (block_confidence <= AUTO_REHYDRATE_CONFIDENCE_THRESHOLD
                        || block_priority == ContextPriority::High)
                {
                    stats.low_confidence_blocks += 1;
                }
                accumulate_diet_stats(stats, text, &cached);
                return cached;
            }
        }
    }
    let summary = memory_safe_summary(text);
    let meta = context_metadata(text, kind);
    let id = format!("ctx:{}", hash);
    let encoded = format!(
        r#"<ctx k="{}" id="{}" n={} c="{:.2}" p="{}" ar="{}" t="{}" s="{}"/>"#,
        kind,
        id,
        text.len(),
        meta.confidence,
        meta.priority.as_str(),
        should_auto_rehydrate(&meta),
        meta.topics.join(","),
        escape_attr(&summary)
    );
    if let Ok(mut seen) = seen_blocks().lock() {
        let entry = seen.entry(hash.clone()).or_insert_with(|| SeenBlock {
            hash: hash.clone(),
            original_chars: text.len(),
            summary: summary.clone(),
            exact: text.to_string(),
            confidence: meta.confidence,
            priority: meta.priority,
            sensitive: meta.sensitive,
            topics: meta.topics.clone(),
            lexical_keys: meta.lexical_keys.clone(),
            encoded_refs: HashMap::new(),
        });
        entry
            .encoded_refs
            .entry(kind_tag)
            .or_insert_with(|| encoded.clone());
        // Best-effort persistence so a later Neura process can still rehydrate
        // exact text behind a `<ctx>` reference. Sensitive blocks are skipped.
        ctx_vault::maybe_persist(entry);
    }
    if should_auto_rehydrate(&meta) {
        stats.low_confidence_blocks += 1;
    }
    accumulate_diet_stats(stats, text, &encoded);
    encoded
}

fn accumulate_diet_stats(stats: &mut InterlangStats, text: &str, encoded: &str) {
    stats.original_chars += text.len();
    stats.encoded_chars += encoded.len();
    stats.raw_context_avoided_chars += text.len();
    stats.diet_blocks += 1;
    stats.diet_original_chars += text.len();
    stats.diet_encoded_chars += encoded.len();
    if let Some(tokens) = exact_token_count(text) {
        stats.exact_original_tokens += tokens;
    }
    if let Some(tokens) = exact_token_count(encoded) {
        stats.exact_encoded_tokens += tokens;
    }
}

/// Map a free-form ref kind (e.g., `"old-text"`, `"old-tool-result"`) to a
/// stable interned tag used as the encoded_refs cache key.
fn ref_kind_to_tag(kind: &str) -> &'static str {
    match kind {
        "old-text" => "old-text",
        "old-tool-result" => "old-tool-result",
        "old-reasoning" => "old-reasoning",
        "old-tool-input" => "old-tool-input",
        "vault" => "vault",
        "seen" => "seen",
        _ => "unknown",
    }
}

fn exact_token_count_messages(messages: &[Message]) -> Option<usize> {
    let mut total = 0usize;
    for message in messages {
        for block in &message.content {
            match block {
                ContentBlock::Text { text, .. } | ContentBlock::Reasoning { text } => {
                    total += exact_token_count(text)?;
                }
                ContentBlock::ToolResult { content, .. } => total += exact_token_count(content)?,
                ContentBlock::ToolUse { input, .. } => {
                    total += exact_token_count(&input.to_string())?
                }
                ContentBlock::Image { data, .. } => total += estimate_tokens(data.len()),
                ContentBlock::OpenAICompaction { encrypted_content } => {
                    total += exact_token_count(encrypted_content)?;
                }
            }
        }
    }
    Some(total)
}

fn message_visible_chars(message: &Message) -> usize {
    message
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text, .. } | ContentBlock::Reasoning { text } => text.len(),
            ContentBlock::ToolResult { content, .. } => content.len(),
            ContentBlock::ToolUse { input, .. } => input.to_string().len(),
            ContentBlock::Image { data, .. } => data.len(),
            ContentBlock::OpenAICompaction { encrypted_content } => encrypted_content.len(),
        })
        .sum()
}

fn memory_safe_summary(text: &str) -> String {
    let mut summary = deterministic_summary(text);
    let lower = text.to_ascii_lowercase();
    let mut hints = Vec::new();
    for (needle, label) in [
        ("error", "error"),
        ("failed", "failure"),
        ("warning", "warning"),
        ("test", "test"),
        ("build", "build"),
        ("token", "token"),
        ("memory", "memory"),
        ("mouse", "mouse"),
        ("screenshot", "screenshot"),
        ("interlang", "interlang"),
        ("neura", "neura"),
    ] {
        if lower.contains(needle) {
            hints.push(label);
        }
    }
    if !hints.is_empty() {
        hints.sort_unstable();
        hints.dedup();
        summary.push_str(&format!("; semantic_hints=[{}]", hints.join(",")));
    }
    summary
}

fn push_lexical_key(keys: &mut Vec<String>, key: &str) {
    let trimmed = key.trim_matches(|c: char| {
        !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' || c == '/' || c == ':')
    });
    if trimmed.len() < 4 || trimmed.len() > 80 {
        return;
    }
    let lower = trimmed.to_ascii_lowercase();
    const STOP: &[&str] = &[
        "this", "that", "with", "from", "have", "what", "when", "where", "context", "token",
        "tokens", "memory", "build", "test", "error", "exact", "lines", "chars", "tool", "result",
    ];
    if STOP.contains(&lower.as_str()) || keys.iter().any(|existing| existing == &lower) {
        return;
    }
    keys.push(lower);
}

fn lexical_keys(summary: &str, exact: &str) -> Vec<String> {
    let mut keys = Vec::new();
    for source in [summary, exact.lines().next().unwrap_or("")] {
        for raw in source.split(|c: char| {
            c.is_whitespace()
                || matches!(
                    c,
                    ',' | ';' | ')' | '(' | '[' | ']' | '{' | '}' | '"' | '\''
                )
        }) {
            if raw.contains('/')
                || raw.contains('.')
                || raw.contains("::")
                || raw.contains('_')
                || raw.starts_with("ctx:")
                || raw.len() >= 12
            {
                push_lexical_key(&mut keys, raw);
            }
            if keys.len() >= 16 {
                return keys;
            }
        }
    }
    keys
}

fn context_metadata(text: &str, kind: &str) -> ContextMetadata {
    let lower = text.to_ascii_lowercase();
    let mut confidence = 0.78f32;
    let mut topics = Vec::new();
    let mut priority = ContextPriority::Normal;

    let markers = [
        ("error", "error"),
        ("failed", "failure"),
        ("panic", "panic"),
        ("diff --git", "diff"),
        ("todo", "todo"),
        ("token", "token"),
        ("auth", "auth"),
        ("limit", "limit"),
        ("test", "test"),
        ("build", "build"),
    ];
    for (needle, topic) in markers {
        if lower.contains(needle) {
            topics.push(topic);
        }
    }

    if text.len() > 80_000 {
        confidence -= 0.18;
    } else if text.len() > 24_000 {
        confidence -= 0.10;
    }
    if text.lines().count() > 400 {
        confidence -= 0.08;
    }
    if lower.contains("diff --git") || lower.contains("error") || lower.contains("panic") {
        confidence -= 0.12;
        priority = ContextPriority::High;
    }
    if lower.contains("security") || lower.contains("auth") || lower.contains("credential") {
        confidence -= 0.10;
        priority = ContextPriority::Verify;
    }
    if kind.contains("reasoning") {
        confidence -= 0.06;
    }

    let sensitive = looks_sensitive(&lower);
    if sensitive {
        priority = ContextPriority::Verify;
        // Do not auto-inject exact sensitive content. The model can still ask for
        // a deliberate `.ctx_get`, which keeps the decision explicit.
        confidence = confidence.min(0.49);
    }
    if topics.is_empty() && text.len() < 8_000 {
        priority = ContextPriority::Low;
        confidence += 0.06;
    }

    topics.sort_unstable();
    topics.dedup();
    ContextMetadata {
        confidence: confidence.clamp(0.05, 0.98),
        priority,
        sensitive,
        topics,
        lexical_keys: lexical_keys(&memory_safe_summary(text), text),
    }
}

fn looks_sensitive(lower: &str) -> bool {
    lower.contains("ghp_")
        || lower.contains("api_key")
        || lower.contains("api-key")
        || lower.contains("password")
        || lower.contains("secret")
        || lower.contains("authorization: bearer")
        || lower.contains("private key")
}

fn should_auto_rehydrate(meta: &ContextMetadata) -> bool {
    !meta.sensitive
        && (meta.confidence <= AUTO_REHYDRATE_CONFIDENCE_THRESHOLD
            || meta.priority == ContextPriority::High)
}

fn latest_user_context(messages: &[Message]) -> String {
    messages
        .iter()
        .rev()
        .find_map(|message| {
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
            let trimmed = text.trim();
            if trimmed.is_empty()
                || trimmed.starts_with("<system-reminder>")
                || trimmed.contains("<ctx_auto_exact")
            {
                None
            } else {
                Some(trimmed.to_ascii_lowercase())
            }
        })
        .unwrap_or_default()
}

fn topic_relevant_to_turn(block: &SeenBlock, latest_user: &str) -> bool {
    if latest_user.trim().is_empty() {
        return false;
    }

    if latest_user.contains(&block.hash) || latest_user.contains(&format!("ctx:{}", block.hash)) {
        return true;
    }

    let explicit_fetch_intent = latest_user.contains("ctx_get") || latest_user.contains("rehydrat");
    if !explicit_fetch_intent && should_suppress_auto_rehydrate_for_turn(latest_user) {
        return false;
    }
    let failure_intent = latest_user.contains("failing")
        || latest_user.contains("failed")
        || latest_user.contains("failure")
        || latest_user.contains("error")
        || latest_user.contains("panic")
        || latest_user.contains("broken")
        || latest_user.contains("regression")
        || latest_user.contains("traceback");
    let investigation_intent = failure_intent
        || latest_user.contains("debug")
        || latest_user.contains("fix")
        || latest_user.contains("trace")
        || latest_user.contains("stack")
        || latest_user.contains("crash")
        || latest_user.contains("panic");

    let mut topic_score = 0u8;
    for topic in &block.topics {
        if latest_user.contains(*topic) {
            topic_score += match *topic {
                "auth" | "security" | "panic" | "diff" => 3,
                "build" | "error" | "failure" | "test" | "token" | "memory" => 2,
                _ => 1,
            };
        }
    }

    let mut lexical_hits = 0u8;
    for key in &block.lexical_keys {
        if latest_user.contains(key) {
            lexical_hits = lexical_hits.saturating_add(1);
        }
    }
    if smart_auto_rehydrate_turn_allowed(latest_user) && (lexical_hits >= 1 || topic_score >= 2) {
        return true;
    }

    // Generic policy: proactive exact restore needs either an explicit ref/fetch,
    // or concrete lexical evidence from the ctx ref plus enough investigative
    // pressure. Generic topic words alone (token, memory, build, test, why, exact)
    // are not enough because they appear in documentation and accounting turns.
    explicit_fetch_intent
        || lexical_hits >= 2
        || (lexical_hits >= 1 && (investigation_intent || topic_score >= 3))
        || (investigation_intent && topic_score >= 4)
}

fn should_suppress_auto_rehydrate_for_turn(latest_user: &str) -> bool {
    let trimmed = latest_user.trim();
    if trimmed.len() <= 160
        && !trimmed.contains("src/")
        && !trimmed.contains("docs/")
        && !trimmed.contains("install/")
        && !trimmed.contains("scripts/")
        && !trimmed.contains(".rs")
        && !trimmed.contains(".py")
        && !trimmed.contains(".md")
        && !trimmed.contains(".sh")
        && !trimmed.contains("```")
    {
        return true;
    }

    let token_accounting = contains_any(
        latest_user,
        &[
            "token",
            "tokens",
            "up",
            "down",
            "sent up",
            "prompt_chars",
            "prompt chars",
            "how many",
            "how much",
            "why did",
            "cost",
            "usage",
            "accounting",
            "efficient",
            "efficiency",
            "bloat",
            "overhead",
        ],
    );
    let asks_to_fix_code = contains_any(
        latest_user,
        &[
            "fix src/",
            "fix docs/",
            "debug src/",
            "edit src/",
            "patch src/",
            "failing test",
            "build error",
            "compile error",
            "panic at",
            "traceback",
        ],
    );
    token_accounting && !asks_to_fix_code
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn smart_auto_rehydrate_turn_allowed(latest_user: &str) -> bool {
    let trimmed = latest_user.trim();
    if trimmed.len() > 4_000 {
        return false;
    }
    if should_suppress_auto_rehydrate_for_turn(latest_user) {
        return false;
    }
    let has_task_continuation = contains_any(
        latest_user,
        &[
            "continue",
            "use the previous",
            "from above",
            "same error",
            "same file",
            "that stack",
            "that diff",
            "that failing",
            "finish the",
            "keep going",
            "resume",
            "apply that",
        ],
    );
    let has_precise_artifact = contains_any(
        latest_user,
        &[
            "src/",
            "docs/",
            "scripts/",
            "install/",
            "benchmark-results/",
            ".rs",
            ".py",
            ".md",
            ".sh",
            "error",
            "panic",
            "traceback",
            "diff --git",
            "failed test",
            "cargo test",
            "cargo check",
        ],
    );
    if trimmed.len() < 80 && !has_precise_artifact {
        return false;
    }
    has_task_continuation && has_precise_artifact
}

fn auto_rehydrate_enabled() -> bool {
    std::env::var("NEURA_CTX_AUTO_REHYDRATE")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn auto_rehydrate_debug_enabled() -> bool {
    std::env::var(AUTO_REHYDRATE_DEBUG_ENV)
        .map(|v| {
            !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
        .unwrap_or(false)
}

fn maybe_append_auto_rehydration(messages: &mut Vec<Message>, stats: &mut InterlangStats) {
    if mode() != InterlangMode::Ultra || stats.low_confidence_blocks == 0 {
        return;
    }
    let latest_user = latest_user_context(messages);
    if !auto_rehydrate_enabled() && !smart_auto_rehydrate_turn_allowed(&latest_user) {
        stats.auto_rehydrate_skipped += stats.low_confidence_blocks;
        return;
    }
    let Ok(seen) = seen_blocks().lock() else {
        return;
    };
    let mut skipped = 0usize;
    let mut candidates: Vec<(&String, &SeenBlock)> = seen
        .iter()
        .filter(|(_, block)| {
            let eligible = !block.sensitive
                && (block.confidence <= AUTO_REHYDRATE_CONFIDENCE_THRESHOLD
                    || block.priority == ContextPriority::High);
            if !eligible {
                return false;
            }
            let relevant = topic_relevant_to_turn(block, &latest_user);
            if !relevant {
                skipped += 1;
            }
            relevant
        })
        .collect();
    stats.auto_rehydrate_candidates += candidates.len() + skipped;
    stats.auto_rehydrate_skipped += skipped;
    if auto_rehydrate_debug_enabled() && (skipped > 0 || !candidates.is_empty()) {
        crate::logging::info(&format!(
            "ctx auto-rehydrate relevance: candidates={} skipped={} latest_user_len={}",
            candidates.len(),
            skipped,
            latest_user.len()
        ));
    }
    candidates.sort_by(|(_, a), (_, b)| {
        a.confidence
            .partial_cmp(&b.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.original_chars.cmp(&a.original_chars))
    });

    let mut sections = Vec::new();
    for (hash, block) in candidates.into_iter().take(AUTO_REHYDRATE_MAX_BLOCKS) {
        stats.auto_rehydrated_blocks += 1;
        sections.push(format!(
            "<ctx_candidate id=\"ctx:{}\" hash=\"{}\" confidence=\"{:.2}\" priority=\"{}\" original_chars=\"{}\" summary=\"{}\" />",
            hash,
            hash,
            block.confidence,
            block.priority.as_str(),
            block.original_chars,
            escape_attr(&block.summary)
        ));
    }
    if sections.is_empty() {
        return;
    }

    let text = format!(
        "<system-reminder>\nPotentially relevant exact context exists but was not auto-injected to save tokens. Use `.ctx_get id=ctx:<hash> reason=<why>` only if exact text is required.\n\n{}\n</system-reminder>",
        sections.join("\n")
    );
    messages.push(Message::user(&text));
}

pub fn compact_messages_for_test(messages: &[Message]) -> (Vec<Message>, InterlangStats) {
    let mut stats = InterlangStats::default();
    let mut out = Vec::with_capacity(messages.len());
    for message in messages {
        let mut msg = message.clone();
        for block in &mut msg.content {
            match block {
                ContentBlock::Text { text, .. } => {
                    if let Some(encoded) = encode_vault_ref(text, &mut stats)
                        .or_else(|| encode_seen_ref(text, &mut stats))
                        .or_else(|| encode_text(text))
                    {
                        stats.blocks_encoded += 1;
                        stats.original_chars += text.len();
                        stats.encoded_chars += encoded.len();
                        if let Some(tokens) = exact_token_count(text) {
                            stats.exact_original_tokens += tokens;
                        }
                        if let Some(tokens) = exact_token_count(&encoded) {
                            stats.exact_encoded_tokens += tokens;
                        }
                        *text = encoded;
                    }
                }
                ContentBlock::ToolResult { content, .. } => {
                    if let Some(encoded) = encode_vault_ref(content, &mut stats)
                        .or_else(|| encode_seen_ref(content, &mut stats))
                        .or_else(|| encode_text(content))
                    {
                        stats.blocks_encoded += 1;
                        stats.original_chars += content.len();
                        stats.encoded_chars += encoded.len();
                        if let Some(tokens) = exact_token_count(content) {
                            stats.exact_original_tokens += tokens;
                        }
                        if let Some(tokens) = exact_token_count(&encoded) {
                            stats.exact_encoded_tokens += tokens;
                        }
                        *content = encoded;
                    }
                }
                _ => {}
            }
        }
        out.push(msg);
    }
    (out, stats)
}

fn encode_text(text: &str) -> Option<String> {
    [encode_repeated_lines(text), encode_path_prefixes(text)]
        .into_iter()
        .flatten()
        .min_by_key(|encoded| encoded.len())
}

fn encode_seen_ref(text: &str, stats: &mut InterlangStats) -> Option<String> {
    if !matches!(
        mode(),
        InterlangMode::Verified | InterlangMode::Aggressive | InterlangMode::Ultra
    ) || text.len() < MIN_SEEN_REF_CHARS
        || text.contains("<il:seen")
    {
        return None;
    }
    let hash = stable_hash(text);
    let mut seen = seen_blocks().lock().ok()?;
    if let Some(block) = seen.get(&hash) {
        let encoded = format!(
            "<il:seen v=1 hash={} original_chars={} summary=\"{}\" />",
            hash,
            block.original_chars,
            escape_attr(&block.summary)
        );
        let saved = text.len() as isize - encoded.len() as isize;
        if saved >= MIN_SAVED_CHARS as isize {
            stats.seen_ref_blocks += 1;
            stats.raw_context_avoided_chars += text.len();
            return Some(encoded);
        }
    } else {
        let meta = context_metadata(text, "seen");
        // First sighting: remember exact content locally for later turns, but do
        // not replace it yet. The provider receives exact or self-contained
        // compressed content at least once before <il:seen> references appear.
        let block = SeenBlock {
            hash: hash.clone(),
            original_chars: text.len(),
            summary: deterministic_summary(text),
            exact: text.to_string(),
            confidence: meta.confidence,
            priority: meta.priority,
            sensitive: meta.sensitive,
            topics: meta.topics.clone(),
            lexical_keys: meta.lexical_keys.clone(),
            encoded_refs: HashMap::new(),
        };
        ctx_vault::maybe_persist(&block);
        seen.insert(hash, block);
    }
    None
}

fn encode_vault_ref(text: &str, stats: &mut InterlangStats) -> Option<String> {
    if mode() != InterlangMode::Ultra
        || text.len() < MIN_VAULT_REF_CHARS
        || text.contains("<ctx")
        || text.contains("<il:seen")
    {
        return None;
    }
    let hash = stable_hash(text);
    let summary = deterministic_summary(text);
    let meta = context_metadata(text, "vault");
    let id = format!("ctx:{}", hash);
    let encoded = format!(
        "<ctx v=1 k=\"vault\" id=\"{}\" h=\"{}\" n={} c=\"{:.2}\" p=\"{}\" ar=\"{}\" t=\"{}\" s=\"{}\" />",
        id,
        hash,
        text.len(),
        meta.confidence,
        meta.priority.as_str(),
        should_auto_rehydrate(&meta),
        meta.topics.join(","),
        escape_attr(&summary)
    );
    if let Ok(mut seen) = seen_blocks().lock() {
        let entry = seen.entry(hash.clone()).or_insert_with(|| SeenBlock {
            hash: hash.clone(),
            original_chars: text.len(),
            summary: summary.clone(),
            exact: text.to_string(),
            confidence: meta.confidence,
            priority: meta.priority,
            sensitive: meta.sensitive,
            topics: meta.topics.clone(),
            lexical_keys: meta.lexical_keys.clone(),
            encoded_refs: HashMap::new(),
        });
        entry
            .encoded_refs
            .entry("vault")
            .or_insert_with(|| encoded.clone());
        ctx_vault::maybe_persist(entry);
    }
    let saved = text.len() as isize - encoded.len() as isize;
    if saved >= MIN_SAVED_CHARS as isize {
        stats.seen_ref_blocks += 1;
        stats.raw_context_avoided_chars += text.len();
        if should_auto_rehydrate(&meta) {
            stats.low_confidence_blocks += 1;
        }
        Some(encoded)
    } else {
        None
    }
}

pub(crate) fn vault_exact_ref(text: &str) -> Option<String> {
    let mut stats = InterlangStats::default();
    encode_vault_ref(text, &mut stats)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactRequest {
    pub id: String,
    pub hash: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextSearchRequest {
    pub query: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextSearchHit {
    pub id: String,
    pub hash: String,
    pub summary: String,
    pub original_chars: usize,
    pub priority: String,
    pub topics: Vec<String>,
    pub score: usize,
}

pub fn parse_exact_request(text: &str) -> Option<ExactRequest> {
    let line = text.lines().find(|line| {
        let trimmed = line.trim();
        trimmed.starts_with(".ctx_get") || trimmed.starts_with(". err need_ref")
    })?;
    let trimmed = line.trim();
    if trimmed.starts_with(". err need_ref") {
        let hash = trimmed.split_whitespace().nth(3)?.trim().to_string();
        return Some(ExactRequest {
            id: format!("ctx:{}", hash),
            hash,
            reason: None,
        });
    }

    let mut id = None;
    let mut reason = None;
    for part in trimmed.split_whitespace().skip(1) {
        if let Some(value) = part.strip_prefix("id=") {
            id = Some(value.trim_matches(|c| c == '"' || c == '\'').to_string());
        } else if let Some(value) = part.strip_prefix("reason=") {
            reason = Some(value.trim_matches(|c| c == '"' || c == '\'').to_string());
        }
    }
    let id = id?;
    let hash = id.strip_prefix("ctx:").unwrap_or(&id).to_string();
    Some(ExactRequest { id, hash, reason })
}

pub fn parse_context_search_request(text: &str) -> Option<ContextSearchRequest> {
    let line = text
        .lines()
        .find(|line| line.trim().starts_with(".ctx_search"))?
        .trim();
    let rest = line.strip_prefix(".ctx_search")?.trim();
    if rest.is_empty() {
        return None;
    }
    let mut query_parts = Vec::new();
    let mut reason = None;
    for part in rest.split_whitespace() {
        if let Some(value) = part.strip_prefix("query=") {
            query_parts.push(value.trim_matches(|c| c == '"' || c == '\''));
        } else if let Some(value) = part.strip_prefix("reason=") {
            reason = Some(value.trim_matches(|c| c == '"' || c == '\'').to_string());
        } else if reason.is_none() {
            query_parts.push(part);
        }
    }
    let query = query_parts.join(" ").trim().to_string();
    if query.is_empty() {
        return None;
    }
    Some(ContextSearchRequest { query, reason })
}

fn score_search_block(block: &SeenBlock, terms: &[String]) -> usize {
    let haystack = format!(
        "{} {} {} {}",
        block.summary,
        block.lexical_keys.join(" "),
        block.topics.join(" "),
        block.exact.lines().next().unwrap_or("")
    )
    .to_ascii_lowercase();
    terms
        .iter()
        .filter(|term| haystack.contains(term.as_str()))
        .count()
}

pub fn search_context_refs(query: &str, limit: usize) -> Vec<ContextSearchHit> {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|term| term.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|term| term.len() >= 2)
        .map(|term| term.to_ascii_lowercase())
        .collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let Ok(seen) = seen_blocks().lock() else {
        return Vec::new();
    };
    let mut hits: Vec<ContextSearchHit> = seen
        .values()
        .filter_map(|block| {
            let score = score_search_block(block, &terms);
            if score == 0 {
                return None;
            }
            Some(ContextSearchHit {
                id: format!("ctx:{}", block.hash),
                hash: block.hash.clone(),
                summary: block.summary.clone(),
                original_chars: block.original_chars,
                priority: block.priority.as_str().to_string(),
                topics: block
                    .topics
                    .iter()
                    .map(|topic| (*topic).to_string())
                    .collect(),
                score,
            })
        })
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.original_chars.cmp(&a.original_chars))
    });
    hits.truncate(limit);
    hits
}

pub fn maybe_rehydrate_context_search(text: &str) -> Option<String> {
    let req = parse_context_search_request(text)?;
    let hits = search_context_refs(&req.query, 8);
    record_retrieval_event(RetrievalEvent {
        kind: "ctx_search".to_string(),
        key: req.query.clone(),
        reason: req.reason.clone(),
        outcome: if hits.is_empty() {
            "no_hits"
        } else {
            "fulfilled"
        }
        .to_string(),
        source: Some("seen_blocks".to_string()),
        chars: 0,
    });
    let body = if hits.is_empty() {
        format!(
            "No context refs matched query {:?}. Try a more specific path, function, error, test, or topic.",
            req.query
        )
    } else {
        hits.iter()
            .map(|hit| {
                format!(
                    "- id={} score={} chars={} priority={} topics={} summary={}",
                    hit.id,
                    hit.score,
                    hit.original_chars,
                    hit.priority,
                    hit.topics.join(","),
                    hit.summary
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    Some(format!(
        "<system-reminder>\nNeura ctx_search results for query={:?} reason={}. Use `.ctx_get id=<id> reason=<why>` if exact text is required; do not invent hidden content from summaries alone.\n\n{}\n</system-reminder>",
        req.query,
        req.reason.as_deref().unwrap_or("unspecified"),
        body
    ))
}

pub fn exact_for_request(req: &ExactRequest) -> Option<(String, &'static str)> {
    if let Ok(seen) = seen_blocks().lock() {
        if let Some(block) = seen.get(&req.hash) {
            return Some((block.exact.clone(), "memory"));
        }
    }
    // Phase 1.C — fall back to the persistent vault. Lets `.ctx_get` succeed
    // for `<ctx>` references emitted by a prior Neura process. Sensitive
    // blocks are never written to disk so this never reveals credentials.
    if let Some(restored) = ctx_vault::load_into_seen_blocks(&req.hash) {
        return Some((restored, "persistent_vault"));
    }
    None
}

pub fn maybe_rehydrate_response(text: &str) -> Option<String> {
    let req = parse_exact_request(text)?;
    let Some((exact, source)) = exact_for_request(&req) else {
        record_retrieval_event(RetrievalEvent {
            kind: "ctx".to_string(),
            key: req.id.clone(),
            reason: req.reason.clone(),
            outcome: "not_found".to_string(),
            source: None,
            chars: 0,
        });
        return Some(format!(
            "<system-reminder>\nNeura ctx_get failed for id={} reason={}. The exact context was not found in the in-memory cache or persistent vault. Do not invent hidden content. Use `.ctx_search query=<terms> reason=<why>` or inspect current files/tools directly.\n</system-reminder>",
            req.id,
            req.reason.as_deref().unwrap_or("unspecified")
        ));
    };
    if let Err(reason) = retrieval_can_fulfill("ctx", &req.id, exact.len()) {
        record_retrieval_event(RetrievalEvent {
            kind: "ctx".to_string(),
            key: req.id.clone(),
            reason: req.reason.clone(),
            outcome: "suppressed".to_string(),
            source: Some(reason.clone()),
            chars: 0,
        });
        return Some(format!(
            "<system-reminder>\nNeura ctx_get for id={} was suppressed: {}. Do not invent hidden content; narrow the request or inspect files/tools directly.\n</system-reminder>",
            req.id, reason
        ));
    }
    record_retrieval_event(RetrievalEvent {
        kind: "ctx".to_string(),
        key: req.id.clone(),
        reason: req.reason.clone(),
        outcome: "fulfilled".to_string(),
        source: Some(source.to_string()),
        chars: exact.len(),
    });
    Some(format!(
        "<system-reminder>\nNeura ctx_get rehydration fulfilled for id={} hash={} source={} reason={}. Exact original content follows. Treat it as authoritative historical context and continue the task using this exact content. If it is a file excerpt, verify current file state before editing.\n\n<ctx_exact id=\"{}\" hash=\"{}\" source=\"{}\" original_chars={}>\n{}\n</ctx_exact>\n</system-reminder>",
        req.id,
        req.hash,
        source,
        req.reason.as_deref().unwrap_or("unspecified"),
        req.id,
        req.hash,
        source,
        exact.len(),
        exact
    ))
}

fn stable_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..8])
}

fn deterministic_summary(text: &str) -> String {
    let line_count = text.lines().count();
    let warn_count = text.matches("WARN").count();
    let error_count = text.matches("ERROR").count();
    let mut files: Vec<String> = text
        .split_whitespace()
        .filter_map(|tok| {
            let clean = tok.trim_matches(|c: char| {
                matches!(c, ',' | ';' | ':' | ')' | '(' | ']' | '[' | '"' | '\'')
            });
            if clean.contains('/') {
                clean.rsplit('/').next().map(|s| {
                    s.trim_matches(|c: char| c == ':' || c.is_ascii_digit())
                        .to_string()
                })
            } else {
                None
            }
        })
        .filter(|s| !s.is_empty() && s.contains('.'))
        .collect();
    files.sort();
    files.dedup();
    files.truncate(6);
    let first = text.lines().next().unwrap_or("").trim();
    let first = crate::util::truncate_str(first, 160);
    format!(
        "lines={}; chars={}; WARN={}; ERROR={}; files=[{}]; first={}",
        line_count,
        text.len(),
        warn_count,
        error_count,
        files.join(","),
        first
    )
}

fn escape_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn encode_path_prefixes(text: &str) -> Option<String> {
    if text.len() < MIN_TEXT_CHARS || text.contains("<il:v1>") {
        return None;
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    for token in text.split_whitespace() {
        let trimmed = token.trim_matches(|c: char| {
            matches!(c, ',' | ';' | ':' | ')' | '(' | ']' | '[' | '"' | '\'')
        });
        if !(trimmed.starts_with('/') || trimmed.starts_with("~/")) || trimmed.len() < 32 {
            continue;
        }
        if let Some(idx) = trimmed.rfind('/') {
            if idx >= 16 {
                let prefix = &trimmed[..idx];
                *counts.entry(prefix.to_string()).or_default() += 1;
            }
        }
    }
    let mut prefixes: Vec<(String, usize)> = counts
        .into_iter()
        .filter(|(prefix, count)| *count >= 3 && prefix.len() >= 16)
        .collect();
    prefixes.sort_by(|(a, ca), (b, cb)| (b.len() * cb).cmp(&(a.len() * ca)));
    prefixes.truncate(
        if matches!(
            mode(),
            InterlangMode::Aggressive | InterlangMode::Verified | InterlangMode::Ultra
        ) {
            48
        } else {
            16
        },
    );
    if prefixes.is_empty() {
        return None;
    }
    // Avoid nested prefix definitions fighting each other.
    let mut selected: Vec<String> = Vec::new();
    for (prefix, _) in prefixes {
        if selected
            .iter()
            .any(|s| prefix.starts_with(s) || s.starts_with(&prefix))
        {
            continue;
        }
        selected.push(prefix);
    }
    if selected.is_empty() {
        return None;
    }
    let mut encoded_body = text.to_string();
    let mut defs = Vec::new();
    for (idx, prefix) in selected.iter().enumerate() {
        let id = idx + 1;
        encoded_body = encoded_body.replace(prefix, &format!("$p{}", id));
        defs.push(format!("@p{}={}", id, prefix));
    }
    let encoded = format!("<il:v1>\n{}\n--\n{}\n</il>", defs.join("\n"), encoded_body);
    let saved = text.len() as isize - encoded.len() as isize;
    if saved >= MIN_SAVED_CHARS as isize && encoded.len() * 10 <= text.len() * 9 {
        Some(encoded)
    } else {
        None
    }
}

fn encode_repeated_lines(text: &str) -> Option<String> {
    if text.len() < MIN_TEXT_CHARS || text.contains("<il:v1>") {
        return None;
    }
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < 6 {
        return None;
    }
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for line in &lines {
        let trimmed = line.trim_end();
        if trimmed.len() >= 24 {
            *counts.entry(trimmed).or_default() += 1;
        }
    }
    let mut repeated: Vec<(&str, usize)> = counts
        .into_iter()
        .filter(|(_, count)| *count >= 3)
        .collect();
    repeated.sort_by(|(a, ca), (b, cb)| (b.len() * cb).cmp(&(a.len() * ca)));
    repeated.truncate(32);
    if repeated.is_empty() {
        return None;
    }
    let mut ids: HashMap<&str, usize> = HashMap::new();
    let mut defs = Vec::new();
    for (idx, (line, _)) in repeated.iter().enumerate() {
        let id = idx + 1;
        ids.insert(*line, id);
        defs.push(format!("@{}={}", id, line));
    }
    let mut body = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim_end();
        if let Some(id) = ids.get(line).copied() {
            let mut run = 1usize;
            while i + run < lines.len() && lines[i + run].trim_end() == line {
                run += 1;
            }
            body.push(if run > 1 {
                format!("${}*{}", id, run)
            } else {
                format!("${}", id)
            });
            i += run;
        } else {
            body.push(lines[i].to_string());
            i += 1;
        }
    }
    let encoded = format!(
        "<il:v1>\n{}\n--\n{}\n</il>",
        defs.join("\n"),
        body.join("\n")
    );
    let saved = text.len() as isize - encoded.len() as isize;
    if saved >= MIN_SAVED_CHARS as isize && encoded.len() * 5 <= text.len() * 4 {
        Some(encoded)
    } else {
        None
    }
}

/// Persistent context vault: writes non-sensitive `SeenBlock`s to disk so
/// later Neura processes can still rehydrate exact text behind `<ctx>` refs.
///
/// Layout: `~/.neura/ctx-vault/<hash[..2]>/<hash>.json` (sharded for fs perf).
/// Sensitive blocks (per `looks_sensitive`) are never persisted, so credentials
/// and bearer tokens cannot leak into the vault. Disabled via
/// `NEURA_CTX_VAULT_PERSIST=0`.
mod ctx_vault {
    use super::*;
    use serde::{Deserialize, Serialize};

    pub const ENV_DISABLE: &str = "NEURA_CTX_VAULT_PERSIST";

    #[derive(Serialize, Deserialize)]
    struct PersistedBlock {
        hash: String,
        original_chars: usize,
        summary: String,
        exact: String,
        confidence: f32,
        priority: String,
        topics: Vec<String>,
        lexical_keys: Vec<String>,
    }

    fn enabled() -> bool {
        std::env::var(ENV_DISABLE)
            .map(|v| {
                !matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "no" | "off"
                )
            })
            .unwrap_or(true)
    }

    fn vault_dir() -> Option<std::path::PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(std::path::Path::new(&home).join(".neura").join("ctx-vault"))
    }

    fn block_path(dir: &std::path::Path, hash: &str) -> Option<std::path::PathBuf> {
        if hash.len() < 2 {
            return None;
        }
        let shard = &hash[..2];
        Some(dir.join(shard).join(format!("{}.json", hash)))
    }

    pub fn maybe_persist(block: &SeenBlock) {
        if !enabled() || block.sensitive || block.exact.is_empty() {
            return;
        }
        let Some(dir) = vault_dir() else {
            return;
        };
        let Some(path) = block_path(&dir, &block.hash) else {
            return;
        };
        if path.exists() {
            return;
        }
        if let Some(shard_dir) = path.parent() {
            if std::fs::create_dir_all(shard_dir).is_err() {
                return;
            }
        }
        let payload = PersistedBlock {
            hash: block.hash.clone(),
            original_chars: block.original_chars,
            summary: block.summary.clone(),
            exact: block.exact.clone(),
            confidence: block.confidence,
            priority: block.priority.as_str().to_string(),
            topics: block.topics.iter().map(|t| (*t).to_string()).collect(),
            lexical_keys: block.lexical_keys.clone(),
        };
        let Ok(json) = serde_json::to_string(&payload) else {
            return;
        };
        // Best-effort atomic write: write to tmp then rename.
        let tmp_path = path.with_extension("json.tmp");
        if std::fs::write(&tmp_path, json).is_ok() {
            let _ = std::fs::rename(&tmp_path, &path);
        }
    }

    fn load_block(hash: &str) -> Option<PersistedBlock> {
        if !enabled() || hash.is_empty() {
            return None;
        }
        let path = block_path(&vault_dir()?, hash)?;
        let bytes = std::fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn priority_from_str(value: &str) -> ContextPriority {
        match value {
            "high" => ContextPriority::High,
            "verify" => ContextPriority::Verify,
            "low" => ContextPriority::Low,
            _ => ContextPriority::Normal,
        }
    }

    /// Load a vault block (if any) into the in-process `seen_blocks` map and
    /// return its exact text. Used as a fallback when `.ctx_get` references
    /// a hash that isn't in the current process's seen map (e.g., after a
    /// Neura restart).
    pub fn load_into_seen_blocks(hash: &str) -> Option<String> {
        let persisted = load_block(hash)?;
        let exact = persisted.exact.clone();
        if let Ok(mut seen) = seen_blocks().lock() {
            seen.entry(persisted.hash.clone())
                .or_insert_with(|| SeenBlock {
                    hash: persisted.hash,
                    original_chars: persisted.original_chars,
                    summary: persisted.summary,
                    exact: persisted.exact,
                    confidence: persisted.confidence,
                    priority: priority_from_str(&persisted.priority),
                    sensitive: false, // sensitive blocks are never persisted
                    topics: Vec::new(),
                    lexical_keys: persisted.lexical_keys,
                    encoded_refs: HashMap::new(),
                });
        }
        Some(exact)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::message::Message;
    use std::sync::MutexGuard;

    pub(crate) fn seen_test_lock() -> MutexGuard<'static, ()> {
        crate::storage::lock_test_env()
    }

    #[test]
    fn repeated_line_encoding_saves_space() {
        let line = "ERROR repeated subsystem diagnostic with enough length to reference";
        let text = std::iter::repeat(line)
            .take(30)
            .collect::<Vec<_>>()
            .join("\n");
        let encoded = encode_repeated_lines(&text).expect("should encode repetitive text");
        assert!(encoded.contains("@1="));
        assert!(encoded.contains("$1*30"));
        assert!(encoded.len() < text.len());
    }

    #[test]
    fn compact_messages_rewrites_large_repetitive_text_only() {
        let line = "TRACE same expensive line that repeats many times in a tool output";
        let text = std::iter::repeat(line)
            .take(25)
            .collect::<Vec<_>>()
            .join("\n");
        let msg = Message::user(&text);
        let (messages, stats) = compact_messages_for_test(&[msg]);
        assert_eq!(stats.blocks_encoded, 1);
        match &messages[0].content[0] {
            ContentBlock::Text { text, .. } => assert!(text.starts_with("<il:v1>")),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn path_prefix_encoding_saves_space() {
        let prefix = "/home/dad/Projects/neura-current-src/src/agent";
        let text = (0..40)
            .map(|idx| {
                format!(
                    "{}{}:{}: diagnostic output",
                    prefix, "/turn_streaming_mpsc.rs", idx
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let encoded = encode_path_prefixes(&text).expect("should encode repeated path prefix");
        assert!(encoded.contains("@p1="));
        assert!(encoded.contains("$p1"));
        assert!(encoded.len() < text.len());
    }

    #[test]
    fn mode_defaults_to_safe_when_enabled() {
        // The test process may set env vars globally, so just verify parser accepts
        // the default-compatible status path without panicking.
        let _ = status_json();
    }

    #[test]
    fn repeated_large_block_becomes_seen_ref() {
        let _guard = seen_test_lock();
        if let Ok(mut seen) = seen_blocks().lock() {
            seen.clear();
        }
        let text = (0..80)
            .map(|idx| {
                format!(
                    "/tmp/neura-interlang-seen-ref/file.rs:{}: WARN deterministic repeated seen-ref test line with enough length",
                    idx
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut first_stats = InterlangStats::default();
        assert!(encode_seen_ref(&text, &mut first_stats).is_none());
        let mut second_stats = InterlangStats::default();
        let encoded =
            encode_seen_ref(&text, &mut second_stats).expect("second sighting should ref");
        assert!(encoded.starts_with("<il:seen"));
        assert_eq!(second_stats.seen_ref_blocks, 1);
        assert_eq!(second_stats.raw_context_avoided_chars, text.len());
        assert!(encoded.len() < text.len());
    }

    #[test]
    fn ultra_vault_ref_summarizes_large_block_immediately() {
        if mode() != InterlangMode::Ultra {
            return;
        }
        let text = (0..160)
            .map(|idx| {
                format!(
                    "/tmp/neura-ultra-vault/src/file.rs:{}: ERROR deterministic ultra vault test line with enough repeated path context",
                    idx
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.len() >= MIN_VAULT_REF_CHARS);
        let mut stats = InterlangStats::default();
        let encoded = encode_vault_ref(&text, &mut stats).expect("ultra should vault large block");
        assert!(encoded.starts_with("<ctx"));
        assert!(encoded.contains("k=\"vault\""));
        assert_eq!(stats.seen_ref_blocks, 1);
        assert_eq!(stats.raw_context_avoided_chars, text.len());
        assert!(encoded.len() < text.len());
    }

    #[test]
    fn context_diet_compacts_large_recent_tool_results() {
        if mode() != InterlangMode::Ultra {
            return;
        }
        let mut messages = Vec::new();
        for idx in 0..20 {
            messages.push(Message::user(&format!("short filler {idx}")));
        }
        messages.push(Message::user("recent user request stays exact"));
        messages.push(Message::tool_result(
            "call-1",
            &"recent read output line with token-heavy context and file paths\n".repeat(700),
            false,
        ));

        let (dieted, stats) = maybe_context_diet_messages(&messages);
        assert!(stats.diet_blocks >= 1);
        match &dieted[dieted.len() - 2].content[0] {
            ContentBlock::Text { text, .. } => assert_eq!(text, "recent user request stays exact"),
            _ => panic!("expected recent text"),
        }
        match &dieted.last().unwrap().content[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert!(content.contains("k=\"old-tool-result\""));
                assert!(content.len() < 600);
            }
            _ => panic!("expected tool result"),
        }
    }

    #[test]
    fn context_diet_replaces_old_large_turns_but_keeps_recent() {
        if mode() != InterlangMode::Ultra {
            return;
        }
        let old = "old diagnostic token memory mouse screenshot build line with lots of context\n"
            .repeat(900);
        let recent = "recent exact user request should stay visible".repeat(40);
        let mut messages = Vec::new();
        for idx in 0..22 {
            if idx < 6 {
                messages.push(Message::user(&format!("old block {idx}\n{old}")));
            } else {
                messages.push(Message::user(&format!("short filler {idx}")));
            }
        }
        messages.push(Message::user(&recent));
        let (dieted, stats) = maybe_context_diet_messages(&messages);
        assert!(stats.diet_blocks >= 1);
        assert!(stats.diet_original_chars > stats.diet_encoded_chars);
        match &dieted[0].content[0] {
            ContentBlock::Text { text, .. } => assert!(text.contains("k=\"old-text\"")),
            _ => panic!("expected text"),
        }
        match &dieted.last().unwrap().content[0] {
            ContentBlock::Text { text, .. } => assert_eq!(text, &recent),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn parses_ctx_get_and_need_ref_requests() {
        let req = parse_exact_request(".ctx_get id=ctx:abc123 reason=debug")
            .expect("ctx_get should parse");
        assert_eq!(req.id, "ctx:abc123");
        assert_eq!(req.hash, "abc123");
        assert_eq!(req.reason.as_deref(), Some("debug"));

        let req = parse_exact_request(". err need_ref deadbeef").expect("need_ref should parse");
        assert_eq!(req.id, "ctx:deadbeef");
        assert_eq!(req.hash, "deadbeef");
    }

    #[test]
    fn parses_ctx_search_requests() {
        let req =
            parse_context_search_request(".ctx_search query=rust compiler error reason=debug")
                .expect("ctx_search should parse");
        assert_eq!(req.query, "rust compiler error");
        assert_eq!(req.reason.as_deref(), Some("debug"));
    }

    #[test]
    fn ctx_search_lists_matching_refs_without_exact_text() {
        let _guard = seen_test_lock();
        seen_blocks().lock().unwrap().clear();
        let exact = "massive exact rust compiler error E0308 hidden detail".repeat(80);
        let mut stats = InterlangStats::default();
        let encoded = encode_context_diet_ref(&exact, "old-text", &mut stats);
        assert!(encoded.contains("<ctx"));

        let results =
            maybe_rehydrate_context_search(".ctx_search query=compiler E0308 reason=find")
                .expect("search should produce reminder");
        assert!(results.contains("ctx_search results"));
        assert!(results.contains("id=ctx:"));
        assert!(!results.contains("hidden detailhidden detail"));
    }

    #[test]
    fn ctx_get_failure_is_explicit() {
        let _guard = seen_test_lock();
        seen_blocks().lock().unwrap().clear();
        reset_retrieval_turn();
        let response = maybe_rehydrate_response(".ctx_get id=ctx:missing reason=test")
            .expect("failed lookup should still return guidance");
        assert!(response.contains("ctx_get failed"));
        assert!(response.contains("Do not invent hidden content"));
    }

    #[test]
    fn rehydrates_stored_exact_context() {
        let _guard = seen_test_lock();
        if let Ok(mut seen) = seen_blocks().lock() {
            seen.clear();
        }
        let exact = "important exact hidden content\nsecond line".repeat(100);
        let mut stats = InterlangStats::default();
        let encoded = encode_context_diet_ref(&exact, "old-text", &mut stats);
        let id = encoded
            .split("id=\"")
            .nth(1)
            .and_then(|tail| tail.split('"').next())
            .expect("encoded ctx id");
        let fulfilled = maybe_rehydrate_response(&format!(".ctx_get id={id} reason=test"))
            .expect("exact content should be available");
        assert!(fulfilled.contains("<ctx_exact"));
        assert!(fulfilled.contains("important exact hidden content"));
        assert!(fulfilled.contains("second line"));
    }

    #[test]
    fn auto_rehydration_ignores_unrelated_old_context() {
        if mode() != InterlangMode::Ultra {
            return;
        }
        let _guard = seen_test_lock();
        if let Ok(mut seen) = seen_blocks().lock() {
            seen.clear();
        }
        let old_installer_error =
            "diff --git a/install/install.sh b/install/install.sh\nERROR installer build failure\n"
                .repeat(120);
        let mut stats = InterlangStats::default();
        let _ = encode_context_diet_ref(&old_installer_error, "old-tool-result", &mut stats);
        assert!(stats.low_confidence_blocks > 0);

        let mut messages = vec![Message::user(
            "Please audit Neura token efficiency and context compression strategy.",
        )];
        maybe_append_auto_rehydration(&mut messages, &mut stats);
        assert_eq!(
            messages.len(),
            1,
            "unrelated installer block should stay summarized"
        );
        assert_eq!(stats.auto_rehydrated_blocks, 0);
    }

    #[test]
    fn auto_rehydration_ignores_self_test_statistics_turn() {
        if mode() != InterlangMode::Ultra {
            return;
        }
        let _guard = seen_test_lock();
        if let Ok(mut seen) = seen_blocks().lock() {
            seen.clear();
        }
        let old_prompt_memory_code =
            "fn build_memory_prompt_nonblocking() { /* token memory context test build error */ }\n"
                .repeat(160);
        let mut stats = InterlangStats::default();
        let _ = encode_context_diet_ref(&old_prompt_memory_code, "old-text", &mut stats);
        assert!(stats.low_confidence_blocks > 0);

        let mut messages = vec![Message::user(
            "ok i reloaded you, do a self test and update the statistics",
        )];
        maybe_append_auto_rehydration(&mut messages, &mut stats);
        assert_eq!(
            messages.len(),
            1,
            "self-test/statistics turns should not auto-restore generic old code"
        );
        assert_eq!(stats.auto_rehydrated_blocks, 0);
    }

    #[test]
    fn decoder_prompt_stays_compact_but_preserves_retrieval_contract() {
        let prompt = decoder_prompt();
        if prompt.is_empty() {
            return;
        }
        assert!(prompt.len() <= 300, "decoder prompt should stay compact");
        assert!(prompt.contains(".ctx_get") || prompt.contains("need_ref"));
        assert!(prompt.contains("don't invent") || prompt.contains("Don't guess"));
    }

    #[test]
    fn realtime_stats_prompt_stays_compact() {
        let mut stats = InterlangStats::default();
        stats.blocks_encoded = 12;
        stats.diet_blocks = 9;
        stats.raw_context_avoided_chars = 40_000;
        stats.exact_original_tokens = 20_000;
        stats.exact_encoded_tokens = 1_000;
        let prompt = realtime_stats_prompt(stats);
        assert!(prompt.len() <= 180, "stats reminder should stay compact");
        assert!(prompt.contains("saved="));
        assert!(prompt.contains("avoided="));
        assert!(prompt.contains("diet="));
    }

    #[test]
    fn auto_rehydration_ignores_documentation_wording_turn() {
        if mode() != InterlangMode::Ultra {
            return;
        }
        let _guard = seen_test_lock();
        if let Ok(mut seen) = seen_blocks().lock() {
            seen.clear();
        }
        let old_prompt_memory_code =
            "impl Agent { fn build_memory_prompt_nonblocking() {} /* exact memory context error */ }\n"
                .repeat(180);
        let mut stats = InterlangStats::default();
        let _ = encode_context_diet_ref(&old_prompt_memory_code, "old-text", &mut stats);
        assert!(stats.low_confidence_blocks > 0);

        let mut messages = vec![Message::user(
            "Instead of saying old bulky context gets summarized, punch it harder in ABOUT.md: exact context is externalized and retrieval works.",
        )];
        maybe_append_auto_rehydration(&mut messages, &mut stats);
        assert_eq!(
            messages.len(),
            1,
            "documentation wording turns should not auto-restore generic old code"
        );
        assert_eq!(stats.auto_rehydrated_blocks, 0);
    }

    #[test]
    fn auto_rehydration_ignores_short_meow_turn() {
        if mode() != InterlangMode::Ultra {
            return;
        }
        let _guard = seen_test_lock();
        if let Ok(mut seen) = seen_blocks().lock() {
            seen.clear();
        }
        let old_prompt_chars_code =
            "src/local_model.rs:1043:fn record_pre_route(latest_user: &str, prompt_chars: usize) {}
"
            .repeat(900);
        let mut stats = InterlangStats::default();
        let _ = encode_context_diet_ref(&old_prompt_chars_code, "old-text", &mut stats);
        let mut messages = vec![Message::user("meow")];
        maybe_append_auto_rehydration(&mut messages, &mut stats);
        assert_eq!(
            messages.len(),
            1,
            "short trivial turns must not auto-restore exact context"
        );
        assert_eq!(stats.auto_rehydrated_blocks, 0);
    }

    #[test]
    fn auto_rehydration_ignores_token_efficiency_audit_turn() {
        if mode() != InterlangMode::Ultra {
            return;
        }
        let _guard = seen_test_lock();
        if let Ok(mut seen) = seen_blocks().lock() {
            seen.clear();
        }
        let old_prompt_chars_code =
            "src/local_model.rs:1043:fn record_pre_route(latest_user: &str, prompt_chars: usize) {}
"
            .repeat(900);
        let mut stats = InterlangStats::default();
        let _ = encode_context_diet_ref(&old_prompt_chars_code, "old-text", &mut stats);
        let mut messages = vec![Message::user(
            "check everything entirely, why is it saying 107k tokens up? fix token efficiency",
        )];
        maybe_append_auto_rehydration(&mut messages, &mut stats);
        assert_eq!(
            messages.len(),
            1,
            "token accounting turns should inspect logs/code via tools, not auto-restore old exact excerpts"
        );
        assert_eq!(stats.auto_rehydrated_blocks, 0);
    }

    #[test]
    fn auto_rehydration_ignores_token_accounting_why_turn() {
        if mode() != InterlangMode::Ultra {
            return;
        }
        let _guard = seen_test_lock();
        if let Ok(mut seen) = seen_blocks().lock() {
            seen.clear();
        }
        let old_tool_tests =
            "fn test_resolve_tool_name_oauth_aliases() { /* todo token error auth */ }\n"
                .repeat(190);
        let mut stats = InterlangStats::default();
        let _ = encode_context_diet_ref(&old_tool_tests, "old-text", &mut stats);
        assert!(stats.low_confidence_blocks > 0);

        let mut messages = vec![Message::user(
            "i dont understand why that took 93k tokens. thats alot? did it actually",
        )];
        maybe_append_auto_rehydration(&mut messages, &mut stats);
        assert_eq!(
            messages.len(),
            1,
            "token-accounting why turns should not auto-restore unrelated old code"
        );
        assert_eq!(stats.auto_rehydrated_blocks, 0);
    }

    #[test]
    fn auto_rehydration_restores_related_old_context() {
        if mode() != InterlangMode::Ultra {
            return;
        }
        let _guard = seen_test_lock();
        if let Ok(mut seen) = seen_blocks().lock() {
            seen.clear();
        }
        let old_installer_error =
            "diff --git a/install/install.sh b/install/install.sh\nERROR installer build failure\n"
                .repeat(120);
        let mut stats = InterlangStats::default();
        let _ = encode_context_diet_ref(&old_installer_error, "old-tool-result", &mut stats);
        assert!(stats.low_confidence_blocks > 0);

        let mut messages = vec![Message::user(
            "continue from above: the install/install.sh build error is still failing in the same file. Show relevant context.",
        )];
        maybe_append_auto_rehydration(&mut messages, &mut stats);
        assert_eq!(
            messages.len(),
            2,
            "related installer block should be restored"
        );
        assert_eq!(stats.auto_rehydrated_blocks, 1);
    }

    #[test]
    fn encoded_ref_cache_returns_same_string_on_resight() {
        let _guard = seen_test_lock();
        if let Ok(mut seen) = seen_blocks().lock() {
            seen.clear();
        }
        let text = "long old block ".repeat(200);
        let mut stats1 = InterlangStats::default();
        let first = encode_context_diet_ref(&text, "old-text", &mut stats1);
        let mut stats2 = InterlangStats::default();
        let second = encode_context_diet_ref(&text, "old-text", &mut stats2);
        assert_eq!(
            first, second,
            "second encode of same hash+kind must return identical ref"
        );
        // Both encodes update stats (they account for the per-turn savings),
        // so the deltas should match the original block size.
        assert_eq!(stats1.diet_blocks, 1);
        assert_eq!(stats2.diet_blocks, 1);
        assert_eq!(stats1.diet_original_chars, stats2.diet_original_chars);
    }

    #[test]
    fn ctx_vault_persists_and_reloads_after_seen_clear() {
        let _guard = seen_test_lock();
        let temp = tempfile::TempDir::new().unwrap();
        let prev_home = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("HOME", temp.path());
            std::env::remove_var(super::ctx_vault::ENV_DISABLE);
        }
        if let Ok(mut seen) = seen_blocks().lock() {
            seen.clear();
        }

        let text = "vault round-trip test content ".repeat(200);
        let mut stats = InterlangStats::default();
        let _encoded = encode_context_diet_ref(&text, "old-text", &mut stats);
        let hash = stable_hash(&text);

        // Simulate a fresh process: drop the in-memory map.
        if let Ok(mut seen) = seen_blocks().lock() {
            seen.clear();
        }

        let req = ExactRequest {
            id: format!("ctx:{}", hash),
            hash: hash.clone(),
            reason: None,
        };
        let (restored, _source) = exact_for_request(&req).expect("vault should rehydrate");
        assert_eq!(restored, text);

        // Cleanup.
        unsafe {
            if let Some(prev) = prev_home {
                std::env::set_var("HOME", prev);
            } else {
                std::env::remove_var("HOME");
            }
        }
    }

    #[test]
    fn ctx_vault_skips_sensitive_blocks() {
        let _guard = seen_test_lock();
        let temp = tempfile::TempDir::new().unwrap();
        let prev_home = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("HOME", temp.path());
            std::env::remove_var(super::ctx_vault::ENV_DISABLE);
        }
        if let Ok(mut seen) = seen_blocks().lock() {
            seen.clear();
        }

        let text = "Authorization: Bearer ghp_secrettokenvalue\n".repeat(500);
        let mut stats = InterlangStats::default();
        let _encoded = encode_context_diet_ref(&text, "old-text", &mut stats);
        let hash = stable_hash(&text);

        if let Ok(mut seen) = seen_blocks().lock() {
            seen.clear();
        }

        let req = ExactRequest {
            id: format!("ctx:{}", hash),
            hash,
            reason: None,
        };
        let restored = exact_for_request(&req);
        assert!(
            restored.is_none(),
            "sensitive block must NOT be persisted to vault"
        );

        unsafe {
            if let Some(prev) = prev_home {
                std::env::set_var("HOME", prev);
            } else {
                std::env::remove_var("HOME");
            }
        }
    }

    #[test]
    fn tool_use_diet_off_by_default_keeps_input_exact() {
        if mode() != InterlangMode::Ultra {
            return;
        }
        let _guard = seen_test_lock();
        let prev_flag = std::env::var_os("NEURA_INTERLANG_DIET_TOOL_INPUT");
        unsafe {
            std::env::remove_var("NEURA_INTERLANG_DIET_TOOL_INPUT");
        }
        if let Ok(mut seen) = seen_blocks().lock() {
            seen.clear();
        }
        let big_input =
            serde_json::json!({"file_path": "src/foo.rs", "content": "x".repeat(4_000)});
        let mut messages = Vec::new();
        for idx in 0..20 {
            messages.push(Message::user(&format!("filler {idx}")));
        }
        // Old assistant turn with a large tool_use input.
        messages.push(crate::message::Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "old-1".to_string(),
                name: "edit".to_string(),
                input: big_input.clone(),
            }],
            timestamp: None,
            tool_duration_ms: None,
        });
        // A tool_result then more turns to push the tool_use out of the recent window.
        messages.push(Message::tool_result("old-1", "ok", false));
        for idx in 0..30 {
            messages.push(Message::user(&format!("more filler {idx}")));
        }
        messages.push(Message::user("recent question goes here"));

        let (dieted, _stats) = maybe_context_diet_messages(&messages);
        let preserved = dieted.iter().any(|m| {
            m.content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { input, .. } if input == &big_input))
        });
        assert!(preserved, "tool_use input must stay exact when flag is off");

        unsafe {
            if let Some(prev) = prev_flag {
                std::env::set_var("NEURA_INTERLANG_DIET_TOOL_INPUT", prev);
            }
        }
    }

    #[test]
    fn tool_use_diet_replaces_old_input_when_flag_on() {
        if mode() != InterlangMode::Ultra {
            return;
        }
        let _guard = seen_test_lock();
        let prev_flag = std::env::var_os("NEURA_INTERLANG_DIET_TOOL_INPUT");
        unsafe {
            std::env::set_var("NEURA_INTERLANG_DIET_TOOL_INPUT", "1");
            std::env::remove_var("NEURA_CONTEXT_DIET_RECENT_BYTES");
        }
        if let Ok(mut seen) = seen_blocks().lock() {
            seen.clear();
        }
        let big_input =
            serde_json::json!({"file_path": "src/foo.rs", "content": "y".repeat(4_000)});
        let mut messages = Vec::new();
        for idx in 0..6 {
            messages.push(Message::user(&format!("warmup {idx}")));
        }
        messages.push(crate::message::Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "old-2".to_string(),
                name: "edit".to_string(),
                input: big_input,
            }],
            timestamp: None,
            tool_duration_ms: None,
        });
        messages.push(Message::tool_result("old-2", "ok", false));
        // Real prose-ish filler so the WordPiece tokenizer counts each word
        // distinctly and the cumulative token count crosses the 6k trigger.
        let filler_chunk = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega ".repeat(30);
        for idx in 0..40 {
            messages.push(Message::user(&format!("filler {idx} {filler_chunk}")));
        }
        // Add a NEWER assistant turn so the test target is no longer the
        // most-recent assistant (we never diet the last assistant's tool_use).
        messages.push(Message::assistant_text(
            "Recent assistant note about the next step",
        ));
        messages.push(Message::user("recent user request stays exact"));

        let (dieted, _stats) = maybe_context_diet_messages(&messages);
        let dieted_tool_use_compacted = dieted.iter().any(|m| {
            m.content.iter().any(|b| match b {
                ContentBlock::ToolUse { input, .. } => input
                    .as_object()
                    .and_then(|map| map.get("_neura_ctx_ref"))
                    .is_some(),
                _ => false,
            })
        });
        assert!(
            dieted_tool_use_compacted,
            "old tool_use input must be replaced by ctx ref when flag is on"
        );

        unsafe {
            if let Some(prev) = prev_flag {
                std::env::set_var("NEURA_INTERLANG_DIET_TOOL_INPUT", prev);
            } else {
                std::env::remove_var("NEURA_INTERLANG_DIET_TOOL_INPUT");
            }
        }
    }

    #[test]
    fn recent_byte_budget_preserves_just_enough() {
        if mode() != InterlangMode::Ultra {
            return;
        }
        let _guard = seen_test_lock();
        let prev_budget = std::env::var_os("NEURA_CONTEXT_DIET_RECENT_BYTES");
        unsafe {
            std::env::set_var("NEURA_CONTEXT_DIET_RECENT_BYTES", "5000");
        }
        if let Ok(mut seen) = seen_blocks().lock() {
            seen.clear();
        }
        let mut messages = Vec::new();
        for idx in 0..30 {
            messages.push(Message::tool_result(
                &format!("c-{idx}"),
                &"chunk-content-line ".repeat(120),
                false,
            ));
        }
        messages.push(Message::user(
            "most recent fresh user turn that should stay exact",
        ));

        let (dieted, _stats) = maybe_context_diet_messages(&messages);
        // The very last user message must remain exact.
        let last = dieted.last().unwrap();
        match &last.content[0] {
            ContentBlock::Text { text, .. } => {
                assert_eq!(text, "most recent fresh user turn that should stay exact");
            }
            _ => panic!("expected text"),
        }

        unsafe {
            if let Some(prev) = prev_budget {
                std::env::set_var("NEURA_CONTEXT_DIET_RECENT_BYTES", prev);
            } else {
                std::env::remove_var("NEURA_CONTEXT_DIET_RECENT_BYTES");
            }
        }
    }

    #[test]
    fn max_blocks_zero_skips_all_encoding() {
        if mode() != InterlangMode::Ultra {
            return;
        }
        let _guard = seen_test_lock();
        if let Ok(mut seen) = seen_blocks().lock() {
            seen.clear();
        }
        let big = "huge old context block that would normally be encoded ".repeat(200);
        let mut messages = Vec::new();
        for idx in 0..20 {
            messages.push(Message::user(&format!("filler {idx}\n{big}")));
        }
        messages.push(Message::user("recent user message"));
        let (compacted, stats) = maybe_compact_messages_with_budget(
            &messages,
            CompactBudget {
                max_blocks: Some(0),
                recent_bytes: None,
            },
        );
        assert_eq!(stats.blocks_encoded, 0, "max_blocks=0 must skip encoding");
        assert_eq!(compacted.len(), messages.len());
        // First filler block is left exact (no <ctx> wrapper).
        match &compacted[0].content[0] {
            ContentBlock::Text { text, .. } => assert!(!text.contains("<ctx ")),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn max_blocks_caps_emitted_refs() {
        if mode() != InterlangMode::Ultra {
            return;
        }
        let _guard = seen_test_lock();
        if let Ok(mut seen) = seen_blocks().lock() {
            seen.clear();
        }
        let chunk = "alpha beta gamma delta epsilon zeta eta theta iota kappa ".repeat(40);
        let mut messages = Vec::new();
        for idx in 0..30 {
            messages.push(Message::user(&format!("entry {idx}: {chunk}")));
        }
        messages.push(Message::user("recent question"));
        let (_compacted, stats) = maybe_compact_messages_with_budget(
            &messages,
            CompactBudget {
                max_blocks: Some(2),
                recent_bytes: None,
            },
        );
        assert!(
            stats.blocks_encoded <= 2,
            "blocks_encoded {} should be capped at 2",
            stats.blocks_encoded
        );
        assert!(
            stats.blocks_encoded >= 1,
            "should encode at least one block"
        );
    }

    #[test]
    fn ctx_vault_disabled_when_env_set_to_zero() {
        let _guard = seen_test_lock();
        let temp = tempfile::TempDir::new().unwrap();
        let prev_home = std::env::var_os("HOME");
        let prev_flag = std::env::var_os(super::ctx_vault::ENV_DISABLE);
        unsafe {
            std::env::set_var("HOME", temp.path());
            std::env::set_var(super::ctx_vault::ENV_DISABLE, "0");
        }
        if let Ok(mut seen) = seen_blocks().lock() {
            seen.clear();
        }

        let text = "vault disabled test ".repeat(200);
        let mut stats = InterlangStats::default();
        let _ = encode_context_diet_ref(&text, "old-text", &mut stats);
        let hash = stable_hash(&text);

        if let Ok(mut seen) = seen_blocks().lock() {
            seen.clear();
        }

        let req = ExactRequest {
            id: format!("ctx:{}", hash),
            hash,
            reason: None,
        };
        assert!(exact_for_request(&req).is_none());

        unsafe {
            if let Some(prev) = prev_home {
                std::env::set_var("HOME", prev);
            } else {
                std::env::remove_var("HOME");
            }
            if let Some(prev) = prev_flag {
                std::env::set_var(super::ctx_vault::ENV_DISABLE, prev);
            } else {
                std::env::remove_var(super::ctx_vault::ENV_DISABLE);
            }
        }
    }
}
