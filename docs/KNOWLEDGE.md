# Unified knowledge sources & cognitive integration

Three milestones live here: **v0.12** (repositories become semantic concepts
in the existing memory graph), **v0.13** (the graph becomes the reasoning
substrate: architectural reasoning, impact analysis, per-turn priors,
predictive reasoning with reflection, and architectural intelligence), and
**v0.14** (engineering intelligence: goals, decisions, dependency-ordered
plans, autonomous verification, prediction calibration, architectural
evolution history, and continuous health).

Neura v0.12 introduces a unified knowledge layer: external origins of
knowledge — repositories first — are ingested as semantic concepts inside the
**existing** memory graph. There is no repository index, no separate vector
store, no parallel retrieval path. A knowledge source produces concepts,
typed relationships, and evidence; everything downstream (sleep cycles,
Hebbian association, community detection, consolidation, concept embeddings,
cascade retrieval, MMR selection, contradiction review) is the memory system
that already existed.

This document is source-backed: every path below corresponds to a file in
this repository.

## Layout

| Path | Responsibility |
| --- | --- |
| `src/knowledge/mod.rs` | `KnowledgeSource` trait, `SourceUnit`, per-source incremental state, shared ingest pipeline, sidecar abstraction queue. |
| `src/knowledge/repo.rs` | `RepositorySource`: repository → package/subsystem → module/doc/test concepts with `PartOf` / `DependsOn` / `Supports` edges and git-commit evidence. Static understanding is delegated to `src/agent/codebase_model.rs`. |
| `src/knowledge/evidence.rs` | Tool-execution outcomes queued as evidence and folded onto the concepts derived from the touched files. |
| `src/memory_graph.rs` | `GraphMetadata.knowledge_sources`: per-source incremental state persisted inside the graph itself (no side store). `SleepReport.knowledge_concepts_refreshed`. |
| `src/memory.rs` | `run_full_sleep_cycle` refreshes registered sources (and applies queued tool evidence) before graph maintenance, so repository knowledge stays live without a new scheduler. |
| `src/cli/args.rs`, `src/cli/commands.rs`, `src/cli/dispatch.rs` | `neura knowledge ingest / status / sync`. |
| `src/agent/turn_loops.rs`, `src/agent/turn_execution.rs` | Edit-family tool calls report success/failure into the evidence queue (cheap, fail-quiet, no I/O on the tool path). |
| `src/memory_log.rs` | `log_knowledge`: ingest/abstraction/evidence events land in the existing `memory-events-*.jsonl` stream. |

## The abstraction

A `KnowledgeSource` exposes a deterministic lifecycle:

1. **discover** — enumerate items (files) with structure fingerprints;
2. **extract** — turn changed items into `SourceUnit`s: concept-level
   statements (never raw file dumps) with tags, typed relations to other
   units, and evidence notes.

The shared pipeline (`ingest_source_into_graph`) owns everything else:
fingerprint diffing, deterministic memory ids (`mem-src-<hash>` — idempotent
like `mem-sem-<hash>`), upserting, retirement, embeddings, evidence, the
sidecar abstraction queue, and state persistence. Future sources
(conversations, documentation sets, APIs, websites, PDFs, logs) implement the
two methods and inherit the entire pipeline.

Design rules, mirrored from the memory graph:

- **Deterministic where possible.** Discovery, extraction, ids, and edges are
  pure functions of the source. The sidecar LLM is reserved for one job:
  upgrading structural summaries into architectural prose (responsibility,
  intent, integration), bounded per pass and resumable, with a structural
  fallback when the sidecar is off.
- **Incremental and resumable.** Only changed items are re-extracted, bounded
  by `IngestOptions::max_items_per_pass`; deferred items keep stale
  fingerprints and are picked up next pass.
- **Never delete.** Concepts whose backing items disappear are retired
  (`active = false`, tagged `retired`); aggregates survive while any backing
  item remains; a reappearing item reactivates the same deterministic id.
- **Never confidence without evidence.** Structural extraction, git commits
  (`git log -1` per changed file, bounded), and tool outcomes all land as
  `EvidenceRef`s and flow through the existing evidence→confidence machinery,
  including episodic→semantic promotion.

## Repository concepts

`RepositorySource` reuses the deterministic `CodebaseModel` walk and emits:

- one **repository** concept (project brief + README intro);
- **package/subsystem** concepts (`src/<dir>`, `crates/<name>`, top-level
  dirs) — `PartOf` the repository;
