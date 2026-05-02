use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::RwLock;

const ALLOW_LEGACY_AUTH_ENV: &str = "KCODE_ALLOW_CODEX_LEGACY_AUTH";
pub const LEGACY_CODEX_AUTH_SOURCE_ID: &str = "openai_codex_auth_json";

#[derive(Debug, Clone)]
pub struct CodexCredentials {
    pub access_token: String,
    pub refresh_token: String,
    pub id_token: Option<String>,
    pub account_id: Option<String>,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiAuthPreference {
    OAuth,
    ApiKey,
    Auto,
}

impl Default for OpenAiAuthPreference {
    fn default() -> Self {
        Self::Auto
    }
}

impl OpenAiAuthPreference {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "oauth" | "chatgpt" | "subscription" => Some(Self::OAuth),
            "api" | "api_key" | "apikey" | "key" | "platform" => Some(Self::ApiKey),
            "auto" | "failover" | "fallback" => Some(Self::Auto),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OAuth => "oauth",
            Self::ApiKey => "api_key",
            Self::Auto => "auto",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiAccount {
    pub label: String,
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KcodeOpenAiAuthFile {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub openai_accounts: Vec<OpenAiAccount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_openai_account: Option<String>,
    #[serde(default)]
    pub openai_auth_preference: OpenAiAuthPreference,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyAuthFile {
    tokens: Option<LegacyTokens>,
    #[serde(rename = "OPENAI_API_KEY")]
    api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyTokens {
    access_token: String,
    refresh_token: String,
    id_token: Option<String>,
    account_id: Option<String>,
    expires_at: Option<i64>,
}

static ACTIVE_ACCOUNT_OVERRIDE: RwLock<Option<String>> = RwLock::new(None);
const ACCOUNT_LABEL_PREFIX: &str = "openai";

pub fn set_active_account_override(label: Option<String>) {
    if let Ok(mut guard) = ACTIVE_ACCOUNT_OVERRIDE.write() {
        *guard = label;
    }
}

pub fn get_active_account_override() -> Option<String> {
    ACTIVE_ACCOUNT_OVERRIDE
        .read()
        .ok()
        .and_then(|guard| guard.clone())
}

pub fn primary_account_label() -> String {
    crate::auth::account_store::canonical_account_label(ACCOUNT_LABEL_PREFIX, 1)
}

pub fn next_account_label() -> Result<String> {
    let auth = load_auth_file()?;
    Ok(crate::auth::account_store::next_account_label(
        ACCOUNT_LABEL_PREFIX,
        auth.openai_accounts.len(),
    ))
}

pub fn login_target_label(requested: Option<&str>) -> Result<String> {
    let auth = load_auth_file()?;
    Ok(crate::auth::account_store::login_target_label(
        ACCOUNT_LABEL_PREFIX,
        requested,
        auth.active_openai_account,
        &auth.openai_accounts,
        |account| account.label.as_str(),
    ))
}

fn relabel_accounts(auth: &mut KcodeOpenAiAuthFile) -> bool {
    let outcome = crate::auth::account_store::relabel_accounts(
        ACCOUNT_LABEL_PREFIX,
        &mut auth.openai_accounts,
        &mut auth.active_openai_account,
        get_active_account_override(),
        |account| account.label.as_str(),
        |account, label| account.label = label,
    );
    if let Some(label) = outcome.canonical_override_label {
        set_active_account_override(Some(label));
    }
    outcome.changed
}

fn kcode_auth_path() -> Result<PathBuf> {
    Ok(crate::storage::kcode_dir()?.join("openai-auth.json"))
}

fn legacy_auth_path() -> Result<PathBuf> {
    crate::storage::user_home_path(".codex/auth.json")
}

pub fn legacy_auth_file_path() -> Result<PathBuf> {
    legacy_auth_path()
}

pub fn trust_legacy_auth_for_future_use() -> Result<()> {
    crate::config::Config::allow_external_auth_source_for_path(
        LEGACY_CODEX_AUTH_SOURCE_ID,
        &legacy_auth_path()?,
    )?;
    super::AuthStatus::invalidate_cache();
    Ok(())
}

pub fn legacy_auth_allowed() -> bool {
    std::env::var(ALLOW_LEGACY_AUTH_ENV)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
        || legacy_auth_path()
            .ok()
            .map(|path| {
                crate::config::Config::external_auth_source_allowed_for_path(
                    LEGACY_CODEX_AUTH_SOURCE_ID,
                    &path,
                )
            })
            .unwrap_or(false)
}

pub fn legacy_auth_source_exists() -> bool {
    legacy_auth_path()
        .map(|path| path.exists())
        .unwrap_or(false)
}

pub fn has_unconsented_legacy_credentials() -> bool {
    legacy_auth_source_exists() && !legacy_auth_allowed()
}

pub fn load_auth_file() -> Result<KcodeOpenAiAuthFile> {
    let path = kcode_auth_path()?;
    let mut auth = if path.exists() {
        crate::storage::harden_secret_file_permissions(&path);
        crate::storage::read_json(&path)
            .with_context(|| format!("Could not read OpenAI credentials from {:?}", path))?
    } else {
        KcodeOpenAiAuthFile::default()
    };

    if relabel_accounts(&mut auth) {
        crate::logging::info(
            "Renaming OpenAI accounts to numbered labels (openai-1, openai-2, ...)",
        );
        save_auth_file(&auth)?;
    }

    Ok(auth)
}

pub fn save_auth_file(auth: &KcodeOpenAiAuthFile) -> Result<()> {
    let auth_path = kcode_auth_path()?;
    let clean = KcodeOpenAiAuthFile {
        openai_accounts: auth.openai_accounts.clone(),
        active_openai_account: auth.active_openai_account.clone(),
        openai_auth_preference: auth.openai_auth_preference,
        openai_api_key: auth.openai_api_key.clone(),
    };

    crate::storage::write_json_secret(&auth_path, &clean)?;
    Ok(())
}

pub fn list_accounts() -> Result<Vec<OpenAiAccount>> {
    let auth = load_auth_file()?;
    Ok(auth.openai_accounts)
}

pub fn auth_preference() -> OpenAiAuthPreference {
    load_auth_file()
        .map(|auth| auth.openai_auth_preference)
        .unwrap_or_default()
}

pub fn set_auth_preference(preference: OpenAiAuthPreference) -> Result<()> {
    let mut auth = load_auth_file().unwrap_or_default();
    auth.openai_auth_preference = preference;
    save_auth_file(&auth)?;
    super::AuthStatus::invalidate_cache();
    Ok(())
}

pub fn has_stored_api_key() -> bool {
    load_auth_file()
        .ok()
        .and_then(|auth| auth.openai_api_key)
        .map(|key| !key.trim().is_empty())
        .unwrap_or(false)
}

pub fn set_stored_api_key(api_key: Option<String>) -> Result<()> {
    let mut auth = load_auth_file().unwrap_or_default();
    auth.openai_api_key = api_key
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty());
    save_auth_file(&auth)?;
    super::AuthStatus::invalidate_cache();
    Ok(())
}

pub fn load_api_key_credentials() -> Result<CodexCredentials> {
    let stored = load_auth_file()
        .ok()
        .and_then(|auth| auth.openai_api_key)
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty());
    let legacy = if legacy_auth_allowed() {
        load_legacy_api_key_credentials()
            .ok()
            .map(|creds| creds.access_token)
    } else {
        None
    };
    let api_key = stored
        .or_else(|| std::env::var("OPENAI_API_KEY").ok())
        .or(legacy)
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
        .context("No OpenAI API key found in ~/.kcode/openai-auth.json or OPENAI_API_KEY")?;
    Ok(CodexCredentials {
        access_token: api_key,
        refresh_token: String::new(),
        id_token: None,
        account_id: None,
        expires_at: None,
    })
}

pub fn active_account_label() -> Option<String> {
    let auth = load_auth_file().ok()?;
    crate::auth::account_store::active_account_label(
        get_active_account_override(),
        auth.active_openai_account,
        &auth.openai_accounts,
        |account| account.label.as_str(),
    )
}

pub fn set_active_account(label: &str) -> Result<()> {
    let mut auth = load_auth_file()?;
    crate::auth::account_store::set_active_account(
        label,
        &auth.openai_accounts,
        &mut auth.active_openai_account,
        "No OpenAI account with label '{}' found",
        |account| account.label.as_str(),
    )?;
    save_auth_file(&auth)?;
    set_active_account_override(Some(label.to_string()));
    Ok(())
}

pub fn upsert_account(account: OpenAiAccount) -> Result<String> {
    let mut auth = load_auth_file()?;
    let label = crate::auth::account_store::upsert_account(
        ACCOUNT_LABEL_PREFIX,
        &mut auth.openai_accounts,
        &mut auth.active_openai_account,
        account,
        |account| account.label.as_str(),
        |account, label| account.label = label,
    );
    save_auth_file(&auth)?;
    Ok(label)
}

pub fn remove_account(label: &str) -> Result<()> {
    let mut auth = load_auth_file()?;
    let before = auth.openai_accounts.len();
    auth.openai_accounts
        .retain(|account| account.label != label);
    if auth.openai_accounts.len() == before {
        anyhow::bail!("No OpenAI account with label '{}' found", label);
    }

    if auth.active_openai_account.as_deref() == Some(label) {
        auth.active_openai_account = auth.openai_accounts.first().map(|a| a.label.clone());
    }

    save_auth_file(&auth)?;

    if get_active_account_override().as_deref() == Some(label) {
        set_active_account_override(auth.active_openai_account.clone());
    }

    Ok(())
}

pub fn update_account_tokens(
    label: &str,
    access_token: &str,
    refresh_token: &str,
    id_token: Option<String>,
    account_id: Option<String>,
    expires_at: Option<i64>,
) -> Result<()> {
    let mut auth = load_auth_file()?;
    if let Some(account) = auth
        .openai_accounts
        .iter_mut()
        .find(|account| account.label == label)
    {
        account.access_token = access_token.to_string();
        account.refresh_token = refresh_token.to_string();
        account.id_token = id_token.clone();
        account.account_id =
            account_id.or_else(|| id_token.as_deref().and_then(extract_account_id));
        account.expires_at = expires_at;
        account.email = id_token.as_deref().and_then(extract_email);
        save_auth_file(&auth)?;
        Ok(())
    } else {
        anyhow::bail!(
            "No OpenAI account with label '{}' found for token update",
            label
        );
    }
}

pub fn update_account_profile(label: &str, email: Option<String>) -> Result<()> {
    let mut auth = load_auth_file()?;
    if let Some(account) = auth
        .openai_accounts
        .iter_mut()
        .find(|account| account.label == label)
    {
        account.email = email;
        save_auth_file(&auth)?;
        Ok(())
    } else {
        anyhow::bail!(
            "No OpenAI account with label '{}' found for profile update",
            label
        );
    }
}

pub fn load_credentials() -> Result<CodexCredentials> {
    match auth_preference() {
        OpenAiAuthPreference::ApiKey => return load_api_key_credentials(),
        OpenAiAuthPreference::OAuth => return load_oauth_credentials_only(),
        OpenAiAuthPreference::Auto => {}
    }

    load_oauth_credentials_only().or_else(|_| load_api_key_credentials())
}

fn load_oauth_credentials_only() -> Result<CodexCredentials> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut expired_candidates: Vec<(&str, CodexCredentials)> = Vec::new();
    let legacy_allowed = legacy_auth_allowed();

    if let Ok(creds) = load_kcode_credentials() {
        if creds
            .expires_at
            .map(|expires_at| expires_at > now_ms)
            .unwrap_or(true)
        {
            return Ok(creds);
        }
        expired_candidates.push(("kcode", creds));
    }

    if legacy_allowed {
        if let Ok(creds) = load_legacy_oauth_credentials() {
            if creds
                .expires_at
                .map(|expires_at| expires_at > now_ms)
                .unwrap_or(true)
            {
                return Ok(creds);
            }
            expired_candidates.push(("legacy", creds));
        }

        // Legacy API keys are handled by load_api_key_credentials() so the
        // preference/failover policy is centralized.
    }

    if let Some(tokens) = crate::auth::external::load_openai_oauth_tokens() {
        let creds = CodexCredentials {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            id_token: None,
            account_id: None,
            expires_at: Some(tokens.expires_at),
        };
        if creds
            .expires_at
            .map(|expires_at| expires_at > now_ms)
            .unwrap_or(true)
        {
            return Ok(creds);
        }
        expired_candidates.push(("external", creds));
    }

    if let Some((_source, creds)) = expired_candidates.into_iter().next() {
        return Ok(creds);
    }

    anyhow::bail!("No OpenAI OAuth tokens found")
}

pub fn load_credentials_for_account(label: &str) -> Result<CodexCredentials> {
    let auth = load_auth_file()?;
    let account = auth
        .openai_accounts
        .iter()
        .find(|account| account.label == label)
        .with_context(|| format!("No OpenAI account with label '{}'", label))?;
    Ok(credentials_from_account(account))
}

pub fn upsert_account_from_tokens(
    label: &str,
    access_token: &str,
    refresh_token: &str,
    id_token: Option<String>,
    expires_at: Option<i64>,
) -> Result<String> {
    let creds = CodexCredentials {
        access_token: access_token.to_string(),
        refresh_token: refresh_token.to_string(),
        account_id: id_token.as_deref().and_then(extract_account_id),
        id_token,
        expires_at,
    };
    let email = creds.id_token.as_deref().and_then(extract_email);
    upsert_account(account_from_credentials(label, &creds, email))
}

fn load_kcode_credentials() -> Result<CodexCredentials> {
    let auth = load_auth_file()?;
    if auth.openai_accounts.is_empty() {
        anyhow::bail!("No OpenAI accounts configured in kcode auth file")
    }

    let active_label = get_active_account_override()
        .or(auth.active_openai_account)
        .unwrap_or_else(primary_account_label);

    let account = auth
        .openai_accounts
        .iter()
        .find(|account| account.label == active_label)
        .or_else(|| auth.openai_accounts.first())
        .context("No OpenAI accounts in kcode auth file")?;

    Ok(credentials_from_account(account))
}

fn load_legacy_oauth_credentials() -> Result<CodexCredentials> {
    let file = load_legacy_auth_file()?;
    let tokens = file
        .tokens
        .context("No OAuth tokens found in legacy Codex auth file")?;
    Ok(credentials_from_legacy_tokens(&tokens))
}

fn load_legacy_api_key_credentials() -> Result<CodexCredentials> {
    let file = load_legacy_auth_file()?;
    let api_key = file
        .api_key
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .context("No API key found in legacy Codex auth file")?;
    Ok(CodexCredentials {
        access_token: api_key,
        refresh_token: String::new(),
        id_token: None,
        account_id: None,
        expires_at: None,
    })
}

fn load_legacy_auth_file() -> Result<LegacyAuthFile> {
    let path = crate::storage::validate_external_auth_file(&legacy_auth_path()?)?;
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Could not read credentials from {:?}", path))?;
    serde_json::from_str(&content).context("Could not parse Codex credentials")
}

fn credentials_from_account(account: &OpenAiAccount) -> CodexCredentials {
    CodexCredentials {
        access_token: account.access_token.clone(),
        refresh_token: account.refresh_token.clone(),
        id_token: account.id_token.clone(),
        account_id: account
            .account_id
            .clone()
            .or_else(|| account.id_token.as_deref().and_then(extract_account_id)),
        expires_at: account.expires_at,
    }
}

fn credentials_from_legacy_tokens(tokens: &LegacyTokens) -> CodexCredentials {
    CodexCredentials {
        access_token: tokens.access_token.clone(),
        refresh_token: tokens.refresh_token.clone(),
        id_token: tokens.id_token.clone(),
        account_id: tokens
            .account_id
            .clone()
            .or_else(|| tokens.id_token.as_deref().and_then(extract_account_id)),
        expires_at: tokens.expires_at,
    }
}

fn account_from_credentials(
    label: &str,
    credentials: &CodexCredentials,
    email: Option<String>,
) -> OpenAiAccount {
    OpenAiAccount {
        label: label.to_string(),
        access_token: credentials.access_token.clone(),
        refresh_token: credentials.refresh_token.clone(),
        id_token: credentials.id_token.clone(),
        account_id: credentials
            .account_id
            .clone()
            .or_else(|| credentials.id_token.as_deref().and_then(extract_account_id)),
        expires_at: credentials.expires_at,
        email,
    }
}

pub fn extract_account_id(id_token: &str) -> Option<String> {
    let payload = decode_jwt_payload(id_token)?;
    let auth = payload.get("https://api.openai.com/auth")?;
    auth.get("chatgpt_account_id")?
        .as_str()
        .map(|value| value.to_string())
}

pub fn extract_email(id_token: &str) -> Option<String> {
    let payload = decode_jwt_payload(id_token)?;
    payload
        .get("email")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
}

fn decode_jwt_payload(token: &str) -> Option<Value> {
    let payload_b64 = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload_b64.as_bytes()).ok()?;
    serde_json::from_slice::<Value>(&decoded).ok()
}

#[cfg(test)]
#[path = "codex_tests.rs"]
mod tests;
