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

fn append(record: serde_json::Value) {
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(telemetry_path())
    {
        let _ = writeln!(file, "{}", record);
    }
}
