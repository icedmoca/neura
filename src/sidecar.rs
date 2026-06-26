//! Local sidecar model integration for Neura internals.
//!
//! The sidecar is intentionally optional: memory/context systems can ask it for
//! cheap local summarization/classification, but must always fall back when it is
//! unavailable so the main agent experience remains reliable.

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::env;
use std::time::Duration;

const DEFAULT_NEURA_SIDECAR_URL: &str = "http://127.0.0.1:8080/v1";
const DEFAULT_NEURA_SIDECAR_MODEL: &str = "gpt-oss-20b-mxfp4_moe";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SidecarKind {
    Ollama,
    OpenAiCompatible,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarConfig {
    pub enabled: bool,
    pub url: String,
    pub model: String,
    pub kind: SidecarKind,
    pub timeout_ms: u64,
    /// Max transcript chars sent per extraction. Small local models have tiny
    /// context windows; oversized prompts get truncated (garbage) and run slow.
    pub max_transcript_chars: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarHealth {
    pub enabled: bool,
    pub ok: bool,
    pub url: String,
    pub model: String,
    pub kind: SidecarKind,
    pub message: String,
}

fn discover_neura_model_name() -> Option<String> {
    let home = env::var("HOME").ok()?;
    let dir = std::path::Path::new(&home).join(".neura/models/gguf");
    let preferred = [
        "neura-oss-20b-mxfp4.gguf",
        "gpt-oss-20b-mxfp4_moe.gguf",
        "jcode-gpt-oss-20b.gguf",
        "deepseek-coder-6.7b-instruct.Q4_K_M.gguf",
    ];
    for file in preferred {
        if dir.join(file).exists() {
            return Some(file.trim_end_matches(".gguf").to_string());
        }
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            entry
                .path()
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
        })
        .next()
}

impl SidecarConfig {
    pub fn from_env() -> Self {
        // Config provides persistent defaults; environment variables override them.
        let agents = &crate::config::config().agents;
        let url = env::var("NEURA_SIDECAR_URL")
            .ok()
            .or_else(|| env::var("NEURA_LOCAL_MODEL_BASE_URL").ok())
            .or_else(|| agents.memory_sidecar_url.clone())
            .unwrap_or_else(|| DEFAULT_NEURA_SIDECAR_URL.to_string());
        let model = env::var("NEURA_SIDECAR_MODEL")
            .ok()
            .or_else(|| env::var("NEURA_LOCAL_MODEL").ok())
            .or_else(|| agents.memory_model.clone())
            .unwrap_or_else(|| {
                discover_neura_model_name()
                    .unwrap_or_else(|| DEFAULT_NEURA_SIDECAR_MODEL.to_string())
            });
        let kind = match env::var("NEURA_SIDECAR_KIND")
            .ok()
            .or_else(|| agents.memory_sidecar_kind.clone())
            .unwrap_or_else(|| "openai-compatible".to_string())
            .to_lowercase()
            .as_str()
        {
            "openai" | "openai-compatible" | "v1" => SidecarKind::OpenAiCompatible,
            "ollama" => SidecarKind::Ollama,
            _ => SidecarKind::OpenAiCompatible,
        };
        let enabled = env::var("NEURA_SIDECAR_ENABLED")
            .map(|v| !matches!(v.as_str(), "0" | "false" | "FALSE" | "off" | "OFF"))
            .unwrap_or(true);
        let timeout_ms = env::var("NEURA_SIDECAR_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .or(agents.memory_sidecar_timeout_ms)
            .unwrap_or(2500);
        let max_transcript_chars = env::var("NEURA_SIDECAR_MAX_TRANSCRIPT_CHARS")
            .ok()
            .and_then(|v| v.parse().ok())
            .or(agents.memory_sidecar_max_transcript_chars)
            .unwrap_or(24_000);
        Self {
            enabled,
            url,
            model,
            kind,
            timeout_ms,
            max_transcript_chars,
        }
    }
}

pub struct SidecarClient {
    cfg: SidecarConfig,
    http: reqwest::blocking::Client,
}

impl SidecarClient {
    pub fn new(cfg: SidecarConfig) -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(cfg.timeout_ms))
            .build()
            .context("build sidecar HTTP client")?;
        Ok(Self { cfg, http })
    }

    pub fn from_env() -> Result<Self> {
        Self::new(SidecarConfig::from_env())
    }

    pub fn health(&self) -> SidecarHealth {
        if !self.cfg.enabled {
            return SidecarHealth {
                enabled: false,
                ok: false,
                url: self.cfg.url.clone(),
                model: self.cfg.model.clone(),
                kind: self.cfg.kind.clone(),
                message: "disabled by NEURA_SIDECAR_ENABLED".to_string(),
            };
        }
        let result = match self.cfg.kind {
            SidecarKind::Ollama => self.http.get(format!("{}/api/tags", self.cfg.url)).send(),
            SidecarKind::OpenAiCompatible => self
                .http
                .get(format!("{}/models", self.cfg.url.trim_end_matches('/')))
                .send(),
        };
        match result {
            Ok(resp) if resp.status().is_success() => SidecarHealth {
                enabled: true,
                ok: true,
                url: self.cfg.url.clone(),
                model: self.cfg.model.clone(),
                kind: self.cfg.kind.clone(),
                message: "sidecar reachable".to_string(),
            },
            Ok(resp) => SidecarHealth {
                enabled: true,
                ok: false,
                url: self.cfg.url.clone(),
                model: self.cfg.model.clone(),
                kind: self.cfg.kind.clone(),
                message: format!("health endpoint returned {}", resp.status()),
            },
            Err(err) => SidecarHealth {
                enabled: true,
                ok: false,
                url: self.cfg.url.clone(),
                model: self.cfg.model.clone(),
                kind: self.cfg.kind.clone(),
                message: err.to_string(),
            },
        }
    }

    pub fn summarize_memory(&self, text: &str) -> Result<String> {
        if !self.cfg.enabled {
            return Err(anyhow!("sidecar disabled"));
        }
        let prompt = format!(
            "Summarize this Neura memory/context note in one concise technical sentence. Preserve paths, commands, errors, and decisions.\n\n{}",
            text
        );
        match self.cfg.kind {
            SidecarKind::Ollama => self.generate_ollama(&prompt),
            SidecarKind::OpenAiCompatible => self.generate_openai_compatible(&prompt),
        }
    }

    fn generate_ollama(&self, prompt: &str) -> Result<String> {
        #[derive(Serialize)]
        struct Req<'a> {
            model: &'a str,
            prompt: &'a str,
            stream: bool,
        }
        #[derive(Deserialize)]
        struct Resp {
            response: Option<String>,
        }
        let resp: Resp = self
            .http
            .post(format!("{}/api/generate", self.cfg.url))
            .json(&Req {
                model: &self.cfg.model,
                prompt,
                stream: false,
            })
            .send()
            .context("call ollama sidecar")?
            .error_for_status()
            .context("ollama sidecar status")?
            .json()
            .context("decode ollama response")?;
        resp.response
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("empty sidecar response"))
    }

    fn generate_openai_compatible(&self, prompt: &str) -> Result<String> {
        #[derive(Serialize)]
        struct Msg<'a> {
            role: &'a str,
            content: &'a str,
        }
        #[derive(Serialize)]
        struct Req<'a> {
            model: &'a str,
            messages: Vec<Msg<'a>>,
            temperature: f32,
            stream: bool,
        }
        #[derive(Deserialize)]
        struct Resp {
            choices: Vec<Choice>,
        }
        #[derive(Deserialize)]
        struct Choice {
            message: Message,
        }
        #[derive(Deserialize)]
        struct Message {
            content: String,
        }
        let resp: Resp = self
            .http
            .post(format!(
                "{}/chat/completions",
                self.cfg.url.trim_end_matches('/')
            ))
            .json(&Req {
                model: &self.cfg.model,
                messages: vec![Msg {
                    role: "user",
                    content: prompt,
                }],
                temperature: 0.0,
                stream: false,
            })
            .send()
            .context("call openai-compatible sidecar")?
            .error_for_status()
            .context("openai-compatible sidecar status")?
            .json()
            .context("decode openai-compatible response")?;
        resp.choices
            .into_iter()
            .next()
            .map(|c| c.message.content.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("empty sidecar response"))
    }
}

