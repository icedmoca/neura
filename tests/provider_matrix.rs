use anyhow::Result;
use kcode::auth::{AuthState, AuthStatus};
use kcode::provider::openrouter::OpenRouterProvider;
use kcode::provider_catalog::{
    OPENAI_COMPAT_PROFILE, apply_openai_compatible_profile_env, openai_compatible_profiles,
    resolve_openai_compatible_profile,
};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn lock_env() -> MutexGuard<'static, ()> {
    let mutex = ENV_LOCK.get_or_init(|| Mutex::new(()));
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn tracked_env_vars() -> Vec<String> {
    let mut keys: HashSet<String> = [
        "KCODE_HOME",
        "XDG_CONFIG_HOME",
        "KCODE_OPENROUTER_API_BASE",
        "KCODE_OPENROUTER_API_KEY_NAME",
        "KCODE_OPENROUTER_ENV_FILE",
        "KCODE_OPENROUTER_CACHE_NAMESPACE",
        "KCODE_OPENROUTER_PROVIDER_FEATURES",
        "KCODE_OPENROUTER_ALLOW_NO_AUTH",
        "KCODE_OPENROUTER_PROVIDER",
        "KCODE_OPENROUTER_NO_FALLBACK",
        "KCODE_OPENROUTER_MODEL",
        "KCODE_OPENROUTER_THINKING",
        "KCODE_OPENAI_COMPAT_API_BASE",
        "KCODE_OPENAI_COMPAT_API_KEY_NAME",
        "KCODE_OPENAI_COMPAT_ENV_FILE",
        "KCODE_OPENAI_COMPAT_SETUP_URL",
        "KCODE_OPENAI_COMPAT_DEFAULT_MODEL",
        "KCODE_OPENAI_COMPAT_LOCAL_ENABLED",
        "OPENROUTER_API_KEY",
    ]
    .into_iter()
    .map(ToString::to_string)
    .collect();

    for profile in openai_compatible_profiles() {
        keys.insert(profile.api_key_env.to_string());
    }

    let mut keys: Vec<_> = keys.into_iter().collect();
    keys.sort();
    keys
}

struct TestEnv {
    _lock: MutexGuard<'static, ()>,
    saved: Vec<(String, Option<String>)>,
    temp: tempfile::TempDir,
}

impl TestEnv {
    fn new() -> Result<Self> {
        let lock = lock_env();
        let temp = tempfile::Builder::new()
            .prefix("kcode-provider-matrix-")
            .tempdir()?;
        let saved = tracked_env_vars()
            .into_iter()
            .map(|key| {
                let value = std::env::var(&key).ok();
                (key, value)
            })
            .collect::<Vec<_>>();

        for (key, _) in &saved {
            kcode::env::remove_var(key);
        }

        let config_root = temp.path().join("config").join("kcode");
        std::fs::create_dir_all(&config_root)?;
        kcode::env::set_var("KCODE_HOME", temp.path());
        apply_openai_compatible_profile_env(None);
        AuthStatus::invalidate_cache();

        Ok(Self {
            _lock: lock,
            saved,
            temp,
        })
    }

    fn config_dir(&self) -> PathBuf {
        self.temp.path().join("config").join("kcode")
    }

    fn clear_profile_keys(&self) {
        kcode::env::remove_var("OPENROUTER_API_KEY");
        for profile in openai_compatible_profiles() {
            kcode::env::remove_var(profile.api_key_env);
        }
        AuthStatus::invalidate_cache();
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        apply_openai_compatible_profile_env(None);
        AuthStatus::invalidate_cache();
        for (key, value) in &self.saved {
            if let Some(value) = value {
                kcode::env::set_var(key, value);
            } else {
                kcode::env::remove_var(key);
            }
        }
        AuthStatus::invalidate_cache();
    }
}

#[test]
fn provider_matrix_env_credentials_activate_openrouter_runtime() -> Result<()> {
    let env = TestEnv::new()?;

    for &profile in openai_compatible_profiles() {
        env.clear_profile_keys();
        apply_openai_compatible_profile_env(Some(profile));
        let resolved = resolve_openai_compatible_profile(profile);
        kcode::env::set_var(&resolved.api_key_env, "matrix-env-secret");
        AuthStatus::invalidate_cache();

        assert_eq!(
            std::env::var("KCODE_OPENROUTER_API_BASE").ok().as_deref(),
            Some(resolved.api_base.as_str())
        );
        assert_eq!(
            std::env::var("KCODE_OPENROUTER_API_KEY_NAME")
                .ok()
                .as_deref(),
            Some(resolved.api_key_env.as_str())
        );
        assert_eq!(
            std::env::var("KCODE_OPENROUTER_ENV_FILE").ok().as_deref(),
            Some(resolved.env_file.as_str())
        );
        assert_eq!(
            std::env::var("KCODE_OPENROUTER_CACHE_NAMESPACE")
                .ok()
                .as_deref(),
            Some(resolved.id.as_str())
        );
        assert_eq!(
            std::env::var("KCODE_OPENROUTER_PROVIDER_FEATURES")
                .ok()
                .as_deref(),
            Some("0")
        );
        assert!(
            OpenRouterProvider::has_credentials(),
            "expected credentials for {}",
            resolved.id
        );
        OpenRouterProvider::new()?;
        assert_eq!(AuthStatus::check().openrouter, AuthState::Available);

        kcode::env::remove_var(&resolved.api_key_env);
    }

    Ok(())
}

