use crate::build;
use crate::bus::{Bus, BusEvent, ClientMaintenanceAction, SessionUpdateStatus};
use crate::storage;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

const GITHUB_REPO: &str = "1jehuang/neura";
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(60); // minimum gap between checks
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(5);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
const BACKGROUND_UPDATE_THRESHOLD: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct UpdateEstimate {
    pub duration: Duration,
    pub summary: String,
    pub should_background: bool,
}

pub enum PreparedUpdate {
    None {
        current: String,
    },
    Stable {
        release: GitHubRelease,
        estimate: UpdateEstimate,
    },
    MainSource {
        latest_sha: String,
        estimate: UpdateEstimate,
    },
}

pub fn print_centered(msg: &str) {
    let width = crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80);
    for line in msg.lines() {
        let visible_len = unicode_display_width(line);
        if visible_len >= width {
            println!("{}", line);
        } else {
            let pad = (width - visible_len) / 2;
            println!("{:>pad$}{}", "", line, pad = pad);
        }
    }
}

fn unicode_display_width(s: &str) -> usize {
    use unicode_width::UnicodeWidthChar;
    let mut w = 0;
    let mut in_escape = false;
    for c in s.chars() {
        if in_escape {
            if c == 'm' {
                in_escape = false;
            }
            continue;
        }
        if c == '\x1b' {
            in_escape = true;
            continue;
        }
        w += UnicodeWidthChar::width(c).unwrap_or(0);
    }
    w
}

pub fn is_release_build() -> bool {
    option_env!("NEURA_RELEASE_BUILD").is_some()
}

fn current_update_semver() -> &'static str {
    env!("NEURA_UPDATE_SEMVER")
}

#[derive(Debug, Clone, Deserialize)]
pub struct GitHubRelease {
    pub tag_name: String,
    #[serde(rename = "name")]
    pub _name: Option<String>,
    #[serde(rename = "html_url")]
    pub _html_url: String,
    #[serde(rename = "published_at")]
    pub _published_at: Option<String>,
    pub assets: Vec<GitHubAsset>,
    #[serde(default)]
    #[serde(rename = "target_commitish")]
    pub _target_commitish: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GitHubAsset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(rename = "size")]
    pub _size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateMetadata {
    pub last_check: SystemTime,
    pub installed_version: Option<String>,
    pub installed_from: Option<String>,
    #[serde(default)]
    pub last_release_update_secs: Option<f64>,
    #[serde(default)]
    pub last_source_update_secs: Option<f64>,
}

impl Default for UpdateMetadata {
    fn default() -> Self {
        Self {
            last_check: SystemTime::UNIX_EPOCH,
            installed_version: None,
            installed_from: None,
            last_release_update_secs: None,
            last_source_update_secs: None,
        }
    }
}

