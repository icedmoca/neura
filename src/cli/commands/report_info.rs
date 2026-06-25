use anyhow::Result;
use serde::Serialize;

use crate::cli::provider_init::{self, ProviderChoice};

#[derive(Debug, Serialize)]
struct AuthStatusProviderReport {
    id: String,
    display_name: String,
    status: String,
    method: String,
    health: String,
    credential_source: String,
    expiry_confidence: String,
    refresh_support: String,
    validation_method: String,
    last_refresh: Option<String>,
    validation: Option<String>,
    auth_kind: String,
    recommended: bool,
}

#[derive(Debug, Serialize)]
struct AuthStatusReport {
    any_available: bool,
    providers: Vec<AuthStatusProviderReport>,
}

#[derive(Debug, Serialize)]
struct AuthDoctorProviderReport {
    id: String,
    display_name: String,
    status: String,
    method: String,
    health: String,
    validation: Option<String>,
    validation_result: Option<String>,
    needs_attention: bool,
    recommended_actions: Vec<String>,
}

#[derive(Debug, Serialize)]
struct AuthDoctorReport {
    checked_provider: Option<String>,
    validate: bool,
    any_issue: bool,
    providers: Vec<AuthDoctorProviderReport>,
}

#[derive(Debug, Serialize)]
pub(super) struct ProviderListEntry {
    pub(super) id: String,
    pub(super) display_name: String,
    pub(super) auth_kind: Option<String>,
    pub(super) recommended: bool,
    pub(super) aliases: Vec<String>,
    pub(super) detail: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProviderListReport {
    providers: Vec<ProviderListEntry>,
}

#[derive(Debug, Serialize)]
struct ProviderCurrentReport {
    requested_provider: String,
    requested_model: Option<String>,
    resolved_provider: String,
    selected_model: String,
}

#[derive(Debug, Serialize)]
pub(super) struct VersionReport {
    pub(super) version: String,
    pub(super) semver: String,
    pub(super) base_semver: String,
    pub(super) update_semver: String,
    pub(super) git_hash: String,
    pub(super) git_tag: String,
    pub(super) build_time: String,
    pub(super) git_date: String,
    pub(super) release_build: bool,
}

#[derive(Debug, Serialize)]
struct UsageLimitReport {
    name: String,
    usage_percent: f32,
    resets_at: Option<String>,
    reset_in: Option<String>,
}

#[derive(Debug, Serialize)]
struct UsageProviderReport {
    provider_name: String,
    limits: Vec<UsageLimitReport>,
    extra_info: Vec<(String, String)>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct UsageReport {
    providers: Vec<UsageProviderReport>,
}

pub(super) fn run_auth_status_command(emit_json: bool) -> Result<()> {
    let status = crate::auth::AuthStatus::check();
    let validation = crate::auth::validation::load_all();
    let providers = crate::provider_catalog::auth_status_login_providers();
    let reports = providers
        .into_iter()
        .map(|provider| {
            let assessment = status.assessment_for_provider(provider);
            AuthStatusProviderReport {
                id: provider.id.to_string(),
                display_name: provider.display_name.to_string(),
                status: auth_state_label(assessment.state).to_string(),
                method: assessment.method_detail.clone(),
                health: assessment.health_summary(),
                credential_source: assessment.credential_source.label().to_string(),
                expiry_confidence: assessment.expiry_confidence.label().to_string(),
                refresh_support: assessment.refresh_support.label().to_string(),
                validation_method: assessment.validation_method.label().to_string(),
                last_refresh: assessment
                    .last_refresh
                    .as_ref()
                    .map(crate::auth::refresh_state::format_record_label),
                validation: validation
                    .get(provider.id)
                    .map(crate::auth::validation::format_record_label),
                auth_kind: provider.auth_kind.label().to_string(),
                recommended: provider.recommended,
            }
        })
        .collect::<Vec<_>>();

    if emit_json {
        let report = AuthStatusReport {
            any_available: status.has_any_available(),
            providers: reports,
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for provider in reports {
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}",
                provider.id,
                provider.status,
                provider.auth_kind,
                provider.method,
                provider.health,
                provider.validation.as_deref().unwrap_or("not validated")
            );
        }
    }

