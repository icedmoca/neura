use crate::message::StreamEvent;
use crate::message::{ContentBlock, Message, Role, ToolDefinition};
use crate::provider::EventStream;
use crate::util::truncate_str;
use serde::Serialize;
use serde_json::json;
use std::io::Write;
use std::sync::{
    OnceLock, RwLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

const ENV_ENABLE: &str = "KCODE_LOCAL_MODEL_BRIDGE";
const ENV_ENRICH: &str = "KCODE_LOCAL_MODEL_ENRICH";
const ENV_PREROUTER: &str = "KCODE_LOCAL_PREROUTER";
const ENV_PROMOTER: &str = "KCODE_LOCAL_MEMORY_PROMOTER";
const ENV_OLLAMA_MODEL: &str = "KCODE_LOCAL_OLLAMA_MODEL";
const DEFAULT_OLLAMA_MODEL: &str = "kcode-oss-20b-mxfp4";
pub const LOCAL_MODEL_ID: &str = "kcode-oss-20b-mxfp4";
pub const AUTO_LOCAL_MODEL_ID: &str = "local-auto";
pub const DEEPSEEK_CODER_MODEL_ID: &str = "deepseek-coder-6.7b-instruct.Q4_K_M.gguf";
pub const DOLPHIN_LLAMA3_INSTRUCT_MODEL_ID: &str = "dolphin-llama3-8b-instruct.gguf";
pub const LEGACY_LOCAL_MODEL_ID: &str = "kcode-gpt-oss-20b-local";
pub const LEGACY_JCODE_LOCAL_MODEL_ID: &str = "jcode-gpt-oss-20b";
const LLAMA_COMPLETION_PATH: &str =
    "/home/dad/.kcode/build-src/llama.cpp/build-cuda/bin/llama-completion";
const LOCAL_GGUF_MODEL_PATH: &str = "/home/dad/.kcode/models/gguf/kcode-oss-20b-mxfp4.gguf";
const DEFAULT_LLAMA_GPU_LAYERS: &str = "4";
const DEFAULT_LLAMA_BATCH: &str = "32";
const DEFAULT_LLAMA_UBATCH: &str = "16";

#[derive(Clone, Copy)]
struct LocalModelProfile {
    id: &'static str,
    aliases: &'static [&'static str],
    path: &'static str,
    default_gpu_layers: &'static str,
    default_batch: &'static str,
    default_ubatch: &'static str,
    default_context: usize,
    default_predict: usize,
    transcript_char_limit: usize,
    prompt_style: LocalPromptStyle,
    detail: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LocalPromptStyle {
    Plain,
    DeepSeekCoderInstruct,
    ChatMlInstruct,
}

const LOCAL_MODEL_PROFILES: &[LocalModelProfile] = &[
    LocalModelProfile {
        id: AUTO_LOCAL_MODEL_ID,
        aliases: &["auto", "local", "local:auto", "kcode-local-auto"],
        path: "/home/dad/.kcode/models/gguf/dolphin-llama3-8b-instruct.gguf",
        default_gpu_layers: "28",
        default_batch: "64",
        default_ubatch: "32",
        default_context: 4096,
        default_predict: 256,
        transcript_char_limit: 6_000,
        prompt_style: LocalPromptStyle::ChatMlInstruct,
        detail: "automatic local router: chooses the best installed GGUF profile per request with llama.cpp fallback",
    },
    LocalModelProfile {
        id: DOLPHIN_LLAMA3_INSTRUCT_MODEL_ID,
        aliases: &[
            "dolphin-llama3",
            "dolphin-llama3:8b",
            "dolphin-llama3-8b",
            "dolphin-llama3-instruct",
            "local:dolphin-llama3",
            "instruct",
        ],
        path: "/home/dad/.kcode/models/gguf/dolphin-llama3-8b-instruct.gguf",
        default_gpu_layers: "28",
        default_batch: "64",
        default_ubatch: "32",
        default_context: 4096,
        default_predict: 256,
        transcript_char_limit: 6_000,
        prompt_style: LocalPromptStyle::ChatMlInstruct,
        detail: "local Dolphin Llama 3 8B instruct GGUF imported from Ollama, ChatML tool-use prompt",
    },
    LocalModelProfile {
        id: DEEPSEEK_CODER_MODEL_ID,
        aliases: &[
            "deepseek-coder",
            "local:deepseek-coder",
            "deepseek-coder-6.7b",
        ],
        path: "/home/dad/.kcode/models/gguf/deepseek-coder-6.7b-instruct.Q4_K_M.gguf",
        default_gpu_layers: "32",
        default_batch: "64",
        default_ubatch: "32",
        default_context: 2048,
        default_predict: 128,
        transcript_char_limit: 2_400,
        prompt_style: LocalPromptStyle::DeepSeekCoderInstruct,
        detail: "local DeepSeek Coder 6.7B Q4_K_M GGUF via llama.cpp, latency-tuned for RTX 2060 SUPER",
    },
    LocalModelProfile {
        id: LOCAL_MODEL_ID,
        aliases: &[
            LEGACY_LOCAL_MODEL_ID,
            LEGACY_JCODE_LOCAL_MODEL_ID,
            "kcode-gpt-oss-20b",
            "kcode-oss-20b",
            "kcode-sidecar-20b",
            "oss",
            "local:gpt-oss-20b",
            "local:kcode-oss-20b-mxfp4",
            "gpt-oss-20b",
            "gpt-oss-20b-mxfp4_moe",
            "gpt-oss-20b-mxfp4_moe.gguf",
        ],
        path: LOCAL_GGUF_MODEL_PATH,
        default_gpu_layers: DEFAULT_LLAMA_GPU_LAYERS,
        default_batch: DEFAULT_LLAMA_BATCH,
        default_ubatch: DEFAULT_LLAMA_UBATCH,
        default_context: 2048,
        default_predict: 512,
        transcript_char_limit: 18_000,
        prompt_style: LocalPromptStyle::Plain,
        detail: "local GPT-OSS 20B MXFP4 MoE GGUF via llama.cpp",
    },
];

static ACTIVE_LOCAL_MODEL_ID: OnceLock<RwLock<String>> = OnceLock::new();
static ENRICH_RUNTIME_ENABLED: OnceLock<AtomicBool> = OnceLock::new();

fn kcode_home() -> String {
    std::env::var("KCODE_HOME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| dirs::home_dir().map(|home| home.join(".kcode").display().to_string()))
        .unwrap_or_else(|| "/home/dad/.kcode".to_string())
}

fn portable_path(path: &str) -> String {
    if let Some(suffix) = path.strip_prefix("/home/dad/.kcode") {
        format!("{}{}", kcode_home(), suffix)
    } else {
        path.to_string()
    }
}

fn llama_completion_path_owned() -> String {
    std::env::var("KCODE_LLAMA_COMPLETION_PATH")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| portable_path(LLAMA_COMPLETION_PATH))
}

fn profile_path(profile: LocalModelProfile) -> String {
    let key = format!(
        "KCODE_LOCAL_MODEL_PATH_{}",
        profile
            .id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            })
            .collect::<String>()
    );
    std::env::var(&key)
        .or_else(|_| std::env::var("KCODE_LOCAL_GGUF_MODEL_PATH"))
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| portable_path(profile.path))
}

pub fn llama_completion_path() -> String {
    llama_completion_path_owned()
}

pub fn local_gguf_model_path() -> String {
    profile_path(active_profile())
}

pub fn available() -> bool {
    available_for(active_model_id().as_str())
}

pub fn availability_detail() -> String {
    availability_detail_for(active_model_id().as_str())
}

pub fn model_ids() -> Vec<&'static str> {
    LOCAL_MODEL_PROFILES
        .iter()
        .map(|profile| profile.id)
        .collect()
}

