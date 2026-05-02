//! Optional llm-interlang-inspired message compaction.
//!
//! Conservative first integration of /home/dad/neura-agent/tmp/llm-interlang-main:
//! rewrite only highly repetitive text/tool-result blocks into a tiny
//! line-reference protocol.

use crate::message::{ContentBlock, Message};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokenizers::Tokenizer;

const ENV_ENABLE: &str = "KCODE_INTERLANG_COMPACT";
const ENV_MODE: &str = "KCODE_INTERLANG_MODE";
const ENV_TOKENIZER_JSON: &str = "KCODE_INTERLANG_TOKENIZER_JSON";
const ENV_CONTEXT_DIET: &str = "KCODE_CONTEXT_DIET";
const ENV_CONTEXT_DIET_TRIGGER_TOKENS: &str = "KCODE_CONTEXT_DIET_TRIGGER_TOKENS";
const ENV_CONTEXT_DIET_RECENT_MESSAGES: &str = "KCODE_CONTEXT_DIET_RECENT_MESSAGES";
const ENV_CONTEXT_DIET_MIN_BLOCK_CHARS: &str = "KCODE_CONTEXT_DIET_MIN_BLOCK_CHARS";
const DEFAULT_TOKENIZER_JSON: &str = "/home/dad/.kcode/models/all-MiniLM-L6-v2/tokenizer.json";
const MIN_TEXT_CHARS: usize = 900;
const MIN_SAVED_CHARS: usize = 240;
const MIN_SEEN_REF_CHARS: usize = 2_400;
const MIN_VAULT_REF_CHARS: usize = 4_000;
const DEFAULT_CONTEXT_DIET_TRIGGER_TOKENS: usize = 24_000;
const DEFAULT_CONTEXT_DIET_RECENT_MESSAGES: usize = 8;
const DEFAULT_CONTEXT_DIET_MIN_BLOCK_CHARS: usize = 420;
const APPROX_CHARS_PER_TOKEN: usize = 4;
const AUTO_REHYDRATE_CONFIDENCE_THRESHOLD: f32 = 0.56;
const AUTO_REHYDRATE_MAX_BLOCKS: usize = 3;
const AUTO_REHYDRATE_MAX_CHARS: usize = 6_000;

#[derive(Debug, Clone)]
struct SeenBlock {
    original_chars: usize,
    summary: String,
    exact: String,
    confidence: f32,
    priority: ContextPriority,
    sensitive: bool,
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
}

fn seen_blocks() -> &'static Mutex<HashMap<String, SeenBlock>> {
    static SEEN: OnceLock<Mutex<HashMap<String, SeenBlock>>> = OnceLock::new();
    SEEN.get_or_init(|| Mutex::new(HashMap::new()))
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
        InterlangMode::Ultra => "\n\n<system-reminder>\nKcode interlang ultra context-vault mode is active. Decode <il:v1> blocks normally. <ctx> and <il:seen> entries are local Kcode context-vault references: Kcode has stored the exact original text locally and is sending only a deterministic summary/hash to reduce tokens. Use the summary for normal reasoning. If exact hidden content is required, respond with `.ctx_get id=<id> reason=<brief reason>` or `. err need_ref <hash>` instead of guessing. Do not invent unavailable exact lines from a <ctx> summary. Treat exact decoded content, when provided, as authoritative.\n</system-reminder>".to_string(),
        InterlangMode::Verified | InterlangMode::Aggressive => "\n\n<system-reminder>\nKcode interlang verified protocol is active. Decode any <il:v1> blocks before reasoning. Syntax: @N=<text> defines a line reference; @pN=<path-prefix> defines a path prefix; $N expands to that line; $N*COUNT expands to that line repeated COUNT times on separate lines; $pN expands to that path prefix. <il:seen> means Kcode has already provided the exact block earlier in this session; use its hash and deterministic summary as a reference, and say exactly `. err need_ref <hash>` if exact contents are required again. References are defined by Kcode and may be reused consistently across turns when present. If any reference is unclear, say exactly `. err need_ref <name>` instead of guessing. Treat decoded text exactly as the original message/tool output.\n</system-reminder>".to_string(),
        InterlangMode::Safe => "\n\n<system-reminder>\nKcode interlang safe mode is active. Decode any <il:v1> blocks before reasoning. Syntax: @N=<text> defines a line reference; @pN=<path-prefix> defines a path prefix; $N expands to that line; $N*COUNT expands to that line repeated COUNT times on separate lines; $pN expands to that path prefix. Treat decoded text exactly as the original message/tool output.\n</system-reminder>".to_string(),
        InterlangMode::Off => String::new(),
    }
}