impl UpdateMetadata {
    pub fn load() -> Result<Self> {
        let path = metadata_path()?;
        if path.exists() {
            let content = fs::read_to_string(&path)?;
            Ok(serde_json::from_str(&content)?)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = metadata_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        fs::write(&path, content)?;
        Ok(())
    }

    pub fn should_check(&self) -> bool {
        match self.last_check.elapsed() {
            Ok(elapsed) => elapsed > UPDATE_CHECK_INTERVAL,
            Err(_) => true,
        }
    }
}

fn metadata_path() -> Result<PathBuf> {
    Ok(storage::neura_dir()?.join("update_metadata.json"))
}

fn source_build_root() -> Result<PathBuf> {
    Ok(storage::neura_dir()?.join("builds").join("source"))
}

fn source_build_repo_dir() -> Result<PathBuf> {
    Ok(source_build_root()?.join("neura"))
}

fn record_release_update_duration(duration: Duration) {
    if let Ok(mut metadata) = UpdateMetadata::load() {
        metadata.last_release_update_secs = Some(duration.as_secs_f64());
        let _ = metadata.save();
    }
}

fn record_source_update_duration(duration: Duration) {
    if let Ok(mut metadata) = UpdateMetadata::load() {
        metadata.last_source_update_secs = Some(duration.as_secs_f64());
        let _ = metadata.save();
    }
}

fn format_duration_estimate(duration: Duration) -> String {
    match duration.as_secs() {
        0..=15 => "under 15s".to_string(),
        16..=45 => "~30s".to_string(),
        46..=90 => "~1 min".to_string(),
        91..=180 => "~2-3 min".to_string(),
        181..=360 => "~3-6 min".to_string(),
        _ => "5+ min".to_string(),
    }
}

fn estimate_release_update_duration(
    asset_size_bytes: u64,
    historical_secs: Option<f64>,
) -> Duration {
    if let Some(previous) = historical_secs {
        return Duration::from_secs(previous.max(5.0).round() as u64);
    }

    let size_mb = asset_size_bytes as f64 / (1024.0 * 1024.0);
    let secs = if size_mb <= 15.0 {
        10
    } else if size_mb <= 35.0 {
        20
    } else if size_mb <= 60.0 {
        35
    } else {
        50
    };
    Duration::from_secs(secs)
}

fn estimate_source_update_duration(
    repo_exists: bool,
    has_previous_build: bool,
    historical_secs: Option<f64>,
) -> Duration {
    if let Some(previous) = historical_secs {
        return Duration::from_secs(previous.max(20.0).round() as u64);
    }

    let secs = if !repo_exists {
        420
    } else if has_previous_build {
        90
    } else {
        180
    };
    Duration::from_secs(secs)
}

fn update_estimate(summary: String, duration: Duration) -> UpdateEstimate {
    UpdateEstimate {
        duration,
        summary,
        should_background: duration >= BACKGROUND_UPDATE_THRESHOLD,
    }
}

fn get_asset_name() -> &'static str {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "neura-linux-x86_64"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "neura-linux-aarch64"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "neura-macos-x86_64"
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "neura-macos-aarch64"
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        "neura-windows-x86_64.exe"
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        "neura-windows-aarch64.exe"
    }
    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "aarch64"),
    )))]
    {
        "neura-unknown"
    }
}

pub fn should_auto_update() -> bool {
    if std::env::var("NEURA_NO_AUTO_UPDATE").is_ok() {
        return false;
    }

    if !is_release_build() {
        return false;
    }

    if let Ok(exe) = std::env::current_exe()
        && is_inside_git_repo(&exe)
    {
        return false;
    }

    true
}

fn summarize_git_pull_failure(stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let text = stderr.trim();
    if text.is_empty() {
        return "git pull failed".to_string();
    }

    if text.contains("Need to specify how to reconcile divergent branches")
        || text.contains("Not possible to fast-forward")
        || text.contains("refusing to merge unrelated histories")
    {
        return "git pull requires manual reconciliation (local and upstream have diverged)"
            .to_string();
    }

    if text.contains("There is no tracking information for the current branch") {
        return "git pull failed: current branch has no upstream tracking branch".to_string();
    }

    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("hint:"))
        .unwrap_or("git pull failed");
    let line = line.strip_prefix("fatal: ").unwrap_or(line);
    if line.eq_ignore_ascii_case("git pull failed") {
        "git pull failed".to_string()
    } else {
        format!("git pull failed: {}", line)
    }
}

pub fn run_git_pull_ff_only(repo_dir: &Path, quiet: bool) -> Result<()> {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("pull").arg("--ff-only");
    if quiet {
        cmd.arg("-q");
    }
    let output = cmd
        .current_dir(repo_dir)
        .output()
        .context("Failed to run git pull")?;

    if output.status.success() {
        Ok(())
    } else {
        anyhow::bail!("{}", summarize_git_pull_failure(&output.stderr));
    }
}

fn is_inside_git_repo(path: &std::path::Path) -> bool {
    let mut dir = if path.is_dir() {
        Some(path)
    } else {
        path.parent()
    };

    while let Some(d) = dir {
        if d.join(".git").exists() {
            return true;
        }
        dir = d.parent();
    }
    false
}

pub fn fetch_latest_release_blocking() -> Result<GitHubRelease> {
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        GITHUB_REPO
    );

    let client = reqwest::blocking::Client::builder()
        .timeout(UPDATE_CHECK_TIMEOUT)
        .user_agent("neura-updater")
        .build()?;

    let response = client
        .get(&url)
        .send()
        .context("Failed to fetch release info")?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        anyhow::bail!("No releases found");
    }

    if !response.status().is_success() {
        anyhow::bail!("GitHub API error: {}", response.status());
    }

    let release: GitHubRelease = response.json().context("Failed to parse release info")?;

    Ok(release)
}