pub fn active_model_id() -> String {
    ACTIVE_LOCAL_MODEL_ID
        .get_or_init(|| RwLock::new(AUTO_LOCAL_MODEL_ID.to_string()))
        .read()
        .map(|guard| guard.clone())
        .unwrap_or_else(|_| AUTO_LOCAL_MODEL_ID.to_string())
}

pub fn set_active_model_id(model: &str) -> bool {
    let Some(profile) = profile_for_model(model) else {
        return false;
    };
    if let Ok(mut active) = ACTIVE_LOCAL_MODEL_ID
        .get_or_init(|| RwLock::new(AUTO_LOCAL_MODEL_ID.to_string()))
        .write()
    {
        *active = profile.id.to_string();
    }
    // Model switches should not leave hidden local enrichment running. The user
    // can explicitly re-enable it for the newly selected model with `/enrich on`.
    set_enrich_enabled(false);
    true
}

pub fn is_local_model_id(model: &str) -> bool {
    profile_for_model(model).is_some()
}

pub fn available_for(model: &str) -> bool {
    let Some(profile) = profile_for_model(model) else {
        return false;
    };
    if profile.id == AUTO_LOCAL_MODEL_ID {
        return enabled()
            && std::path::Path::new(&llama_completion_path_owned()).is_file()
            && LOCAL_MODEL_PROFILES.iter().any(|candidate| {
                candidate.id != AUTO_LOCAL_MODEL_ID
                    && std::path::Path::new(&profile_path(*candidate)).is_file()
            });
    }
    enabled()
        && std::path::Path::new(&llama_completion_path_owned()).is_file()
        && std::path::Path::new(&profile_path(profile)).is_file()
}

pub fn availability_detail_for(model: &str) -> String {
    let Some(profile) = profile_for_model(model) else {
        return format!("unknown local model: {}", model);
    };
    if !enabled() {
        return "disabled by KCODE_LOCAL_MODEL_BRIDGE".to_string();
    }
    let llama_path = llama_completion_path_owned();
    if !std::path::Path::new(&llama_path).is_file() {
        return format!("missing llama.cpp runner: {}", llama_path);
    }
    if profile.id == AUTO_LOCAL_MODEL_ID {
        let available = LOCAL_MODEL_PROFILES
            .iter()
            .filter(|candidate| candidate.id != AUTO_LOCAL_MODEL_ID)
            .filter(|candidate| std::path::Path::new(&profile_path(**candidate)).is_file())
            .map(|candidate| candidate.id)
            .collect::<Vec<_>>();
        if available.is_empty() {
            return "local-auto has no installed GGUF profiles".to_string();
        }
        return format!(
            "{}; installed profiles: {}",
            profile.detail,
            available.join(", ")
        );
    }
    let model_path = profile_path(profile);
    if !std::path::Path::new(&model_path).is_file() {
        return format!("missing GGUF model: {}", model_path);
    }
    profile.detail.to_string()
}

fn active_profile() -> LocalModelProfile {
    profile_for_model(active_model_id().as_str()).unwrap_or(LOCAL_MODEL_PROFILES[0])
}

fn profile_for_model(model: &str) -> Option<LocalModelProfile> {
    LOCAL_MODEL_PROFILES
        .iter()
        .copied()
        .find(|profile| profile.id == model || profile.aliases.iter().any(|alias| *alias == model))
}

fn concrete_profile(id: &str) -> Option<LocalModelProfile> {
    LOCAL_MODEL_PROFILES
        .iter()
        .copied()
        .find(|profile| profile.id == id && profile.id != AUTO_LOCAL_MODEL_ID)
}

fn resolve_profile_for_request(
    active: LocalModelProfile,
    transcript: &str,
    tools: &[ToolDefinition],
) -> LocalModelProfile {
    if active.id != AUTO_LOCAL_MODEL_ID {
        return active;
    }
    let latest = latest_user_from_transcript(transcript).to_lowercase();
    let wants_code = [
        "code",
        "build",
        "cargo",
        "compile",
        "error",
        "stack trace",
        "rust",
        "python",
        "edit",
        "patch",
        "debug",
        "function",
        "test",
        "repo",
        "file",
    ]
    .iter()
    .any(|needle| latest.contains(needle));
    let preferred = if wants_code || !tools.is_empty() && !casual_short_request(&latest) {
        DEEPSEEK_CODER_MODEL_ID
    } else {
        DOLPHIN_LLAMA3_INSTRUCT_MODEL_ID
    };
    concrete_profile(preferred)
        .filter(|profile| std::path::Path::new(&profile_path(*profile)).is_file())
        .or_else(|| concrete_profile(DOLPHIN_LLAMA3_INSTRUCT_MODEL_ID))
        .or_else(|| concrete_profile(DEEPSEEK_CODER_MODEL_ID))
        .unwrap_or(active)
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

fn ollama_model() -> String {
    std::env::var(ENV_OLLAMA_MODEL)
        .or_else(|_| std::env::var("JCODE_LOCAL_OLLAMA_MODEL"))
        .map(|model| match model.as_str() {
            "jcode-gpt-oss-20b" | "kcode-gpt-oss-20b" | "kcode-gpt-oss-20b-local" => {
                LOCAL_MODEL_ID.to_string()
            }
            _ => model,
        })
        .unwrap_or_else(|_| DEFAULT_OLLAMA_MODEL.to_string())
}

pub fn enrich_enabled() -> bool {
    enrich_flag().load(Ordering::Relaxed)
}

pub fn set_enrich_enabled(enabled: bool) {
    enrich_flag().store(enabled, Ordering::Relaxed);
}

pub fn enrich_status_message() -> String {
    if enrich_enabled() {
        "Local model enrichment: **ON**. Hidden llama.cpp enrichment jobs may run after turns. Use `/enrich off` to disable.".to_string()
    } else {
        "Local model enrichment: **OFF**. Use `/enrich on` to enable hidden local enrichment jobs."
            .to_string()
    }
}

fn enrich_flag() -> &'static AtomicBool {
    ENRICH_RUNTIME_ENABLED.get_or_init(|| AtomicBool::new(enrich_env_default()))
}

