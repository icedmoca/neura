use serde::{Deserialize, Serialize};
use std::env;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum LocalModelProviderKind {
    LmStudio,
    Ollama,
    OpenAiCompatible,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum LocalRuntimeOsMode {
    NativeLinux,
    Wsl,
    Windows,
    MacOs,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocalModelConfig {
    pub provider: LocalModelProviderKind,
    pub base_url: String,
    pub chat_path: String,
    pub models_path: String,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub timeout_ms: u64,
    pub prefer_local: bool,
    pub allow_remote_fallback: bool,
}

impl Default for LocalModelConfig {
    fn default() -> Self {
        Self {
            provider: LocalModelProviderKind::LmStudio,
            base_url: env::var("KCODE_LM_STUDIO_BASE_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:1234/v1".into()),
            chat_path: "/chat/completions".into(),
            models_path: "/models".into(),
            api_key: env::var("KCODE_LM_STUDIO_API_KEY").ok(),
            model: env::var("KCODE_LM_STUDIO_MODEL").ok(),
            timeout_ms: env::var("KCODE_LOCAL_MODEL_TIMEOUT_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(750),
            prefer_local: env::var("KCODE_PREFER_LOCAL_MODEL")
                .map(|v| v != "0" && v != "false")
                .unwrap_or(false),
            allow_remote_fallback: env::var("KCODE_ALLOW_REMOTE_FALLBACK")
                .map(|v| v != "0" && v != "false")
                .unwrap_or(true),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocalModelHealth {
    pub reachable: bool,
    pub endpoint: String,
    pub os_mode: LocalRuntimeOsMode,
    pub wsl_hint: Option<String>,
    pub latency_ms: Option<u128>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ModelRouteDecision {
    UseLocal,
    UseRemoteFallback,
    BlockedNoProvider,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocalModelRoute {
    pub decision: ModelRouteDecision,
    pub reason: String,
    pub provider: LocalModelProviderKind,
    pub endpoint: String,
}

pub fn default_lm_studio_config() -> LocalModelConfig {
    LocalModelConfig::default()
}

pub fn detect_os_mode() -> LocalRuntimeOsMode {
    if cfg!(target_os = "windows") {
        return LocalRuntimeOsMode::Windows;
    }
    if cfg!(target_os = "macos") {
        return LocalRuntimeOsMode::MacOs;
    }
    if cfg!(target_os = "linux") {
        let proc_version = std::fs::read_to_string("/proc/version")
            .unwrap_or_default()
            .to_lowercase();
        if proc_version.contains("microsoft") || env::var("WSL_DISTRO_NAME").is_ok() {
            return LocalRuntimeOsMode::Wsl;
        }
        return LocalRuntimeOsMode::NativeLinux;
    }
    LocalRuntimeOsMode::Unknown
}

pub fn wsl_lm_studio_hint(mode: &LocalRuntimeOsMode, base_url: &str) -> Option<String> {
    if *mode != LocalRuntimeOsMode::Wsl {
        return None;
    }
    if base_url.contains("127.0.0.1") || base_url.contains("localhost") {
        Some("WSL detected: if LM Studio runs on Windows and localhost fails, bind LM Studio to 0.0.0.0 or set KCODE_LM_STUDIO_BASE_URL to http://<windows-host-ip>:1234/v1".into())
    } else {
        Some("WSL detected with non-localhost LM Studio URL; ensure Windows firewall allows port 1234".into())
    }
}

pub fn check_local_model_health(config: &LocalModelConfig) -> LocalModelHealth {
    let endpoint = config.base_url.clone();
    let mode = detect_os_mode();
    let hint = wsl_lm_studio_hint(&mode, &endpoint);
    let start = std::time::Instant::now();
    match parse_host_port(&endpoint).and_then(|addr| {
        tcp_probe(&addr, Duration::from_millis(config.timeout_ms)).map_err(|e| e.to_string())
    }) {
        Ok(()) => LocalModelHealth {
            reachable: true,
            endpoint,
            os_mode: mode,
            wsl_hint: hint,
            latency_ms: Some(start.elapsed().as_millis()),
            error: None,
        },
        Err(error) => LocalModelHealth {
            reachable: false,
            endpoint,
            os_mode: mode,
            wsl_hint: hint,
            latency_ms: None,
            error: Some(error),
        },
    }
}

pub fn route_local_model(config: &LocalModelConfig, health: &LocalModelHealth) -> LocalModelRoute {
    let decision = if config.prefer_local && health.reachable {
        ModelRouteDecision::UseLocal
    } else if config.allow_remote_fallback {
        ModelRouteDecision::UseRemoteFallback
    } else {
        ModelRouteDecision::BlockedNoProvider
    };
    let reason = match decision {
        ModelRouteDecision::UseLocal => "local LM Studio endpoint reachable and preferred".into(),
        ModelRouteDecision::UseRemoteFallback => {
            if config.prefer_local {
                "local endpoint unavailable; remote fallback allowed".into()
            } else {
                "local preference disabled; remote route allowed".into()
            }
        }
        ModelRouteDecision::BlockedNoProvider => {
            "local endpoint unavailable and remote fallback disabled".into()
        }
    };
    LocalModelRoute {
        decision,
        reason,
        provider: config.provider.clone(),
        endpoint: config.base_url.clone(),
    }
}

pub fn compact_local_model_status() -> String {
    let config = default_lm_studio_config();
    let health = check_local_model_health(&config);
    let route = route_local_model(&config, &health);
    format!(
        "Local model: provider={:?} reachable={} route={:?} os={:?} endpoint={}",
        config.provider, health.reachable, route.decision, health.os_mode, config.base_url
    )
}

fn parse_host_port(url: &str) -> Result<String, String> {
    let without_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let host_port = without_scheme.split('/').next().unwrap_or(without_scheme);
    if host_port.is_empty() {
        return Err("missing host".into());
    }
    if host_port.contains(':') {
        Ok(host_port.into())
    } else {
        Ok(format!("{host_port}:80"))
    }
}

fn tcp_probe(host_port: &str, timeout: Duration) -> std::io::Result<()> {
    let mut addrs = host_port.to_socket_addrs()?;
    let addr = addrs.next().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::AddrNotAvailable, "no socket address")
    })?;
    TcpStream::connect_timeout(&addr, timeout).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_lm_studio_config_uses_openai_compatible_paths() {
        let cfg = LocalModelConfig::default();
        assert_eq!(cfg.provider, LocalModelProviderKind::LmStudio);
        assert_eq!(cfg.chat_path, "/chat/completions");
        assert_eq!(cfg.models_path, "/models");
    }

    #[test]
    fn route_falls_back_when_local_unreachable_and_allowed() {
        let cfg = LocalModelConfig {
            prefer_local: true,
            allow_remote_fallback: true,
            ..LocalModelConfig::default()
        };
        let health = LocalModelHealth {
            reachable: false,
            endpoint: cfg.base_url.clone(),
            os_mode: LocalRuntimeOsMode::NativeLinux,
            wsl_hint: None,
            latency_ms: None,
            error: Some("offline".into()),
        };
        let route = route_local_model(&cfg, &health);
        assert_eq!(route.decision, ModelRouteDecision::UseRemoteFallback);
    }

    #[test]
    fn route_blocks_when_no_provider_available() {
        let cfg = LocalModelConfig {
            prefer_local: true,
            allow_remote_fallback: false,
            ..LocalModelConfig::default()
        };
        let health = LocalModelHealth {
            reachable: false,
            endpoint: cfg.base_url.clone(),
            os_mode: LocalRuntimeOsMode::NativeLinux,
            wsl_hint: None,
            latency_ms: None,
            error: Some("offline".into()),
        };
        let route = route_local_model(&cfg, &health);
        assert_eq!(route.decision, ModelRouteDecision::BlockedNoProvider);
    }

    #[test]
    fn wsl_hint_mentions_windows_host_for_localhost() {
        let hint =
            wsl_lm_studio_hint(&LocalRuntimeOsMode::Wsl, "http://127.0.0.1:1234/v1").unwrap();
        assert!(hint.contains("Windows"));
    }
}

pub const LOCAL_MODEL_ID: &str = "local-model";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocalProviderInfo {
    pub name: &'static str,
    pub display_name: &'static str,
    pub base_url: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocalModelStatus {
    pub enabled: bool,
    pub config: LocalModelConfig,
    pub health: LocalModelHealth,
    pub route: LocalModelRoute,
}

pub fn providers() -> Vec<LocalProviderInfo> {
    let cfg = default_lm_studio_config();
    vec![LocalProviderInfo {
        name: LOCAL_MODEL_ID,
        display_name: "Local model (LM Studio)",
        base_url: cfg.base_url,
        enabled: is_local_model_enabled(),
    }]
}

pub fn is_local_model_enabled() -> bool {
    env::var("KCODE_PREFER_LOCAL_MODEL")
        .map(|v| v != "0" && v != "false")
        .unwrap_or(false)
}

pub fn is_local_model_requested<T: AsRef<str>>(model: T) -> bool {
    let model = model.as_ref();
    model == LOCAL_MODEL_ID || model.starts_with("local/") || model.starts_with("lmstudio/")
}

pub fn is_local_model<T: AsRef<str>>(model: T) -> bool {
    is_local_model_requested(model)
}

pub fn is_local_mode() -> bool {
    is_local_model_enabled()
}

pub fn set_enabled(enabled: bool) -> bool {
    enabled
}

pub fn local_model_status() -> LocalModelStatus {
    let config = default_lm_studio_config();
    let health = check_local_model_health(&config);
    let route = route_local_model(&config, &health);
    LocalModelStatus {
        enabled: config.prefer_local,
        config,
        health,
        route,
    }
}

pub fn perform_health_check() -> LocalModelHealth {
    check_local_model_health(&default_lm_studio_config())
}

pub fn format_status_table() -> String {
    let status = local_model_status();
    format!(
        "Local model provider: {}\nEndpoint: {}\nEnabled: {}\nReachable: {}\nRoute: {:?}\nOS: {:?}\n{}",
        LOCAL_MODEL_ID,
        status.config.base_url,
        status.enabled,
        status.health.reachable,
        status.route.decision,
        status.health.os_mode,
        status.health.wsl_hint.unwrap_or_default()
    )
}

pub fn model_ids() -> std::vec::IntoIter<&'static str> {
    Vec::<&'static str>::new().into_iter()
}

pub fn active_model_id() -> String {
    default_lm_studio_config()
        .model
        .unwrap_or_else(|| LOCAL_MODEL_ID.to_string())
}

pub fn set_active_model_id(_model: &str) -> bool {
    true
}

pub fn is_local_model_id<T: AsRef<str>>(model: T) -> bool {
    is_local_model_requested(model)
}

pub fn available_for<T: AsRef<str>>(_model: T) -> bool {
    perform_health_check().reachable
}

pub fn availability_detail_for<T: AsRef<str>>(model: T) -> String {
    let health = perform_health_check();
    format!(
        "model={} reachable={} endpoint={}",
        model.as_ref(),
        health.reachable,
        health.endpoint
    )
}

pub fn enrich_enabled() -> bool {
    env::var("KCODE_LOCAL_MODEL_ENRICH")
        .map(|v| v != "0" && v != "false")
        .unwrap_or(false)
}

pub fn set_enrich_enabled(_enabled: bool) -> std::io::Result<()> {
    Ok(())
}

pub fn enrich_status_message() -> String {
    if enrich_enabled() {
        "local model enrichment enabled".into()
    } else {
        "local model enrichment disabled".into()
    }
}

pub fn status_json() -> serde_json::Value {
    serde_json::to_value(local_model_status())
        .unwrap_or_else(|_| serde_json::json!({"error":"status serialization failed"}))
}

pub fn pre_route_async(_messages: &[crate::message::Message]) {}

pub fn record_api_exchange_async(
    _messages: &[crate::message::Message],
    _response: &str,
    _provider: &str,
    _model: &str,
) {
}

pub async fn complete_local(
    _messages: &[crate::message::Message],
    _tools: &[crate::message::ToolDefinition],
    _system: &str,
    _dynamic: &str,
) -> anyhow::Result<crate::provider::EventStream> {
    anyhow::bail!(
        "local model inference is not wired yet; LM Studio compatibility layer currently supports config, health, and routing"
    )
}
