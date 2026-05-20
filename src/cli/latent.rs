use crate::latent_learning::{
    LatentLearningState, convergence_metrics, counterfactual_probe, learning_state_path,
    remap_plan, render_learning_report,
};
use crate::latent_learning_background as background;
use crate::latent_memory::{LatentMemoryBank, latent_memory_path, render_memory_report};
use crate::latent_operational_recurrence::{
    LatentOperationalState, OperationalEvent, default_invariants, encode_event, remap_vector,
    render_report, state_path, translate_invariants,
};
use crate::live_operational_fabric as fabric;
use crate::operational_policy::{self, PolicyDomain};
use crate::policy_outcome_credit;
use anyhow::Context;
use serde_json::json;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum LatentCommand {
    Status,
    Vector,
    Observe {
        kind: String,
        outcome: String,
        tag: Vec<String>,
        tool: Option<String>,
        provider: Option<String>,
        weight: f32,
    },
    Translate {
        kind: String,
        outcome: String,
        tag: Vec<String>,
    },
    Drift,
    Remap {
        schema_version: u32,
    },
    Invariants,
    Provenance,
    Temporal,
    Influence {
        kind: String,
        outcome: String,
        tag: Vec<String>,
    },
    Report {
        output: Option<PathBuf>,
    },
    Learn {
        kind: String,
        outcome: String,
        tag: Vec<String>,
        tool: Option<String>,
        weight: f32,
    },
    LearnedVectors,
    Attractors,
    Counterfactual {
        kind: String,
        outcome: String,
        tag: Vec<String>,
        alternate_tag: Vec<String>,
    },
    Doctrine,
    Immune,
    Topology,
    Convergence,
    EvolutionReport {
        output: Option<PathBuf>,
    },
    Ingest {
        kind: String,
        outcome: String,
        tag: Vec<String>,
        tool: Option<String>,
        source: String,
    },
    LearnNow {
        limit: usize,
    },
    BackgroundStatus,
    Samples,
    Outcomes,
    Doctrines,
    Pause,
    Resume,
    FabricStatus,
    FabricEvents,
    FabricReport {
        output: Option<PathBuf>,
    },
    FabricPause,
    FabricResume,
    FabricPing,
    LatentMemoryStatus,
    LatentMemoryBlocks,
    LatentMemoryReport {
        output: Option<PathBuf>,
    },
    LatentMemoryUsefulness,
    PolicyStatus,
    PolicyRules,
    PolicyDecide {
        domain: String,
        target: String,
    },
    PolicyAudit,
    PolicyReport {
        output: Option<PathBuf>,
    },
    PolicyDomains,
    PolicyCreditReport {
        output: Option<PathBuf>,
    },
    PolicyCreditAssign {
        audit_id: String,
        outcome: String,
    },
}