fn enrich_env_default() -> bool {
    std::env::var(ENV_ENRICH)
        .map(|v| {
            !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
        // Local chat models share the same llama.cpp runner/GPU as this optional
        // enrichment path. Keep deterministic bridge records/promotions enabled,
        // but do not spawn hidden llama-completion jobs by default because they
        // can race the user's selected local model and surface confusing CUDA or
        // context errors. Set KCODE_LOCAL_MODEL_ENRICH=1 to opt back in.
        .unwrap_or(false)
}

#[derive(Debug, Serialize)]
struct LocalBridgeEvent {
    timestamp_ms: u128,
    upstream_provider: String,
    upstream_model: String,
    local_model: String,
    prompt_chars: usize,
    response_chars: usize,
    prompt_summary: String,
    response_summary: String,
}

#[derive(Debug, Serialize)]
struct PreRouteEvent {
    timestamp_ms: u128,
    route: &'static str,
    confidence: f32,
    reason: String,
    prompt_chars: usize,
    latest_user: String,
}

#[derive(Debug, Serialize)]
struct PromotedMemoryEvent {
    timestamp_ms: u128,
    source: &'static str,
    confidence: f32,
    memory_type: &'static str,
    text: String,
}

pub fn record_api_exchange_async(
    messages: &[Message],
    response_text: &str,
    upstream_provider: &str,
    upstream_model: &str,
) {
    if !enabled() || response_text.trim().is_empty() {
        return;
    }
    let prompt_text = pre_route_transcript_text(messages);
    let response = response_text.to_string();
    let provider = upstream_provider.to_string();
    let model = upstream_model.to_string();
    tokio::spawn(async move {
        if let Err(err) = record_api_exchange(&prompt_text, &response, &provider, &model).await {
            crate::logging::warn(&format!("local model bridge record failed: {err}"));
        }
    });
}

pub async fn complete_local(
    messages: &[Message],
    tools: &[ToolDefinition],
    system_static: &str,
    system_dynamic: &str,
) -> anyhow::Result<EventStream> {
    let active_profile = active_profile();
    let transcript = transcript_text(messages, active_profile.transcript_char_limit);
    let relevant_tools = select_relevant_tools(&transcript, tools);
    let profile = resolve_profile_for_request(active_profile, &transcript, &relevant_tools);
    if let Some(action) = explicit_tool_action_from_request(&transcript, &relevant_tools) {
        log_local_tool_trace(&transcript, "<deterministic-preroute>", Some(&action));
        return local_action_stream(action);
    }
    let prompt = format_prompt_for_profile(
        profile,
        &transcript,
        &relevant_tools,
        system_static,
        system_dynamic,
    );
    let context = if relevant_tools.is_empty() {
        profile.default_context
    } else {
        profile.default_context.max(4096)
    };
    let predict = if relevant_tools.is_empty() {
        profile.default_predict
    } else {
        profile.default_predict.max(384)
    };
    let output = run_llama_completion(&prompt, predict, context, profile).await?;
    let cleaned = normalize_local_response(
        &latest_user_from_transcript(&transcript).to_lowercase(),
        &clean_local_model_output(&output),
    );
    let action = parse_local_action(&cleaned, &relevant_tools)
        .or_else(|| repair_local_action_from_text(&transcript, &cleaned, &relevant_tools));
    log_local_tool_trace(&transcript, &cleaned, action.as_ref());
    if let Some(LocalAction::Tool { name, input }) = action {
        return local_action_stream(LocalAction::Tool { name, input });
    }
    let text = match action {
        Some(LocalAction::Final { content }) if !content.trim().is_empty() => content,
        _ => cleaned,
    };
    let events = vec![
        Ok(StreamEvent::ConnectionType {
            connection: "local-llama.cpp".to_string(),
        }),
        Ok(StreamEvent::TextDelta(text)),
        Ok(StreamEvent::MessageEnd {
            stop_reason: Some("end_turn".to_string()),
        }),
    ];
    Ok(Box::pin(futures::stream::iter(events)))
}

fn local_action_stream(action: LocalAction) -> anyhow::Result<EventStream> {
    if let LocalAction::Tool { name, input } = action {
        let id = format!("local_tool_{}", now_ms());
        let input_text = serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());
        let events = vec![
            Ok(StreamEvent::ConnectionType {
                connection: "local-llama.cpp".to_string(),
            }),
            Ok(StreamEvent::ToolUseStart { id, name }),
            Ok(StreamEvent::ToolInputDelta(input_text)),
            Ok(StreamEvent::ToolUseEnd),
            Ok(StreamEvent::MessageEnd {
                stop_reason: Some("tool_use".to_string()),
            }),
        ];
        return Ok(Box::pin(futures::stream::iter(events)));
    }
    unreachable!("only tool actions are streamed through local_action_stream")
}

fn explicit_tool_action_from_request(
    transcript: &str,
    tools: &[ToolDefinition],
) -> Option<LocalAction> {
    let latest = latest_user_from_transcript(transcript);
    let lower = latest.to_lowercase();
    let explicit = lower.contains("use the ")
        || lower.contains("call the ")
        || lower.contains("invoke the ")
        || lower.contains("run the ");
    if !explicit {
        return None;
    }
    for tool in tools {
        if lower.contains(&format!("{} tool", tool.name))
            || lower.contains(&format!("tool {}", tool.name))
            || lower.contains(&format!("use {}", tool.name))
        {
            let input = if tool.name == "todo" {
                todo_input_from_latest_user(&latest)
            } else {
                serde_json::Value::Object(Default::default())
            };
            return Some(LocalAction::Tool {
                name: tool.name.clone(),
                input,
            });
        }
    }
    None
}

fn todo_input_from_latest_user(latest: &str) -> serde_json::Value {
    let content = latest
        .split_once("one item:")
        .map(|(_, rest)| rest)
        .or_else(|| latest.split_once("item:").map(|(_, rest)| rest))
        .unwrap_or("local model requested todo")
        .split('.')
        .next()
        .unwrap_or("local model requested todo")
        .trim()
        .trim_matches('"');
    json!({"todos":[{"id":"1","content":content,"status":"pending","priority":"high"}]})
}

#[derive(Debug, Clone, PartialEq)]
enum LocalAction {
    Final {
        content: String,
    },
    Tool {
        name: String,
        input: serde_json::Value,
    },
}

fn format_prompt_for_profile(
    profile: LocalModelProfile,
    transcript: &str,
    tools: &[ToolDefinition],
    system_static: &str,
    system_dynamic: &str,
) -> String {
    let tool_instructions = local_tool_instructions(tools, transcript);
    let system = [system_static.trim(), system_dynamic.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let system = match profile.prompt_style {
        LocalPromptStyle::DeepSeekCoderInstruct | LocalPromptStyle::ChatMlInstruct => {
            truncate_str(&system, 4_000).to_string()
        }
        LocalPromptStyle::Plain => system,
    };
    match profile.prompt_style {
        LocalPromptStyle::Plain => format!(
            "You are Kcode running fully locally. Follow the system instructions.\n\n{system}\n\n{tool_instructions}\n\n{transcript}"
        ),
        LocalPromptStyle::DeepSeekCoderInstruct => format!(
            "You are Kcode, an AI programming assistant running fully locally. \
	Follow the system instructions and use tools when needed.\n\n### Instruction:\n{system}\n\n{tool_instructions}\n\nConversation:\n{transcript}\n\n### Response:\n"
        ),
        LocalPromptStyle::ChatMlInstruct => format!(
            "<|im_start|>system\nYou are Kcode, a local AI programming assistant. Follow the system instructions and use tools when needed.\n\n{system}\n\n{tool_instructions}<|im_end|>\n<|im_start|>user\n{}<|im_end|>\n<|im_start|>assistant\n",
            chatml_safe_transcript(transcript)
        ),
    }
}

fn chatml_safe_transcript(transcript: &str) -> String {
    transcript
        .replace("<user>", "User:")
        .replace("</user>", "")
        .replace("<assistant>", "Assistant:")
        .replace("</assistant>", "")
        .replace("<tool_result>", "Tool result:")
        .replace("</tool_result>", "")
        .replace("<tool_use", "Tool use")
        .replace("</tool_use>", "")
}

fn local_tool_instructions(tools: &[ToolDefinition], transcript: &str) -> String {
    if tools.is_empty() {
        return "No tools are available. Answer directly.".to_string();
    }
    let mut out = String::from(
        "You have access to Kcode tools. Decide using exactly one compact JSON action and no other text. Never use XML tags or <tool_use>.\n\
Return either:\n\
{\"type\":\"final\",\"content\":\"your answer\"}\n\
OR:\n\
{\"type\":\"tool\",\"name\":\"tool_name\",\"input\":{...}}\n\
Only use tool names listed below. After a tool result appears in the conversation, continue with another JSON action or final answer.\n\
If the user asks to update todos, inspect files, search code, run commands, use browser, or otherwise interact with the computer, choose a tool.\n",
    );
    out.push_str(
        "If the user explicitly says to use/call/run/invoke a tool, you MUST return a tool action. Do not claim you used a tool in final text.\n",
    );
    if let Some(hint) = preroute_hint(transcript, tools) {
        out.push_str(&format!(
            "Likely best tool for the latest request: {hint}. Prefer it unless clearly wrong.\n"
        ));
    }
    out.push_str("\nExamples:\n");
    out.push_str(r#"User: list files in src
{"type":"tool","name":"ls","input":{"path":"src","ignore":null}}
User: search for complete_local in the repo
{"type":"tool","name":"agentgrep","input":{"mode":"grep","path":".","query":"complete_local","max_regions":20}}
User: mark these todos complete
{"type":"tool","name":"todo","input":{"todos":[{"id":"1","content":"verify","status":"pending","priority":"high"}]}}
User: thanks
{"type":"final","content":"You're welcome."}
"#);
    out.push_str("\nAvailable tools:\n");
    for tool in tools.iter().take(8) {
        let description_text = tool.description.replace('\n', " ");
        let schema_text = tool.input_schema.to_string();
        let description = truncate_str(&description_text, 120);
        let schema = truncate_str(&schema_text, 350);
        out.push_str(&format!(
            "- {}: {} input_schema={}\n",
            tool.name, description, schema
        ));
    }
    out
}

fn select_relevant_tools<'a>(transcript: &str, tools: &'a [ToolDefinition]) -> Vec<ToolDefinition> {
    if tools.is_empty() {
        return Vec::new();
    }
    let lower = latest_user_from_transcript(transcript).to_lowercase();
    let mut preferred = Vec::new();
    let mut matched_rule = false;
    let rules: &[(&[&str], &[&str])] = &[
        (&["todo", "todos", "incomplete"], &["todo"]),
        (
            &["search", "find", "grep", "where", "locate"],
            &["agentgrep", "grep", "glob"],
        ),
        (
            &["file", "read", "open", "show", "cat"],
            &["read", "open", "ls"],
        ),
        (&["list", "directory", "folder", "ls"], &["ls", "glob"]),
        (
            &["edit", "patch", "change", "modify", "fix", "implement"],
            &[
                "edit",
                "multiedit",
                "apply_patch",
                "bash",
                "read",
                "agentgrep",
            ],
        ),
        (
            &["run", "test", "build", "cargo", "npm", "python", "command"],
            &["bash"],
        ),
        (
            &["browser", "web", "website", "click", "page"],
            &["browser", "webfetch", "websearch"],
        ),
        (&["email", "gmail"], &["gmail"]),
        (&["remember", "memory"], &["memory"]),
    ];
    for (needles, names) in rules {
        if needles.iter().any(|needle| lower.contains(needle)) {
            matched_rule = true;
            preferred.extend_from_slice(names);
        }
    }
    if !matched_rule && direct_answer_request(&lower) {
        return Vec::new();
    }
    let always = [
        "todo",
        "bash",
        "read",
        "edit",
        "multiedit",
        "apply_patch",
        "agentgrep",
        "ls",
    ];
    preferred.extend_from_slice(&always);
    let mut selected = Vec::new();
    for name in preferred {
        if selected
            .iter()
            .any(|tool: &ToolDefinition| tool.name == *name)
        {
            continue;
        }
        if let Some(tool) = tools.iter().find(|tool| tool.name == *name) {
            selected.push(tool.clone());
        }
    }
    let max_tools = if matched_rule { 12 } else { 8 };
    for tool in tools {
        if selected.len() >= max_tools {
            break;
        }
        if !selected.iter().any(|existing| existing.name == tool.name) {
            selected.push(tool.clone());
        }
    }
    selected
}

fn direct_answer_request(lower_latest_user: &str) -> bool {
    let trimmed = lower_latest_user.trim();
    trimmed.len() <= 80
        && !trimmed.contains("file")
        && !trimmed.contains("todo")
        && !trimmed.contains("run")
        && !trimmed.contains("search")
        && !trimmed.contains("build")
        && !trimmed.contains("edit")
        && (trimmed.starts_with("reply")
            || trimmed.starts_with("say")
            || casual_short_request(trimmed))
}

fn casual_short_request(lower_latest_user: &str) -> bool {
    let trimmed = lower_latest_user.trim();
    trimmed.len() <= 80
        && matches!(
            trimmed,
            "hi" | "hey" | "hello" | "ok" | "thanks" | "thank you" | "meow" | "woof" | "lol"
        )
}

fn preroute_hint(transcript: &str, tools: &[ToolDefinition]) -> Option<String> {
    let lower = latest_user_from_transcript(transcript).to_lowercase();
    let candidates: &[(&[&str], &str)] = &[
        (&["todo", "todos", "incomplete"], "todo"),
        (&["search", "find", "grep"], "agentgrep"),
        (&["list", "directory", "folder", "ls"], "ls"),
        (&["read", "show", "cat"], "read"),
        (
            &["edit", "patch", "modify", "fix", "implement"],
            "edit or apply_patch",
        ),
        (&["run", "test", "build", "cargo", "npm"], "bash"),
        (&["browser", "click", "website"], "browser"),
    ];
    for (needles, tool_name) in candidates {
        if needles.iter().any(|needle| lower.contains(needle))
            && tools
                .iter()
                .any(|tool| tool.name == *tool_name || tool_name.contains(&tool.name))
        {
            return Some((*tool_name).to_string());
        }
    }
    None
}

fn parse_local_action(text: &str, tools: &[ToolDefinition]) -> Option<LocalAction> {
    if let Some(action) = parse_legacy_tool_use(text, tools) {
        return Some(action);
    }
    let json_text = extract_action_json(text)?;
    let value: serde_json::Value = serde_json::from_str(json_text.trim()).ok()?;
    let kind = value
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or_else(|| {
            if value.get("name").is_some() {
                "tool"
            } else {
                "final"
            }
        });
    match kind {
        "tool" => {
            if tools.is_empty() {
                return None;
            }
            let name = value.get("name")?.as_str()?.trim().to_string();
            if !tools.iter().any(|tool| tool.name == name) {
                return None;
            }
            let input = value
                .get("input")
                .cloned()
                .unwrap_or_else(|| serde_json::Value::Object(Default::default()));
            Some(LocalAction::Tool { name, input })
        }
        "final" => Some(LocalAction::Final {
            content: value
                .get("content")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string(),
        }),
        _ => None,
    }
}

fn parse_legacy_tool_use(text: &str, tools: &[ToolDefinition]) -> Option<LocalAction> {
    let start = text.find("<tool_use")?;
    let after_start = &text[start..];
    let name_attr = "name=\"";
    let name_start = after_start.find(name_attr)? + name_attr.len();
    let name_end = after_start[name_start..].find('"')? + name_start;
    let name = after_start[name_start..name_end].trim().to_string();
    if !tools.iter().any(|tool| tool.name == name) {
        return None;
    }
    let body_start = after_start.find('>')? + 1;
    let body = &after_start[body_start..];
    let json_start = body.find('{')?;
    let body_after_json = &body[json_start..];
    let json_end = body_after_json
        .rfind('}')
        .map(|idx| idx + 1)
        .unwrap_or(body_after_json.len());
    let input = serde_json::from_str(&body_after_json[..json_end])
        .unwrap_or_else(|_| serde_json::Value::Object(Default::default()));
    Some(LocalAction::Tool { name, input })
}

#[cfg(test)]
fn parse_local_tool_call(
    text: &str,
    tools: &[ToolDefinition],
) -> Option<(String, serde_json::Value)> {
    match parse_local_action(text, tools)? {
        LocalAction::Tool { name, input } => Some((name, input)),
        LocalAction::Final { .. } => None,
    }
}

fn repair_local_action_from_text(
    transcript: &str,
    text: &str,
    tools: &[ToolDefinition],
) -> Option<LocalAction> {
    if tools.is_empty() {
        return None;
    }
    let lower = text.to_lowercase();
    for tool in tools {
        if lower.contains(&format!("{}(", tool.name))
            || lower.contains(&format!("tool: {}", tool.name))
            || lower.contains(&format!("name: {}", tool.name))
        {
            return Some(LocalAction::Tool {
                name: tool.name.clone(),
                input: serde_json::Value::Object(Default::default()),
            });
        }
    }
    if latest_user_from_transcript(transcript)
        .to_lowercase()
        .contains("todo")
    {
        if let Some(tool) = tools.iter().find(|tool| tool.name == "todo") {
            return Some(LocalAction::Tool {
                name: tool.name.clone(),
                input: json!({"todos": null}),
            });
        }
    }
    None
}

fn extract_action_json(text: &str) -> Option<&str> {
    let start_tag = "<tool_call>";
    let end_tag = "</tool_call>";
    if let Some(start) = text.find(start_tag) {
        let after_start = start + start_tag.len();
        let end = text[after_start..].find(end_tag)? + after_start;
        return Some(&text[after_start..end]);
    }
    let trimmed = text.trim();
    if trimmed.starts_with('{') {
        return Some(trimmed);
    }
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if end > start {
        Some(&trimmed[start..=end])
    } else {
        None
    }
}

fn latest_user_from_transcript(transcript: &str) -> String {
    if let Some(start) = transcript.rfind("<user>") {
        let after = start + "<user>".len();
        if let Some(end) = transcript[after..].find("</user>") {
            return transcript[after..after + end].trim().to_string();
        }
    }
    transcript
        .lines()
        .rev()
        .take(20)
        .collect::<Vec<_>>()
        .join("\n")
}

fn log_local_tool_trace(transcript: &str, raw_output: &str, action: Option<&LocalAction>) {
    let Ok(home) = std::env::var("HOME") else {
        return;
    };
    let dir = std::path::Path::new(&home).join(".kcode/local-model-bridge");
    let _ = std::fs::create_dir_all(&dir);
    let record = json!({
        "ts": now_ms(),
        "latest_user": truncate_str(&latest_user_from_transcript(transcript), 1000),
        "raw_output": truncate_str(raw_output, 2000),
        "action": match action {
            Some(LocalAction::Tool { name, input }) => json!({"type":"tool","name":name,"input":input}),
            Some(LocalAction::Final { content }) => json!({"type":"final","content":truncate_str(content, 1000)}),
            None => json!(null),
        },
    });
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("tool-traces.jsonl"))
    {
        let _ = writeln!(file, "{}", record);
    }
}

pub fn status_json() -> serde_json::Value {
    let bridge_dir = match std::env::var("HOME") {
        Ok(home) => std::path::Path::new(&home).join(".kcode/local-model-bridge"),
        Err(_) => return json!({ "enabled": enabled(), "available": false }),
    };
    let api = file_stats(&bridge_dir.join("api-exchanges.jsonl"));
    let distilled = file_stats(&bridge_dir.join("distilled-memory.jsonl"));
    let promoted = file_stats(&bridge_dir.join("promoted-memory.jsonl"));
    let preroute = file_stats(&bridge_dir.join("pre-route.jsonl"));
    json!({
        "enabled": enabled(),
        "enrich_enabled": enrich_enabled(),
        "prerouter_enabled": prerouter_enabled(),
        "promoter_enabled": promoter_enabled(),
        "local_model": ollama_model(),
        "bridge_dir": bridge_dir.display().to_string(),
        "api_exchanges": api.lines,
        "api_bytes": api.bytes,
        "api_mtime_ms": api.mtime_ms,
        "distilled_records": distilled.lines,
        "distilled_bytes": distilled.bytes,
        "distilled_mtime_ms": distilled.mtime_ms,
        "promoted_records": promoted.lines,
        "promoted_bytes": promoted.bytes,
        "promoted_mtime_ms": promoted.mtime_ms,
        "pre_route_events": preroute.lines,
        "pre_route_bytes": preroute.bytes,
        "pre_route_mtime_ms": preroute.mtime_ms,
    })
}

fn prerouter_enabled() -> bool {
    env_bool(ENV_PREROUTER, true)
}

fn promoter_enabled() -> bool {
    env_bool(ENV_PROMOTER, true)
}

fn env_bool(name: &str, default: bool) -> bool {
    std::env::var(name)
        .map(|v| {
            !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
        .unwrap_or(default)
}

pub fn pre_route_async(messages: &[Message]) {
    if !enabled() || !prerouter_enabled() {
        return;
    }
    let latest_user = latest_user_text(messages);
    let prompt_chars: usize = messages.iter().map(render_message).map(|s| s.len()).sum();
    tokio::spawn(async move {
        if let Err(err) = record_pre_route(&latest_user, prompt_chars) {
            crate::logging::warn(&format!("local pre-router record failed: {err}"));
        }
    });
}

fn record_pre_route(latest_user: &str, prompt_chars: usize) -> anyhow::Result<()> {
    let lower = latest_user.to_ascii_lowercase();
    let (route, confidence, reason) = if lower.trim().len() <= 12
        || matches!(
            lower.trim(),
            "hi" | "hello" | "hey" | "meow" | "ok" | "thanks"
        ) {
        ("local-first", 0.86, "short/trivial user turn")
    } else if lower.contains("modify")
        || lower.contains("code")
        || lower.contains("fix")
        || lower.contains("build")
    {
        (
            "remote-with-local-critic",
            0.78,
            "coding/self-modification likely benefits from GPT-5.5 plus local critique",
        )
    } else if lower.contains("remember") || lower.contains("memory") || lower.contains("preference")
    {
        ("memory-first", 0.82, "memory operation detected")
    } else {
        ("remote-primary", 0.62, "general request")
    };
    append_named_jsonl(
        "pre-route.jsonl",
        PreRouteEvent {
            timestamp_ms: now_ms(),
            route,
            confidence,
            reason: reason.to_string(),
            prompt_chars,
            latest_user: truncate_str(latest_user, 500).to_string(),
        },
    )
}

#[derive(Default)]
struct FileStats {
    lines: usize,
    bytes: u64,
    mtime_ms: u128,
}

fn file_stats(path: &std::path::Path) -> FileStats {
    let Ok(text) = std::fs::read_to_string(path) else {
        return FileStats::default();
    };
    let bytes = std::fs::metadata(path)
        .map(|m| m.len())
        .unwrap_or(text.len() as u64);
    let mtime_ms = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or_default();
    FileStats {
        lines: text.lines().count(),
        bytes,
        mtime_ms,
    }
}

async fn record_api_exchange(
    prompt_text: &str,
    response_text: &str,
    upstream_provider: &str,
    upstream_model: &str,
) -> anyhow::Result<()> {
    let prompt_summary = summarize(prompt_text);
    let response_summary = summarize(response_text);
    append_jsonl(LocalBridgeEvent {
        timestamp_ms: now_ms(),
        upstream_provider: upstream_provider.to_string(),
        upstream_model: upstream_model.to_string(),
        local_model: ollama_model(),
        prompt_chars: prompt_text.len(),
        response_chars: response_text.len(),
        prompt_summary: prompt_summary.clone(),
        response_summary: response_summary.clone(),
    })?;

    append_distillation(
        upstream_provider,
        upstream_model,
        &serde_json::json!({
            "source": "deterministic-local-bridge",
            "kind": "api_exchange_summary",
            "prompt_summary": prompt_summary,
            "response_summary": response_summary,
            "prompt_chars": prompt_text.len(),
            "response_chars": response_text.len(),
            "note": "Immediate fallback memory-graph candidate written before optional local-model enrichment."
        })
        .to_string(),
    )?;
    promote_memory_candidates(prompt_text, response_text)?;

    // Best-effort distillation through the local model. This does not block the
    // main provider path and is intentionally non-fatal: it lets Kcode begin
    // learning from upstream traffic as soon as a local Ollama model is ready.
    if enrich_enabled() {
        let distill_prompt = format!(
            "You are Kcode's local model bridge. Distill this upstream API exchange into compact memory graph candidates. Return concise JSONL-style facts, preferences, entities, corrections, and reusable reasoning patterns. Do not include secrets.\n\n<upstream_prompt>\n{}\n</upstream_prompt>\n\n<upstream_response>\n{}\n</upstream_response>",
            truncate_str(prompt_text, 24_000),
            truncate_str(response_text, 16_000)
        );
        if let Ok(distilled) = ollama_generate(&distill_prompt).await {
            let distilled = clean_local_model_output(&distilled);
            append_distillation(upstream_provider, upstream_model, &distilled)?;
        }
    }
    Ok(())
}

fn clean_local_model_output(text: &str) -> String {
    let mut text = text
        .replace("<|channel|>analysis<|message|>", "")
        .replace("<|channel|>final<|message|>", "")
        .replace("<|im_end|>", "")
        .replace("<|end_of_text|>", "")
        .replace("<|eot_id|>", "")
        .replace("[end of text]", "")
        .replace("<assistant>", "")
        .replace("</assistant>", "")
        .replace("> EOF by user", "");
    for prefix in ["Assistant:", "assistant:", "<|im_start|>assistant"] {
        text = text
            .trim_start()
            .strip_prefix(prefix)
            .unwrap_or(&text)
            .to_string();
    }
    text.trim().to_string()
}

fn normalize_local_response(lower_latest_user: &str, cleaned: &str) -> String {
    let lower_response = cleaned.to_lowercase();
    if casual_short_request(lower_latest_user)
        && (cleaned.trim().is_empty()
            || lower_response.contains("don't have the ability to interpret")
            || lower_response.contains("do not have the ability to interpret")
            || lower_response.contains("could you please provide more context")
            || lower_response.contains("as an ai"))
    {
        return match lower_latest_user.trim() {
            "meow" => "meow 😺".to_string(),
            "woof" => "woof 🐶".to_string(),
            "lol" => "lol 😄".to_string(),
            "hi" | "hey" | "hello" => "hey!".to_string(),
            "thanks" | "thank you" => "you're welcome!".to_string(),
            _ => "got it.".to_string(),
        };
    }
    cleaned.trim().to_string()
}

async fn ollama_generate(prompt: &str) -> anyhow::Result<String> {
    let prompt = prompt.to_string();
    run_llama_completion(&prompt, 256, 1024, active_profile()).await
}

async fn run_llama_completion(
    prompt: &str,
    predict: usize,
    context: usize,
    profile: LocalModelProfile,
) -> anyhow::Result<String> {
    let prompt = prompt.to_string();
    tokio::task::spawn_blocking(move || {
        let env_gpu_layers = std::env::var("KCODE_LOCAL_LLAMA_NGL")
            .ok()
            .filter(|value| value.parse::<u32>().is_ok());
        let gpu_layers = env_gpu_layers
            .clone()
            .unwrap_or_else(|| profile.default_gpu_layers.to_string());
        let batch = std::env::var("KCODE_LOCAL_LLAMA_BATCH")
            .ok()
            .filter(|value| value.parse::<u32>().is_ok())
            .unwrap_or_else(|| profile.default_batch.to_string());
        let ubatch = std::env::var("KCODE_LOCAL_LLAMA_UBATCH")
            .ok()
            .filter(|value| value.parse::<u32>().is_ok())
            .unwrap_or_else(|| profile.default_ubatch.to_string());

        let mut ngl_attempts = vec![gpu_layers.clone()];
        if profile.id == DEEPSEEK_CODER_MODEL_ID {
            // CUDA memory on small cards can be fragmented by the desktop or
            // other processes. Always have a path to recover automatically,
            // including CPU-only -ngl 0, instead of requiring the user to tune
            // KCODE_LOCAL_LLAMA_NGL after a failed request.
            let fallback_layers: &[&str] = if env_gpu_layers.is_none() {
                &["24", "16", "8", "4", "2", "0"]
            } else if std::env::var("KCODE_LOCAL_LLAMA_STRICT_NGL").is_ok() {
                &[]
            } else {
                &["0"]
            };
            for fallback in fallback_layers {
                if fallback != &gpu_layers {
                    ngl_attempts.push((*fallback).to_string());
                }
            }
        } else if env_gpu_layers.is_none() && gpu_layers != "0" {
            for fallback in ["24", "16", "8", "4", "2", "0"] {
                if fallback != gpu_layers {
                    ngl_attempts.push(fallback.to_string());
                }
            }
        }

        let mut size_attempts = vec![(batch.clone(), ubatch.clone(), context)];
        if batch != "32" || ubatch != "16" {
            size_attempts.push(("32".to_string(), "16".to_string(), context));
        }

        let mut last_failure: Option<(String, String)> = None;
        for (attempt_batch, attempt_ubatch, attempt_context) in size_attempts {
            for attempt_gpu_layers in &ngl_attempts {
                let llama_path = llama_completion_path_owned();
                let model_path = profile_path(profile);
                let output = std::process::Command::new(&llama_path)
                    .args([
                        "-m",
                        &model_path,
                        "-p",
                        &prompt,
                        "-n",
                        &predict.to_string(),
                        "-c",
                        &attempt_context.to_string(),
                        "-b",
                        &attempt_batch,
                        "-ub",
                        &attempt_ubatch,
                        "-ngl",
                        attempt_gpu_layers,
                        "--temp",
                        "0.2",
                        "--no-display-prompt",
                        "--simple-io",
                        "--no-warmup",
                    ])
                    .output()?;
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                if output.status.success() {
                    return Ok(stdout.trim().to_string());
                }
                if stderr.contains("prompt is too long") {
                    last_failure = Some((attempt_gpu_layers.clone(), stderr));
                    break;
                }
                last_failure = Some((attempt_gpu_layers.clone(), stderr));
            }
        }

        let (failed_gpu_layers, stderr) =
            last_failure.unwrap_or_else(|| (gpu_layers.clone(), String::new()));
        let stderr_tail = stderr
            .lines()
            .rev()
            .take(80)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        anyhow::bail!(
            "llama-completion failed for {} with -ngl {} -b {} -ub {} -c {}. \
             Set KCODE_LOCAL_LLAMA_NGL to a lower value if CUDA memory is fragmented. stderr: {}",
            profile.id,
            failed_gpu_layers,
            batch,
            ubatch,
            context,
            crate::util::truncate_str(&stderr_tail, 2400)
        );
    })
    .await?
}

fn append_jsonl(event: LocalBridgeEvent) -> anyhow::Result<()> {
    append_named_jsonl("api-exchanges.jsonl", event)
}

fn append_named_jsonl<T: Serialize>(name: &str, event: T) -> anyhow::Result<()> {
    let path = local_bridge_path(name)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{}", serde_json::to_string(&event)?)?;
    Ok(())
}

fn append_distillation(provider: &str, model: &str, distilled: &str) -> anyhow::Result<()> {
    if distilled.trim().is_empty() {
        return Ok(());
    }
    let path = local_bridge_path("distilled-memory.jsonl")?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let value = serde_json::json!({
        "timestamp_ms": now_ms(),
        "upstream_provider": provider,
        "upstream_model": model,
        "local_model": ollama_model(),
        "distilled": distilled,
    });
    writeln!(file, "{}", serde_json::to_string(&value)?)?;
    Ok(())
}

fn promote_memory_candidates(prompt_text: &str, response_text: &str) -> anyhow::Result<()> {
    if !promoter_enabled() {
        return Ok(());
    }
    let combined = format!("{}\n{}", prompt_text, response_text);
    let lower = combined.to_ascii_lowercase();
    let mut candidates = Vec::new();
    for marker in [
        "remember",
        "i like",
        "i prefer",
        "my preference",
        "when i say",
        "call me",
    ] {
        if let Some(idx) = lower.find(marker) {
            let start = idx.saturating_sub(80);
            let end = (idx + 500).min(combined.len());
            let text = truncate_str(combined.get(start..end).unwrap_or(&combined), 500).to_string();
            candidates.push(PromotedMemoryEvent {
                timestamp_ms: now_ms(),
                source: "deterministic-promoter",
                confidence: 0.72,
                memory_type: if marker == "when i say" {
                    "preference"
                } else {
                    "candidate"
                },
                text,
            });
            break;
        }
    }
    if lower.contains("kcode") && lower.contains("interlang") {
        candidates.push(PromotedMemoryEvent {
            timestamp_ms: now_ms(),
            source: "deterministic-promoter",
            confidence: 0.68,
            memory_type: "project_fact",
            text: "Conversation involved Kcode interlang/local-model behavior; useful for future self-modification context.".to_string(),
        });
    }
    for candidate in candidates {
        append_named_jsonl("promoted-memory.jsonl", candidate)?;
    }
    Ok(())
}

fn latest_user_text(messages: &[Message]) -> String {
    messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, Role::User))
        .map(render_message)
        .unwrap_or_default()
}

fn local_bridge_path(name: &str) -> anyhow::Result<std::path::PathBuf> {
    let home = std::env::var("HOME")?;
    let dir = std::path::Path::new(&home).join(".kcode/local-model-bridge");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(name))
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

fn pre_route_transcript_text(messages: &[Message]) -> String {
    let budget = adaptive_pre_route_transcript_budget(messages);
    let latest_user_index = messages
        .iter()
        .rposition(|message| message.role == Role::User);
    transcript_text_with_options(
        messages,
        budget,
        TranscriptOptions {
            compress_large_blocks: true,
            latest_user_index,
        },
    )
}

#[derive(Clone, Copy)]
struct TranscriptOptions {
    compress_large_blocks: bool,
    latest_user_index: Option<usize>,
}

impl Default for TranscriptOptions {
    fn default() -> Self {
        Self {
            compress_large_blocks: false,
            latest_user_index: None,
        }
    }
}

fn adaptive_pre_route_transcript_budget(messages: &[Message]) -> usize {
    let latest = latest_user_plain_text(messages).unwrap_or_default();
    let latest_chars = latest.chars().count();
    let lower = latest.to_ascii_lowercase();
    let total_visible: usize = messages.iter().map(message_visible_chars).sum();

    let asks_for_prior_context = contains_any(
        &lower,
        &[
            "continue",
            "keep going",
            "previous",
            "above",
            "earlier",
            "last",
            "that",
            "those",
            "it",
            "same",
            "again",
            "reload",
            "did that",
            "what happened",
            "why did",
            "how many",
        ],
    );
    let repo_or_debug_work = contains_any(
        &lower,
        &[
            "fix",
            "debug",
            "bug",
            "build",
            "test",
            "error",
            "failed",
            "panic",
            "trace",
            "repo",
            "code",
            "commit",
            "push",
            "diff",
            "grep",
            "read",
            "file",
            "src/",
            "docs/",
            ".rs",
            ".py",
            ".md",
            "token",
            "prompt",
            "context",
            "memory",
            "tool",
            "benchmark",
            "optimize",
        ],
    );
    let has_structural_detail = latest.contains('\n')
        || latest.contains("```")
        || latest.contains("/")
        || latest.contains("::")
        || latest.contains("->")
        || latest.contains("http://")
        || latest.contains("https://");
    let simple_latest = latest_chars <= 80
        && !has_structural_detail
        && !repo_or_debug_work
        && !asks_for_prior_context
        && lexical_diversity(&lower) <= 1.0;

    let budget = if simple_latest {
        4_000
    } else if latest_chars <= 160 && !repo_or_debug_work && !has_structural_detail {
        8_000
    } else if repo_or_debug_work || has_structural_detail {
        24_000
    } else if asks_for_prior_context {
        16_000
    } else {
        12_000
    };

    // Scale up when the latest user input itself is large, but never return to
    // the old fixed 48k sidecar transcript unless the user actually supplied a
    // very large, structured prompt.
    let latest_floor = latest.len().saturating_add(2_000);
    budget
        .max(latest_floor)
        .min(total_visible)
        .clamp(2_000, 32_000)
}

fn latest_user_plain_text(messages: &[Message]) -> Option<String> {
    messages.iter().rev().find_map(|message| {
        if message.role != Role::User {
            return None;
        }
        let mut text = String::new();
        for block in &message.content {
            if let ContentBlock::Text { text: chunk, .. } = block {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(chunk);
            }
        }
        if text.trim().is_empty() {
            None
        } else {
            Some(text)
        }
    })
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn lexical_diversity(text: &str) -> f32 {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return 0.0;
    }
    let unique = words.iter().collect::<std::collections::HashSet<_>>().len();
    unique as f32 / words.len() as f32
}

fn message_visible_chars(message: &Message) -> usize {
    message
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text, .. } => text.len(),
            ContentBlock::ToolResult { content, .. } => content.len(),
            ContentBlock::ToolUse { name, input, .. } => name.len() + input.to_string().len(),
            ContentBlock::Reasoning { text } => text.len(),
            ContentBlock::Image { .. } => 32,
            ContentBlock::OpenAICompaction { encrypted_content } => encrypted_content.len(),
        })
        .sum()
}

