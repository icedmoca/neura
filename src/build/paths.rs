use super::{
    SelfDevBuildCommand, canary_binary_path, current_binary_path, shared_server_binary_path,
    stable_binary_path,
};
use crate::storage;
use anyhow::Result;
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

/// Get the kcode repository directory
pub fn get_repo_dir() -> Option<PathBuf> {
    // First try: compile-time directory
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = PathBuf::from(manifest_dir);
    if is_kcode_repo(&path) {
        return Some(path);
    }

    // Fallback: check relative to executable
    if let Ok(exe) = std::env::current_exe() {
        // Assume structure: repo/target/<profile>/<binary> (platform-specific executable name)
        if let Some(repo) = exe
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            && is_kcode_repo(repo)
        {
            return Some(repo.to_path_buf());
        }
    }

    // Final fallback: search upward from current working directory.
    // This matters for self-dev sessions launched from the repo but running
    // from an installed canary/stable binary whose current_exe() is outside
    // the source tree.
    if let Ok(cwd) = std::env::current_dir()
        && let Some(repo) = find_repo_in_ancestors(&cwd)
    {
        return Some(repo);
    }

    None
}

pub fn find_repo_in_ancestors(start: &Path) -> Option<PathBuf> {
    for dir in start.ancestors() {
        if is_kcode_repo(dir) {
            return Some(dir.to_path_buf());
        }
    }
    None
}

pub fn binary_stem() -> &'static str {
    "kcode"
}

pub fn binary_name() -> &'static str {
    if cfg!(windows) {
        "kcode.exe"
    } else {
        binary_stem()
    }
}

pub const SELFDEV_CARGO_PROFILE: &str = "selfdev";

fn profile_binary_path(repo_dir: &Path, profile: &str) -> PathBuf {
    repo_dir.join("target").join(profile).join(binary_name())
}

pub fn release_binary_path(repo_dir: &Path) -> PathBuf {
    profile_binary_path(repo_dir, "release")
}

pub fn selfdev_binary_path(repo_dir: &Path) -> PathBuf {
    profile_binary_path(repo_dir, SELFDEV_CARGO_PROFILE)
}

fn binary_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
}

fn newest_existing_binary(
    candidates: Vec<(PathBuf, &'static str)>,
) -> Option<(PathBuf, &'static str)> {
    candidates
        .into_iter()
        .filter(|(path, _)| path.exists())
        .max_by_key(|(path, _)| binary_mtime(path))
}

pub fn selfdev_build_command(repo_dir: &Path) -> SelfDevBuildCommand {
    let wrapper = repo_dir.join("scripts").join("dev_cargo.sh");
    if wrapper.is_file() {
        return SelfDevBuildCommand {
            program: "bash".to_string(),
            args: vec![
                wrapper.to_string_lossy().into_owned(),
                "build".to_string(),
                "--profile".to_string(),
                SELFDEV_CARGO_PROFILE.to_string(),
                "-p".to_string(),
                "kcode".to_string(),
                "--bin".to_string(),
                "kcode".to_string(),
            ],
            display: format!(
                "scripts/dev_cargo.sh build --profile {} -p kcode --bin kcode",
                SELFDEV_CARGO_PROFILE
            ),
        };
    }

    SelfDevBuildCommand {
        program: "cargo".to_string(),
        args: vec![
            "build".to_string(),
            "--profile".to_string(),
            SELFDEV_CARGO_PROFILE.to_string(),
            "-p".to_string(),
            "kcode".to_string(),
            "--bin".to_string(),
            "kcode".to_string(),
        ],
        display: format!(
            "cargo build --profile {} -p kcode --bin kcode",
            SELFDEV_CARGO_PROFILE
        ),
    }
}

pub fn run_selfdev_build(repo_dir: &Path) -> Result<SelfDevBuildCommand> {
    let source = super::current_source_state(repo_dir)?;
    let build = selfdev_build_command(repo_dir);
    let status = Command::new(&build.program)
        .args(&build.args)
        .current_dir(repo_dir)
        .status()?;

    if !status.success() {
        anyhow::bail!("Build failed: {}", build.display);
    }

    let source_after_build = super::ensure_source_state_matches(repo_dir, &source)?;
    super::write_current_dev_binary_source_metadata(repo_dir, &source_after_build)?;

    Ok(build)
}

pub fn current_binary_built_at() -> Option<DateTime<Utc>> {
    let modified: SystemTime = std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|meta| meta.modified().ok())?;
    Some(DateTime::<Utc>::from(modified))
}

pub fn current_binary_build_time_string() -> Option<String> {
    current_binary_built_at().map(|dt| dt.format("%Y-%m-%d %H:%M:%S %z").to_string())
}

/// Find the best development binary in the repo.
/// Prefers the newest local self-dev or release binary.
pub fn find_dev_binary(repo_dir: &Path) -> Option<PathBuf> {
    newest_existing_binary(vec![
        (selfdev_binary_path(repo_dir), "repo-selfdev"),
        (release_binary_path(repo_dir), "repo-release"),
    ])
    .map(|(path, _)| path)
}

fn home_dir() -> Result<PathBuf> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("USERPROFILE").map(PathBuf::from))
        .map_err(|_| anyhow::anyhow!("HOME/USERPROFILE not set"))
}

/// Directory for the single launcher path users execute from PATH.
///
/// Defaults to `~/.local/bin` on Unix, `%LOCALAPPDATA%\kcode\bin` on Windows.
/// Overridable with `KCODE_INSTALL_DIR`.
pub fn launcher_dir() -> Result<PathBuf> {
    if let Ok(custom) = std::env::var("KCODE_INSTALL_DIR") {
        let trimmed = custom.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }

    if let Ok(sandbox_home) = std::env::var("KCODE_HOME") {
        let trimmed = sandbox_home.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed).join("bin"));
        }
    }

    #[cfg(windows)]
    {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            return Ok(PathBuf::from(local).join("kcode").join("bin"));
        }
        Ok(home_dir()?
            .join("AppData")
            .join("Local")
            .join("kcode")
            .join("bin"))
    }
    #[cfg(not(windows))]
    {
        Ok(home_dir()?.join(".local").join("bin"))
    }
}