#[test]
fn provider_matrix_file_credentials_activate_openrouter_runtime() -> Result<()> {
    let env = TestEnv::new()?;

    for &profile in openai_compatible_profiles() {
        env.clear_profile_keys();
        apply_openai_compatible_profile_env(Some(profile));
        let resolved = resolve_openai_compatible_profile(profile);
        let env_file = env.config_dir().join(&resolved.env_file);
        std::fs::write(
            &env_file,
            format!("{}=matrix-file-secret\n", resolved.api_key_env),
        )?;
        AuthStatus::invalidate_cache();

        assert!(
            OpenRouterProvider::has_credentials(),
            "expected file credentials for {}",
            resolved.id
        );
        OpenRouterProvider::new()?;
        assert_eq!(AuthStatus::check().openrouter, AuthState::Available);

        std::fs::remove_file(env_file)?;
    }

    Ok(())
}

#[test]
fn provider_matrix_custom_compat_overrides_flow_into_runtime() -> Result<()> {
    let env = TestEnv::new()?;
    env.clear_profile_keys();

    kcode::env::set_var(
        "KCODE_OPENAI_COMPAT_API_BASE",
        "https://api.groq.com/openai/v1/",
    );
    kcode::env::set_var("KCODE_OPENAI_COMPAT_API_KEY_NAME", "GROQ_API_KEY");
    kcode::env::set_var("KCODE_OPENAI_COMPAT_ENV_FILE", "groq.env");
    kcode::env::set_var("KCODE_OPENAI_COMPAT_DEFAULT_MODEL", "openai/gpt-oss-120b");

    apply_openai_compatible_profile_env(Some(OPENAI_COMPAT_PROFILE));
    let resolved = resolve_openai_compatible_profile(OPENAI_COMPAT_PROFILE);
    let env_file = env.config_dir().join(&resolved.env_file);
    std::fs::write(
        &env_file,
        format!("{}=matrix-file-secret\n", resolved.api_key_env),
    )?;
    AuthStatus::invalidate_cache();

    assert_eq!(resolved.api_base, "https://api.groq.com/openai/v1");
    assert_eq!(resolved.api_key_env, "GROQ_API_KEY");
    assert_eq!(resolved.env_file, "groq.env");
    assert_eq!(
        std::env::var("KCODE_OPENROUTER_API_BASE").ok().as_deref(),
        Some("https://api.groq.com/openai/v1")
    );
    assert_eq!(
        std::env::var("KCODE_OPENROUTER_API_KEY_NAME")
            .ok()
            .as_deref(),
        Some("GROQ_API_KEY")
    );
    assert_eq!(
        std::env::var("KCODE_OPENROUTER_ENV_FILE").ok().as_deref(),
        Some("groq.env")
    );
    assert!(OpenRouterProvider::has_credentials());
    OpenRouterProvider::new()?;
    assert_eq!(AuthStatus::check().openrouter, AuthState::Available);

    Ok(())
}

#[test]
fn provider_matrix_custom_local_compat_without_api_key_activates_openrouter_runtime() -> Result<()>
{
    let env = TestEnv::new()?;
    env.clear_profile_keys();

    kcode::env::set_var("KCODE_OPENAI_COMPAT_API_BASE", "http://localhost:11434/v1");

    apply_openai_compatible_profile_env(Some(OPENAI_COMPAT_PROFILE));
    let resolved = resolve_openai_compatible_profile(OPENAI_COMPAT_PROFILE);
    AuthStatus::invalidate_cache();

    assert_eq!(resolved.api_base, "http://localhost:11434/v1");
    assert!(!resolved.requires_api_key);
    assert_eq!(
        std::env::var("KCODE_OPENROUTER_ALLOW_NO_AUTH")
            .ok()
            .as_deref(),
        Some("1")
    );
    assert!(OpenRouterProvider::has_credentials());
    OpenRouterProvider::new()?;
    assert_eq!(AuthStatus::check().openrouter, AuthState::Available);

    Ok(())
}