fn transcript_text(messages: &[Message], max_chars: usize) -> String {
    transcript_text_with_options(messages, max_chars, TranscriptOptions::default())
}

fn transcript_text_with_options(
    messages: &[Message],
    max_chars: usize,
    options: TranscriptOptions,
) -> String {
    let mut out = String::new();
    for (idx, message) in messages.iter().enumerate().rev() {
        let mut rendered = render_message_with_options(message, idx, options);
        if rendered.len() > max_chars {
            rendered = tail_to_char_limit(&rendered, max_chars);
        }
        if out.len() + rendered.len() + 2 > max_chars && !out.is_empty() {
            break;
        }
        out = if out.is_empty() {
            rendered
        } else {
            format!("{}\n\n{}", rendered, out)
        };
    }
    out
}

fn tail_to_char_limit(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let prefix = "[earlier content truncated]\n";
    let body_chars = max_chars.saturating_sub(prefix.len());
    let mut start = text.len().saturating_sub(body_chars);
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    format!("{}{}", prefix, &text[start..])
}

fn render_message(message: &Message) -> String {
    render_message_with_options(message, usize::MAX, TranscriptOptions::default())
}

fn render_message_with_options(
    message: &Message,
    message_index: usize,
    options: TranscriptOptions,
) -> String {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    let preserve_text_exact = options.latest_user_index == Some(message_index);
    let mut parts = Vec::new();
    for block in &message.content {
        match block {
            ContentBlock::Text { text, .. } => parts.push(render_transcript_text_block(
                text,
                options.compress_large_blocks && !preserve_text_exact,
            )),
            ContentBlock::Reasoning { text } => parts.push(format!(
                "<reasoning>{}</reasoning>",
                render_transcript_text_block(text, options.compress_large_blocks)
            )),
            ContentBlock::ToolResult { content, .. } => parts.push(format!(
                "<tool_result>{}</tool_result>",
                render_transcript_tool_result(content, options.compress_large_blocks)
            )),
            ContentBlock::ToolUse { name, input, .. } => {
                let input_text = input.to_string();
                parts.push(format!(
                    "<tool_use name=\"{name}\">{}</tool_use>",
                    render_transcript_text_block(&input_text, options.compress_large_blocks)
                ))
            }
            ContentBlock::Image { .. } => parts.push("<image />".to_string()),
            ContentBlock::OpenAICompaction { encrypted_content } => parts.push(format!(
                "<openai_compaction chars=\"{}\" />",
                encrypted_content.len()
            )),
        }
    }
    format!("<{role}>\n{}\n</{role}>", parts.join("\n"))
}

