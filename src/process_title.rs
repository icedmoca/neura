use crate::cli::args::{AmbientCommand, Args, Command};

const LINUX_PROCESS_TITLE_LIMIT: usize = 15;
const KILLALL_PROCESS_NAME: &str = "neura";

fn compact_process_title(prefix: &str, name: Option<&str>) -> String {
    let mut title = prefix.to_string();
    if let Some(name) = name.filter(|name| !name.is_empty()) {
        let remaining = LINUX_PROCESS_TITLE_LIMIT.saturating_sub(title.len());
        if remaining > 0 {
            title.push_str(&name.chars().take(remaining).collect::<String>());
        }
    }
    title
}

fn session_name(session_id: &str) -> String {
    crate::id::extract_session_name(session_id)
        .map(|name| name.to_string())
        .unwrap_or_else(|| session_id.to_string())
}

pub(crate) fn set_title(title: impl AsRef<str>) {
    proctitle::set_title(title.as_ref());
    set_killall_process_name();
}

fn set_killall_process_name() {
    #[cfg(target_os = "linux")]
    unsafe {
        let mut name = [0u8; 16];
        let bytes = KILLALL_PROCESS_NAME.as_bytes();
        let len = bytes.len().min(name.len().saturating_sub(1));
        name[..len].copy_from_slice(&bytes[..len]);
        let _ = libc::prctl(libc::PR_SET_NAME, name.as_ptr(), 0, 0, 0);
    }
}

pub(crate) fn set_server_title(server_name: &str) {
    set_title(compact_process_title("neura:s:", Some(server_name)));
}

pub(crate) fn set_client_generic_title(is_selfdev: bool) {
    let prefix = if is_selfdev {
        "neura:selfdev"
    } else {
        "neura:client"
    };
    set_title(compact_process_title(prefix, None));
}

pub(crate) fn set_client_session_title(session_id: &str, is_selfdev: bool) {
    set_client_display_title(&session_name(session_id), is_selfdev);
}

pub(crate) fn set_client_display_title(session_name: &str, is_selfdev: bool) {
    let prefix = if is_selfdev { "neura:d:" } else { "neura:c:" };
    set_title(compact_process_title(prefix, Some(session_name)));
}

pub(crate) fn set_client_remote_display_title(
    server_name: &str,
    session_name: &str,
    is_selfdev: bool,
) {
    if server_name.is_empty() || server_name.eq_ignore_ascii_case("neura") {
        set_client_display_title(session_name, is_selfdev);
        return;
    }
    let prefix = if is_selfdev { "neura:d:" } else { "neura:c:" };
    set_title(format!("{prefix}{server_name}/{session_name}"));
}

pub(crate) fn initial_title(args: &Args) -> String {
    match &args.command {
        Some(Command::Serve) => "neura:server".to_string(),
        Some(Command::CodebaseModel { .. }) => "codebase-model".to_string(),
        Some(Command::Connect) => "neura:client".to_string(),
        Some(Command::Run { .. }) => "neura run".to_string(),
        Some(Command::Login { .. }) => "neura login".to_string(),
        Some(Command::Repl) => "neura repl".to_string(),
        Some(Command::Update) => "neura update".to_string(),
        Some(Command::Version { .. }) => "neura version".to_string(),
        Some(Command::Usage { .. }) => "neura usage".to_string(),
        Some(Command::SelfDev { .. }) => "neura:selfdev".to_string(),
        Some(Command::Debug { .. }) => "neura debug".to_string(),
        Some(Command::Auth(_)) => "neura auth".to_string(),
        Some(Command::Provider(_)) => "neura provider".to_string(),
        Some(Command::Memory(_)) => "neura memory".to_string(),
        Some(Command::Knowledge(_)) => "neura knowledge".to_string(),
        Some(Command::Latent(_)) => "neura latent".to_string(),
        Some(Command::SelfImprove(_)) => "neura self-improve".to_string(),
        Some(Command::Ambient(subcommand)) => match subcommand {
            AmbientCommand::RunVisible => "neura ambient visible".to_string(),
            _ => "neura ambient".to_string(),
        },
        Some(Command::Pair { .. }) => "neura pair".to_string(),
        Some(Command::Permissions) => "neura permissions".to_string(),
        Some(Command::Transcript { .. }) => "neura transcript".to_string(),
        Some(Command::Dictate { .. }) => "neura dictate".to_string(),
        Some(Command::SetupHotkey {
            listen_macos_hotkey,
        }) => {
            if *listen_macos_hotkey {
                "neura hotkey listener".to_string()
            } else {
                "neura hotkey setup".to_string()
            }
        }
        Some(Command::Browser { .. }) => "neura browser".to_string(),
        Some(Command::Replay { .. }) => "neura replay".to_string(),
        Some(Command::Model(_)) => "neura model".to_string(),
        Some(Command::AuthTest { .. }) => "neura auth-test".to_string(),
        Some(Command::Restart { .. }) => "neura restart".to_string(),
        Some(Command::SetupLauncher) => "neura setup-launcher".to_string(),
        None => {
            if let Some(resume) = args.resume.as_deref().filter(|resume| !resume.is_empty()) {
                let prefix = if crate::cli::selfdev::client_selfdev_requested() {
                    "neura:d:"
                } else {
                    "neura:c:"
                };
                compact_process_title(prefix, Some(&session_name(resume)))
            } else if crate::cli::selfdev::client_selfdev_requested() {
                "neura:selfdev".to_string()
            } else {
                "neura:client".to_string()
            }
        }
    }
}

pub(crate) fn set_initial_title(args: &Args) {
    set_title(initial_title(args));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::Args;
    use crate::storage::lock_test_env;
    use clap::Parser;

    const SELFDEV_ENV: &str = crate::cli::selfdev::CLIENT_SELFDEV_ENV;

    fn with_selfdev_env_removed<T>(f: impl FnOnce() -> T) -> T {
        let _guard = lock_test_env();
        let previous = std::env::var_os(SELFDEV_ENV);
        crate::env::remove_var(SELFDEV_ENV);
        let result = f();
        if let Some(value) = previous {
            crate::env::set_var(SELFDEV_ENV, value);
        }
        result
    }

    #[test]
    fn initial_title_labels_server() {
        with_selfdev_env_removed(|| {
            let args = Args::parse_from(["neura", "serve"]);
            assert_eq!(initial_title(&args), "neura:server");
        });
    }

    #[test]
    fn initial_title_labels_resume_client_with_short_name() {
        with_selfdev_env_removed(|| {
            let args = Args::parse_from(["neura", "--resume", "session_fox_123"]);
            assert_eq!(initial_title(&args), "neura:c:fox");
        });
    }

    #[test]
    fn initial_title_labels_selfdev_command() {
        with_selfdev_env_removed(|| {
            let args = Args::parse_from(["neura", "self-dev"]);
            assert_eq!(initial_title(&args), "neura:selfdev");
        });
    }
}
