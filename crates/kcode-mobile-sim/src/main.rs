use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use kcode_mobile_core::{
    ReplayTrace, ScenarioName, ScreenshotSnapshot, SimulatorAction, SimulatorState, SimulatorStore,
    VisualScene,
};
use kcode_mobile_sim::{
    AutomationRequest, default_socket_path, request_status, run_server, send_request,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(name = "kcode-mobile-sim")]
#[command(about = "Headless-first mobile simulator and automation CLI for kcode")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Serve {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long, default_value = "onboarding")]
        scenario: String,
    },
    Start {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long, default_value = "onboarding")]
        scenario: String,
    },
    Status {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    State {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    Tree {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    Scene {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    Preview {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long, default_value = "connected_chat")]
        scenario: String,
    },
    PreviewMesh {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long, default_value = "connected_chat")]
        scenario: String,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    Render {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    Screenshot {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long, default_value = "json")]
        format: String,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    AssertScreenshot {
        #[arg(long)]
        socket: Option<PathBuf>,
        path: PathBuf,
    },
    FindNode {
        #[arg(long)]
        socket: Option<PathBuf>,
        node_id: String,
    },
    HitTest {
        #[arg(long)]
        socket: Option<PathBuf>,
        x: i32,
        y: i32,
    },
    TapAt {
        #[arg(long)]
        socket: Option<PathBuf>,
        x: i32,
        y: i32,
    },
    TypeText {
        #[arg(long)]
        socket: Option<PathBuf>,
        node_id: String,
        text: String,
    },
    Keypress {
        #[arg(long)]
        socket: Option<PathBuf>,
        key: String,
        #[arg(long)]
        node_id: Option<String>,
    },
    Scroll {
        #[arg(long)]
        socket: Option<PathBuf>,
        node_id: String,
        delta_y: i64,
    },
    Gesture {
        #[arg(long)]
        socket: Option<PathBuf>,
        gesture_type: String,
    },
    Wait {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        screen: Option<String>,
        #[arg(long)]
        contains: Option<String>,
        #[arg(long)]
        node_id: Option<String>,
        #[arg(long, default_value_t = 1000)]
        timeout_ms: u64,
    },
    InjectFault {
        #[arg(long)]
        socket: Option<PathBuf>,
        kind: String,
        #[arg(long)]
        message: Option<String>,
    },
    AssertHit {
        #[arg(long)]
        socket: Option<PathBuf>,
        x: i32,
        y: i32,
        node_id: String,
    },
    AssertScreen {
        #[arg(long)]
        socket: Option<PathBuf>,
        screen: String,
    },
    AssertText {
        #[arg(long)]
        socket: Option<PathBuf>,
        contains: String,
    },
    AssertNode {
        #[arg(long)]
        socket: Option<PathBuf>,
        node_id: String,
        #[arg(long)]
        visible: Option<bool>,
        #[arg(long)]
        enabled: Option<bool>,
        #[arg(long)]
        role: Option<String>,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        value: Option<String>,
    },
    AssertNoError {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    AssertTransition {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long = "type")]
        transition_type: Option<String>,
        #[arg(long)]
        contains: Option<String>,
    },
    AssertEffect {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long = "type")]
        effect_type: Option<String>,
        #[arg(long)]
        contains: Option<String>,
    },
    Log {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long)]
        limit: Option<usize>,
    },
    ExportReplay {
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long, default_value = "mobile-sim-replay")]
        name: String,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    AssertReplay {
        path: PathBuf,
    },
    AssertLiveReplay {
        #[arg(long)]
        socket: Option<PathBuf>,
        path: PathBuf,
    },
    Reset {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    LoadScenario {
        #[arg(long)]
        socket: Option<PathBuf>,
        scenario: String,
    },
    SetField {
        #[arg(long)]
        socket: Option<PathBuf>,
        field: String,
        value: String,
    },
    Tap {
        #[arg(long)]
        socket: Option<PathBuf>,
        node_id: String,
    },
    DispatchJson {
        #[arg(long)]
        socket: Option<PathBuf>,
        action_json: String,
    },
    Shutdown {
        #[arg(long)]
        socket: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve { socket, scenario } => {
            let socket = socket.unwrap_or_else(default_socket_path);
            let scenario = parse_scenario(&scenario)?;
            run_server(&socket, scenario).await
        }
        Command::Start { socket, scenario } => start_background(socket, &scenario).await,
        Command::Status { socket } => {
            let status = request_status(&resolve_socket(socket)).await?;
            println!("{}", serde_json::to_string_pretty(&status)?);
            Ok(())
        }
        Command::State { socket } => {
            print_result(send_simple(&resolve_socket(socket), "state", Value::Null).await?)
        }
        Command::Tree { socket } => {
            print_result(send_simple(&resolve_socket(socket), "tree", Value::Null).await?)
        }
        Command::Scene { socket, output } => {
            let scene = send_simple(&resolve_socket(socket), "scene", Value::Null).await?;
            write_or_print_json(scene, output)
        }
        Command::Preview { socket, scenario } => {
            let scene = resolve_preview_scene(socket, &scenario).await?;
            kcode_mobile_sim::gpu_preview::run_preview(scene)
        }
        Command::PreviewMesh {
            socket,
            scenario,
            output,
        } => {
            let scene = resolve_preview_scene(socket, &scenario).await?;
            let mesh = kcode_mobile_sim::gpu_preview::build_preview_mesh(&scene);
            write_or_print_json(serde_json::to_value(mesh)?, output)
        }
        Command::Render { socket, output } => {
            let rendered = send_simple(&resolve_socket(socket), "render", Value::Null).await?;
            write_text_output(rendered, output)
        }
        Command::Screenshot {
            socket,
            format,
            output,
        } => {
            let snapshot = send_simple(&resolve_socket(socket), "screenshot", Value::Null).await?;
            write_screenshot(snapshot, &format, output)
        }
        Command::AssertScreenshot { socket, path } => {
            let snapshot = read_screenshot_snapshot(&path)?;
            print_result(
                send_simple(
                    &resolve_socket(socket),
                    "assert_screenshot",
                    json!({ "snapshot": snapshot }),
                )
                .await?,
            )
        }
        Command::FindNode { socket, node_id } => print_result(
            send_simple(
                &resolve_socket(socket),
                "find_node",
                json!({ "node_id": node_id }),
            )
            .await?,
        ),
        Command::HitTest { socket, x, y } => print_result(
            send_simple(
                &resolve_socket(socket),
                "hit_test",
                json!({ "x": x, "y": y }),
            )
            .await?,
        ),
        Command::TapAt { socket, x, y } => print_result(
            send_simple(&resolve_socket(socket), "tap_at", json!({ "x": x, "y": y })).await?,
        ),
        Command::TypeText {
            socket,
            node_id,
            text,
        } => print_result(
            send_simple(
                &resolve_socket(socket),
                "type_text",
                json!({ "node_id": node_id, "text": text }),
            )
            .await?,
        ),
        Command::Keypress {
            socket,
            key,
            node_id,
        } => print_result(
            send_simple(
                &resolve_socket(socket),
                "keypress",
                json!({ "key": key, "node_id": node_id }),
            )
            .await?,
        ),
        Command::Scroll {
            socket,
            node_id,
            delta_y,
        } => print_result(
            send_simple(
                &resolve_socket(socket),
                "scroll",
                json!({ "node_id": node_id, "delta_y": delta_y }),
            )
            .await?,
        ),
        Command::Gesture {
            socket,
            gesture_type,
        } => print_result(
            send_simple(
                &resolve_socket(socket),
                "gesture",
                json!({ "type": gesture_type }),
            )
            .await?,
        ),
        Command::Wait {
            socket,
            screen,
            contains,
            node_id,
            timeout_ms,
        } => print_result(
            send_simple(
                &resolve_socket(socket),
                "wait",
                json!({
                    "screen": screen,
                    "contains": contains,
                    "node_id": node_id,
                    "timeout_ms": timeout_ms,
                }),
            )
            .await?,
        ),
        Command::InjectFault {
            socket,
            kind,
            message,
        } => print_result(
            send_simple(
                &resolve_socket(socket),
                "inject_fault",
                json!({ "kind": kind, "message": message }),
            )
            .await?,
        ),
        Command::AssertHit {
            socket,
            x,
            y,
            node_id,
        } => print_result(
            send_simple(
                &resolve_socket(socket),
                "assert_hit",
                json!({ "x": x, "y": y, "node_id": node_id }),
            )
            .await?,
        ),
        Command::AssertScreen { socket, screen } => print_result(
            send_simple(
                &resolve_socket(socket),
                "assert_screen",
                json!({ "screen": screen }),
            )
            .await?,
        ),
        Command::AssertText { socket, contains } => print_result(
            send_simple(
                &resolve_socket(socket),
                "assert_text",
                json!({ "contains": contains }),
            )
            .await?,
        ),
        Command::AssertNode {
            socket,
            node_id,
            visible,
            enabled,
            role,
            label,
            value,
        } => print_result(
            send_simple(
                &resolve_socket(socket),
                "assert_node",
                json!({
                    "node_id": node_id,
                    "visible": visible,
                    "enabled": enabled,
                    "role": role,
                    "label": label,
                    "value": value,
                }),
            )
            .await?,
        ),
        Command::AssertNoError { socket } => print_result(
            send_simple(&resolve_socket(socket), "assert_no_error", Value::Null).await?,
        ),
        Command::AssertTransition {
            socket,
            transition_type,
            contains,
        } => print_result(
            send_simple(
                &resolve_socket(socket),
                "assert_transition",
                json!({ "type": transition_type, "contains": contains }),
            )
            .await?,
        ),
        Command::AssertEffect {
            socket,
            effect_type,
            contains,
        } => print_result(
            send_simple(
                &resolve_socket(socket),
                "assert_effect",
                json!({ "type": effect_type, "contains": contains }),
            )
            .await?,
        ),
        Command::Log { socket, limit } => print_result(
            send_simple(&resolve_socket(socket), "log", json!({ "limit": limit })).await?,
        ),
        Command::ExportReplay {
            socket,
            name,
            output,
        } => {
            let replay =
                send_simple(&resolve_socket(socket), "replay", json!({ "name": name })).await?;
            write_or_print_json(replay, output)
        }
        Command::AssertReplay { path } => {
            let trace = read_replay_trace(&path)?;
            trace.assert_replays()?;
            print_result(json!({ "name": trace.name, "matched": true }))
        }
        Command::AssertLiveReplay { socket, path } => {
            let trace = read_replay_trace(&path)?;
            print_result(
                send_simple(
                    &resolve_socket(socket),
                    "assert_replay",
                    json!({ "trace": trace }),
                )
                .await?,
            )
        }
        Command::Reset { socket } => {
            print_result(send_simple(&resolve_socket(socket), "reset", Value::Null).await?)
        }
        Command::LoadScenario { socket, scenario } => print_result(
            send_simple(
                &resolve_socket(socket),
                "load_scenario",
                json!({ "scenario": parse_scenario(&scenario)?.as_str() }),
            )
            .await?,
        ),
        Command::SetField {
            socket,
            field,
            value,
        } => {
            let action = map_set_field(&field, value)?;
            print_result(dispatch_action(&resolve_socket(socket), action).await?)
        }
        Command::Tap { socket, node_id } => print_result(
            dispatch_action(
                &resolve_socket(socket),
                SimulatorAction::TapNode { node_id },
            )
            .await?,
        ),
        Command::DispatchJson {
            socket,
            action_json,
        } => {
            let action: SimulatorAction =
                serde_json::from_str(&action_json).with_context(|| "parse action JSON")?;
            print_result(dispatch_action(&resolve_socket(socket), action).await?)
        }
        Command::Shutdown { socket } => {
            print_result(send_simple(&resolve_socket(socket), "shutdown", Value::Null).await?)
        }
    }
}