fn render_transcript_text_block(text: &str, compress: bool) -> String {
    if !compress || text.len() <= 1_200 || text.contains("<ctx") || text.contains("<il:") {
        return text.to_string();
    }
    format!("<summary {} />", summarize_attrs(text))
}

fn render_transcript_tool_result(content: &str, compress: bool) -> String {
    if !compress || content.len() <= 900 || content.contains("<ctx") || content.contains("<il:") {
        return content.to_string();
    }
    format!("<summary {} />", summarize_attrs(content))
}

fn summarize_attrs(text: &str) -> String {
    let first = truncate_str(text.lines().next().unwrap_or_default().trim(), 180);
    format!(
        "lines=\"{}\" chars=\"{}\" first=\"{}\"",
        text.lines().count(),
        text.len(),
        escape_attr(&first)
    )
}

fn escape_attr(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn summarize(text: &str) -> String {
    let first = truncate_str(text.lines().next().unwrap_or_default().trim(), 160);
    format!(
        "lines={}; chars={}; first={}",
        text.lines().count(),
        text.len(),
        first
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[tokio::test]
    async fn deterministic_distillation_writes_files_without_enrichment() {
        let _guard = test_env_lock();
        let root =
            std::env::temp_dir().join(format!("kcode-local-model-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        unsafe {
            std::env::set_var("HOME", &root);
            std::env::set_var(ENV_ENRICH, "0");
        }

        record_api_exchange(
            "<user>remember that local bridge deterministic test works</user>",
            "<assistant>ok, local bridge deterministic test works</assistant>",
            "test-provider",
            "test-model",
        )
        .await
        .unwrap();

        let bridge_dir = root.join(".kcode/local-model-bridge");
        let exchanges = std::fs::read_to_string(bridge_dir.join("api-exchanges.jsonl")).unwrap();
        let distilled = std::fs::read_to_string(bridge_dir.join("distilled-memory.jsonl")).unwrap();
        assert!(exchanges.contains("test-provider"));
        assert!(distilled.contains("deterministic-local-bridge"));
        assert!(distilled.contains("api_exchange_summary"));
    }

    #[test]
    fn pre_router_writes_route_event() {
        let _guard = test_env_lock();
        let root =
            std::env::temp_dir().join(format!("kcode-local-preroute-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        unsafe {
            std::env::set_var("HOME", &root);
            std::env::set_var(ENV_PREROUTER, "1");
        }
        record_pre_route("hello", 5).unwrap();
        let routed =
            std::fs::read_to_string(root.join(".kcode/local-model-bridge/pre-route.jsonl"))
                .unwrap();
        assert!(routed.contains("local-first"));
    }

    #[test]
    fn promoter_writes_candidate_memory() {
        let _guard = test_env_lock();
        let root =
            std::env::temp_dir().join(format!("kcode-local-promoter-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        unsafe {
            std::env::set_var("HOME", &root);
            std::env::set_var(ENV_PROMOTER, "1");
        }
        promote_memory_candidates(
            "user: remember that I prefer short answers",
            "assistant: remembered",
        )
        .unwrap();
        let promoted =
            std::fs::read_to_string(root.join(".kcode/local-model-bridge/promoted-memory.jsonl"))
                .unwrap();
        assert!(promoted.contains("deterministic-promoter"));
        assert!(promoted.contains("short answers"));
    }

    #[test]
    fn pre_route_budget_is_small_for_simple_low_entropy_turns() {
        let mut messages = Vec::new();
        for idx in 0..30 {
            messages.push(Message::user(&format!(
                "historical verbose context {idx} {}",
                "alpha beta gamma delta ".repeat(120)
            )));
        }
        messages.push(Message::user("meow"));

        let transcript = pre_route_transcript_text(&messages);
        assert!(transcript.len() <= 4_200, "len={}", transcript.len());
        assert!(transcript.contains("meow"));
    }

    #[test]
    fn pre_route_budget_expands_for_repo_debug_turns() {
        let mut messages = Vec::new();
        for idx in 0..12 {
            messages.push(Message::user(&format!(
                "src/provider.rs error history {idx} {}",
                "compile failure token context ".repeat(60)
            )));
        }
        messages.push(Message::user(
            "fix the failing build in src/provider.rs and run tests",
        ));

        let budget = adaptive_pre_route_transcript_budget(&messages);
        assert!(budget >= 20_000, "budget={budget}");
    }

    #[test]
    fn pre_route_transcript_compresses_large_old_tool_results() {
        let messages = vec![
            Message::user("please inspect the repo"),
            Message::tool_result(
                "call-1",
                &"/tmp/project/src/file.rs: ERROR repeated diagnostic output
"
                .repeat(200),
                false,
            ),
            Message::user("ok thanks"),
        ];

        let transcript = pre_route_transcript_text(&messages);
        assert!(transcript.contains("<summary"));
        assert!(!transcript.contains(
            "repeated diagnostic output
/tmp/project"
        ));
        assert!(transcript.contains("ok thanks"));
    }

    #[test]
    fn parses_local_tool_call_tag() {
        let tools = vec![ToolDefinition {
            name: "todo".to_string(),
            description: "Read or update the todo list.".to_string(),
            input_schema: serde_json::json!({"type":"object"}),
        }];
        let parsed = parse_local_tool_call(
            r#"<tool_call>{"name":"todo","input":{"todos":[]}}</tool_call>"#,
            &tools,
        )
        .expect("tool call should parse");
        assert_eq!(parsed.0, "todo");
        assert_eq!(parsed.1["todos"], serde_json::json!([]));
    }

    #[test]
    fn rejects_unknown_local_tool_call() {
        let tools = vec![ToolDefinition {
            name: "todo".to_string(),
            description: "Read or update the todo list.".to_string(),
            input_schema: serde_json::json!({"type":"object"}),
        }];
        assert!(
            parse_local_tool_call(
                r#"<tool_call>{"name":"rm_everything","input":{}}</tool_call>"#,
                &tools,
            )
            .is_none()
        );
    }

    #[test]
    fn parses_json_final_action() {
        let action = parse_local_action(r#"{"type":"final","content":"Done."}"#, &[])
            .expect("final action should parse");
        assert_eq!(
            action,
            LocalAction::Final {
                content: "Done.".to_string()
            }
        );
    }

    #[test]
    fn parses_json_tool_action() {
        let tools = vec![ToolDefinition {
            name: "agentgrep".to_string(),
            description: "Search code".to_string(),
            input_schema: serde_json::json!({"type":"object"}),
        }];
        let parsed = parse_local_action(
            r#"{"type":"tool","name":"agentgrep","input":{"query":"complete_local"}}"#,
            &tools,
        )
        .expect("tool action should parse");
        assert_eq!(
            parsed,
            LocalAction::Tool {
                name: "agentgrep".to_string(),
                input: serde_json::json!({"query":"complete_local"})
            }
        );
    }

    #[test]
    fn selects_relevant_todo_tool_first() {
        let tools = vec![
            ToolDefinition {
                name: "bash".to_string(),
                description: "Run command".to_string(),
                input_schema: serde_json::json!({}),
            },
            ToolDefinition {
                name: "todo".to_string(),
                description: "Read or update todos".to_string(),
                input_schema: serde_json::json!({}),
            },
        ];
        let selected = select_relevant_tools("<user>update the incomplete todos</user>", &tools);
        assert_eq!(
            selected.first().map(|tool| tool.name.as_str()),
            Some("todo")
        );
    }
}