fn latest_main_sha_blocking() -> Result<String> {
    let url = format!("https://api.github.com/repos/{}/commits/main", GITHUB_REPO);
    let client = reqwest::blocking::Client::builder()
        .timeout(UPDATE_CHECK_TIMEOUT)
        .user_agent("neura-updater")
        .build()?;

    let response = client
        .get(&url)
        .send()
        .context("Failed to check main branch")?;
    if !response.status().is_success() {
        anyhow::bail!("GitHub API error checking main: {}", response.status());
    }

    let commit: serde_json::Value = response.json().context("Failed to parse commit info")?;
    Ok(commit["sha"]
        .as_str()
        .unwrap_or("")
        .get(..7)
        .unwrap_or("")
        .to_string())
}

fn platform_asset(release: &GitHubRelease) -> Result<&GitHubAsset> {
    let asset_name = get_asset_name();
    release
        .assets
        .iter()
        .find(|a| a.name.starts_with(asset_name))
        .ok_or_else(|| anyhow::anyhow!("No asset found for platform: {}", asset_name))
}

fn synthetic_main_release(latest_sha: &str) -> GitHubRelease {
    GitHubRelease {
        tag_name: format!("main-{}", latest_sha),
        _name: Some(format!("Built from main ({})", latest_sha)),
        _html_url: format!("https://github.com/{}/commit/{}", GITHUB_REPO, latest_sha),
        _published_at: None,
        assets: vec![],
        _target_commitish: latest_sha.to_string(),
    }
}

fn install_main_source_update_blocking(latest_sha: &str) -> Result<PathBuf> {
    let path = build_from_source()?;
    crate::logging::info(&format!(
        "Main channel: built successfully at {}",
        path.display()
    ));

    let mut metadata = UpdateMetadata::load().unwrap_or_default();
    let channel_version = format!("main-{}", latest_sha);
    build::install_binary_at_version(&path, &channel_version)
        .context("Failed to install built binary")?;
    build::update_stable_symlink(&channel_version)?;
    build::update_current_symlink(&channel_version)?;
    build::update_launcher_symlink_to_current()?;

    metadata.installed_version = Some(channel_version.clone());
    metadata.installed_from = Some("source".to_string());
    metadata.last_check = SystemTime::now();
    metadata.save()?;

    Ok(path)
}

fn prepare_stable_update_blocking() -> Result<PreparedUpdate> {
    let current_version = env!("NEURA_VERSION");
    let current_update_version = current_update_semver();
    let release = fetch_latest_release_blocking()?;
    let release_version = release.tag_name.trim_start_matches('v');

    if release_version == current_update_version.trim_start_matches('v')
        || !version_is_newer(
            release_version,
            current_update_version.trim_start_matches('v'),
        )
    {
        return Ok(PreparedUpdate::None {
            current: current_version.to_string(),
        });
    }

    let Ok(asset) = platform_asset(&release) else {
        return Ok(PreparedUpdate::None {
            current: current_version.to_string(),
        });
    };
    let metadata = UpdateMetadata::load().unwrap_or_default();
    let duration = estimate_release_update_duration(asset._size, metadata.last_release_update_secs);
    let size_mb = asset._size as f64 / (1024.0 * 1024.0);
    let summary = format!(
        "Prebuilt update {} → {} (~{:.0} MB, {}). {}",
        current_version,
        release.tag_name,
        size_mb,
        format_duration_estimate(duration),
        if duration >= BACKGROUND_UPDATE_THRESHOLD {
            "Running in the background and will reload when it is ready."
        } else {
            "This should be quick."
        }
    );

    Ok(PreparedUpdate::Stable {
        release,
        estimate: update_estimate(summary, duration),
    })
}