fn resolve_socket(socket: Option<PathBuf>) -> PathBuf {
    socket.unwrap_or_else(default_socket_path)
}

fn parse_scenario(input: &str) -> Result<ScenarioName> {
    ScenarioName::parse(input).ok_or_else(|| anyhow!("unknown scenario: {input}"))
}

async fn resolve_preview_scene(socket: Option<PathBuf>, scenario: &str) -> Result<VisualScene> {
    if let Some(socket) = socket {
        let value = send_simple(&resolve_socket(Some(socket)), "scene", Value::Null).await?;
        return serde_json::from_value(value).context("decode live mobile visual scene");
    }

    let scenario = parse_scenario(scenario)?;
    let store = SimulatorStore::new(SimulatorState::for_scenario(scenario));
    Ok(store.visual_scene())
}

fn map_set_field(field: &str, value: String) -> Result<SimulatorAction> {
    match field {
        "host" | "pair.host" => Ok(SimulatorAction::SetHost { value }),
        "port" | "pair.port" => Ok(SimulatorAction::SetPort { value }),
        "pair_code" | "code" | "pair.code" => Ok(SimulatorAction::SetPairCode { value }),
        "device_name" | "pair.device_name" => Ok(SimulatorAction::SetDeviceName { value }),
        "draft" | "chat.draft" => Ok(SimulatorAction::SetDraft { value }),
        _ => bail!("unknown field: {field}"),
    }
}

