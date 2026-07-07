use crate::message::{ContentBlock, Message, Role};
use crate::protocol::ServerEvent;
use crate::subtext_client::{
    SubtextChatMessage, SubtextChatRequest, SubtextEvent, stream_subtext_chat,
};
use tokio::sync::mpsc;

/// Environment variable used to enable live Subtext latent-frame observation.
pub const SUBTEXT_WS_ENV: &str = "NEURA_SUBTEXT_WS";

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
    let Ok(websocket_url) = std::env::var(SUBTEXT_WS_ENV) else {
        return;
    };
    if websocket_url.trim().is_empty() {
        return;
    }

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