fn prepare_main_update_blocking() -> Result<PreparedUpdate> {
    let current_hash = env!("NEURA_GIT_HASH");
    if current_hash.is_empty() || current_hash == "unknown" {
        crate::logging::info("Main channel: no git hash in binary, skipping update check");
        return Ok(PreparedUpdate::None {
            current: env!("NEURA_VERSION").to_string(),
        });
    }

    let latest_sha = latest_main_sha_blocking()?;
    if latest_sha.is_empty() {
        return Ok(PreparedUpdate::None {
            current: current_hash.to_string(),
        });
    }

    let current_short = if current_hash.len() >= 7 {
        &current_hash[..7]
    } else {
        current_hash
    };

    if current_short == latest_sha {
        crate::logging::info(&format!("Main channel: up to date ({})", current_short));
        return Ok(PreparedUpdate::None {
            current: format!("main-{}", current_short),
        });
    }

    crate::logging::info(&format!(
        "Main channel: new commit {} -> {}",
        current_short, latest_sha
    ));

    if has_cargo() {
        let repo_dir = source_build_repo_dir()?;
        let repo_exists = repo_dir.join(".git").exists();
        let has_previous_build = build::release_binary_path(&repo_dir).exists();
        let metadata = UpdateMetadata::load().unwrap_or_default();
        let duration = estimate_source_update_duration(
            repo_exists,
            has_previous_build,
            metadata.last_source_update_secs,
        );
        let action = if repo_exists {
            if has_previous_build {
                "git pull + cargo build with a warm build cache"
            } else {
                "git pull + cargo build"
            }
        } else {
            "initial clone + cargo build"
        };
        let summary = format!(
            "Source update {} → main-{} requires {} ({}). Running in the background and will reload when it is ready.",
            current_short,
            latest_sha,
            action,
            format_duration_estimate(duration)
        );
        return Ok(PreparedUpdate::MainSource {
            latest_sha,
            estimate: update_estimate(summary, duration),
        });
    }

    crate::logging::info("Main channel: cargo not found, falling back to latest release");
    prepare_stable_update_blocking()
}

pub fn prepare_update_blocking() -> Result<PreparedUpdate> {
    let channel = crate::config::config().features.update_channel;
    match channel {
        crate::config::UpdateChannel::Main => prepare_main_update_blocking(),
        crate::config::UpdateChannel::Stable => prepare_stable_update_blocking(),
    }
}

pub const SOURCE_UPDATE_REMOTE: &str = "https://github.com/icedmoca/neura.git";
pub const SOURCE_UPDATE_BRANCH: &str = "main";

pub fn run_source_update(repo: &Path) -> Result<String> {
    run_git(repo, &["fetch", SOURCE_UPDATE_REMOTE, SOURCE_UPDATE_BRANCH])?;
    run_git(repo, &["checkout", SOURCE_UPDATE_BRANCH])?;
    run_git(
        repo,
        &[
            "pull",
            "--ff-only",
            SOURCE_UPDATE_REMOTE,
            SOURCE_UPDATE_BRANCH,
        ],
    )?;

    let status = Command::new("cargo")
        .arg("build")
        .arg("--release")
        .arg("--bin")
        .arg("neura")
        .current_dir(repo)
        .status()?;
    anyhow::ensure!(status.success(), "cargo build --release --bin neura failed");

    let path = build::publish_local_current_build(repo)?;
    Ok(format!(
        "source update built and published to {}; run /reload to switch to it",
        path.display()
    ))
}

fn run_git(repo: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new("git").args(args).current_dir(repo).status()?;
    anyhow::ensure!(status.success(), "git {} failed", args.join(" "));
    Ok(())
}

pub fn spawn_background_session_update(session_id: String) {
    std::thread::spawn(move || {
        let action = ClientMaintenanceAction::Update;
        let publish = |status| Bus::global().publish(BusEvent::SessionUpdateStatus(status));
        publish(SessionUpdateStatus::Status {
            session_id: session_id.clone(),
            action,
            message: format!(
                "pulling {} {} and building in the background",
                SOURCE_UPDATE_REMOTE, SOURCE_UPDATE_BRANCH
            ),
        });

        let repo = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
        match run_source_update(&repo) {
            Ok(message) => {
                publish(SessionUpdateStatus::Status {
                    session_id: session_id.clone(),
                    action,
                    message,
                });
                publish(SessionUpdateStatus::ReadyToReload {
                    session_id,
                    action,
                    version: SOURCE_UPDATE_BRANCH.to_string(),
                });
            }
            Err(err) => publish(SessionUpdateStatus::Error {
                session_id,
                action,
                message: err.to_string(),
            }),
        }
    });
}

