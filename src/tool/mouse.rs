use super::{Tool, ToolContext, ToolOutput};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use base64::Engine;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use tokio::process::Command;

const MAX_COORD: i32 = 100_000;
const MAX_SCROLL: i32 = 100;
const MAX_DURATION_MS: u64 = 10_000;

pub struct MouseTool;

impl MouseTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct MouseInput {
    action: MouseAction,
    #[serde(default)]
    x: Option<i32>,
    #[serde(default)]
    y: Option<i32>,
    #[serde(default)]
    dx: Option<i32>,
    #[serde(default)]
    dy: Option<i32>,
    #[serde(default)]
    button: Option<String>,
    #[serde(default)]
    clicks: Option<u8>,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MouseAction {
    Status,
    Position,
    Move,
    RelativeMove,
    Click,
    DoubleClick,
    Drag,
    Scroll,
    Screenshot,
}

#[derive(Debug, Clone, Copy)]
enum Backend {
    Xdotool,
    Xte,
    Ydotool,
    Dotool,
}

impl Backend {
    fn name(self) -> &'static str {
        match self {
            Backend::Xdotool => "xdotool",
            Backend::Xte => "xte",
            Backend::Ydotool => "ydotool",
            Backend::Dotool => "dotool",
        }
    }
}

#[async_trait]
impl Tool for MouseTool {
    fn name(&self) -> &str {
        "mouse"
    }

    fn description(&self) -> &str {
        "Control the local mouse cursor and take screenshots using installed OS automation utilities. Prefer browser tool for web pages."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "intent": super::intent_schema_property(),
                "action": {
                    "type": "string",
                    "enum": ["status", "position", "move", "relative_move", "click", "double_click", "drag", "scroll", "screenshot"],
                    "description": "Mouse action to perform."
                },
                "x": { "type": "integer", "description": "Absolute X coordinate for move/click/drag." },
                "y": { "type": "integer", "description": "Absolute Y coordinate for move/click/drag." },
                "dx": { "type": "integer", "description": "Relative X delta, or horizontal scroll amount." },
                "dy": { "type": "integer", "description": "Relative Y delta, or vertical scroll amount." },
                "button": { "type": "string", "enum": ["left", "middle", "right", "1", "2", "3", "4", "5"], "description": "Mouse button." },
                "clicks": { "type": "integer", "minimum": 1, "maximum": 10, "description": "Number of clicks for click action." },
                "duration_ms": { "type": "integer", "minimum": 0, "maximum": 10000, "description": "Optional drag duration." },
                "path": { "type": "string", "description": "Optional screenshot output path. If omitted, image is returned inline." }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: MouseInput = serde_json::from_value(input)?;
        match params.action {
            MouseAction::Status => status().await,
            MouseAction::Position => position().await,
            MouseAction::Move => {
                let (x, y) = required_xy(&params)?;
                run_move(x, y).await
            }
            MouseAction::RelativeMove => {
                let dx = params.dx.unwrap_or(0);
                let dy = params.dy.unwrap_or(0);
                validate_delta(dx, dy)?;
                run_relative_move(dx, dy).await
            }
            MouseAction::Click => {
                let button = button_number(params.button.as_deref())?;
                let clicks = params.clicks.unwrap_or(1).clamp(1, 10);
                if let (Some(x), Some(y)) = (params.x, params.y) {
                    validate_coord(x, y)?;
                    run_move(x, y).await?;
                }
                run_click(button, clicks).await
            }
            MouseAction::DoubleClick => {
                let button = button_number(params.button.as_deref())?;
                if let (Some(x), Some(y)) = (params.x, params.y) {
                    validate_coord(x, y)?;
                    run_move(x, y).await?;
                }
                run_click(button, 2).await
            }
            MouseAction::Drag => {
                let (x, y) = required_xy(&params)?;
                let duration = params.duration_ms.unwrap_or(300).min(MAX_DURATION_MS);
                run_drag(x, y, duration).await
            }
            MouseAction::Scroll => {
                let dx = params.dx.unwrap_or(0);
                let dy = params.dy.unwrap_or(0);
                validate_scroll(dx, dy)?;
                run_scroll(dx, dy).await
            }
            MouseAction::Screenshot => screenshot(params.path, ctx).await,
        }
    }
}

