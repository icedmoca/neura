use crate::message::{ContentBlock, Message, Role};
use crate::protocol::ServerEvent;
use crate::subtext_client::{
    SubtextChatMessage, SubtextChatRequest, SubtextEvent, stream_subtext_chat,
};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::mpsc;

/// Environment variable used to enable live Subtext latent-frame observation.
pub const SUBTEXT_WS_ENV: &str = "NEURA_SUBTEXT_WS";
pub const SUBTEXT_PATH_ENV: &str = "NEURA_SUBTEXT_PATH";
pub const SUBTEXT_AUTO_ENV: &str = "NEURA_SUBTEXT_AUTO";
pub const SUBTEXT_MODEL_ENV: &str = "NEURA_SUBTEXT_MODEL_ID";
const DEFAULT_SUBTEXT_WS: &str = "ws://127.0.0.1:8765/ws";
static AUTOSTARTED_SUBTEXT_URL: OnceLock<Option<String>> = OnceLock::new();

/// Spawn a best-effort "thought observer" for the current turn.
///
/// This does not replace the primary provider. By default it streams live
/// thinking notes from the local Neura OSS model (Ollama) through the reasoning
/// UI channel while the normal turn continues. The upstream Jacobian-lens
/// Subtext server is used only when explicitly configured (see
/// `resolve_subtext_websocket_url`).
pub(crate) fn spawn_subtext_observer_for_turn(
    session_id: String,
    messages: &[Message],
    event_sender: Option<mpsc::UnboundedSender<ServerEvent>>,
) {
    let Some(event_sender) = event_sender else {
        return;
    };

    let request_messages = messages_to_subtext(messages);
    if request_messages.is_empty() {
        return;
    }

    // Live real-20B logit-lens introspection (opt-in; stays quiet if the
    // logit-lens service is not running). Runs alongside whichever verbal /
    // companion observer is selected below.
    maybe_spawn_logitlens_observer(&session_id, messages, &event_sender);

    let Some(websocket_url) = resolve_subtext_websocket_url() else {
        // Default path: stream live thoughts from the local Neura OSS model.
        spawn_local_model_observer(session_id, messages, event_sender);
        return;
    };

    let request = SubtextChatRequest::new(
        request_messages,
        std::env::var("NEURA_SUBTEXT_MAX_TOKENS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(128),
    );

    let fallback_messages = messages.to_vec();
    let hydrate_session = session_id.clone();
    tokio::spawn(async move {
        send_subtext_latent(
            &event_sender,
            "companion:connecting".to_string(),
            None,
            Vec::new(),
            "[latent] connecting companion Jacobian-lens observer".to_string(),
        );

        let result = stream_subtext_chat(&websocket_url, &request, |event| {
            if let Some(rendered) = render_subtext_event(&event) {
                if let ServerEvent::SubtextLatent { phase, latent, .. } = &rendered {
                    if phase.starts_with("companion") && !latent.is_empty() {
                        crate::agent::latent_hydration::record(
                            &hydrate_session,
                            "companion",
                            &latent.join(", "),
                            latent,
                        );
                    }
                }
                send_rendered_subtext_event(&event_sender, rendered);
            }
        })
        .await;

        match result {
            Ok(_) => {
                send_subtext_latent(
                    &event_sender,
                    "companion:done".to_string(),
                    None,
                    Vec::new(),
                    "[latent] companion lens observer complete".to_string(),
                );
            }
            Err(error) => {
                send_subtext_latent(
                    &event_sender,
                    "companion:error".to_string(),
                    None,
                    Vec::new(),
                    format!("[latent] companion lens unavailable: {error}"),
                );
                // Degrade gracefully to the OSS verbal observer for this turn.
                spawn_local_model_observer(session_id, &fallback_messages, event_sender.clone());
            }
        }
    });
}

/// Environment for the real-20B logit-lens service (see `logitlens_server.py`).
pub const LOGITLENS_URL_ENV: &str = "NEURA_LOGITLENS_URL";
pub const LOGITLENS_ENABLE_ENV: &str = "NEURA_LOGITLENS";
const DEFAULT_LOGITLENS_URL: &str = "http://127.0.0.1:8801";

fn latest_user_text(messages: &[Message]) -> Option<String> {
    for message in messages.iter().rev() {
        if message.role != Role::User {
            continue;
        }
        let mut text = String::new();
        for block in &message.content {
            if let ContentBlock::Text { text: t, .. } = block {
                text.push_str(t);
            }
        }
        let text = text.trim().to_string();
        if !text.is_empty() {
            return Some(text);
        }
    }
    None
}

/// Probe the resident gpt-oss-20b logit-lens service for the turn's last user
/// message, surface its real "current belief" trajectory as a `logit:` frame,
/// and record it for next-turn ctx hydration. No-op (quiet) when disabled or
/// when the service is unreachable, so it never blocks or spams a turn.
fn maybe_spawn_logitlens_observer(
    session_id: &str,
    messages: &[Message],
    event_sender: &mpsc::UnboundedSender<ServerEvent>,
) {
    let disabled = std::env::var(LOGITLENS_ENABLE_ENV)
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "off" | "false" | "no"))
        .unwrap_or(false);
    if disabled {
        return;
    }
    let Some(text) = latest_user_text(messages) else {
        return;
    };
    let url = std::env::var(LOGITLENS_URL_ENV)
        .unwrap_or_else(|_| DEFAULT_LOGITLENS_URL.to_string());
    let session_id = session_id.to_string();
    let event_sender = event_sender.clone();

    tokio::spawn(async move {
        let client = reqwest::Client::new();
        let endpoint = format!("{}/introspect", url.trim_end_matches('/'));
        let response = match client
            .post(&endpoint)
            .timeout(Duration::from_secs(30))
            .json(&serde_json::json!({ "text": text }))
            .send()
            .await
            .and_then(|r| r.error_for_status())
        {
            Ok(response) => response,
            Err(_) => return, // service down: stay silent
        };
        let Ok(value) = response.json::<serde_json::Value>().await else {
            return;
        };

        let belief = value
            .get("final_belief")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if belief.is_empty() {
            return;
        }
        let conv = value.get("convergence").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let entropy = value.get("mean_entropy").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let validated = value.get("validated").and_then(|v| v.as_bool()).unwrap_or(false);

        // belief first, then the competing concepts that rose/fell across depth
        let mut words = vec![belief.clone()];
        if let Some(hyps) = value.get("hypotheses").and_then(|v| v.as_array()) {
            for h in hyps.iter().take(6) {
                if let Some(tok) = h.get("token").and_then(|t| t.as_str()) {
                    let tok = tok.to_string();
                    if tok != belief && !words.contains(&tok) {
                        words.push(tok);
                    }
                }
            }
        }

        crate::agent::latent_hydration::record(
            &session_id,
            "logit",
            &format!("real-model current belief: {belief}"),
            &words,
        );

        let text_line = format!(
            "[logit] real 20B believes → {belief} (converge {:.0}%, entropy {:.2}{})",
            conv * 100.0,
            entropy,
            if validated { "" } else { " ⚠unverified" },
        );
        send_subtext_latent(
            &event_sender,
            "logit:belief".to_string(),
            Some(belief),
            words,
            text_line,
        );
    });
}

