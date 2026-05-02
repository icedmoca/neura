use super::wait_for_reloading_server;
use crate::build;
use crate::{provider, session, storage, tool};
use std::ffi::OsString;
use std::sync::Arc;

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    storage::lock_test_env()
}

struct TestEnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    prev_home: Option<OsString>,
    prev_test_session: Option<OsString>,
    _temp_home: tempfile::TempDir,
}

impl TestEnvGuard {
    fn new() -> anyhow::Result<Self> {
        let lock = lock_env();
        let temp_home = tempfile::Builder::new()
            .prefix("kcode-selfdev-test-home-")
            .tempdir()?;
        let prev_home = std::env::var_os("KCODE_HOME");
        let prev_test_session = std::env::var_os("KCODE_TEST_SESSION");

        crate::env::set_var("KCODE_HOME", temp_home.path());
        crate::env::set_var("KCODE_TEST_SESSION", "1");

        Ok(Self {
            _lock: lock,
            prev_home,
            prev_test_session,
            _temp_home: temp_home,
        })
    }
}

impl Drop for TestEnvGuard {
    fn drop(&mut self) {
        if let Some(prev_home) = &self.prev_home {
            crate::env::set_var("KCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("KCODE_HOME");
        }

        if let Some(prev_test_session) = &self.prev_test_session {
            crate::env::set_var("KCODE_TEST_SESSION", prev_test_session);
        } else {
            crate::env::remove_var("KCODE_TEST_SESSION");
        }
    }
}

fn setup_test_env() -> TestEnvGuard {
    TestEnvGuard::new().expect("failed to setup isolated test environment")
}

struct TestProvider;

#[async_trait::async_trait]
impl provider::Provider for TestProvider {
    fn name(&self) -> &str {
        "test"
    }

    fn model(&self) -> String {
        "test".to_string()
    }

    fn available_models(&self) -> Vec<&'static str> {
        vec![]
    }

    fn available_models_display(&self) -> Vec<String> {
        vec![]
    }

    async fn prefetch_models(&self) -> anyhow::Result<()> {
        Ok(())
    }

    fn set_model(&self, _model: &str) -> anyhow::Result<()> {
        Ok(())
    }

    fn handles_tools_internally(&self) -> bool {
        false
    }

