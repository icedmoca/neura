use crate::latent_operational_recurrence::{
    LatentOperationalState, OperationalEvent, default_invariants, encode_event, remap_vector,
    render_report, state_path, translate_invariants,
};
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
            state.previous_vector = Some(state.vector.clone());
            state.vector = remap_vector(&state.vector, schema_version);
            state.schema_version = schema_version;
            state.save(&path)?;
            println!("{}", serde_json::to_string_pretty(&state.vector)?);
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
    }
    Ok(())
}
