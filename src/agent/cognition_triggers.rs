//! Phase D — adaptive orchestration from the latent cognitive state.
//!
//! This is the **opt-in, empirically-gated, soft** consumer of the fused
//! cognitive state produced by the logit-lens + companion-Jacobian services
//! (`logitlens_server.py` `/fuse`). Given the turn's user input, it asks the
//! service for the current-vs-future belief trajectory and, *only* when the
//! heuristic flags a warn-level action (`verify` / `retrieve`), returns a short
//! system-reminder **prior** that is appended to the turn's reminder.
//!
//! Design guarantees (deliberate, per the project's caveats):
//!   * **Off by default.** Nothing runs unless `NEURA_COGNITION_TRIGGERS` is
//!     truthy, so there is zero added latency in the normal path.
//!   * **Soft only.** It never changes control flow, forces a tool, or blocks
//!     the model. It injects a clearly-labelled, unvalidated *prior* the model
//!     is free to ignore.
//!   * **Fail-quiet.** Any error, timeout, or unreachable service yields `None`.
//!   * **Conservative gate.** Only `warn`-level actions surface by default
//!     (tunable via `NEURA_COGNITION_MIN_LEVEL`).

use std::time::Duration;

/// Master switch (default OFF). Truthy values: 1/true/on/yes.
pub const TRIGGERS_ENABLE_ENV: &str = "NEURA_COGNITION_TRIGGERS";
/// Fusion service base URL (shared with the logit-lens observer).
pub const TRIGGERS_URL_ENV: &str = "NEURA_LOGITLENS_URL";
/// Minimum `suggested.level` that will surface a prior (ok < watch < warn).
pub const TRIGGERS_MIN_LEVEL_ENV: &str = "NEURA_COGNITION_MIN_LEVEL";
/// Bounded request timeout in milliseconds (default 6000).
pub const TRIGGERS_TIMEOUT_ENV: &str = "NEURA_COGNITION_TIMEOUT_MS";

const DEFAULT_URL: &str = "http://127.0.0.1:8801";
const DEFAULT_TIMEOUT_MS: u64 = 6_000;

fn enabled() -> bool {
    std::env::var(TRIGGERS_ENABLE_ENV)
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "on" | "yes"
            )
        })
        .unwrap_or(false)
}

fn level_rank(level: &str) -> u8 {
    match level.trim().to_ascii_lowercase().as_str() {
        "warn" => 2,
        "watch" => 1,
        _ => 0,
    }
}

/// The web UI wraps the real user message in a dapp-preview context block
/// (`[Neura UI — live dapp preview context]` … ending with `[User message]\n`
/// followed by the actual question — see `scripts/dapp_engine.py`). The
/// cognition probe must read the *clean question*, not the context block:
/// otherwise the answer-position logit-lens walk drifts onto context words
/// ("user", "dapp", …) and reads a false belief/confidence. When the marker is
/// present, return everything after the last `[User message]\n`; otherwise the
/// input is already clean. Kept here (rather than in the web UI) so the probe is
/// correct no matter which entry point drove the turn.
fn probe_text(user_text: &str) -> &str {
    const DAPP_MARKER: &str = "[Neura UI — live dapp preview context]";
    const USER_TAG: &str = "[User message]\n";
    if user_text.contains(DAPP_MARKER) {
        if let Some(idx) = user_text.rfind(USER_TAG) {
            let clean = user_text[idx + USER_TAG.len()..].trim();
            if !clean.is_empty() {
                return clean;
            }
        }
    }
    user_text.trim()
}