/// Last-resort deterministic narrator: when no Subtext service, logit-lens,
/// or local model is reachable, narrate the actual pipeline stages so the
/// thought stream is never empty. Labels are honest (`[stage]`) — these are
/// pipeline facts, not model internals.
async fn run_stage_narration(
    event_sender: &mpsc::UnboundedSender<ServerEvent>,
    preview: &str,
) {
    let stages: [(&str, String); 4] = [
        (
            "stage:reading",
            format!("[stage] reading the request: “{preview}”"),
        ),
        (
            "stage:recall",
            "[stage] recalling memory + project knowledge for context".to_string(),
        ),
        (
            "stage:reasoning",
            "[stage] provider is reasoning over the assembled context".to_string(),
        ),
        (
            "stage:responding",
            "[stage] streaming the response and executing any tools".to_string(),
        ),
    ];
    for (phase, text) in stages {
        send_subtext_latent(event_sender, phase.to_string(), None, Vec::new(), text);
        tokio::time::sleep(Duration::from_millis(650)).await;
    }
}

fn spawn_local_model_observer(
    session_id: String,
    messages: &[Message],
    event_sender: mpsc::UnboundedSender<ServerEvent>,
) {
    let request_messages = messages_to_subtext(messages);
    if request_messages.is_empty() {
        return;
    }
    let preview: String = request_messages
        .last()
        .map(|m| m.content.chars().take(72).collect())
        .unwrap_or_default();
    // Prefer the same local sidecar the memory system uses (e.g. Ollama on
    // :11434), so the fallback observer targets a model that is actually
    // running rather than the LM Studio default port.
    let sidecar = crate::sidecar::SidecarConfig::from_env();
    tokio::spawn(async move {
        if !sidecar.enabled {
            // Never go silent: degrade to deterministic stage narration.
            run_stage_narration(&event_sender, &preview).await;
            return;
        }
        let model = sidecar.model.clone();
        let url = format!("{}/chat/completions", sidecar.url.trim_end_matches('/'));
        send_subtext_latent(
            &event_sender,
            "oss:start".to_string(),
            None,
            Vec::new(),
            format!("[oss] using Neura OSS model observer ({model})"),
        );

        let mut prompt = String::from(
            "You are watching another AI assistant work. In one or two terse sentences, narrate what it should be thinking about / focusing on next for this conversation. Output only the thought narration, not a final answer.\n\n",
        );
        for message in request_messages.iter().rev().take(4).rev() {
            prompt.push_str(&format!("{}: {}\n", message.role, message.content));
        }
        let max_tokens: usize = std::env::var("NEURA_SUBTEXT_MAX_TOKENS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(96);
        let body = serde_json::json!({
            "model": model,
            "messages": [
                {"role":"system","content":"You are a local latent observer for Neura. Produce terse thinking/status notes only."},
                {"role":"user","content": prompt}
            ],
            "max_tokens": max_tokens,
            "temperature": 0.2,
            "stream": true
        });
        // Streaming can legitimately run longer than a single-shot memory call;
        // give it room but keep it bounded so a stuck sidecar never lingers.
        let stream_timeout = Duration::from_millis(sidecar.timeout_ms.max(15_000));
        let client = reqwest::Client::new();
        let response = match client
            .post(url)
            .timeout(stream_timeout)
            .json(&body)
            .send()
            .await
            .and_then(|res| res.error_for_status())
        {
            Ok(response) => response,
            Err(error) => {
                send_subtext_latent(
                    &event_sender,
                    "oss:unavailable".to_string(),
                    None,
                    Vec::new(),
                    format!("[oss] Neura OSS model observer unavailable: {error}"),
                );
                // Never go silent: degrade to deterministic stage narration.
                run_stage_narration(&event_sender, &preview).await;
                return;
            }
        };

        let final_thought = stream_openai_thoughts(&event_sender, response).await;
        // Persist the turn's OSS reflection for next-turn ctx hydration.
        if !final_thought.trim().is_empty() {
            crate::agent::latent_hydration::record(
                &session_id,
                "oss",
                final_thought.trim(),
                &[],
            );
        }
        send_subtext_latent(
            &event_sender,
            "oss:done".to_string(),
            None,
            Vec::new(),
            "[oss] thought observer complete".to_string(),
        );
    });
}

/// Consume an OpenAI-compatible streaming (`text/event-stream`) chat response
/// and emit incremental latent-thought frames so the UI updates live.
async fn stream_openai_thoughts(
    event_sender: &mpsc::UnboundedSender<ServerEvent>,
    response: reqwest::Response,
) -> String {
    use futures::StreamExt;

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut accumulated = String::new();

    while let Some(chunk) = stream.next().await {
        let Ok(bytes) = chunk else { break };
        buffer.push_str(&String::from_utf8_lossy(&bytes));

        // SSE frames are separated by newlines; process complete lines only.
        while let Some(newline) = buffer.find('\n') {
            let line = buffer[..newline].trim().to_string();
            buffer.drain(..=newline);
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };
            let delta = value.pointer("/choices/0/delta");
            // gpt-oss via Ollama streams live thinking in `reasoning` (with
            // `content` empty until the thought resolves); other backends use
            // `reasoning_content` or plain `content`.
            let field = |name: &str| {
                delta
                    .and_then(|d| d.get(name))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
            };
            let piece = field("reasoning")
                .or_else(|| field("reasoning_content"))
                .or_else(|| field("content"))
                .unwrap_or("");
            if piece.is_empty() {
                continue;
            }
            accumulated.push_str(piece);
            emit_thought_progress(event_sender, &accumulated);
        }
    }

    // Flush any trailing buffered line (streams may omit a final newline).
    if let Some(data) = buffer.trim().strip_prefix("data:") {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(data.trim()) {
            let delta = value.pointer("/choices/0/delta");
            let piece = delta
                .and_then(|d| d.get("reasoning"))
                .or_else(|| delta.and_then(|d| d.get("reasoning_content")))
                .or_else(|| delta.and_then(|d| d.get("content")))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            accumulated.push_str(piece);
        }
    }
    if !accumulated.trim().is_empty() {
        emit_thought_progress(event_sender, &accumulated);
    }
    accumulated
}