- **module** concepts per code file — doc-comment intent line, symbol
  inventory, key symbols, `PartOf` their package, `DependsOn` the modules
  they import;
- **documentation** concepts — title + first paragraph, `Supports` the
  modules whose paths they mention;
- **test** concepts — `Supports` the module they exercise (deterministic
  name mapping).

Module tags are deliberately minimal (`<stem>`, `pkg-<package>`) so the
existing tag-Jaccard co-occurrence bootstrap does not blanket-link a large
repository; typed edges carry the structure instead.

## Continuous learning

- `neura knowledge ingest [path]` registers a repository and runs the first
  pass. Ingesting Neura's own repository gives it a live semantic model of
  itself.
- Every `neura memory sleep` (and `neura knowledge sync`) refreshes all
  registered sources incrementally and folds queued tool evidence, then runs
  the normal maintenance pipeline over the refreshed concepts — communities,
  consolidation, contradiction review, concept-embedding refresh.
- Successful edits reinforce the touched concepts; failed edits append
  evidence and decay confidence slightly (`src/knowledge/evidence.rs`).

## Retrieval

Repository concepts are ordinary memories: keyword search, embedding
similarity, cascade retrieval, `memory reason concept/path/why`, and prompt
injection all apply unchanged. Concept-first retrieval falls out of the
design: what is stored *is* the concept; the file is evidence.

## Observability

- `neura knowledge status` — sources, concept counts, tracked items, pending
  abstraction, last-pass report.
- `neura knowledge ingest/sync --json` — full `IngestReport` per pass.
- `neura memory sleep` — reports `knowledge concepts refreshed`.
- `~/.neura/logs/memory-events-*.jsonl` — `knowledge_ingest`,
  `knowledge_abstracted`, `knowledge_tool_evidence` events.
- `neura memory graph / report / health` — repository concepts appear in the
  same graph views and integrity checks as all other memories.

## Architectural reasoning (v0.13)

`src/knowledge/reasoning.rs` reasons directly over the graph — no hosted
model in the loop:

- **Traces** (`neura knowledge reason <query>`): deterministic keyword seeds
  (embeddings assist discovery when the local model is available), cascade
  expansion through typed edges, ranked relations with weights/confidence,
  community membership, and the evidence notes behind each conclusion. Same
  graph + same query → same trace.
- **Impact analysis** (`neura knowledge impact <target>`): the typed-edge
  closure of a change — reverse `DependsOn` dependents, `PartOf` containers,
  test concepts that `Supports` the area (likely failing tests) — with
  per-hop confidence attenuation and an explicit uncertainty figure derived
  from edge evidence.
- **Per-turn architectural prior** (`turn_brief`, wired into
  `Agent::apply_cognition_prior` and the streaming turn path): when the
  project has registered knowledge sources and the input matches concepts,
  a compact, clearly-labelled brief (concepts, relations, likely impact,
  covering tests, confidence) is folded into the turn's system reminder.
  Deterministic, mtime-cached graph load off the async runtime, instant
  no-op otherwise; mirrors the existing cognition-trigger prior pattern.

## Predictive reasoning and reflection (v0.13)

Every turn brief records which concepts it expects the coming work to touch
(`TurnPrediction`, logged to memory events). When tool-outcome evidence is
folded back into the graph (sleep / `knowledge sync`),
`reflect_on_outcomes` compares prediction against reality: confirmed
expectations reinforce the concepts through `record_fact_observation`, and
the comparison (precision, confirmed/missed/unexpected) is appended to the
evidence ledger as a `Reflection` block — explicit history, never hidden
state, never overwritten. `neura knowledge reflect` shows pending
predictions and recent reflections.

## Architectural intelligence (v0.13)

`src/knowledge/insights.rs` computes read-only observations from the graph:
high-centrality concepts, coupling hotspots (dependency degree), strong and
weak communities, dead concepts (isolated, never used), duplicate
abstractions (near-identical content), and documentation drift (docs left
behind by the modules they support). `neura knowledge insights [--record]`
renders them and optionally appends them to the evidence ledger as an
`ArchitecturalInsight` block. Observations never modify code or the graph.

## Engineering intelligence (v0.14)

`src/knowledge/engineering.rs`, `src/knowledge/verify.rs`, and extensions to
reasoning/insights complete the engineering loop:

- **Goals** — goal *memories* already exist (`src/goal.rs` mirrors every goal
  into the graph as `goal:<id>` concepts). `sync_goals_into_graph` adds the
  missing half: `SimilarTo` edges from each active goal to the architectural
  concepts its text matches, refreshed on every maintenance pass, so
  reasoning traverses from "why" to "where". `neura knowledge goals`.
- **Architectural decisions** — `neura knowledge decision <text> --reasoning
  … --alternative … --tradeoffs … --assumption … --confidence …` preserves a
  decision as a first-class concept (tag `decision`) with `Supports` edges to
  the concepts it concerns, plus an `EngineeringDecision` evidence-ledger
  block. Turn briefs surface relevant prior decisions ("reuse before
  reinventing"); re-recording the same decision evolves the same concept.
- **Long-horizon plans** — `neura knowledge plan <topic>` decomposes a topic
  into dependency-ordered stages: seeds → impact closure → topological order
  over `DependsOn` (dependencies staged first, cycles fall back to stable
  order), each stage carrying rationale, covering tests, and confidence.
  Plans persist as evolving concepts (same topic → same id) linked to their
  stages; completed work reinforces them through the normal evidence loop.
- **Autonomous verification** — `neura knowledge verify [--tests]` runs the
  project's own toolchain (detected from Cargo.toml / package.json /
  pyproject.toml), graph integrity (`validate_graphs`), and knowledge
  synchronization; the outcome lands as a `Validation` ledger block and as
  evidence on the repository concept (repeated passes raise trust; failures
  decay it). Exit code reflects the result.
- **Adaptive planning (calibration)** — every reflection updates
  `GraphMetadata.prediction_stats` (EWMA precision); turn briefs and plans
  report historical prediction precision so expectations are calibrated by
  accumulated evidence.
- **Architectural evolution** — each changing ingest pass appends a bounded
  `EvolutionPoint` to the source state. `neura knowledge history` shows the
  per-source timeline; `neura knowledge history <query>` answers "when did
  this concept appear, how did it evolve, and why" from creation dates and
  the evidence chain (including git commits).
- **Continuous health** — `neura knowledge health` reports coupling, duplicate
  groups, doc coverage, abstraction coverage, confidence distribution,
  prediction precision, and co-change pairs (items that repeatedly change
  together across passes). Deterministic, read-only, explainable.
- **Multi-repository understanding** — multiple repositories register in one
  project graph with independent incremental state; reasoning, plans, and
  health span all of them while retirement/refresh respect source boundaries
  (covered by tests).

## Web UI: cognition mission control

The Neura web cockpit (`scripts/neuraui`, served from `ui/dist`) exposes the
knowledge layer through a Cognition panel (network-icon button in the
header). It is strictly read-only over the same files the runtime writes:

- `GET /api/knowledge?project=<path>` — sources + evolution history, totals
  (edge kinds, confidence distribution, communities), goals/decisions/plans,
  prediction calibration, last sleep report, consolidations, a degree-ranked
  explorable node list, the evidence-ledger tail, and `knowledge_*` events.
- `GET /api/knowledge/concept?project=<path>&id=<memory-id>` — one concept's
  content, tags, confidence/strength, communities, full evidence chain, and
  typed edges in/out (embeddings stripped).

The panel (`ui/src/CognitionPanel.tsx`) has five views: **overview** (goal
strip, sources with evolution sparklines, edge/confidence summaries,
decisions/plans, calibration gauge), **graph explorer** (searchable concepts,
a clickable radial neighbor map, evidence chain — traverse the graph like a
database), **cognition timeline** (memory events + ledger blocks mapped onto
loop stages), **prediction vs reality** (reflection blocks with precision
meters), and **sleep cycle** (last report visualized, consolidations
clickable into the explorer). The graph file for a project is discovered by
matching `knowledge_sources.locator`, with an mtime-based parse cache.

## Current limitations (deliberate scope)

- Repository is the only implemented source kind; the trait and pipeline are
  the extension point (`source_from_state` in `src/knowledge/mod.rs`).
- Call-graph analysis is not performed; dependencies come from the import
  graph in `CodebaseModel`.
- Aggregate (package) concepts refresh only on passes that process at least
  one changed item.
- On very small repositories a single detected community can consolidate
  aggressively; this is existing consolidation behavior and self-corrects as
  the graph grows.