async fn status() -> Result<ToolOutput> {
    let backend = detect_backend().await;
    let screenshot_backend = detect_screenshot_backend().await;
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".into());
    let display = std::env::var("DISPLAY").unwrap_or_default();
    let wayland = std::env::var("WAYLAND_DISPLAY").unwrap_or_default();
    let mut warnings = Vec::new();
    if session == "wayland" && !matches!(backend, Some(Backend::Ydotool | Backend::Dotool)) {
        warnings.push("Wayland may block synthetic mouse input unless ydotool/dotool is installed and permitted".to_string());
    }
    Ok(ToolOutput::new(format!(
        "mouse status\ninput_backend: {}\nscreenshot_backend: {}\nsession: {}\nDISPLAY: {}\nWAYLAND_DISPLAY: {}\nwarnings: {}",
        backend.map(|b| b.name()).unwrap_or("none"),
        screenshot_backend.unwrap_or("none"),
        session,
        if display.is_empty() {
            "<unset>"
        } else {
            &display
        },
        if wayland.is_empty() {
            "<unset>"
        } else {
            &wayland
        },
        if warnings.is_empty() {
            "none".into()
        } else {
            warnings.join("; ")
        }
    )))
}

async fn position() -> Result<ToolOutput> {
    if has_cmd("xdotool").await {
        let out = cmd_output("xdotool", &["getmouselocation", "--shell"]).await?;
        return Ok(ToolOutput::new(out));
    }
    Err(anyhow!(
        "mouse position requires xdotool; installed backends can still move/click but cannot reliably report position"
    ))
}

async fn run_move(x: i32, y: i32) -> Result<ToolOutput> {
    validate_coord(x, y)?;
    match detect_backend().await {
        Some(Backend::Xdotool) => {
            run("xdotool", &["mousemove", &x.to_string(), &y.to_string()]).await?
        }
        Some(Backend::Xte) => run("xte", &[&format!("mousemove {x} {y}")]).await?,
        Some(Backend::Ydotool) => {
            run(
                "ydotool",
                &["mousemove", "--absolute", &x.to_string(), &y.to_string()],
            )
            .await?
        }
        Some(Backend::Dotool) => run_dotool(&format!("mouseto {x} {y}\n")).await?,
        None => return Err(no_backend_error()),
    }
    Ok(ToolOutput::new(format!("moved mouse to {x},{y}")))
}

async fn run_relative_move(dx: i32, dy: i32) -> Result<ToolOutput> {
    validate_delta(dx, dy)?;
    match detect_backend().await {
        Some(Backend::Xdotool) => {
            run(
                "xdotool",
                &["mousemove_relative", "--", &dx.to_string(), &dy.to_string()],
            )
            .await?
        }
        Some(Backend::Xte) => run("xte", &[&format!("mousermove {dx} {dy}")]).await?,
        Some(Backend::Ydotool) => {
            run("ydotool", &["mousemove", &dx.to_string(), &dy.to_string()]).await?
        }
        Some(Backend::Dotool) => run_dotool(&format!("mousermove {dx} {dy}\n")).await?,
        None => return Err(no_backend_error()),
    }
    Ok(ToolOutput::new(format!("moved mouse by {dx},{dy}")))
}

async fn run_click(button: u8, clicks: u8) -> Result<ToolOutput> {
    match detect_backend().await {
        Some(Backend::Xdotool) => {
            for _ in 0..clicks {
                run("xdotool", &["click", &button.to_string()]).await?;
            }
        }
        Some(Backend::Xte) => {
            for _ in 0..clicks {
                run("xte", &[&format!("mouseclick {button}")]).await?;
            }
        }
        Some(Backend::Ydotool) => {
            let code = ydotool_button_code(button)?;
            for _ in 0..clicks {
                run("ydotool", &["click", &code.to_string()]).await?;
            }
        }
        Some(Backend::Dotool) => {
            let name = dotool_button_name(button)?;
            let mut script = String::new();
            for _ in 0..clicks {
                script.push_str(&format!("button {name}\n"));
            }
            run_dotool(&script).await?;
        }
        None => return Err(no_backend_error()),
    }
    Ok(ToolOutput::new(format!(
        "clicked button {button} {clicks} time(s)"
    )))
}