/// Emit a latent frame from the accumulated thought text so far. `latent`
/// carries the most recent words for compact chip-style rendering, while `text`
/// keeps the full running narration.
fn emit_thought_progress(event_sender: &mpsc::UnboundedSender<ServerEvent>, accumulated: &str) {
    let trimmed = accumulated.trim();
    if trimmed.is_empty() {
        return;
    }
    let recent_words: Vec<String> = trimmed
        .split_whitespace()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|word| word.to_string())
        .collect();
    send_subtext_latent(
        event_sender,
        "oss:thinking".to_string(),
        recent_words.last().cloned(),
        recent_words,
        format!("[oss-thought] {trimmed}"),
    );
}

/// Resolve the optional Jacobian-lens Subtext websocket.
///
/// The default thought observer is the local Neura OSS model (see
/// `spawn_local_model_observer`). The Jacobian-lens server (upstream Subtext) is
/// entirely opt-in because it requires a lens fitted to a specific model and a
/// GPU large enough to host it. It is used only when:
///   * `NEURA_SUBTEXT_WS` points at a running server, or
///   * a server is already listening on the default port, or
///   * `NEURA_SUBTEXT_AUTO` is explicitly enabled (autostart the local repo).
fn resolve_subtext_websocket_url() -> Option<String> {
    if let Ok(url) = std::env::var(SUBTEXT_WS_ENV) {
        let trimmed = url.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    AUTOSTARTED_SUBTEXT_URL
        .get_or_init(|| {
            if localhost_port_is_open("127.0.0.1:8765") {
                return Some(DEFAULT_SUBTEXT_WS.to_string());
            }
            if jacobian_autostart_enabled() && try_spawn_subtext_sidecar() {
                return Some(DEFAULT_SUBTEXT_WS.to_string());
            }
            None
        })
        .clone()
}

/// Whether to autostart the opt-in Jacobian-lens Subtext server. Default off:
/// the Neura OSS model observer is used unless explicitly enabled.
fn jacobian_autostart_enabled() -> bool {
    std::env::var(SUBTEXT_AUTO_ENV)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "on" | "yes"
            )
        })
        .unwrap_or(false)
}