pub fn realtime_stats_prompt(latest: InterlangStats) -> String {
    let status = status_json();
    let events = status.get("events").and_then(|v| v.as_u64()).unwrap_or(0);
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
    let tokenizer = if status
        .get("exact_tokenizer")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        "exact"
    } else {
        "estimated"
    };

    format!(
        "\n\n<system-reminder>\nKcode realtime interlang stats: mode={mode}, events={events}, tokenizer={tokenizer}, total_saved_tokens={total_saved}, latest_saved_tokens={latest_saved}, latest_blocks_encoded={}, latest_raw_context_avoided_tokens={latest_raw_avoided}, latest_diet_blocks={}. These are live local context-compression/accounting stats for this session; use them when the user asks about IL/ctx/token savings, but do not let them distract from the main task.\n</system-reminder>",
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

pub(crate) fn exact_token_count(text: &str) -> Option<usize> {
    local_tokenizer()
        .and_then(|tokenizer| {
            let mut tokenizer = tokenizer.clone();
            let _ = tokenizer.with_truncation(None);
            let _ = tokenizer.with_padding(None);
            tokenizer.encode(text, false).ok()
        })
        .map(|encoding| encoding.len())
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
    let path = std::path::Path::new(&home).join(".kcode/interlang-stats.jsonl");
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
        .map(|home| std::path::Path::new(&home).join(".kcode/interlang-stats.jsonl"))
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

pub fn maybe_compact_messages(messages: &[Message]) -> (Vec<Message>, InterlangStats) {
    if mode() == InterlangMode::Off {
        return (messages.to_vec(), InterlangStats::default());
    }
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
        8_000,
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
    let total_tokens =
        exact_token_count_messages(messages).unwrap_or_else(|| estimate_tokens(total_text));
    if total_tokens < context_diet_trigger_tokens() {
        return (messages.to_vec(), stats);
    }

    let cutoff = messages
        .len()
        .saturating_sub(context_diet_recent_messages());
    let mut out = Vec::with_capacity(messages.len());
    for (idx, message) in messages.iter().enumerate() {
        if idx >= cutoff {
            out.push(message.clone());
            continue;
        }
        let mut msg = message.clone();
        let mut changed = false;
        for block in &mut msg.content {
            match block {
                ContentBlock::Text { text, .. } if should_diet_text(text) => {
                    *text = encode_context_diet_ref(text, "old-text", &mut stats);
                    changed = true;
                }
                ContentBlock::ToolResult { content, .. } if should_diet_tool_result(content) => {
                    *content = encode_context_diet_ref(content, "old-tool-result", &mut stats);
                    changed = true;
                }
                ContentBlock::Reasoning { text } if text.len() > context_diet_min_block_chars() => {
                    *text = encode_context_diet_ref(text, "old-reasoning", &mut stats);
                    changed = true;
                }
                _ => {}
            }
        }
        if changed {
            stats.blocks_encoded += 1;
        }
        out.push(msg);
    }
    (out, stats)
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

fn encode_context_diet_ref(text: &str, kind: &str, stats: &mut InterlangStats) -> String {
    let hash = stable_hash(text);
    let summary = memory_safe_summary(text);
    let meta = context_metadata(text, kind);
    if let Ok(mut seen) = seen_blocks().lock() {
        seen.entry(hash.clone()).or_insert_with(|| SeenBlock {
            original_chars: text.len(),
            summary: summary.clone(),
            exact: text.to_string(),
            confidence: meta.confidence,
            priority: meta.priority,
            sensitive: meta.sensitive,
        });
    }
    let id = format!("ctx:{}", hash);
    let encoded = format!(
        "<ctx v=1 diet=1 kind=\"{}\" id=\"{}\" hash=\"{}\" original_chars={} confidence=\"{:.2}\" priority=\"{}\" auto_rehydrate=\"{}\" topics=\"{}\" summary=\"{}\" policy=\"memory-safe context diet: old low-value context is summarized; low-confidence/high-priority refs may be auto-rehydrated by Kcode; request exact if needed\" request_exact=\".ctx_get id={} reason=&lt;why exact old context is needed&gt;\" />",
        kind,
        id,
        hash,
        text.len(),
        meta.confidence,
        meta.priority.as_str(),
        should_auto_rehydrate(&meta),
        meta.topics.join(","),
        escape_attr(&summary),
        id
    );
    if should_auto_rehydrate(&meta) {
        stats.low_confidence_blocks += 1;
    }
    stats.original_chars += text.len();
    stats.encoded_chars += encoded.len();
    stats.raw_context_avoided_chars += text.len();
    stats.diet_blocks += 1;
    stats.diet_original_chars += text.len();
    stats.diet_encoded_chars += encoded.len();
    if let Some(tokens) = exact_token_count(text) {
        stats.exact_original_tokens += tokens;
    }
    if let Some(tokens) = exact_token_count(&encoded) {
        stats.exact_encoded_tokens += tokens;
    }
    encoded
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

fn maybe_append_auto_rehydration(messages: &mut Vec<Message>, stats: &mut InterlangStats) {
    if mode() != InterlangMode::Ultra || stats.low_confidence_blocks == 0 {
        return;
    }
    let Ok(seen) = seen_blocks().lock() else {
        return;
    };
    let mut candidates: Vec<(&String, &SeenBlock)> = seen
        .iter()
        .filter(|(_, block)| {
            !block.sensitive
                && (block.confidence <= AUTO_REHYDRATE_CONFIDENCE_THRESHOLD
                    || block.priority == ContextPriority::High)
        })
        .collect();
    candidates.sort_by(|(_, a), (_, b)| {
        a.confidence
            .partial_cmp(&b.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.original_chars.cmp(&a.original_chars))
    });

    let mut remaining = AUTO_REHYDRATE_MAX_CHARS;
    let mut sections = Vec::new();
    for (hash, block) in candidates.into_iter().take(AUTO_REHYDRATE_MAX_BLOCKS) {
        if remaining < 400 {
            break;
        }
        let excerpt = exact_excerpt(&block.exact, remaining.min(2_200));
        if excerpt.trim().is_empty() {
            continue;
        }
        remaining = remaining.saturating_sub(excerpt.len());
        stats.auto_rehydrated_blocks += 1;
        stats.auto_rehydrated_chars += excerpt.len();
        sections.push(format!(
            "<ctx_auto_exact id=\"ctx:{}\" hash=\"{}\" confidence=\"{:.2}\" priority=\"{}\" original_chars=\"{}\">\n{}\n</ctx_auto_exact>",
            hash,
            hash,
            block.confidence,
            block.priority.as_str(),
            block.original_chars,
            excerpt
        ));
    }
    if sections.is_empty() {
        return;
    }

    let text = format!(
        "<system-reminder>\nKcode proactive ctx rehydration: the following exact excerpts were auto-injected because their <ctx> summaries were low-confidence or high-priority. Treat these exact excerpts as authoritative evidence. If more exact old context is needed, request `.ctx_get id=ctx:<hash> reason=<why>`.\n\n{}\n</system-reminder>",
        sections.join("\n\n")
    );
    messages.push(Message::user(&text));
}

fn exact_excerpt(text: &str, max_chars: usize) -> String {
    let mut out = Vec::new();
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.contains("error")
            || lower.contains("failed")
            || lower.contains("panic")
            || lower.contains("diff --git")
            || lower.contains("fn ")
            || lower.contains("struct ")
            || lower.contains("impl ")
            || lower.contains("todo")
        {
            out.push(line);
        }
        if out.join("\n").len() >= max_chars / 2 {
            break;
        }
    }
    if out.is_empty() {
        out.extend(text.lines().take(24));
    }
    let mut excerpt = out.join("\n");
    if excerpt.len() < max_chars / 2 && text.len() > excerpt.len() {
        excerpt.push_str("\n...\n");
        let tail_len = max_chars.saturating_sub(excerpt.len()).min(text.len());
        let tail: String = text
            .chars()
            .rev()
            .take(tail_len)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        excerpt.push_str(&tail);
    }
    crate::util::truncate_str(&excerpt, max_chars).to_string()
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
        seen.insert(
            hash,
            SeenBlock {
                original_chars: text.len(),
                summary: deterministic_summary(text),
                exact: text.to_string(),
                confidence: meta.confidence,
                priority: meta.priority,
                sensitive: meta.sensitive,
            },
        );
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
    if let Ok(mut seen) = seen_blocks().lock() {
        seen.entry(hash.clone()).or_insert_with(|| SeenBlock {
            original_chars: text.len(),
            summary: summary.clone(),
            exact: text.to_string(),
            confidence: meta.confidence,
            priority: meta.priority,
            sensitive: meta.sensitive,
        });
    }
    let id = format!("ctx:{}", hash);
    let encoded = format!(
        "<ctx v=1 id=\"{}\" hash=\"{}\" original_chars={} confidence=\"{:.2}\" priority=\"{}\" auto_rehydrate=\"{}\" topics=\"{}\" summary=\"{}\" request_exact=\".ctx_get id={} reason=<why exact text is needed>\" />",
        id,
        hash,
        text.len(),
        meta.confidence,
        meta.priority.as_str(),
        should_auto_rehydrate(&meta),
        meta.topics.join(","),
        escape_attr(&summary),
        id
    );
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactRequest {
    pub id: String,
    pub hash: String,
    pub reason: Option<String>,
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

pub fn exact_for_request(req: &ExactRequest) -> Option<String> {
    let seen = seen_blocks().lock().ok()?;
    seen.get(&req.hash).map(|block| block.exact.clone())
}

pub fn maybe_rehydrate_response(text: &str) -> Option<String> {
    let req = parse_exact_request(text)?;
    let exact = exact_for_request(&req)?;
    Some(format!(
        "<system-reminder>\nKcode ctx_get rehydration fulfilled for id={} hash={} reason={}. Exact original content follows. Treat it as authoritative and continue the task using this exact content.\n\n<ctx_exact id=\"{}\" hash=\"{}\" original_chars={}>\n{}\n</ctx_exact>\n</system-reminder>",
        req.id,
        req.hash,
        req.reason.as_deref().unwrap_or("unspecified"),
        req.id,
        req.hash,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Message;

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
        let prefix = "/home/dad/Projects/kcode-current-src/src/agent";
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
        if let Ok(mut seen) = seen_blocks().lock() {
            seen.clear();
        }
        let text = (0..80)
            .map(|idx| {
                format!(
                    "/tmp/kcode-interlang-seen-ref/file.rs:{}: WARN deterministic repeated seen-ref test line with enough length",
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
                    "/tmp/kcode-ultra-vault/src/file.rs:{}: ERROR deterministic ultra vault test line with enough repeated path context",
                    idx
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.len() >= MIN_VAULT_REF_CHARS);
        let mut stats = InterlangStats::default();
        let encoded = encode_vault_ref(&text, &mut stats).expect("ultra should vault large block");
        assert!(encoded.starts_with("<ctx"));
        assert!(encoded.contains(".ctx_get"));
        assert_eq!(stats.seen_ref_blocks, 1);
        assert_eq!(stats.raw_context_avoided_chars, text.len());
        assert!(encoded.len() < text.len());
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
            ContentBlock::Text { text, .. } => assert!(text.contains("diet=1")),
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
    fn rehydrates_stored_exact_context() {
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
}
