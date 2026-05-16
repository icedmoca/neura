use super::{App, DisplayMessage};
use crate::build::{
    BuildManifest, current_source_state, publish_local_current_build_for_source,
    read_current_version,
};
use std::path::Path;
use std::process::Command;

pub(super) const UPDATE_REMOTE: &str = "https://github.com/icedmoca/kcode.git";
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
        "Kcode {}\nBinary: {}\nReload candidate: {}",
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
    run_git(&repo, &["fetch", UPDATE_REMOTE, UPDATE_BRANCH])?;
    run_git(&repo, &["checkout", UPDATE_BRANCH])?;
    run_git(&repo, &["pull", "--ff-only", UPDATE_REMOTE, UPDATE_BRANCH])?;

    let status = Command::new("cargo")
        .arg("build")
        .arg("--release")
        .current_dir(&repo)
        .status()?;
    anyhow::ensure!(status.success(), "cargo build --release failed");

    publish_reload_binary(&repo)?;
    Ok("update built and published; run /reload to switch to it".to_string())
}

fn run_git(repo: &Path, args: &[&str]) -> anyhow::Result<()> {
    let status = Command::new("git").args(args).current_dir(repo).status()?;
    anyhow::ensure!(status.success(), "git {} failed", args.join(" "));
    Ok(())
}

fn publish_reload_binary(repo: &Path) -> anyhow::Result<()> {
    let source = current_source_state(repo)?;
    let _info = publish_local_current_build_for_source(&repo.to_path_buf(), &source)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_points_at_requested_remote_and_branch() {
        assert_eq!(UPDATE_REMOTE, "https://github.com/icedmoca/kcode.git");
        assert_eq!(UPDATE_BRANCH, "main");
    }

    #[test]
    fn version_message_mentions_reload_candidate() {
        let msg = version_message();
        assert!(msg.contains("Kcode"));
        assert!(msg.contains("Reload candidate"));
    }
}