/// Opt-in Phase D probe. Returns a soft system-reminder prior when the fused
/// cognitive state flags an actionable (`verify`/`retrieve`) warn-level move for
/// `user_text`, else `None`. Returns `None` instantly when disabled.
pub async fn maybe_cognition_reminder(user_text: &str) -> Option<String> {
    if !enabled() {
        return None;
    }
    // Probe the clean user question, stripping any web-UI dapp-context wrapper.
    let text = probe_text(user_text);
    if text.is_empty() {
        return None;
    }

    let url = std::env::var(TRIGGERS_URL_ENV).unwrap_or_else(|_| DEFAULT_URL.to_string());
    let endpoint = format!("{}/fuse", url.trim_end_matches('/'));
    let timeout_ms = std::env::var(TRIGGERS_TIMEOUT_ENV)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_TIMEOUT_MS);
    let min_level = std::env::var(TRIGGERS_MIN_LEVEL_ENV).unwrap_or_else(|_| "warn".to_string());

    let client = reqwest::Client::new();
    let response = client
        .post(&endpoint)
        .timeout(Duration::from_millis(timeout_ms))
        .json(&serde_json::json!({ "text": text }))
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .ok()?;
    let v = response.json::<serde_json::Value>().await.ok()?;

    let suggested = v.get("suggested")?;
    let action = suggested
        .get("action")
        .and_then(|a| a.as_str())
        .unwrap_or("continue");
    let level = suggested
        .get("level")
        .and_then(|a| a.as_str())
        .unwrap_or("watch");
    if level_rank(level) < level_rank(&min_level) {
        return None;
    }

    // Only surface *actionable* moves; `continue` / `allow-early-final` are
    // informational and intentionally do not perturb the turn.
    let guidance = match action {
        "verify" => "double-check the key facts / reasoning before finalizing",
        "retrieve" => "gather more context (search or read the relevant files) before answering",
        _ => return None,
    };

    let reason = suggested
        .get("reason")
        .and_then(|a| a.as_str())
        .unwrap_or("");
    let cur = v.get("cur_belief").and_then(|a| a.as_str()).unwrap_or("");
    let fut = v.get("future_belief").and_then(|a| a.as_str()).unwrap_or("");
    let div = v.get("cur_future_divergence").and_then(|a| a.as_f64());
    let conv_pct = v.get("convergence").and_then(|a| a.as_f64()).unwrap_or(0.0) * 100.0;
    let entropy = v.get("mean_entropy").and_then(|a| a.as_f64()).unwrap_or(0.0);

    let comp = if fut.is_empty() {
        ""
    } else {
        " + companion Jacobian lens"
    };
    let belief = if cur.is_empty() || fut.is_empty() {
        String::new()
    } else {
        format!(" (belief now \"{cur}\" vs projected next \"{fut}\")")
    };
    let div_str = div
        .map(|d| format!(", divergence {d:.2}"))
        .unwrap_or_default();

    Some(format!(
        "[cognition — experimental heuristic prior, unvalidated]\n\
         A latent cognitive-state probe (real-model logit lens{comp}) of the current input suggests you may want to {guidance}.\n\
         Signal: {reason}{belief}{div_str}; convergence {conv_pct:.0}%, entropy {entropy:.2}.\n\
         Treat this ONLY as a weak prior — not an instruction. Use your own judgment about whether it applies."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;

    // These tests mutate process-global env; serialize them so parallel test
    // threads don't clobber each other's NEURA_COGNITION_* variables.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Minimal one-shot HTTP server: returns `body` as JSON to the first request
    /// on an ephemeral port, then exits. Good enough to exercise the real
    /// reqwest client path in `maybe_cognition_reminder`.
    fn spawn_mock_fuse(body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 8192];
                let _ = stream.read(&mut buf); // drain the (small) request
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{addr}")
    }

    /// Run `maybe_cognition_reminder` against a mock `/fuse` returning `body`,
    /// with the given enable flag and optional min-level, cleaning env after.
    fn run_against(body: &'static str, enable: &str, min_level: Option<&str>) -> Option<String> {
        let _guard = ENV_LOCK.lock().unwrap();
        let url = spawn_mock_fuse(body);
        // SAFETY: env access is serialized by ENV_LOCK for all env-touching tests.
        unsafe {
            std::env::set_var(TRIGGERS_ENABLE_ENV, enable);
            std::env::set_var(TRIGGERS_URL_ENV, &url);
            std::env::set_var(TRIGGERS_TIMEOUT_ENV, "4000");
            match min_level {
                Some(l) => std::env::set_var(TRIGGERS_MIN_LEVEL_ENV, l),
                None => std::env::remove_var(TRIGGERS_MIN_LEVEL_ENV),
            }
        }
        let out = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(maybe_cognition_reminder("some user input"));
        unsafe {
            std::env::remove_var(TRIGGERS_ENABLE_ENV);
            std::env::remove_var(TRIGGERS_URL_ENV);
            std::env::remove_var(TRIGGERS_TIMEOUT_ENV);
            std::env::remove_var(TRIGGERS_MIN_LEVEL_ENV);
        }
        out
    }

    const VERIFY_WARN: &str = r#"{"suggested":{"action":"verify","level":"warn","reason":"low commitment (final confidence 36%)"},"cur_belief":"w","future_belief":"tungsten","cur_future_divergence":0.4,"convergence":0.1,"mean_entropy":0.55}"#;
    const RETRIEVE_WARN: &str = r#"{"suggested":{"action":"retrieve","level":"warn","reason":"low commitment and conflict"},"cur_belief":"x","future_belief":"y","cur_future_divergence":1.0,"convergence":0.05,"mean_entropy":0.7}"#;
    const CONTINUE_WATCH: &str = r#"{"suggested":{"action":"continue","level":"watch","reason":"forming"},"cur_belief":"paris","future_belief":"paris","cur_future_divergence":0.2,"convergence":0.1,"mean_entropy":0.6}"#;
    const EARLY_FINAL_OK: &str = r#"{"suggested":{"action":"allow-early-final","level":"ok","reason":"committed"},"cur_belief":"paris","cur_future_divergence":0.2,"convergence":0.1,"mean_entropy":0.4}"#;
    const VERIFY_WATCH: &str = r#"{"suggested":{"action":"verify","level":"watch","reason":"weak"},"cur_belief":"a","cur_future_divergence":0.3,"convergence":0.1,"mean_entropy":0.5}"#;

    #[test]
    fn disabled_by_default_returns_none_fast() {
        // Even with a reachable (warn) service, the master switch off => None.
        let out = run_against(VERIFY_WARN, "0", None);
        assert!(out.is_none());
    }

    #[test]
    fn warn_verify_surfaces_reminder() {
        let out = run_against(VERIFY_WARN, "1", None).expect("warn/verify should surface a prior");
        assert!(out.contains("double-check"), "reminder was: {out}");
        assert!(out.contains("weak prior"), "must be labelled a weak prior: {out}");
        // The calibrated signal (final confidence) rides along via `reason`.
        assert!(out.contains("final confidence"), "should relay the reason: {out}");
    }

    #[test]
    fn warn_retrieve_surfaces_retrieve_guidance() {
        let out = run_against(RETRIEVE_WARN, "1", None).expect("warn/retrieve should surface");
        assert!(out.contains("gather more context"), "reminder was: {out}");
    }

    #[test]
    fn continue_and_early_final_do_not_surface() {
        assert!(run_against(CONTINUE_WATCH, "1", None).is_none());
        assert!(run_against(EARLY_FINAL_OK, "1", None).is_none());
    }

    #[test]
    fn min_level_gate_blocks_below_threshold() {
        // A verify action but only watch-level, with default min-level=warn => None.
        assert!(run_against(VERIFY_WATCH, "1", Some("warn")).is_none());
        // Lowering the min level to watch lets the same payload through.
        assert!(run_against(VERIFY_WATCH, "1", Some("watch")).is_some());
    }

    #[test]
    fn level_ordering() {
        assert!(level_rank("warn") > level_rank("watch"));
        assert!(level_rank("watch") > level_rank("ok"));
        assert_eq!(level_rank("anything-else"), 0);
    }

    #[test]
    fn probe_text_strips_dapp_wrapper() {
        // Clean input passes through untouched.
        assert_eq!(probe_text("What is the capital of France?"),
                   "What is the capital of France?");
        // Wrapped input yields only the question after the last [User message].
        let wrapped = "[Neura UI — live dapp preview context]\n\
                       some summary of the dapp\n\
                       The dapp lives at `.neura/dapp/`; edit when the user asks.\n\n\
                       [User message]\n\
                       Who won the Nobel Prize in Physics in 1912?";
        assert_eq!(probe_text(wrapped), "Who won the Nobel Prize in Physics in 1912?");
        // Marker present but (degenerate) empty question -> falls back, not empty.
        let empty_q = "[Neura UI — live dapp preview context]\nx\n\n[User message]\n   ";
        assert!(!probe_text(empty_q).is_empty());
    }
}