async fn run_drag(x: i32, y: i32, duration_ms: u64) -> Result<ToolOutput> {
    validate_coord(x, y)?;
    match detect_backend().await {
        Some(Backend::Xdotool) => {
            run(
                "xdotool",
                &[
                    "mousedown",
                    "1",
                    "mousemove",
                    "--sync",
                    &x.to_string(),
                    &y.to_string(),
                    "mouseup",
                    "1",
                ],
            )
            .await?
        }
        Some(Backend::Xte) => {
            run(
                "xte",
                &["mousedown 1", &format!("mousemove {x} {y}"), "mouseup 1"],
            )
            .await?
        }
        Some(Backend::Ydotool) => {
            run("ydotool", &["click", "0xC0"]).await?;
            tokio::time::sleep(std::time::Duration::from_millis(
                duration_ms.min(MAX_DURATION_MS),
            ))
            .await;
            run(
                "ydotool",
                &["mousemove", "--absolute", &x.to_string(), &y.to_string()],
            )
            .await?;
            run("ydotool", &["click", "0x80"]).await?;
        }
        Some(Backend::Dotool) => {
            run_dotool(&format!(
                "buttondown left\nmouseto {x} {y}\nbuttonup left\n"
            ))
            .await?
        }
        None => return Err(no_backend_error()),
    }
    Ok(ToolOutput::new(format!("dragged to {x},{y}")))
}

async fn run_scroll(dx: i32, dy: i32) -> Result<ToolOutput> {
    validate_scroll(dx, dy)?;
    let mut actions = Vec::new();
    if dy < 0 {
        actions.extend(std::iter::repeat(4).take((-dy) as usize));
    }
    if dy > 0 {
        actions.extend(std::iter::repeat(5).take(dy as usize));
    }
    if dx < 0 {
        actions.extend(std::iter::repeat(6).take((-dx) as usize));
    }
    if dx > 0 {
        actions.extend(std::iter::repeat(7).take(dx as usize));
    }
    for button in actions {
        run_click(button, 1).await?;
    }
    Ok(ToolOutput::new(format!("scrolled dx={dx} dy={dy}")))
}

async fn screenshot(path: Option<String>, ctx: ToolContext) -> Result<ToolOutput> {
    let requested_path = path.map(|p| ctx.resolve_path(std::path::Path::new(&p)));
    let capture_path = requested_path
        .clone()
        .unwrap_or_else(|| temp_screenshot_path());
    if has_cmd("spectacle").await {
        run(
            "spectacle",
            &["-b", "-n", "-o", capture_path.to_string_lossy().as_ref()],
        )
        .await?;
    } else if has_cmd("gnome-screenshot").await {
        run(
            "gnome-screenshot",
            &["-f", capture_path.to_string_lossy().as_ref()],
        )
        .await?;
    } else if has_cmd("grim").await {
        run("grim", &[capture_path.to_string_lossy().as_ref()]).await?;
    } else if has_cmd("import").await {
        run(
            "import",
            &["-window", "root", capture_path.to_string_lossy().as_ref()],
        )
        .await?;
    } else if has_cmd("scrot").await {
        run("scrot", &[capture_path.to_string_lossy().as_ref()]).await?;
    } else {
        return Err(anyhow!(
            "no screenshot utility found; install spectacle, gnome-screenshot, grim, import, or scrot"
        ));
    }

    if requested_path.is_some() {
        Ok(ToolOutput::new(format!(
            "screenshot saved to {}",
            capture_path.display()
        )))
    } else {
        let bytes = tokio::fs::read(&capture_path)
            .await
            .with_context(|| format!("reading screenshot {}", capture_path.display()))?;
        let _ = tokio::fs::remove_file(&capture_path).await;
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        Ok(ToolOutput::new("screenshot captured").with_labeled_image(
            "image/png",
            encoded,
            "mouse screenshot",
        ))
    }
}

async fn detect_backend() -> Option<Backend> {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    if session.eq_ignore_ascii_case("wayland") {
        if has_cmd("ydotool").await {
            return Some(Backend::Ydotool);
        }
        if has_cmd("dotool").await {
            return Some(Backend::Dotool);
        }
        // xte and xdotool usually only affect X11/XWayland clients under Wayland,
        // but keep them as a last-resort fallback for XWayland windows.
        if has_cmd("xdotool").await {
            return Some(Backend::Xdotool);
        }
        if has_cmd("xte").await {
            return Some(Backend::Xte);
        }
        return None;
    }
    if has_cmd("xdotool").await {
        return Some(Backend::Xdotool);
    }
    if has_cmd("ydotool").await {
        return Some(Backend::Ydotool);
    }
    if has_cmd("dotool").await {
        return Some(Backend::Dotool);
    }
    if has_cmd("xte").await {
        return Some(Backend::Xte);
    }
    None
}