pub fn try_summarize_memory(text: &str) -> Option<String> {
    let client = SidecarClient::from_env().ok()?;
    if !client.health().ok {
        return None;
    }
    client.summarize_memory(text).ok()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedMemory {
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub trust: String,
    #[serde(default)]
    pub topics: Vec<String>,
    #[serde(default)]
    pub confidence: f32,
    #[serde(default)]
    pub importance: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryExtractionResult {
    pub extracted: Vec<ExtractedMemory>,
    pub sidecar_used: bool,
    pub message: String,
}

impl MemoryExtractionResult {
    pub fn is_empty(&self) -> bool {
        self.extracted.is_empty()
    }
}

impl IntoIterator for MemoryExtractionResult {
    type Item = ExtractedMemory;
    type IntoIter = std::vec::IntoIter<ExtractedMemory>;
    fn into_iter(self) -> Self::IntoIter {
        self.extracted.into_iter()
    }
}

impl<'a> IntoIterator for &'a MemoryExtractionResult {
    type Item = &'a ExtractedMemory;
    type IntoIter = std::slice::Iter<'a, ExtractedMemory>;
    fn into_iter(self) -> Self::IntoIter {
        self.extracted.iter()
    }
}

/// Compatibility facade used by Neura memory/context internals.
#[derive(Clone)]
pub struct Sidecar {
    cfg: SidecarConfig,
}

impl Sidecar {
    pub fn new() -> Self {
        Self {
            cfg: SidecarConfig::from_env(),
        }
    }

    pub fn health() -> SidecarHealth {
        SidecarClient::from_env()
            .map(|c| c.health())
            .unwrap_or_else(|err| {
                let cfg = SidecarConfig::from_env();
                SidecarHealth {
                    enabled: cfg.enabled,
                    ok: false,
                    url: cfg.url,
                    model: cfg.model,
                    kind: cfg.kind,
                    message: err.to_string(),
                }
            })
    }

    pub async fn extract_memories_with_existing(
        &self,
        transcript: &str,
        existing: &[String],
    ) -> Result<Vec<ExtractedMemory>> {
        let cfg = self.cfg.clone();
        let transcript = transcript
            .chars()
            .take(cfg.max_transcript_chars)
            .collect::<String>();
        let existing = existing.iter().take(40).cloned().collect::<Vec<_>>();
        tokio::task::spawn_blocking(move || {
            let client = SidecarClient::new(cfg)?;
            if !client.health().ok {
                return Ok(Vec::new());
            }
            extract_memories_blocking(&client, &transcript, &existing).map(|r| r.extracted)
        })
        .await
        .unwrap_or_else(|err| Err(anyhow!(err.to_string())))
    }

    pub async fn extract_memories(&self, transcript: &str) -> Result<MemoryExtractionResult> {
        let extracted = self.extract_memories_with_existing(transcript, &[]).await?;
        Ok(MemoryExtractionResult {
            extracted,
            sidecar_used: true,
            message: "sidecar extraction completed".into(),
        })
    }

    pub async fn check_relevance(&self, memory: &str, context: &str) -> Result<(bool, String)> {
        let cfg = self.cfg.clone();
        let memory = memory.to_string();
        let context = context.to_string();
        tokio::task::spawn_blocking(move || {
            let client = SidecarClient::new(cfg)?;
            if !client.health().ok { return Ok((true, "sidecar unavailable; kept by fallback".to_string())); }
            let prompt = format!("Is this Neura memory relevant to this current context? Reply only yes or no.\nMemory: {memory}\nContext: {context}");
            let answer = match client.cfg.kind {
                SidecarKind::Ollama => client.generate_ollama(&prompt)?,
                SidecarKind::OpenAiCompatible => client.generate_openai_compatible(&prompt)?,
            };
            Ok((answer.to_lowercase().contains("yes"), answer))
        }).await.unwrap_or_else(|err| Err(anyhow!(err.to_string())))
    }

    pub async fn check_contradiction(
        &self,
        existing: &str,
        candidate: &str,
    ) -> Result<(bool, String)> {
        let cfg = self.cfg.clone();
        let existing = existing.to_string();
        let candidate = candidate.to_string();
        tokio::task::spawn_blocking(move || {
            let client = SidecarClient::new(cfg)?;
            if !client.health().ok { return Ok((false, "sidecar unavailable; no contradiction assumed".to_string())); }
            let prompt = format!("Do these two Neura memories contradict each other? Reply yes or no, then a short reason.\nExisting: {existing}\nCandidate: {candidate}");
            let answer = match client.cfg.kind {
                SidecarKind::Ollama => client.generate_ollama(&prompt)?,
                SidecarKind::OpenAiCompatible => client.generate_openai_compatible(&prompt)?,
            };
            Ok((answer.to_lowercase().contains("yes"), answer))
        }).await.unwrap_or_else(|err| Err(anyhow!(err.to_string())))
    }

    pub async fn complete(&self, system: &str, prompt: &str) -> Result<String> {
        let cfg = self.cfg.clone();
        let prompt = format!("{system}\n\n{prompt}");
        tokio::task::spawn_blocking(move || {
            let client = SidecarClient::new(cfg)?;
            match client.cfg.kind {
                SidecarKind::Ollama => client.generate_ollama(&prompt),
                SidecarKind::OpenAiCompatible => client.generate_openai_compatible(&prompt),
            }
        })
        .await
        .unwrap_or_else(|err| Err(anyhow!(err.to_string())))
    }

    pub fn backend_name(&self) -> &str {
        match self.cfg.kind {
            SidecarKind::Ollama => "ollama",
            SidecarKind::OpenAiCompatible => "openai-compatible",
        }
    }

    pub fn model_name(&self) -> &str {
        &self.cfg.model
    }
}

impl Default for Sidecar {
    fn default() -> Self {
        Self::new()
    }
}

fn extract_memories_blocking(
    client: &SidecarClient,
    transcript: &str,
    existing: &[String],
) -> Result<MemoryExtractionResult> {
    let existing_text = if existing.is_empty() {
        "<none>".to_string()
    } else {
        existing.join("\n- ")
    };
    let prompt = format!(
        "You are Neura's local memory sidecar. Extract only durable, user-relevant memories from this session. Avoid duplicates of existing memories. Return strict JSON in exactly this shape: {{\"extracted\":[{{\"content\":\"...\",\"kind\":\"fact\",\"confidence\":0.0}}]}}. The \"kind\" field must be a SINGLE word chosen from: preference, project, workflow, fact, decision (do not output the list or multiple words). Keep content concise and technical.\nExisting memories:\n- {existing_text}\n\nTranscript:\n{transcript}"
    );
    let raw = match client.cfg.kind {
        SidecarKind::Ollama => client.generate_ollama(&prompt)?,
        SidecarKind::OpenAiCompatible => client.generate_openai_compatible(&prompt)?,
    };
    let json_slice = raw
        .find('{')
        .and_then(|start| raw.rfind('}').map(|end| &raw[start..=end]))
        .ok_or_else(|| anyhow!("sidecar did not return JSON"))?;
    #[derive(Deserialize)]
    struct Wire {
        extracted: Vec<ExtractedMemory>,
    }
    let mut wire: Wire = serde_json::from_str(json_slice).context("parse sidecar memory JSON")?;
    wire.extracted.retain(|m| {
        let text = if m.content.trim().is_empty() {
            &m.summary
        } else {
            &m.content
        };
        !text.trim().is_empty() && (m.confidence >= 0.45 || m.importance >= 0.45)
    });
    for memory in &mut wire.extracted {
        if memory.summary.trim().is_empty() {
            memory.summary = memory.content.trim().to_string();
        }
        if memory.content.trim().is_empty() {
            memory.content = memory.summary.trim().to_string();
        }
        memory.content = memory.content.trim().to_string();
        memory.summary = memory.summary.trim().to_string();
        if memory.kind.trim().is_empty() {
            memory.kind = "fact".to_string();
        }
        if memory.category.trim().is_empty() {
            memory.category = memory.kind.clone();
        }
        if memory.trust.trim().is_empty() {
            memory.trust = "medium".to_string();
        }
        memory.confidence = memory.confidence.clamp(0.0, 1.0);
        if memory.importance == 0.0 {
            memory.importance = memory.confidence;
        }
        memory.importance = memory.importance.clamp(0.0, 1.0);
    }
    Ok(MemoryExtractionResult {
        extracted: wire.extracted,
        sidecar_used: true,
        message: "sidecar extraction completed".into(),
    })
}