    Ok(())
}

pub(super) async fn run_auth_doctor_command(
    provider_arg: Option<&str>,
    validate: bool,
    emit_json: bool,
) -> Result<()> {
    let status = crate::auth::AuthStatus::check();
    let providers = select_auth_doctor_providers(provider_arg, &status)?;
    let mut reports = Vec::new();

    for provider in providers {
        let assessment = status.assessment_for_provider(provider);
        let mut validation = crate::auth::validation::get(provider.id)
            .map(|record| crate::auth::validation::format_record_label(&record));
        let validation_result =
            if validate && assessment.state != crate::auth::AuthState::NotConfigured {
                match super::run_post_login_validation(provider).await {
                    Ok(()) => {
                        validation = crate::auth::validation::get(provider.id)
                            .map(|record| crate::auth::validation::format_record_label(&record));
                        Some("validation passed".to_string())
                    }
                    Err(err) => {
                        validation = crate::auth::validation::get(provider.id)
                            .map(|record| crate::auth::validation::format_record_label(&record));
                        Some(err.to_string())
                    }
                }
            } else {
                None
            };
        let recommended_actions = crate::auth::doctor::recommended_actions(
            provider,
            assessment.state,
            validation.as_deref(),
            validation_result.as_deref(),
        );
        let method = assessment.method_detail.clone();
        let health = assessment.health_summary();
        let needs_attention = assessment.state != crate::auth::AuthState::Available
            || validation_result
                .as_deref()
                .is_some_and(|result| result != "validation passed")
            || validation
                .as_deref()
                .is_some_and(|value| value.contains("validation failed"));

        reports.push(AuthDoctorProviderReport {
            id: provider.id.to_string(),
            display_name: provider.display_name.to_string(),
            status: auth_state_label(assessment.state).to_string(),
            method,
            health,
            validation,
            validation_result,
            needs_attention,
            recommended_actions,
        });
    }

    let report = AuthDoctorReport {
        checked_provider: provider_arg.map(str::to_string),
        validate,
        any_issue: reports.iter().any(|provider| provider.needs_attention),
        providers: reports,
    };

    if emit_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    for (index, provider) in report.providers.iter().enumerate() {
        if index > 0 {
            println!();
        }
        println!("{} ({})", provider.display_name, provider.id);
        println!("status: {}", provider.status);
        println!("method: {}", provider.method);
        println!("health: {}", provider.health);
        println!(
            "validation: {}",
            provider.validation.as_deref().unwrap_or("not validated")
        );
        if let Some(validation_result) = provider.validation_result.as_deref() {
            println!("validation_run: {}", validation_result);
        }
        println!("needs_attention: {}", provider.needs_attention);
        if !provider.recommended_actions.is_empty() {
            println!("next_steps:");
            for action in &provider.recommended_actions {
                println!("- {}", action);
            }
        }
    }

    Ok(())
}

pub(super) fn run_provider_list_command(emit_json: bool) -> Result<()> {
    let providers = list_cli_providers();

    if emit_json {
        let report = ProviderListReport { providers };
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for provider in providers {
            if let Some(detail) = provider.detail.as_deref() {
                println!("{}\t{}\t{}", provider.id, provider.display_name, detail);
            } else {
                println!("{}\t{}", provider.id, provider.display_name);
            }
        }
    }

    Ok(())
}

pub(super) async fn run_provider_current_command(
    choice: &ProviderChoice,
    model: Option<&str>,
    emit_json: bool,
) -> Result<()> {
    let provider = provider_init::init_provider_quiet(choice, model).await?;
    let report = ProviderCurrentReport {
        requested_provider: choice.as_arg_value().to_string(),
        requested_model: model.map(str::to_string),
        resolved_provider: provider.name().to_string(),
        selected_model: provider.model(),
    };

    if emit_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("requested_provider\t{}", report.requested_provider);
        if let Some(requested_model) = report.requested_model.as_deref() {
            println!("requested_model\t{}", requested_model);
        }
        println!("resolved_provider\t{}", report.resolved_provider);
        println!("selected_model\t{}", report.selected_model);
    }

    Ok(())
}

pub(super) fn run_version_command(emit_json: bool) -> Result<()> {
    let report = VersionReport {
        version: env!("NEURA_VERSION").to_string(),
        semver: env!("NEURA_SEMVER").to_string(),
        base_semver: env!("NEURA_BASE_SEMVER").to_string(),
        update_semver: env!("NEURA_UPDATE_SEMVER").to_string(),
        git_hash: env!("NEURA_GIT_HASH").to_string(),
        git_tag: env!("NEURA_GIT_TAG").to_string(),
        build_time: crate::build::current_binary_build_time_string()
            .unwrap_or_else(|| "unknown".to_string()),
        git_date: env!("NEURA_GIT_DATE").to_string(),
        release_build: option_env!("NEURA_RELEASE_BUILD").is_some(),
    };

    if emit_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("version\t{}", report.version);
        println!("semver\t{}", report.semver);
        println!("base_semver\t{}", report.base_semver);
        println!("update_semver\t{}", report.update_semver);
        println!("git_hash\t{}", report.git_hash);
        println!("git_tag\t{}", report.git_tag);
        println!("build_time\t{}", report.build_time);
        println!("git_date\t{}", report.git_date);
        println!("release_build\t{}", report.release_build);
    }

    Ok(())
}