fn localhost_port_is_open(addr: &str) -> bool {
    let Ok(addr) = addr.parse() else {
        return false;
    };
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(120)).is_ok()
}

/// Default Subtext model when none is configured. Matches the upstream
/// `server.py` default so autostart works out of the box once the repo + venv
/// are present.
const DEFAULT_SUBTEXT_MODEL_ID: &str = "Qwen/Qwen3.5-4B";

fn try_spawn_subtext_sidecar() -> bool {
    let model_id = std::env::var(SUBTEXT_MODEL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_SUBTEXT_MODEL_ID.to_string());

    let Some(root) = find_subtext_root() else {
        return false;
    };
    let server = root.join("server.py");
    if !server.is_file() {
        return false;
    }

    let python = std::env::var("NEURA_SUBTEXT_PYTHON")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            let venv_python = root.join(".venv/bin/python");
            venv_python.is_file().then_some(venv_python)
        })
        .unwrap_or_else(|| PathBuf::from("python3"));
    std::process::Command::new(python)
        .arg(server)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("SUBTEXT_MODEL_ID", model_id)
        .spawn()
        .is_ok()
}

fn find_subtext_root() -> Option<PathBuf> {
    if let Ok(path) = std::env::var(SUBTEXT_PATH_ENV) {
        let path = PathBuf::from(path);
        if path.join("server.py").is_file() {
            return Some(path);
        }
    }

    let home = std::env::var_os("HOME").map(PathBuf::from);
    let cwd = std::env::current_dir().ok();
    let mut candidates = Vec::new();
    // Vendored copy — Subtext ships with Neura (vendor/subtext), installed to
    // $NEURA_HOME/vendor/subtext by install.sh. Checked first so the bundled
    // integration wins unless SUBTEXT_PATH points elsewhere.
    let neura_home = std::env::var_os("NEURA_HOME")
        .map(PathBuf::from)
        .or_else(|| home.clone().map(|h| h.join(".neura")));
    if let Some(neura_home) = &neura_home {
        candidates.push(neura_home.join("vendor/subtext"));
        candidates.push(neura_home.join("build-src/neura/vendor/subtext"));
    }
    if let Some(home) = home {
        candidates.push(home.join("Subtext"));
        candidates.push(home.join("subtext"));
        candidates.push(home.join("src/Subtext"));
        candidates.push(home.join("src/subtext"));
        candidates.push(home.join("code/Subtext"));
        candidates.push(home.join("code/subtext"));
    }
    if let Some(cwd) = cwd {
        candidates.push(cwd.join("Subtext"));
        candidates.push(cwd.join("subtext"));
        if let Some(parent) = cwd.parent() {
            candidates.push(parent.join("Subtext"));
            candidates.push(parent.join("subtext"));
        }
    }

    candidates
        .into_iter()
        .find(|path| path.join("server.py").is_file())
}

