//! Client for the Subtext latent-thought websocket protocol.
//!
//! Subtext (<https://github.com/ninjahawk/Subtext>) exposes a websocket at
//! `/ws` that streams:
//! - `ready` once connected,
//! - per-token latent `frame` payloads while reading/thinking,
//! - `done` with the final assistant text.
//!
//! This client keeps Neura's integration typed and observable without depending
//! on Subtext's Python packages at compile time.

use anyhow::{Context, Result, anyhow};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// One user/assistant message sent to Subtext.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubtextChatMessage {
    pub role: String,
    pub content: String,
}

impl SubtextChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
        }
    }
}

/// Request body expected by Subtext's websocket server.
///
/// The upstream server ignores any frame whose `type` is not `"chat"`
/// (`if req.get("type") != "chat": continue`), so the discriminator must always
/// be sent on the wire.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubtextChatRequest {
    #[serde(rename = "type")]
    pub request_type: String,
    pub messages: Vec<SubtextChatMessage>,
    pub max_tokens: usize,
}

impl SubtextChatRequest {
    pub fn new(messages: Vec<SubtextChatMessage>, max_tokens: usize) -> Self {
        Self {
            request_type: "chat".to_string(),
            messages,
            max_tokens,
        }
    }

    pub fn single_user(content: impl Into<String>, max_tokens: usize) -> Self {
        Self::new(vec![SubtextChatMessage::user(content)], max_tokens)
    }
}

/// Normalized event emitted by the Subtext client.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SubtextEvent {
    Ready {
        model: Option<String>,
        vocab: Option<usize>,
    },
    /// Latent frame for a token while Subtext is reading or generating.
    Frame(SubtextLatentFrame),
    Done {
        text: String,
    },
    Error {
        error: String,
    },
    Unknown(serde_json::Value),
}

/// A ranked latent "silent word" decoded from the model's residual stream.
/// Upstream sends `{"w": word, "p": strength, "d": depth}` objects under
/// `thoughts`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubtextThought {
    #[serde(alias = "word", alias = "token")]
    pub w: String,
    #[serde(default)]
    pub p: Option<f64>,
    #[serde(default)]
    pub d: Option<f64>,
}

/// A Subtext latent frame. The upstream server sends fields including `phase`,
/// the surface token (`tok` while reading, `out` while thinking), and a ranked
/// set of decoded latent thoughts under `thoughts`. `extra` preserves any
/// upstream fields we do not yet model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubtextLatentFrame {
    pub phase: Option<String>,
    pub pos: Option<usize>,
    /// Surface token being emitted. `tok` is used during the reading phase and
    /// `out` during the thinking/speaking phase; both map here.
    #[serde(default, alias = "tok")]
    pub out: Option<String>,
    /// Structured latent thoughts (current upstream format).
    #[serde(default)]
    pub thoughts: Vec<SubtextThought>,
    /// Legacy/plain-string latent list, kept for forward/backward compatibility.
    #[serde(default, alias = "top", alias = "tokens", alias = "latent")]
    pub latent_tokens: Vec<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl SubtextLatentFrame {
    /// Ranked latent words for display, regardless of upstream wire shape.
    pub fn latent_words(&self) -> Vec<String> {
        if !self.thoughts.is_empty() {
            return self
                .thoughts
                .iter()
                .map(|thought| thought.w.trim().to_string())
                .filter(|word| !word.is_empty())
                .collect();
        }
        self.latent_tokens.clone()
    }
}

impl SubtextEvent {
    fn from_value(value: serde_json::Value) -> Self {
        let event_type = value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        match event_type {
            "ready" | "hello" => SubtextEvent::Ready {
                model: value
                    .get("model")
                    .and_then(|v| v.as_str())
                    .map(ToOwned::to_owned),
                vocab: value
                    .get("vocab")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize),
            },
            "frame" => match serde_json::from_value::<SubtextLatentFrame>(value) {
                Ok(frame) => SubtextEvent::Frame(frame),
                Err(error) => SubtextEvent::Error {
                    error: format!("invalid Subtext frame: {error}"),
                },
            },
            "done" => SubtextEvent::Done {
                text: value
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            },
            "error" => SubtextEvent::Error {
                error: value
                    .get("error")
                    .or_else(|| value.get("message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown Subtext error")
                    .to_string(),
            },
            _ => SubtextEvent::Unknown(value),
        }
    }
}