async fn dispatch_action(socket: &Path, action: SimulatorAction) -> Result<Value> {
    send_simple(socket, "dispatch", json!({ "action": action })).await
}

async fn send_simple(socket: &Path, method: &str, params: Value) -> Result<Value> {
    let response = send_request(
        socket,
        AutomationRequest {
            id: format!("{}-{}", method, unique_id()),
            method: method.to_string(),
            params,
        },
    )
    .await?;

    if !response.ok {
        bail!(
            response
                .error
                .unwrap_or_else(|| format!("{} failed", method))
        );
    }
    Ok(response.result)
}

fn print_result(value: Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn write_or_print_json(value: Value, output: Option<PathBuf>) -> Result<()> {
    let json = serde_json::to_string_pretty(&value)?;
    if let Some(output) = output {
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create replay output directory {}", parent.display()))?;
        }
        std::fs::write(&output, format!("{json}\n"))
            .with_context(|| format!("write replay trace {}", output.display()))?;
    } else {
        println!("{json}");
    }
    Ok(())
}

fn write_text_output(value: Value, output: Option<PathBuf>) -> Result<()> {
    let text = value
        .get("output")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("render response missing output field"))?;
    if let Some(output) = output {
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create render output directory {}", parent.display()))?;
        }
        std::fs::write(&output, text)
            .with_context(|| format!("write render output {}", output.display()))?;
    } else {
        print!("{text}");
    }
    Ok(())
}