fn send_rendered_subtext_event(
    event_sender: &mpsc::UnboundedSender<ServerEvent>,
    event: ServerEvent,
) {
    let _ = event_sender.send(event);
}

fn send_subtext_latent(
    event_sender: &mpsc::UnboundedSender<ServerEvent>,
    phase: String,
    token: Option<String>,
    latent: Vec<String>,
    text: String,
) {
    let _ = event_sender.send(ServerEvent::SubtextLatent {
        phase,
        token,
        latent,
        text,
    });
}

fn messages_to_subtext(messages: &[Message]) -> Vec<SubtextChatMessage> {
    messages
        .iter()
        .filter_map(|message| {
            let content = message_text(message).trim().to_string();
            if content.is_empty() {
                return None;
            }
            let role = match message.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };
            Some(SubtextChatMessage {
                role: role.to_string(),
                content,
            })
        })
        .collect()
}

fn message_text(message: &Message) -> String {
    let mut parts = Vec::new();
    for block in &message.content {
        match block {
            ContentBlock::Text { text, .. } | ContentBlock::Reasoning { text } => {
                parts.push(text.clone());
            }
            ContentBlock::ToolResult { content, .. } => {
                parts.push(format!("[tool result]\n{content}"));
            }
            ContentBlock::ToolUse { name, input, .. } => {
                parts.push(format!("[tool use:{name}] {input}"));
            }
            ContentBlock::Image { .. } => {
                parts.push("[image]".to_string());
            }
            ContentBlock::OpenAICompaction { .. } => {
                parts.push("[compacted context]".to_string());
            }
        }
    }
    parts.join("\n")
}

fn render_subtext_event(event: &SubtextEvent) -> Option<ServerEvent> {
    match event {
        SubtextEvent::Ready { model, .. } => Some(ServerEvent::SubtextLatent {
            phase: "companion:ready".to_string(),
            token: None,
            latent: Vec::new(),
            text: format!(
                "[latent] companion lens ready{}",
                model
                    .as_ref()
                    .map(|model| format!(" ({model})"))
                    .unwrap_or_default()
            ),
        }),
        SubtextEvent::Frame(frame) => {
            let phase = format!(
                "companion:{}",
                frame.phase.as_deref().unwrap_or("latent")
            );
            let token = frame.out.clone();
            let latent = frame.latent_words();
            let subphase = frame.phase.as_deref().unwrap_or("latent");
            let text = if latent.is_empty() {
                format!("[latent:{subphase}] {}", token.as_deref().unwrap_or_default())
            } else {
                format!(
                    "[latent:{subphase}] {} → {}",
                    token.as_deref().unwrap_or_default(),
                    latent.join(", ")
                )
            };
            Some(ServerEvent::SubtextLatent {
                phase,
                token,
                latent,
                text,
            })
        }
        SubtextEvent::Error { error } => Some(ServerEvent::SubtextLatent {
            phase: "companion:error".to_string(),
            token: None,
            latent: Vec::new(),
            text: format!("[latent] error: {error}"),
        }),
        SubtextEvent::Done { .. } | SubtextEvent::Unknown(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_messages_to_subtext_chat_messages() {
        let messages = vec![Message::user("hello")];
        let converted = messages_to_subtext(&messages);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].role, "user");
        assert_eq!(converted[0].content, "hello");
    }

    #[test]
    fn renders_latent_frame_for_reasoning_ui() {
        let event = SubtextEvent::Frame(crate::subtext_client::SubtextLatentFrame {
            phase: Some("thinking".to_string()),
            pos: Some(1),
            out: Some("token".to_string()),
            thoughts: Vec::new(),
            latent_tokens: vec!["plan".to_string(), "search".to_string()],
            extra: Default::default(),
        });
        let Some(ServerEvent::SubtextLatent {
            phase,
            token,
            latent,
            text,
            ..
        }) = render_subtext_event(&event)
        else {
            panic!("expected subtext latent event");
        };
        assert_eq!(phase, "companion:thinking");
        assert_eq!(token.as_deref(), Some("token"));
        assert_eq!(latent, vec!["plan", "search"]);
        assert_eq!(text, "[latent:thinking] token → plan, search");
    }
}
