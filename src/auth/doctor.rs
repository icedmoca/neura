use crate::auth::AuthState;
use crate::provider_catalog::{LoginProviderAuthKind, LoginProviderDescriptor};

pub fn recommended_actions(
    provider: LoginProviderDescriptor,
    state: AuthState,
    validation: Option<&str>,
    validation_result: Option<&str>,
) -> Vec<String> {
    let mut actions = Vec::new();
    match state {
        AuthState::NotConfigured => actions.push(format!(
            "Connect it: `kcode login --provider {}`",
            provider.id
        )),
        AuthState::Expired => actions.push(format!(
            "Refresh or replace the current login: `kcode login --provider {}`",
            provider.id
        )),
        AuthState::Available => {}
    }

    if validation.is_none() {
        actions.push(format!(
            "Run runtime verification: `kcode auth-test --provider {}`",
            provider.id
        ));
    }
    if validation.is_some_and(|value| value.contains("validation failed"))
        || validation_result.is_some_and(|value| value != "validation passed")
    {
        actions.push(format!(
            "Inspect runtime readiness: `kcode auth-test --provider {}`",
            provider.id
        ));
    }

    if matches!(provider.auth_kind, LoginProviderAuthKind::OAuth)
        || matches!(provider.auth_kind, LoginProviderAuthKind::DeviceCode)
        || matches!(provider.auth_kind, LoginProviderAuthKind::Hybrid)
    {
        actions.push(format!(
            "Use the manual-safe fallback if browser/callback flow is flaky: `kcode login --provider {} --print-auth-url`",
            provider.id
        ));
    }

    actions.push("Review current state: `kcode auth status --json`".to_string());
    actions.dedup();
    actions
}
