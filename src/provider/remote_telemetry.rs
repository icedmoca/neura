use serde_json::json;
use std::fs::OpenOptions;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

fn telemetry_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".kcode")
        .join("remote-provider-requests.jsonl")
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

pub(crate) fn log_request(provider: &str, model: &str, endpoint: &str, body_bytes: usize) {
    let record = json!({
        "timestamp_ms": now_ms(),
        "event": "request",
        "provider": provider,
        "model": model,
        "endpoint": endpoint,
        "serialized_http_request_body_bytes": body_bytes,
        "serialized_http_request_body_chars": body_bytes,
        "excludes_local_sidecar_prompt": true,
    });
    append(record);
}

pub(crate) fn log_usage(
    provider: &str,
    model: &str,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    total_tokens: Option<u64>,
) {
    let record = json!({
        "timestamp_ms": now_ms(),
        "event": "usage",
        "provider": provider,
        "model": model,
        "provider_reported_input_tokens": input_tokens,
        "provider_reported_output_tokens": output_tokens,
        "provider_reported_total_tokens": total_tokens,
    });
    append(record);
}

pub(crate) fn log_pre_provider_payload(
    provider: &str,
    messages: &[crate::message::Message],
    tools: &[crate::message::ToolDefinition],
    static_prompt: &str,
    dynamic_prompt: &str,
    compacted_short_turn: bool,
) {
    let messages_chars = message_chars(messages);
    let tools_json_chars = serde_json::to_vec(tools).map(|v| v.len()).unwrap_or(0);
    let interlang_refs_chars = messages
        .iter()
        .flat_map(|message| message.content.iter())
        .map(content_text)
        .map(|text| count_ref_chars(&text))
        .sum::<usize>();
    let memory_chars = dynamic_prompt
        .matches("<system-reminder>")
        .count()
        .saturating_mul(0)
        + if dynamic_prompt.contains("memory") || dynamic_prompt.contains("Memory") {
            dynamic_prompt.len()
        } else {
            0
        };
    let final_request_json_chars = serde_json::json!({
        "system_static": static_prompt,
        "system_dynamic": dynamic_prompt,
        "messages": messages,
        "tools": tools,
    })
    .to_string()
    .len();
    let record = json!({
        "timestamp_ms": now_ms(),
        "event": "pre_provider_payload",
        "provider": provider,
        "system_prompt_chars": static_prompt.len(),
        "developer_prompt_chars": dynamic_prompt.len(),
        "messages_chars": messages_chars,
        "tools_json_chars": tools_json_chars,
        "memory_chars": memory_chars,
        "interlang_refs_chars": interlang_refs_chars,
        "sidecar_prompt_chars": null,
        "final_request_json_chars_estimate": final_request_json_chars,
        "compact_provider_messages_for_short_turn_ran": compacted_short_turn,
        "excludes_local_sidecar_prompt": true,
    });
    append(record);
}

fn message_chars(messages: &[crate::message::Message]) -> usize {
    messages
        .iter()
        .flat_map(|message| message.content.iter())
        .map(content_text)
        .map(|text| text.len())
        .sum()
}

fn content_text(block: &crate::message::ContentBlock) -> String {
    match block {
        crate::message::ContentBlock::Text { text, .. } => text.clone(),
        crate::message::ContentBlock::Reasoning { text } => text.clone(),
        crate::message::ContentBlock::ToolResult { content, .. } => content.clone(),
        crate::message::ContentBlock::ToolUse { name, input, .. } => {
            format!("{}{}", name, input)
        }
        crate::message::ContentBlock::Image { data, .. } => data.clone(),
        crate::message::ContentBlock::OpenAICompaction { encrypted_content } => {
            encrypted_content.clone()
        }
    }
}

fn count_ref_chars(text: &str) -> usize {
    ["<ctx ", "<il:seen", "<ctx_candidate", "<il:v1>"]
        .iter()
        .filter(|needle| text.contains(**needle))
        .map(|_| text.len())
        .sum()
}

fn append(record: serde_json::Value) {
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(telemetry_path())
    {
        let _ = writeln!(file, "{}", record);
    }
}