async fn detect_screenshot_backend() -> Option<&'static str> {
    for cmd in ["spectacle", "gnome-screenshot", "grim", "import", "scrot"] {
        if has_cmd(cmd).await {
            return Some(cmd);
        }
    }
    None
}

async fn has_cmd(name: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {} >/dev/null 2>&1", shell_escape(name)))
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

async fn run(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .await
        .with_context(|| format!("running {program}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("{program} exited with {status}"))
    }
}

async fn cmd_output(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .await
        .with_context(|| format!("running {program}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(anyhow!(
            "{} failed: {}",
            program,
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

async fn run_dotool(script: &str) -> Result<()> {
    let mut child = Command::new("dotool")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .context("starting dotool")?;
    if let Some(stdin) = child.stdin.as_mut() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(script.as_bytes()).await?;
    }
    let status = child.wait().await?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("dotool exited with {status}"))
    }
}

fn required_xy(params: &MouseInput) -> Result<(i32, i32)> {
    let x = params
        .x
        .ok_or_else(|| anyhow!("x is required for this action"))?;
    let y = params
        .y
        .ok_or_else(|| anyhow!("y is required for this action"))?;
    validate_coord(x, y)?;
    Ok((x, y))
}

fn validate_coord(x: i32, y: i32) -> Result<()> {
    if !(0..=MAX_COORD).contains(&x) || !(0..=MAX_COORD).contains(&y) {
        return Err(anyhow!(
            "coordinates out of safe range 0..={MAX_COORD}: {x},{y}"
        ));
    }
    Ok(())
}

fn validate_delta(dx: i32, dy: i32) -> Result<()> {
    if dx.abs() > MAX_COORD || dy.abs() > MAX_COORD {
        return Err(anyhow!("relative movement too large: {dx},{dy}"));
    }
    Ok(())
}

fn validate_scroll(dx: i32, dy: i32) -> Result<()> {
    if dx.abs() > MAX_SCROLL || dy.abs() > MAX_SCROLL {
        return Err(anyhow!(
            "scroll amount too large; max absolute value is {MAX_SCROLL}"
        ));
    }
    Ok(())
}

fn button_number(button: Option<&str>) -> Result<u8> {
    match button.unwrap_or("left") {
        "left" | "1" => Ok(1),
        "middle" | "2" => Ok(2),
        "right" | "3" => Ok(3),
        "4" => Ok(4),
        "5" => Ok(5),
        other => Err(anyhow!("unsupported mouse button: {other}")),
    }
}

fn ydotool_button_code(button: u8) -> Result<String> {
    // ydotool click accepts Linux input event button codes as hex bitmasks.
    match button {
        1 => Ok("0xC0".into()),
        2 => Ok("0xC1".into()),
        3 => Ok("0xC2".into()),
        4 => Ok("0xC8".into()),
        5 => Ok("0xC9".into()),
        _ => Err(anyhow!("unsupported ydotool button {button}")),
    }
}

fn dotool_button_name(button: u8) -> Result<&'static str> {
    match button {
        1 => Ok("left"),
        2 => Ok("middle"),
        3 => Ok("right"),
        4 => Ok("wheelup"),
        5 => Ok("wheeldown"),
        _ => Err(anyhow!("unsupported dotool button {button}")),
    }
}

fn temp_screenshot_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "neura-mouse-screenshot-{}-{}.png",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ))
}

fn no_backend_error() -> anyhow::Error {
    anyhow!(
        "no mouse input backend found; install xdotool (X11), ydotool/dotool (Wayland with daemon/permissions), or xautomation/xte"
    )
}

fn shell_escape(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_buttons() {
        assert_eq!(button_number(None).unwrap(), 1);
        assert_eq!(button_number(Some("right")).unwrap(), 3);
        assert!(button_number(Some("side")).is_err());
    }

    #[test]
    fn rejects_unsafe_coordinates_and_scroll() {
        assert!(validate_coord(10, 10).is_ok());
        assert!(validate_coord(-1, 10).is_err());
        assert!(validate_coord(10, MAX_COORD + 1).is_err());
        assert!(validate_scroll(0, MAX_SCROLL).is_ok());
        assert!(validate_scroll(0, MAX_SCROLL + 1).is_err());
    }
}
