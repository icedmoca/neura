use super::{App, DisplayMessage};
use crate::build::{BuildManifest, read_current_version};

pub(super) const UPDATE_REMOTE: &str = "https://github.com/icedmoca/neura.git";
pub(super) const UPDATE_BRANCH: &str = "main";

pub(super) fn handle_version_command(app: &mut App) {
    app.push_display_message(DisplayMessage::system(version_message()));
}

pub(super) fn handle_update_command(app: &mut App) {
    app.push_display_message(DisplayMessage::system(format!(
        "Starting background update from `{}` branch `{}`. I’ll build a reloadable binary when it finishes.",
        UPDATE_REMOTE, UPDATE_BRANCH
    )));

    std::thread::spawn(|| {
        let outcome = run_update();
        match outcome {
            Ok(message) => crate::logging::info(&format!("/update completed: {message}")),
            Err(err) => crate::logging::error(&format!("/update failed: {err}")),
        }
    });
}

pub(super) fn handle_version_or_update_command(app: &mut App, trimmed: &str) -> bool {
    match trimmed {
        "/version" => {
            handle_version_command(app);
            true
        }
        "/update" => {
            handle_update_command(app);
            true
        }
        _ => false,
    }
}

pub(super) fn version_message() -> String {
    let candidate = BuildManifest::load()
        .ok()
        .and_then(|manifest| {
            manifest
                .history
                .first()
                .map(|info| format!("{} built {}", info.hash, info.built_at))
        })
        .unwrap_or_else(|| "none".to_string());
    format!(
        "Neura {}\nBinary: {}\nReload candidate: {}",
        read_current_version()
            .ok()
            .flatten()
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string()),
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "unknown".to_string()),
        candidate
    )
}

pub(super) fn run_update() -> anyhow::Result<String> {
    let repo = std::env::current_dir()?;
    Ok(crate::update::run_source_update(&repo)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_points_at_requested_remote_and_branch() {
        assert_eq!(UPDATE_REMOTE, "https://github.com/icedmoca/neura.git");
        assert_eq!(UPDATE_BRANCH, "main");
    }

    #[test]
    fn version_message_mentions_reload_candidate() {
        let msg = version_message();
        assert!(msg.contains("Neura"));
        assert!(msg.contains("Reload candidate"));
    }
}