/// Stream a request to Subtext and call `on_event` for every received event.
pub async fn stream_subtext_chat<F>(
    websocket_url: &str,
    request: &SubtextChatRequest,
    mut on_event: F,
) -> Result<Option<String>>
where
    F: FnMut(SubtextEvent) + Send,
{
    let (mut socket, _) = connect_async(websocket_url)
        .await
        .with_context(|| format!("connect Subtext websocket at {websocket_url}"))?;

    let mut final_text = None;

    // Subtext sends `ready` before it receives a request.
    if let Some(message) = socket.next().await {
        dispatch_message(message?, &mut on_event, &mut final_text)?;
    }

    socket
        .send(Message::Text(serde_json::to_string(request)?.into()))
        .await
        .context("send Subtext chat request")?;

    while let Some(message) = socket.next().await {
        dispatch_message(message?, &mut on_event, &mut final_text)?;
        if final_text.is_some() {
            break;
        }
    }

    Ok(final_text)
}

fn dispatch_message<F>(
    message: Message,
    on_event: &mut F,
    final_text: &mut Option<String>,
) -> Result<()>
where
    F: FnMut(SubtextEvent),
{
    match message {
        Message::Text(text) => {
            let value: serde_json::Value = serde_json::from_str(&text)
                .with_context(|| format!("parse Subtext websocket message: {text}"))?;
            let event = SubtextEvent::from_value(value);
            if let SubtextEvent::Done { text } = &event {
                *final_text = Some(text.clone());
            }
            on_event(event);
            Ok(())
        }
        Message::Binary(_) => Err(anyhow!("unexpected binary Subtext websocket message")),
        Message::Close(_) => Ok(()),
        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{SinkExt, StreamExt};
    use std::net::SocketAddr;
    use tokio::net::TcpListener;
    use tokio_tungstenite::{accept_async, tungstenite::Message};

    async fn spawn_mock_subtext() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            // Real Subtext greets with `hello`, not `ready`.
            socket
                .send(Message::Text(
                    serde_json::json!({"type":"hello","model":"mock","n_layers":4,"layers":[1,2]})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            let request = socket.next().await.unwrap().unwrap().into_text().unwrap();
            let request: SubtextChatRequest = serde_json::from_str(&request).unwrap();
            // The server ignores anything whose `type` is not `chat`.
            assert_eq!(request.request_type, "chat");
            assert_eq!(request.max_tokens, 8);
            // Reading phase uses `tok` and structured `thoughts` objects.
            socket
                .send(Message::Text(
                    serde_json::json!({
                        "type":"frame",
                        "phase":"reading",
                        "tok":"hello",
                        "thoughts":[
                            {"w":"greeting","p":0.9,"d":0.5},
                            {"w":"intent","p":0.4,"d":0.6}
                        ]
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            socket
                .send(Message::Text(
                    serde_json::json!({"type":"done","text":"mock reply"})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
        });
        addr
    }

    #[tokio::test]
    async fn streams_subtext_events_until_done() {
        let addr = spawn_mock_subtext().await;
        let request = SubtextChatRequest::single_user("hello", 8);
        let mut events = Vec::new();
        let final_text = stream_subtext_chat(&format!("ws://{addr}/ws"), &request, |event| {
            events.push(event);
        })
        .await
        .unwrap();

        assert_eq!(final_text.as_deref(), Some("mock reply"));
        assert!(matches!(events[0], SubtextEvent::Ready { .. }));
        assert!(matches!(events[1], SubtextEvent::Frame(_)));
        assert!(matches!(events[2], SubtextEvent::Done { .. }));
        let SubtextEvent::Frame(frame) = &events[1] else {
            panic!("expected frame");
        };
        assert_eq!(frame.phase.as_deref(), Some("reading"));
        // `tok` maps onto `out`, and structured thoughts decode into words.
        assert_eq!(frame.out.as_deref(), Some("hello"));
        assert_eq!(frame.latent_words(), vec!["greeting", "intent"]);
    }
}
