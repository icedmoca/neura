//! Launcher for the local Neura cockpit UI (`scripts/neuraui`).
//!
//! All slash-command / skill entry points (`/neuraui` in the TUI, the REPL skill
//! fallback, and the agent turn-execution fallback) funnel through [`launch`] so
//! they share one behaviour: reuse a server that is already up, otherwise spawn
//! the launcher and report the *actual* URL the server bound to (the server may
//! pick a different port via its own `find_port`, so hard-coding `:8768` lied).

use std::io::{BufRead, BufReader};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Default port the launcher prefers; kept in sync with `scripts/neura-ui-server.py`.
pub const DEFAULT_PORT: u16 = 8768;

fn script_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/neuraui")
}

/// Best-effort check that a server is already accepting connections on `port`.
fn already_running(port: u16) -> bool {
    let addr = format!("127.0.0.1:{port}");
    match addr.parse() {
        Ok(addr) => TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok(),
        Err(_) => false,
    }
}

/// Start (or reuse) the Neura UI server and return a human-readable status line
/// that always points at a URL the user can actually open.
pub fn launch() -> String {
    match try_launch() {
        Ok(url) => format!("Neura UI is running: {url}\nLive state API: {url}/api/state"),
        Err(msg) => msg,
    }
}

fn try_launch() -> Result<String, String> {
    if already_running(DEFAULT_PORT) {
        return Ok(format!("http://127.0.0.1:{DEFAULT_PORT}"));
    }

    let script = script_path();
    if !script.exists() {
        return Err(format!(
            "Neura UI launcher not found at {}.\nRun `scripts/neuraui` from the repo root, then open http://127.0.0.1:{DEFAULT_PORT}.",
            script.display()
        ));
    }

    let mut child = Command::new(&script)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| {
            format!(
                "Failed to start Neura UI via {}: {err}\nRun `scripts/neuraui` from the repo root, then open http://127.0.0.1:{DEFAULT_PORT}.",
                script.display()
            )
        })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Neura UI launcher produced no output stream.".to_string())?;

    // Read the server's announced `NEURA_UI_URL=...` on a detached thread so the
    // caller (often the TUI event loop) never blocks while the server boots or,
    // on first run, builds the UI bundle. Keep draining stdout until the launcher
    // exits so we do not close the pipe early (that would crash the Python server
    // on its next print with BrokenPipeError before it binds the HTTP port).
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _child = child;
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if let Some(url) = line.trim().strip_prefix("NEURA_UI_URL=") {
                let _ = tx.send(url.to_string());
            }
        }
    });

    match rx.recv_timeout(Duration::from_secs(4)) {
        Ok(url) => Ok(url),
        // Server is still coming up (e.g. building the bundle). Point at the
        // preferred port rather than blocking the UI any longer.
        Err(_) => Ok(format!(
            "http://127.0.0.1:{DEFAULT_PORT} (still starting — give it a moment, or run `scripts/neuraui` from the repo root to watch logs)"
        )),
    }
}
