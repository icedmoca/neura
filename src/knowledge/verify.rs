//! Autonomous verification: after engineering work, check the result with
//! the project's own toolchain plus Neura's graph invariants, and fold the
//! outcome back into semantic memory as evidence.
//!
//! Deterministic and explainable: each check reports what ran, whether it
//! passed, and a bounded output excerpt. Results append a `Validation` block
//! to the evidence ledger and land as evidence on the repository concept —
//! repeated successful verification makes repository knowledge progressively
//! more trusted; failures make it appropriately uncertain.

use crate::memory::MemoryManager;
use crate::memory_graph::EvidenceRef;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Instant;

/// Bounded excerpt of a failing command's output.
const MAX_OUTPUT_CHARS: usize = 700;
/// Verification commands are bounded; a wedged build must not hang Neura.
const CHECK_TIMEOUT_SECS: u64 = 600;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationCheck {
    pub name: String,
    pub command: String,
    pub passed: bool,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VerificationReport {
    pub root: String,
    pub checks: Vec<VerificationCheck>,
    pub passed: bool,
}

/// Build/test commands for the repository, detected deterministically from
/// its manifest files. `with_tests` adds the (slower) test run.
pub fn detect_checks(root: &Path, with_tests: bool) -> Vec<(String, Vec<String>)> {
    let mut checks: Vec<(String, Vec<String>)> = Vec::new();
    if root.join("Cargo.toml").exists() {
        checks.push((
            "build".to_string(),
            vec!["cargo".into(), "check".into(), "--quiet".into()],
        ));
        if with_tests {
            checks.push((
                "tests".to_string(),
                vec!["cargo".into(), "test".into(), "--quiet".into()],
            ));
        }
    } else if root.join("package.json").exists() {
        checks.push((
            "build".to_string(),
            vec!["npm".into(), "run".into(), "--if-present".into(), "build".into()],
        ));
        if with_tests {
            checks.push((
                "tests".to_string(),
                vec!["npm".into(), "test".into(), "--silent".into()],
            ));
        }
    } else if root.join("pyproject.toml").exists() || root.join("setup.py").exists() {
        checks.push((
            "build".to_string(),
            vec!["python3".into(), "-m".into(), "compileall".into(), "-q".into(), ".".into()],
        ));
        if with_tests {
            checks.push(("tests".to_string(), vec!["python3".into(), "-m".into(), "pytest".into(), "-q".into()]));
        }
    }
    checks
}

fn run_command(root: &Path, argv: &[String]) -> (bool, String, u64) {
    let started = Instant::now();
    let mut cmd = std::process::Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .current_dir(root)
        .stdin(std::process::Stdio::null());
    let result = (|| -> std::io::Result<(bool, String)> {
        let mut child = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        let deadline = Instant::now() + std::time::Duration::from_secs(CHECK_TIMEOUT_SECS);
        loop {
            if let Some(status) = child.try_wait()? {
                let out = child.wait_with_output()?;
                let mut text = String::from_utf8_lossy(&out.stderr).to_string();
                if text.trim().is_empty() {
                    text = String::from_utf8_lossy(&out.stdout).to_string();
                }
                text.truncate(MAX_OUTPUT_CHARS);
                return Ok((status.success(), text));
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                return Ok((false, format!("timed out after {CHECK_TIMEOUT_SECS}s")));
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    })();
    let elapsed = started.elapsed().as_millis() as u64;
    match result {
        Ok((ok, text)) => (ok, text, elapsed),
        Err(e) => (false, e.to_string(), elapsed),
    }
}

/// Run autonomous verification for `root`:
///   1. toolchain checks detected from the repo manifest (build, optional tests);
///   2. memory-graph integrity (`validate_graphs`);
///   3. knowledge synchronization (registered sources have no pending drift).
/// Appends the report to the evidence ledger and records the outcome as
/// evidence on the repository concept.
pub async fn run_verification(
    manager: &MemoryManager,
    root: &Path,
    with_tests: bool,
) -> Result<VerificationReport> {
    let mut report = VerificationReport {
        root: root.display().to_string(),
        ..Default::default()
    };

    // ---- Toolchain checks ----
    for (name, argv) in detect_checks(root, with_tests) {
        let (passed, detail, duration_ms) = run_command(root, &argv);
        report.checks.push(VerificationCheck {
            name,
            command: argv.join(" "),
            passed,
            duration_ms,
            detail: if passed { None } else { Some(detail) },
        });
    }

    // ---- Graph integrity (existing validator) ----
    let started = Instant::now();
    let (project_issues, global_issues) = manager.validate_graphs()?;
    let clean = project_issues.is_empty() && global_issues.is_empty();
    report.checks.push(VerificationCheck {
        name: "graph-integrity".to_string(),
        command: "memory validate_graphs".to_string(),
        passed: clean,
        duration_ms: started.elapsed().as_millis() as u64,
        detail: if clean {
            None
        } else {
            Some(format!(
                "{} project / {} global issue(s)",
                project_issues.len(),
                global_issues.len()
            ))
        },
    });

    // ---- Knowledge synchronization (incremental refresh = the check) ----
    let started = Instant::now();
    let mut graph = manager.load_project_graph()?;
    let refreshed = super::refresh_sources_in_graph(&mut graph, super::IngestOptions::default()).await;
    let drift: usize = refreshed
        .iter()
        .map(|(_, r)| r.items_changed + r.items_removed)
        .sum();
    report.checks.push(VerificationCheck {
        name: "knowledge-sync".to_string(),
        command: "knowledge sync (incremental)".to_string(),
        passed: true,
        duration_ms: started.elapsed().as_millis() as u64,
        detail: if drift > 0 {
            Some(format!("{drift} item(s) had drifted; knowledge refreshed"))
        } else {
            None
        },
    });

    report.passed = report.checks.iter().all(|c| c.passed);

    // ---- Outcome becomes evidence on the repository concept(s) ----
    let repo_concept_ids: Vec<String> = graph
        .metadata
        .knowledge_sources
        .keys()
        .map(|source_id| super::unit_memory_id(source_id, super::repo::REPO_KEY))
        .collect();
    for id in repo_concept_ids {
        if report.passed {
            graph.record_fact_observation(
                &id,
                EvidenceRef::observation("autonomous verification passed"),
            );
        } else if let Some(m) = graph.get_memory_mut(&id) {
            m.decay_confidence(0.05);
            m.record_evidence(EvidenceRef::observation(format!(
                "verification failed: {}",
                report
                    .checks
                    .iter()
                    .filter(|c| !c.passed)
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    }
    manager.save_project_graph(&graph)?;

    if !cfg!(test) {
        let _ = crate::evidence_ledger::append_evidence(
            crate::evidence_ledger::EvidenceKind::Validation,
            "knowledge.verification",
            format!(
                "{}/{} checks passed",
                report.checks.iter().filter(|c| c.passed).count(),
                report.checks.len()
            ),
            None,
            Some(report.passed),
            &report,
        );
    }
    crate::memory_log::log_knowledge(
        "knowledge_verification",
        serde_json::json!({
            "passed": report.passed,
            "checks": report.checks.len(),
        }),
    );
    Ok(report)
}

pub fn render_report(report: &VerificationReport) -> String {
    let mut out = format!(
        "Autonomous verification: {} ({} checks)\n",
        if report.passed { "PASSED" } else { "FAILED" },
        report.checks.len()
    );
    for c in &report.checks {
        out.push_str(&format!(
            "  [{}] {} — {} ({} ms)\n",
            if c.passed { "ok" } else { "FAIL" },
            c.name,
            c.command,
            c.duration_ms
        ));
        if let Some(d) = &c.detail {
            for line in d.lines().take(6) {
                out.push_str(&format!("        {line}\n"));
            }
        }
    }
    out.push_str("Outcome recorded as evidence on the repository concept and in the ledger.\n");
    out
}