pub fn run(command: LatentCommand) -> anyhow::Result<()> {
    let path = state_path();
    let mut state = LatentOperationalState::load_or_default(&path)
        .with_context(|| format!("loading latent state from {}", path.display()))?;

    match command {
        LatentCommand::Status => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "schema_version": state.schema_version,
                    "events_seen": state.events_seen,
                    "state_path": path,
                    "vector_magnitude": state.vector.magnitude(),
                    "drift": state.drift(),
                    "temporal_memory_len": state.temporal_memory.len(),
                    "invariants": state.invariants.len(),
                    "anti_sludge": state.anti_sludge_report(),
                }))?
            );
        }
        LatentCommand::Vector => println!("{}", serde_json::to_string_pretty(&state.vector)?),
        LatentCommand::Observe {
            kind,
            outcome,
            tag,
            tool,
            provider,
            weight,
        } => {
            let mut event = OperationalEvent::new(kind, outcome);
            event.tags = tag;
            event.tool = tool;
            event.provider = provider;
            event.weight = weight;
            let gate = state.observe(event);
            state.save(&path)?;
            println!("{}", serde_json::to_string_pretty(&gate)?);
        }
        LatentCommand::Translate { kind, outcome, tag } => {
            let mut event = OperationalEvent::new(kind, outcome);
            event.tags = tag;
            println!(
                "{}",
                serde_json::to_string_pretty(&translate_invariants(&event, &state.invariants))?
            );
        }
        LatentCommand::Drift => println!("{:.6}", state.drift()),
        LatentCommand::Remap { schema_version } => {
            let plan = remap_plan(&state.vector, schema_version);
            state.previous_vector = Some(state.vector.clone());
            state.vector = remap_vector(&state.vector, schema_version);
            state.schema_version = schema_version;
            state.save(&path)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "plan": plan,
                    "vector": state.vector,
                }))?
            );
        }
        LatentCommand::Invariants => {
            println!("{}", serde_json::to_string_pretty(&state.invariants)?)
        }
        LatentCommand::Provenance => {
            let records: Vec<_> = state
                .temporal_memory
                .iter()
                .map(|entry| &entry.provenance)
                .collect();
            println!("{}", serde_json::to_string_pretty(&records)?);
        }
        LatentCommand::Temporal => {
            println!("{}", serde_json::to_string_pretty(&state.temporal_memory)?)
        }
        LatentCommand::Influence { kind, outcome, tag } => {
            let mut event = OperationalEvent::new(kind, outcome);
            event.tags = tag;
            let encoded = encode_event(&event);
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "encoded": encoded,
                    "translations": translate_invariants(&event, &default_invariants()),
                    "similarity_to_current": encoded.cosine_similarity(&state.vector),
                }))?
            );
        }
        LatentCommand::Report { output } => {
            let rendered = render_report(&state);
            if let Some(output) = output {
                if let Some(parent) = output.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&output, rendered)?;
                println!("wrote {}", output.display());
            } else {
                println!("{rendered}");
            }
        }
        LatentCommand::Learn {
            kind,
            outcome,
            tag,
            tool,
            weight,
        } => {
            let learning_path = learning_state_path();
            let mut learning = LatentLearningState::load_or_default(&learning_path)?;
            let mut event = OperationalEvent::new(kind, outcome);
            event.tags = tag;
            event.tool = tool;
            event.weight = weight;
            let step = learning.learn(&state, event);
            learning.save(&learning_path)?;
            println!("{}", serde_json::to_string_pretty(&step)?);
        }
        LatentCommand::LearnedVectors => {
            let learning = LatentLearningState::load_or_default(&learning_state_path())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&learning.learned_vectors)?
            );
        }
        LatentCommand::Attractors => {
            let learning = LatentLearningState::load_or_default(&learning_state_path())?;
            println!("{}", serde_json::to_string_pretty(&learning.attractors)?);
        }
        LatentCommand::Counterfactual {
            kind,
            outcome,
            tag,
            alternate_tag,
        } => {
            let mut event = OperationalEvent::new(kind, outcome);
            event.tags = tag;
            println!(
                "{}",
                serde_json::to_string_pretty(&counterfactual_probe(&state, &event, alternate_tag))?
            );
        }
        LatentCommand::Doctrine => {
            let learning = LatentLearningState::load_or_default(&learning_state_path())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&learning.doctrine_bindings)?
            );
        }
        LatentCommand::Immune => {
            let learning = LatentLearningState::load_or_default(&learning_state_path())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&learning.immune_history)?
            );
        }
        LatentCommand::Topology => {
            let learning = LatentLearningState::load_or_default(&learning_state_path())?;
            println!("{}", serde_json::to_string_pretty(&learning.topology)?);
        }
        LatentCommand::Convergence => {
            let learning = LatentLearningState::load_or_default(&learning_state_path())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&convergence_metrics(&learning, &state))?
            );
        }
        LatentCommand::EvolutionReport { output } => {
            let learning = LatentLearningState::load_or_default(&learning_state_path())?;
            let rendered = render_learning_report(&learning, &state);
            if let Some(output) = output {
                if let Some(parent) = output.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&output, rendered)?;
                println!("wrote {}", output.display());
            } else {
                println!("{rendered}");
            }
        }
        LatentCommand::Ingest {
            kind,
            outcome,
            tag,
            tool,
            source,
        } => {
            let event = background::command_event(kind, outcome, tag, tool);
            let sample = background::ingest_runtime_event(event, source)?;
            println!("{}", serde_json::to_string_pretty(&sample)?);
        }
        LatentCommand::LearnNow { limit } => {
            let result = background::run_background_cycle(limit)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        LatentCommand::BackgroundStatus => {
            println!("{}", serde_json::to_string_pretty(&background::status()?)?);
        }
        LatentCommand::Samples => {
            println!("{}", serde_json::to_string_pretty(&background::samples()?)?);
        }
        LatentCommand::Outcomes => {
            println!(
                "{}",
                serde_json::to_string_pretty(&background::outcome_summary()?)?
            );
        }
        LatentCommand::Doctrines => {
            println!(
                "{}",
                serde_json::to_string_pretty(&background::doctrine_summary()?)?
            );
        }
        LatentCommand::Pause => {
            println!(
                "{}",
                serde_json::to_string_pretty(&background::set_paused(true)?)?
            );
        }
        LatentCommand::Resume => {
            println!(
                "{}",
                serde_json::to_string_pretty(&background::set_paused(false)?)?
            );
        }
        LatentCommand::FabricStatus => {
            println!("{}", serde_json::to_string_pretty(&fabric::status()?)?);
        }
        LatentCommand::FabricEvents => {
            println!("{}", serde_json::to_string_pretty(&fabric::events()?)?);
        }
        LatentCommand::FabricReport { output } => {
            let rendered = fabric::render_markdown_report()?;
            if let Some(output) = output {
                if let Some(parent) = output.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&output, rendered)?;
                println!("wrote {}", output.display());
            } else {
                println!("{rendered}");
            }
        }
        LatentCommand::FabricPause => {
            println!(
                "{}",
                serde_json::to_string_pretty(&fabric::set_paused(true)?)?
            );
        }
        LatentCommand::FabricResume => {
            println!(
                "{}",
                serde_json::to_string_pretty(&fabric::set_paused(false)?)?
            );
        }
        LatentCommand::FabricPing => {
            fabric::emit_system_ping("fabric-ping");
            println!("{}", serde_json::to_string_pretty(&fabric::status()?)?);
        }
        LatentCommand::LatentMemoryStatus => {
            let bank = LatentMemoryBank::load_or_default(&latent_memory_path())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "entries": bank.entries.len(),
                    "synthesis_records": bank.synthesis_records.len(),
                    "drift_threshold": bank.drift_threshold,
                    "path": latent_memory_path(),
                }))?
            );
        }
        LatentCommand::LatentMemoryBlocks => {
            let bank = LatentMemoryBank::load_or_default(&latent_memory_path())?;
            println!("{}", bank.rehydration_blocks(32, 0.05).join("\n"));
        }
        LatentCommand::LatentMemoryUsefulness => {
            let bank = LatentMemoryBank::load_or_default(&latent_memory_path())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&bank.usefulness_report())?
            );
        }
        LatentCommand::PolicyStatus => {
            println!(
                "{}",
                serde_json::to_string_pretty(&operational_policy::report()?)?
            );
        }
        LatentCommand::PolicyRules => {
            let state = operational_policy::load_policy_and_synthesize()?;
            println!("{}", serde_json::to_string_pretty(&state.rules)?);
        }
        LatentCommand::PolicyDecide { domain, target } => {
            let mut state = operational_policy::load_policy_and_synthesize()?;
            let domain = parse_policy_domain(&domain);
            let decision = state.decide(domain, &target);
            state.save(&operational_policy::policy_state_path())?;
            println!("{}", serde_json::to_string_pretty(&decision)?);
        }
        LatentCommand::PolicyAudit => {
            let state = operational_policy::load_policy_and_synthesize()?;
            println!("{}", serde_json::to_string_pretty(&state.audits)?);
        }
        LatentCommand::PolicyCreditReport { output } => {
            let rendered = policy_outcome_credit::render_credit_report()?;
            if let Some(output) = output {
                if let Some(parent) = output.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&output, rendered)?;
                println!("wrote {}", output.display());
            } else {
                println!("{rendered}");
            }
        }
        LatentCommand::PolicyCreditAssign { audit_id, outcome } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&policy_outcome_credit::assign_credit(
                    &audit_id, &outcome
                )?)?
            );
        }
        LatentCommand::PolicyDomains => {
            let report = operational_policy::report()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "active_runtime_domains": report.active_runtime_domains,
                    "policy_api_domains": report.policy_api_domains,
                    "rules_by_domain": report.rules_by_domain,
                }))?
            );
        }
        LatentCommand::PolicyReport { output } => {
            let rendered = operational_policy::render_policy_report()?;
            if let Some(output) = output {
                if let Some(parent) = output.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&output, rendered)?;
                println!("wrote {}", output.display());
            } else {
                println!("{rendered}");
            }
        }
        LatentCommand::LatentMemoryReport { output } => {
            let bank = LatentMemoryBank::load_or_default(&latent_memory_path())?;
            let rendered = render_memory_report(&bank);
            if let Some(output) = output {
                if let Some(parent) = output.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&output, rendered)?;
                println!("wrote {}", output.display());
            } else {
                println!("{rendered}");
            }
        }
    }
    Ok(())
}

fn parse_policy_domain(value: &str) -> PolicyDomain {
    match value.to_ascii_lowercase().as_str() {
        "tool" | "tool-selection" => PolicyDomain::ToolSelection,
        "provider" | "provider-choice" => PolicyDomain::ProviderChoice,
        "context" | "context-budget" => PolicyDomain::ContextBudget,
        "test" | "validation" | "test-validation" => PolicyDomain::TestValidation,
        "memory" | "memory-retrieval" => PolicyDomain::MemoryRetrieval,
        _ => PolicyDomain::DriftControl,
    }
}