pub fn check_for_update_blocking() -> Result<Option<GitHubRelease>> {
    let channel = crate::config::config().features.update_channel;
    match channel {
        crate::config::UpdateChannel::Main => check_for_main_update_blocking(),
        crate::config::UpdateChannel::Stable => check_for_stable_update_blocking(),
    }
}

fn check_for_stable_update_blocking() -> Result<Option<GitHubRelease>> {
    let current_version = current_update_semver();
    let release = fetch_latest_release_blocking()?;

    let release_version = release.tag_name.trim_start_matches('v');
    if release_version == current_version.trim_start_matches('v') {
        return Ok(None);
    }

    if version_is_newer(release_version, current_version.trim_start_matches('v')) {
        let asset_name = get_asset_name();
        let has_asset = release
            .assets
            .iter()
            .any(|a| a.name.starts_with(asset_name));

        if has_asset {
            return Ok(Some(release));
        }
    }

    Ok(None)
}

/// Check for updates on the main branch (cutting edge channel).
/// Compares the current binary's git hash against the latest commit on main.
/// If a new commit is found:
///   - Tries to build from source if cargo is available
///   - Falls back to latest GitHub Release if not
fn check_for_main_update_blocking() -> Result<Option<GitHubRelease>> {
    let current_hash = env!("NEURA_GIT_HASH");
    if current_hash.is_empty() || current_hash == "unknown" {
        crate::logging::info("Main channel: no git hash in binary, skipping update check");
        return Ok(None);
    }

    let latest_sha = latest_main_sha_blocking()?;

    if latest_sha.is_empty() {
        return Ok(None);
    }

    // Compare short hashes
    let current_short = if current_hash.len() >= 7 {
        &current_hash[..7]
    } else {
        current_hash
    };

    if current_short == latest_sha {
        crate::logging::info(&format!("Main channel: up to date ({})", current_short));
        return Ok(None);
    }

    crate::logging::info(&format!(
        "Main channel: new commit {} -> {}",
        current_short, latest_sha
    ));

    // Try to build from source
    if has_cargo() {
        crate::logging::info("Main channel: cargo found, attempting build from source");
        match install_main_source_update_blocking(&latest_sha) {
            Ok(_) => {
                return Ok(Some(synthetic_main_release(&latest_sha)));
            }
            Err(e) => {
                crate::logging::error(&format!("Main channel: build failed: {}", e));
                // Fall through to release fallback
            }
        }
    } else {
        crate::logging::info("Main channel: cargo not found, falling back to latest release");
    }

    // Fallback: use latest stable release if available
    if let Ok(release) = fetch_latest_release_blocking() {
        let asset_name = get_asset_name();
        let has_asset = release
            .assets
            .iter()
            .any(|a| a.name.starts_with(asset_name));
        if has_asset {
            let release_version = release.tag_name.trim_start_matches('v');
            let current_version = current_update_semver().trim_start_matches('v');
            if version_is_newer(release_version, current_version) {
                return Ok(Some(release));
            }
        }
    }

    Ok(None)
}