pub(super) async fn run_usage_command(emit_json: bool) -> Result<()> {
    let providers = crate::usage::fetch_all_provider_usage().await;

    let report = UsageReport {
        providers: providers.iter().map(usage_provider_report).collect(),
    };

    if emit_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    if report.providers.is_empty() {
        println!("No connected providers");
        println!();
        println!("Next steps:");
        println!("- Use `neura login --provider claude` to connect Claude OAuth.");
        println!("- Use `neura login --provider openai` to connect ChatGPT / Codex OAuth.");
        return Ok(());
    }

    for (idx, provider) in report.providers.iter().enumerate() {
        if idx > 0 {
            println!();
        }

        println!("{}", provider.provider_name);
        println!("{}", "-".repeat(provider.provider_name.chars().count()));

        if let Some(error) = &provider.error {
            println!("error: {}", error);
            continue;
        }

        if provider.limits.is_empty() && provider.extra_info.is_empty() {
            println!("No usage data available.");
            continue;
        }

        for limit in &provider.limits {
            match limit.reset_in.as_deref() {
                Some(reset_in) => println!(
                    "{}: {} (resets in {})",
                    limit.name,
                    crate::usage::format_usage_bar(limit.usage_percent, 15),
                    reset_in
                ),
                None => println!(
                    "{}: {}",
                    limit.name,
                    crate::usage::format_usage_bar(limit.usage_percent, 15)
                ),
            }
        }

        if !provider.extra_info.is_empty() {
            if !provider.limits.is_empty() {
                println!();
            }
            for (key, value) in &provider.extra_info {
                println!("{}: {}", key, value);
            }
        }
    }

    Ok(())
}

fn select_auth_doctor_providers(
    provider_arg: Option<&str>,
    status: &crate::auth::AuthStatus,
) -> Result<Vec<crate::provider_catalog::LoginProviderDescriptor>> {
    if let Some(provider_arg) = provider_arg {
        let provider =
            crate::provider_catalog::resolve_login_provider(provider_arg).ok_or_else(|| {
                anyhow::anyhow!(
                    "Unknown provider '{}'. Use `neura provider list` to see valid provider ids.",
                    provider_arg
                )
            })?;
        return Ok(vec![provider]);
    }

    let configured = crate::provider_catalog::auth_status_login_providers()
        .into_iter()
        .filter(|provider| {
            status.state_for_provider(*provider) != crate::auth::AuthState::NotConfigured
        })
        .collect::<Vec<_>>();
    if configured.is_empty() {
        Ok(crate::provider_catalog::auth_status_login_providers().to_vec())
    } else {
        Ok(configured)
    }
}

fn usage_provider_report(provider: &crate::usage::ProviderUsage) -> UsageProviderReport {
    UsageProviderReport {
        provider_name: provider.provider_name.clone(),
        limits: provider
            .limits
            .iter()
            .map(|limit| UsageLimitReport {
                name: limit.name.clone(),
                usage_percent: limit.usage_percent,
                resets_at: limit.resets_at.clone(),
                reset_in: limit
                    .resets_at
                    .as_deref()
                    .map(crate::usage::format_reset_time),
            })
            .collect(),
        extra_info: provider.extra_info.clone(),
        error: provider.error.clone(),
    }
}

pub(super) fn list_cli_providers() -> Vec<ProviderListEntry> {
    let choices = [
        ProviderChoice::Neura,
        ProviderChoice::Claude,
        ProviderChoice::Openai,
        ProviderChoice::Openrouter,
        ProviderChoice::Azure,
        ProviderChoice::Opencode,
        ProviderChoice::OpencodeGo,
        ProviderChoice::Zai,
        ProviderChoice::Kimi,
        ProviderChoice::Groq,
        ProviderChoice::Mistral,
        ProviderChoice::Perplexity,
        ProviderChoice::TogetherAi,
        ProviderChoice::Deepinfra,
        ProviderChoice::Xai,
        ProviderChoice::Chutes,
        ProviderChoice::Cerebras,
        ProviderChoice::AlibabaCodingPlan,
        ProviderChoice::OpenaiCompatible,
        ProviderChoice::Cursor,
        ProviderChoice::Copilot,
        ProviderChoice::Gemini,
        ProviderChoice::Antigravity,
        ProviderChoice::Google,
        ProviderChoice::Auto,
    ];

    choices
        .into_iter()
        .map(|choice| {
            if let Some(provider) = provider_init::login_provider_for_choice(&choice) {
                ProviderListEntry {
                    id: choice.as_arg_value().to_string(),
                    display_name: provider.display_name.to_string(),
                    auth_kind: Some(provider.auth_kind.label().to_string()),
                    recommended: provider.recommended,
                    aliases: provider
                        .aliases
                        .iter()
                        .map(|alias| (*alias).to_string())
                        .collect(),
                    detail: Some(provider.menu_detail.to_string()),
                }
            } else {
                ProviderListEntry {
                    id: choice.as_arg_value().to_string(),
                    display_name: "Auto-detect".to_string(),
                    auth_kind: None,
                    recommended: false,
                    aliases: Vec::new(),
                    detail: Some("Use the best configured provider automatically".to_string()),
                }
            }
        })
        .collect()
}

fn auth_state_label(state: crate::auth::AuthState) -> &'static str {
    match state {
        crate::auth::AuthState::Available => "available",
        crate::auth::AuthState::Expired => "expired",
        crate::auth::AuthState::NotConfigured => "not_configured",
    }
}