    async fn complete(
        &self,
        _messages: &[crate::message::Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _session_id: Option<&str>,
    ) -> anyhow::Result<crate::provider::EventStream> {
        Err(anyhow::anyhow!(
            "TestProvider should not be used for streaming completions in selfdev tests"
        ))
    }

    fn fork(&self) -> Arc<dyn provider::Provider> {
        Arc::new(TestProvider)
    }
}

#[tokio::test]
async fn test_selfdev_tool_registration() {
    let _env = setup_test_env();

    let mut session = session::Session::create(None, Some("Test".to_string()));
    session.set_canary("test");
    assert!(session.is_canary, "Session should be marked as canary");

    let provider = Arc::new(TestProvider) as Arc<dyn provider::Provider>;
    let registry = tool::Registry::new(provider).await;

    let tools_before: Vec<String> = registry.tool_names().await;
    let has_selfdev_before = tools_before.contains(&"selfdev".to_string());

    registry.register_selfdev_tools().await;

    let tools_after: Vec<String> = registry.tool_names().await;
    let has_selfdev_after = tools_after.contains(&"selfdev".to_string());

    println!(
        "Before: selfdev={}, tools={:?}",
        has_selfdev_before,
        tools_before.len()
    );
    println!(
        "After: selfdev={}, tools={:?}",
        has_selfdev_after,
        tools_after.len()
    );

    assert!(has_selfdev_after, "selfdev should be registered");
}

#[tokio::test]
async fn test_selfdev_session_and_registry() {
    let _env = setup_test_env();

    let mut session = session::Session::create(None, Some("Test E2E".to_string()));
    session.set_canary("test-build");
    let session_id = session.id.clone();
    session.save().expect("Failed to save session");

    let loaded = session::Session::load(&session_id).expect("Failed to load session");
    assert!(loaded.is_canary, "Loaded session should be canary");

    let provider = Arc::new(TestProvider) as Arc<dyn provider::Provider>;
    let registry = tool::Registry::new(provider.clone()).await;

    let tools_before = registry.tool_names().await;
    assert!(
        tools_before.contains(&"selfdev".to_string()),
        "selfdev should be available in all sessions initially"
    );

    registry.register_selfdev_tools().await;

    let tools_after = registry.tool_names().await;
    assert!(
        tools_after.contains(&"selfdev".to_string()),
        "selfdev SHOULD be registered after register_selfdev_tools"
    );

    let ctx = tool::ToolContext {
        session_id: session_id.clone(),
        message_id: "test".to_string(),
        tool_call_id: "test".to_string(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: tool::ToolExecutionMode::Direct,
    };
    let result = registry
        .execute("selfdev", serde_json::json!({"action": "status"}), ctx)
        .await;

    println!("selfdev status result: {:?}", result);
    assert!(result.is_ok(), "selfdev tool should execute successfully");

    let _ = std::fs::remove_file(
        storage::kcode_dir()
            .unwrap()
            .join("sessions")
            .join(format!("{}.json", session_id)),
    );
}

#[tokio::test]
async fn test_wait_for_reloading_server_returns_false_when_reload_failed() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_socket = std::env::var_os("KCODE_SOCKET");
    let prev_runtime = std::env::var_os("KCODE_RUNTIME_DIR");
    let socket_path = temp.path().join("kcode.sock");
    crate::server::set_socket_path(socket_path.to_str().expect("utf8 socket path"));
    crate::env::set_var("KCODE_RUNTIME_DIR", temp.path());
    crate::server::write_reload_state(
        "reload-test",
        "hash",
        crate::server::ReloadPhase::Failed,
        Some("boom".to_string()),
    );

    assert!(!wait_for_reloading_server().await);

    crate::server::clear_reload_marker();
    if let Some(prev_socket) = prev_socket {
        crate::env::set_var("KCODE_SOCKET", prev_socket);
    } else {
        crate::env::remove_var("KCODE_SOCKET");
    }
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("KCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("KCODE_RUNTIME_DIR");
    }
}

#[tokio::test]
async fn test_wait_for_reloading_server_returns_true_for_live_listener() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_socket = std::env::var_os("KCODE_SOCKET");
    let prev_runtime = std::env::var_os("KCODE_RUNTIME_DIR");
    let socket_path = temp.path().join("kcode.sock");
    crate::server::set_socket_path(socket_path.to_str().expect("utf8 socket path"));
    crate::env::set_var("KCODE_RUNTIME_DIR", temp.path());
    let _listener = crate::transport::Listener::bind(&socket_path).expect("bind listener");

    assert!(wait_for_reloading_server().await);

    if let Some(prev_socket) = prev_socket {
        crate::env::set_var("KCODE_SOCKET", prev_socket);
    } else {
        crate::env::remove_var("KCODE_SOCKET");
    }
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("KCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("KCODE_RUNTIME_DIR");
    }
}

#[test]
fn test_selfdev_build_command_prefers_repo_wrapper_when_present() {
    let temp = tempfile::tempdir().expect("tempdir");
    let scripts_dir = temp.path().join("scripts");
    std::fs::create_dir_all(&scripts_dir).expect("create scripts dir");
    std::fs::write(scripts_dir.join("dev_cargo.sh"), "#!/usr/bin/env bash\n")
        .expect("write wrapper");

    let build = build::selfdev_build_command(temp.path());
    assert_eq!(build.program, "bash");
    assert_eq!(
        build.args,
        vec![
            temp.path()
                .join("scripts")
                .join("dev_cargo.sh")
                .to_string_lossy()
                .into_owned(),
            "build".to_string(),
            "--profile".to_string(),
            "selfdev".to_string(),
            "-p".to_string(),
            "kcode".to_string(),
            "--bin".to_string(),
            "kcode".to_string(),
        ]
    );
}

#[test]
fn test_selfdev_build_command_falls_back_to_cargo_when_wrapper_missing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let build = build::selfdev_build_command(temp.path());
    assert_eq!(build.program, "cargo");
    assert_eq!(
        build.args,
        vec![
            "build".to_string(),
            "--profile".to_string(),
            "selfdev".to_string(),
            "-p".to_string(),
            "kcode".to_string(),
            "--bin".to_string(),
            "kcode".to_string(),
        ]
    );
}