/// Check if cargo is available on the system
fn has_cargo() -> bool {
    std::process::Command::new("cargo")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Build neura from source by cloning/pulling the repo and running cargo build
fn build_from_source() -> Result<PathBuf> {
    let started = Instant::now();
    let build_dir = source_build_root()?;
    fs::create_dir_all(&build_dir)?;

    let repo_dir = build_dir.join("neura");

    if repo_dir.join(".git").exists() {
        // Pull latest
        crate::logging::info("Main channel: pulling latest from main...");
        let output = std::process::Command::new("git")
            .args(["pull", "--ff-only", "origin", "main"])
            .current_dir(&repo_dir)
            .output()
            .context("Failed to run git pull")?;

        if !output.status.success() {
            // If pull fails (e.g. diverged), reset to origin/main
            let summary = summarize_git_pull_failure(&output.stderr);
            crate::logging::warn(&format!("{}, trying reset", summary));
            let output = std::process::Command::new("git")
                .args(["fetch", "origin", "main"])
                .current_dir(&repo_dir)
                .output()
                .context("Failed to run git fetch")?;
            if !output.status.success() {
                anyhow::bail!(
                    "git fetch failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            let output = std::process::Command::new("git")
                .args(["reset", "--hard", "origin/main"])
                .current_dir(&repo_dir)
                .output()
                .context("Failed to run git reset")?;
            if !output.status.success() {
                anyhow::bail!(
                    "git reset failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
    } else {
        // Clone
        crate::logging::info("Main channel: cloning repository...");
        let clone_url = format!("https://github.com/{}.git", GITHUB_REPO);
        let output = std::process::Command::new("git")
            .args([
                "clone", "--depth", "1", "--branch", "main", &clone_url, "neura",
            ])
            .current_dir(&build_dir)
            .output()
            .context("Failed to run git clone")?;

        if !output.status.success() {
            anyhow::bail!(
                "git clone failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    // Build
    crate::logging::info("Main channel: building with cargo...");
    let output = std::process::Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(&repo_dir)
        .env("NEURA_RELEASE_BUILD", "1")
        .output()
        .context("Failed to run cargo build")?;

    if !output.status.success() {
        anyhow::bail!(
            "cargo build failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let binary = build::release_binary_path(&repo_dir);
    if !binary.exists() {
        anyhow::bail!("Built binary not found at {}", binary.display());
    }

    record_source_update_duration(started.elapsed());

    Ok(binary)
}

fn version_is_newer(release: &str, current: &str) -> bool {
    let parse = |v: &str| -> (u32, u32, u32) {
        let v = v.trim_start_matches('v');
        let parts: Vec<&str> = v.split('.').collect();
        let major = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
        let minor = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        let patch = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
        (major, minor, patch)
    };

    let r = parse(release);
    let c = parse(current);
    r > c
}

pub fn download_and_install_blocking(release: &GitHubRelease) -> Result<PathBuf> {
    let started = Instant::now();
    let asset_name = get_asset_name();
    let asset = release
        .assets
        .iter()
        .find(|a| a.name.starts_with(asset_name))
        .ok_or_else(|| anyhow::anyhow!("No asset found for platform: {}", asset_name))?;

    let download_url = asset.browser_download_url.clone();

    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join(format!("neura-update-{}", std::process::id()));

    let client = reqwest::blocking::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .user_agent("neura-updater")
        .build()?;

    let response = client
        .get(&download_url)
        .send()
        .context("Failed to download update")?;

    if !response.status().is_success() {
        anyhow::bail!("Download failed: {}", response.status());
    }

    let bytes = response.bytes().context("Failed to read download")?;

    if asset.name.ends_with(".tar.gz") {
        let cursor = std::io::Cursor::new(&bytes);
        let gz = flate2::read::GzDecoder::new(cursor);
        let mut archive = tar::Archive::new(gz);
        let mut extracted = false;
        for entry in archive.entries()? {
            let mut entry = entry?;
            let entry_path = entry.path()?.into_owned();
            let file_name = entry_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if file_name.starts_with("neura") && !file_name.ends_with(".tar.gz") {
                entry.unpack(&temp_path)?;
                extracted = true;
                break;
            }
        }
        if !extracted {
            anyhow::bail!("Could not find neura binary inside tar.gz archive");
        }
    } else {
        fs::write(&temp_path, &bytes).context("Failed to write temp file")?;
    }

    crate::platform::set_permissions_executable(&temp_path)?;

    let version = release.tag_name.trim_start_matches('v');
    let mut metadata = UpdateMetadata::load().unwrap_or_default();

    let versioned_path = build::install_binary_at_version(&temp_path, version)?;
    let _ = fs::remove_file(&temp_path);
    build::update_stable_symlink(version)?;
    build::update_current_symlink(version)?;
    build::update_launcher_symlink_to_current()?;

    metadata.installed_version = Some(release.tag_name.clone());
    metadata.installed_from = Some(asset.browser_download_url.clone());
    metadata.last_check = SystemTime::now();
    metadata.save()?;
    record_release_update_duration(started.elapsed());

    Ok(versioned_path)
}

pub enum UpdateCheckResult {
    NoUpdate,
    UpdateAvailable {
        current: String,
        latest: String,
        _release: GitHubRelease,
    },
    UpdateInstalled {
        version: String,
        path: PathBuf,
    },
    Error(String),
}

pub fn check_and_maybe_update(auto_install: bool) -> UpdateCheckResult {
    use crate::bus::{Bus, BusEvent, UpdateStatus};

    if !should_auto_update() {
        return UpdateCheckResult::NoUpdate;
    }

    let metadata = UpdateMetadata::load().unwrap_or_default();
    if !metadata.should_check() {
        return UpdateCheckResult::NoUpdate;
    }

    Bus::global().publish(BusEvent::UpdateStatus(UpdateStatus::Checking));

    match check_for_update_blocking() {
        Ok(Some(release)) => {
            let current = env!("NEURA_VERSION").to_string();
            let latest = release.tag_name.clone();

            Bus::global().publish(BusEvent::UpdateStatus(UpdateStatus::Available {
                current: current.clone(),
                latest: latest.clone(),
            }));

            if auto_install {
                Bus::global().publish(BusEvent::UpdateStatus(UpdateStatus::Downloading {
                    version: latest.clone(),
                }));
                match download_and_install_blocking(&release) {
                    Ok(path) => {
                        Bus::global().publish(BusEvent::UpdateStatus(UpdateStatus::Installed {
                            version: latest.clone(),
                        }));
                        UpdateCheckResult::UpdateInstalled {
                            version: latest,
                            path,
                        }
                    }
                    Err(e) => {
                        let msg = format!("Failed to install: {}", e);
                        Bus::global()
                            .publish(BusEvent::UpdateStatus(UpdateStatus::Error(msg.clone())));
                        UpdateCheckResult::Error(msg)
                    }
                }
            } else {
                let mut metadata = UpdateMetadata::load().unwrap_or_default();
                metadata.last_check = SystemTime::now();
                let _ = metadata.save();
                UpdateCheckResult::UpdateAvailable {
                    current,
                    latest,
                    _release: release,
                }
            }
        }
        Ok(None) => {
            let mut metadata = UpdateMetadata::load().unwrap_or_default();
            metadata.last_check = SystemTime::now();
            let _ = metadata.save();
            UpdateCheckResult::NoUpdate
        }
        Err(e) => UpdateCheckResult::Error(format!("Check failed: {}", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_is_newer() {
        assert!(version_is_newer("0.1.3", "0.1.2"));
        assert!(version_is_newer("0.2.0", "0.1.9"));
        assert!(version_is_newer("1.0.0", "0.9.9"));
        assert!(!version_is_newer("0.1.2", "0.1.2"));
        assert!(!version_is_newer("0.1.1", "0.1.2"));
        assert!(!version_is_newer("0.0.9", "0.1.0"));
    }

    #[test]
    fn test_asset_name() {
        let name = get_asset_name();
        assert!(name.starts_with("neura-"));
    }

    #[test]
    fn test_is_release_build() {
        assert!(!is_release_build());
    }

    #[test]
    fn test_should_auto_update_dev_build() {
        assert!(!should_auto_update());
    }

    #[test]
    fn test_summarize_git_pull_failure_diverged() {
        let stderr = b"hint: You have divergent branches and need to specify how to reconcile them.\nfatal: Need to specify how to reconcile divergent branches.\n";
        assert_eq!(
            summarize_git_pull_failure(stderr),
            "git pull requires manual reconciliation (local and upstream have diverged)"
        );
    }

    #[test]
    fn test_summarize_git_pull_failure_no_tracking_branch() {
        let stderr = b"There is no tracking information for the current branch.\n";
        assert_eq!(
            summarize_git_pull_failure(stderr),
            "git pull failed: current branch has no upstream tracking branch"
        );
    }

    #[test]
    fn test_summarize_git_pull_failure_uses_first_non_hint_line() {
        let stderr = b"hint: test hint\nfatal: repository not found\n";
        assert_eq!(
            summarize_git_pull_failure(stderr),
            "git pull failed: repository not found"
        );
    }

    #[test]
    fn test_estimate_release_update_duration_uses_size_buckets() {
        assert_eq!(
            estimate_release_update_duration(10 * 1024 * 1024, None),
            Duration::from_secs(10)
        );
        assert_eq!(
            estimate_release_update_duration(40 * 1024 * 1024, None),
            Duration::from_secs(35)
        );
    }

    #[test]
    fn test_estimate_source_update_duration_prefers_history() {
        assert_eq!(
            estimate_source_update_duration(true, true, Some(123.4)),
            Duration::from_secs(123)
        );
    }
}
