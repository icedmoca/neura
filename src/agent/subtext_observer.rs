use crate::local_model::LocalModelConfig;
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

/// Spawn a best-effort Subtext observer for the current turn.
///
/// This intentionally does not replace the primary provider. Subtext runs as a
/// local sidecar and streams latent frames through the existing reasoning UI
/// channel while the normal turn continues.
pub(crate) fn spawn_subtext_observer_for_turn(
    _session_id: String,
    messages: &[Message],
    event_sender: Option<mpsc::UnboundedSender<ServerEvent>>,
) {
    let Some(event_sender) = event_sender else {
        return;
    };
    let Some(websocket_url) = resolve_subtext_websocket_url() else {
        let _ = event_sender.send(ServerEvent::SubtextLatent {
            phase: "unavailable".to_string(),
            token: None,
            latent: Vec::new(),
            text: "[subtext] local sidecar not configured or found; continuing without latent observer".to_string(),
        });
        return;
    };

    let request = SubtextChatRequest {
        messages: messages_to_subtext(messages),
        max_tokens: std::env::var("NEURA_SUBTEXT_MAX_TOKENS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(128),
    };
    if request.messages.is_empty() {
        return;
    }

    tokio::spawn(async move {
        send_subtext_latent(
            &event_sender,
            "connecting".to_string(),
            None,
            Vec::new(),
            "[subtext] connecting local latent observer".to_string(),
        );

        let result = stream_subtext_chat(&websocket_url, &request, |event| {
            if let Some(event) = render_subtext_event(&event) {
                send_rendered_subtext_event(&event_sender, event);
            }
        })
        .await;

        match result {
            Ok(_) => {
                send_subtext_latent(
                    &event_sender,
                    "done".to_string(),
                    None,
                    Vec::new(),
                    "[subtext] latent observer complete".to_string(),
                );
            }
            Err(error) => {
                send_subtext_latent(
                    &event_sender,
                    "error".to_string(),
                    None,
                    Vec::new(),
                    format!("[subtext] observer unavailable: {error}"),
                );
                send_subtext_latent(
                    &event_sender,
                    "stopped".to_string(),
                    None,
                    Vec::new(),
                    "[subtext] latent observer stopped".to_string(),
                );
            }
        }
    });
}

fn spawn_local_model_observer(
    _session_id: String,
    messages: &[Message],
    event_sender: mpsc::UnboundedSender<ServerEvent>,
) {
    let request_messages = messages_to_subtext(messages);
    if request_messages.is_empty() {
        return;
    }
    tokio::spawn(async move {
        let config = LocalModelConfig::default();
        let model = config
            .model
            .clone()
            .unwrap_or_else(|| "neura-sidecar-20b".to_string());
        let url = format!(
            "{}{}",
            config.base_url.trim_end_matches('/'),
            config.chat_path
        );
        send_subtext_latent(
            &event_sender,
            "local_model".to_string(),
            None,
            Vec::new(),
            format!("[subtext] using Neura local model observer ({model})"),
        );

        let mut prompt = String::from(
            "Briefly summarize what the assistant should focus on next. Return terse latent notes, not the final answer.\n\n",
        );
        for message in request_messages.iter().rev().take(4).rev() {
            prompt.push_str(&format!("{}: {}\n", message.role, message.content));
        }
        let body = serde_json::json!({
            "model": model,
            "messages": [
                {"role":"system","content":"You are a local latent observer for Neura. Produce terse thinking/status notes only."},
                {"role":"user","content": prompt}
            ],
            "max_tokens": 80,
            "temperature": 0.2,
            "stream": false
        });
        let client = reqwest::Client::new();
        let mut req = client
            .post(url)
            .timeout(Duration::from_millis(config.timeout_ms.max(750)))
            .json(&body);
        if let Some(key) = config.api_key {
            req = req.bearer_auth(key);
        }
        match req.send().await.and_then(|res| res.error_for_status()) {
            Ok(response) => match response.json::<serde_json::Value>().await {
                Ok(value) => {
                    let text = value
                        .pointer("/choices/0/message/content")
                        .and_then(|v| v.as_str())
                        .or_else(|| value.pointer("/choices/0/text").and_then(|v| v.as_str()))
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if !text.is_empty() {
                        send_subtext_latent(
                            &event_sender,
                            "local_model".to_string(),
                            None,
                            Vec::new(),
                            format!("[local-thought] {text}"),
                        );
                    }
                }
                Err(error) => send_subtext_latent(
                    &event_sender,
                    "local_model_error".to_string(),
                    None,
                    Vec::new(),
                    format!("[subtext] local model observer parse error: {error}"),
                ),
            },
            Err(error) => send_subtext_latent(
                &event_sender,
                "local_model_unavailable".to_string(),
                None,
                Vec::new(),
                format!("[subtext] Neura local model observer unavailable: {error}"),
            ),
        }
    });
}

fn resolve_subtext_websocket_url() -> Option<String> {
    if let Ok(url) = std::env::var(SUBTEXT_WS_ENV) {
        let trimmed = url.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    AUTOSTARTED_SUBTEXT_URL
        .get_or_init(|| {
            if subtext_autostart_disabled() {
                return None;
            }
            if localhost_port_is_open("127.0.0.1:8765") {
                return Some(DEFAULT_SUBTEXT_WS.to_string());
            }
            try_spawn_subtext_sidecar().then(|| DEFAULT_SUBTEXT_WS.to_string())
        })
        .clone()
}

fn subtext_autostart_disabled() -> bool {
    std::env::var(SUBTEXT_AUTO_ENV)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
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

fn try_spawn_subtext_sidecar() -> bool {
    let Ok(model_id) = std::env::var(SUBTEXT_MODEL_ENV) else {
        return false;
    };
    let model_id = model_id.trim().to_string();
    if model_id.is_empty() {
        return false;
    }

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
            phase: "ready".to_string(),
            token: None,
            latent: Vec::new(),
            text: format!(
                "[subtext] ready{}",
                model
                    .as_ref()
                    .map(|model| format!(" ({model})"))
                    .unwrap_or_default()
            ),
        }),
        SubtextEvent::Frame(frame) => {
            let phase = frame.phase.as_deref().unwrap_or("latent").to_string();
            let token = frame.out.clone();
            let latent = frame.latent_tokens.clone();
            let text = if latent.is_empty() {
                format!("[subtext:{phase}] {}", token.as_deref().unwrap_or_default())
            } else {
                format!(
                    "[subtext:{phase}] {} → {}",
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
            phase: "error".to_string(),
            token: None,
            latent: Vec::new(),
            text: format!("[subtext] error: {error}"),
        }),
        SubtextEvent::Done { .. } | SubtextEvent::Unknown(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::ContentBlock;

    #[test]
    fn converts_messages_to_subtext_chat_messages() {
        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "hello".to_string(),
                citations: None,
            }],
        }];
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
        assert_eq!(phase, "thinking");
        assert_eq!(token.as_deref(), Some("token"));
        assert_eq!(latent, vec!["plan", "search"]);
        assert_eq!(text, "[subtext:thinking] token → plan, search");
    }
}