/// Path to the launcher binary (`~/.local/bin/kcode` by default).
pub fn launcher_binary_path() -> Result<PathBuf> {
    Ok(launcher_dir()?.join(binary_name()))
}

fn update_launcher_symlink(target: &Path) -> Result<PathBuf> {
    let launcher = launcher_binary_path()?;

    if let Some(parent) = launcher.parent() {
        storage::ensure_dir(parent)?;
    }

    let temp = launcher
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(
            ".{}-launcher-{}",
            binary_stem(),
            std::process::id()
        ));

    crate::platform::atomic_symlink_swap(target, &launcher, &temp)?;
    Ok(launcher)
}

/// Update launcher path to point at the current channel binary.
pub fn update_launcher_symlink_to_current() -> Result<PathBuf> {
    let current = current_binary_path()?;
    update_launcher_symlink(&current)
}

/// Update launcher path to point at the stable channel binary.
pub fn update_launcher_symlink_to_stable() -> Result<PathBuf> {
    let stable = stable_binary_path()?;
    update_launcher_symlink(&stable)
}

/// Resolve which client binary should be considered for launches, updates, and reloads.
///
/// Order matters:
/// - Prefer the published `current` channel first (active local build)
/// - Self-dev sessions can fall back to an unpublished repo build from `target/selfdev` or `target/release`
/// - Then the self-dev canary channel
/// - Then launcher path
/// - Then stable channel path
/// - Finally currently running executable
pub fn client_update_candidate(is_selfdev_session: bool) -> Option<(PathBuf, &'static str)> {
    if let Ok(current) = current_binary_path()
        && current.exists()
    {
        return Some((current, "current"));
    }

    if is_selfdev_session {
        if let Some(repo_dir) = get_repo_dir()
            && let Some(dev) = find_dev_binary(&repo_dir)
            && dev.exists()
        {
            return Some((dev, "dev"));
        }
        if let Ok(canary) = canary_binary_path()
            && canary.exists()
        {
            return Some((canary, "canary"));
        }
    }

    if let Ok(launcher) = launcher_binary_path()
        && launcher.exists()
    {
        return Some((launcher, "launcher"));
    }

    if let Ok(stable) = stable_binary_path()
        && stable.exists()
    {
        return Some((stable, "stable"));
    }

    std::env::current_exe().ok().map(|exe| (exe, "current"))
}

/// Resolve the binary that the shared daemon should spawn or reload into.
///
/// This intentionally does not follow the fast-moving `current` channel. The
/// shared server should only run binaries that were explicitly promoted onto the
/// shared-server channel (or stable as fallback), so local dirty self-dev builds
/// stop taking out every client by accident.
pub fn shared_server_update_candidate(
    _is_selfdev_session: bool,
) -> Option<(PathBuf, &'static str)> {
    if let Ok(shared_server) = shared_server_binary_path()
        && shared_server.exists()
    {
        return Some((shared_server, "shared-server"));
    }

    if let Ok(stable) = stable_binary_path()
        && stable.exists()
    {
        return Some((stable, "stable"));
    }

    std::env::current_exe().ok().map(|exe| (exe, "current"))
}

/// Resolve the best binary to use for `/reload`.
///
/// This mostly follows `client_update_candidate`, but if a freshly built repo
/// release binary exists and is newer than the selected channel binary, prefer
/// that so local rebuilds can reload correctly even if publishing the build
/// failed.
pub fn preferred_reload_candidate(is_selfdev_session: bool) -> Option<(PathBuf, &'static str)> {
    let candidate = client_update_candidate(is_selfdev_session);

    let repo_binary = get_repo_dir().and_then(|repo_dir| {
        if is_selfdev_session {
            newest_existing_binary(vec![
                (selfdev_binary_path(&repo_dir), "repo-selfdev"),
                (release_binary_path(&repo_dir), "repo-release"),
            ])
        } else {
            newest_existing_binary(vec![(release_binary_path(&repo_dir), "repo-release")])
        }
    });

    let repo_is_newer = |repo: &Path, current: &Path| {
        let repo_mtime = std::fs::metadata(repo).ok().and_then(|m| m.modified().ok());
        let current_mtime = std::fs::metadata(current)
            .ok()
            .and_then(|m| m.modified().ok());
        match (repo_mtime, current_mtime) {
            (Some(repo), Some(current)) => repo > current,
            (Some(_), None) => true,
            _ => false,
        }
    };

    match (repo_binary, candidate) {
        (Some((repo, label)), Some((current, _))) if repo_is_newer(&repo, &current) => {
            Some((repo, label))
        }
        (Some((repo, label)), None) => Some((repo, label)),
        (_, Some(candidate)) => Some(candidate),
        (None, None) => None,
    }
}

/// Check if a directory is the kcode repository
pub fn is_kcode_repo(dir: &Path) -> bool {
    // Check for Cargo.toml with name = "kcode"
    let cargo_toml = dir.join("Cargo.toml");
    if !cargo_toml.exists() {
        return false;
    }

    // Check for .git directory
    if !dir.join(".git").exists() {
        return false;
    }

    // Read Cargo.toml and check package name
    if let Ok(content) = std::fs::read_to_string(&cargo_toml)
        && content.contains("name = \"kcode\"")
    {
        return true;
    }

    false
}