fn write_screenshot(value: Value, format: &str, output: Option<PathBuf>) -> Result<()> {
    match format {
        "json" => write_or_print_json(value, output),
        "svg" => {
            let svg = value
                .get("svg")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("screenshot response missing svg field"))?;
            if let Some(output) = output {
                if let Some(parent) = output.parent() {
                    std::fs::create_dir_all(parent).with_context(|| {
                        format!("create screenshot output directory {}", parent.display())
                    })?;
                }
                std::fs::write(&output, svg)
                    .with_context(|| format!("write screenshot SVG {}", output.display()))?;
            } else {
                print!("{svg}");
            }
            Ok(())
        }
        other => bail!("unsupported screenshot format: {other}"),
    }
}

fn read_screenshot_snapshot(path: &Path) -> Result<ScreenshotSnapshot> {
    let json = std::fs::read_to_string(path)
        .with_context(|| format!("read screenshot snapshot {}", path.display()))?;
    serde_json::from_str(&json)
        .with_context(|| format!("parse screenshot snapshot {}", path.display()))
}

fn read_replay_trace(path: &Path) -> Result<ReplayTrace> {
    let json = std::fs::read_to_string(path)
        .with_context(|| format!("read replay trace {}", path.display()))?;
    serde_json::from_str(&json).with_context(|| format!("parse replay trace {}", path.display()))
}

async fn start_background(socket: Option<PathBuf>, scenario: &str) -> Result<()> {
    let socket = resolve_socket(socket);
    let exe = std::env::current_exe()?;
    let mut command = std::process::Command::new(exe);
    command
        .arg("serve")
        .arg("--socket")
        .arg(&socket)
        .arg("--scenario")
        .arg(scenario);
    command.stdout(std::process::Stdio::null());
    command.stderr(std::process::Stdio::null());
    command.stdin(std::process::Stdio::null());
    let _child = command
        .spawn()
        .with_context(|| "spawn background simulator")?;

    for _ in 0..100 {
        if socket.exists() && request_status(&socket).await.is_ok() {
            println!("{}", socket.display());
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    bail!("simulator did not become ready at {}", socket.display())
}

fn unique_id() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}
