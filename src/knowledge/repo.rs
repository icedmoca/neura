//! Repository knowledge source — the first [`KnowledgeSource`] implementation.
//!
//! Static understanding is delegated entirely to the existing deterministic
//! [`CodebaseModel`] (`src/agent/codebase_model.rs`): file walk, symbol
//! extraction, import graph. This module's job is *abstraction*: turning that
//! structure into concept-level [`SourceUnit`]s — repository, package /
//! subsystem, module, documentation, and test concepts — connected by typed
//! edges (`PartOf`, `DependsOn`, `Supports`, `SimilarTo`) with git commits as
//! evidence. Files are supporting evidence; concepts are the unit of
//! knowledge.

use super::{
    KnowledgeSource, KnowledgeSourceKind, SourceManifest, SourceUnit, UnitRelation, content_hash,
};
use crate::agent::codebase_model::{CodeFileModel, CodebaseModel, CodebaseModelBuilder, SymbolKind};
use crate::memory::MemoryCategory;
use crate::memory_graph::{EdgeKind, EvidenceRef};
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Max changed files for which we shell out for per-file git evidence.
const MAX_GIT_EVIDENCE_FILES: usize = 40;
/// Max bytes read when extracting a doc header / doc-comment intent line.
const MAX_HEADER_READ_BYTES: usize = 64 * 1024;

pub struct RepositorySource {
    root: PathBuf,
    /// CodebaseModel built during `discover`, reused by `extract`.
    model: Option<CodebaseModel>,
}

impl RepositorySource {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            model: None,
        }
    }

    fn canonical_root(&self) -> PathBuf {
        self.root
            .canonicalize()
            .unwrap_or_else(|_| self.root.clone())
    }

    fn repo_name(&self) -> String {
        self.canonical_root()
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "repository".to_string())
    }

    fn ensure_model(&mut self) -> Result<&CodebaseModel> {
        if self.model.is_none() {
            self.model = Some(CodebaseModelBuilder::new(&self.root).build()?);
        }
        Ok(self.model.as_ref().expect("model just built"))
    }
}

impl KnowledgeSource for RepositorySource {
    fn kind(&self) -> KnowledgeSourceKind {
        KnowledgeSourceKind::Repository
    }

    fn source_id(&self) -> String {
        format!("repo:{}", self.canonical_root().display())
    }

    fn display_name(&self) -> String {
        self.repo_name()
    }

    fn locator(&self) -> String {
        self.canonical_root().display().to_string()
    }

    fn discover(&mut self) -> Result<SourceManifest> {
        self.model = Some(CodebaseModelBuilder::new(&self.root).build()?);
        let model = self.model.as_ref().expect("model just built");
        let mut items = BTreeMap::new();
        for file in &model.files {
            let key = file.path.to_string_lossy().to_string();
            // Fingerprint the deterministic structural model of the file, so
            // only structural change (symbols, imports, size) re-extracts.
            let fp = content_hash(&serde_json::to_string(file).unwrap_or_default());
            items.insert(key, fp);
        }
        Ok(SourceManifest { items })
    }

    fn extract(
        &mut self,
        changed_items: &[String],
        manifest: &SourceManifest,
    ) -> Result<Vec<SourceUnit>> {
        let root = self.canonical_root();
        let name = self.repo_name();
        let model = self.ensure_model()?;

        let files_by_key: BTreeMap<String, &CodeFileModel> = model
            .files
            .iter()
            .map(|f| (f.path.to_string_lossy().to_string(), f))
            .collect();

        // Import edges indexed by source file, from the model's dependency graph.
        let mut deps_by_from: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for dep in &model.dependencies {
            deps_by_from
                .entry(dep.from.to_string_lossy().to_string())
                .or_default()
                .push(dep.to.to_string_lossy().to_string());
        }

        let mut units = Vec::new();

        // ---- Repository concept (always regenerated; cheap + idempotent) ----
        units.push(repository_unit(&root, &name, model));

        // ---- Package / subsystem concepts (always regenerated) ----
        let mut packages: BTreeMap<String, Vec<&CodeFileModel>> = BTreeMap::new();
        for file in &model.files {
            packages
                .entry(package_of(&file.path))
                .or_default()
                .push(file);
        }
        for (pkg, members) in &packages {
            units.push(package_unit(&name, pkg, members));
        }

        // ---- Per-file concepts, only for changed items ----
        let git_head = git_head_summary(&root);
        for (index, item) in changed_items.iter().enumerate() {
            let Some(file) = files_by_key.get(item) else {
                continue;
            };
            let deps = deps_by_from.get(item).cloned().unwrap_or_default();
            let mut unit = file_unit(&root, &name, file, &deps, manifest);

            // Git history becomes evidence, not merely metadata.
            if index < MAX_GIT_EVIDENCE_FILES {
                if let Some(commit) = git_last_commit_for(&root, item) {
                    unit.evidence.push(EvidenceRef::observation(commit));
                }
            } else if let Some(head) = &git_head {
                unit.evidence.push(EvidenceRef::observation(head.clone()));
            }
            units.push(unit);
        }

        Ok(units)
    }
}

/// Concept key for a code file.
pub fn module_key(path: &str) -> String {
    format!("module:{path}")
}

/// Concept key for a documentation file.
pub fn doc_key(path: &str) -> String {
    format!("doc:{path}")
}

/// Concept key for a test file.
pub fn test_key(path: &str) -> String {
    format!("test:{path}")
}

/// Concept key for a package / subsystem.
pub fn package_key(pkg: &str) -> String {
    format!("package:{pkg}")
}

/// Repository-level concept key.
pub const REPO_KEY: &str = "repo";

/// Which top-level package / subsystem a file belongs to. Mirrors how this
/// project (and most) organize source: `src/<dir>` and `crates/<name>` are
/// subsystems; other top-level dirs (ui, docs, scripts, tests) stand alone.
fn package_of(path: &Path) -> String {
    let comps: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    match comps.first().map(String::as_str) {
        Some("crates") if comps.len() > 1 => format!("crates/{}", comps[1]),
        Some("src") if comps.len() > 2 => format!("src/{}", comps[1]),
        Some(first) => first.to_string(),
        None => "root".to_string(),
    }
}

/// Human module path for a code file: `src/agent/turn_loops.rs` →
/// `agent::turn_loops`; non-Rust files keep a readable path stem.
fn module_display(path: &Path) -> String {
    let no_ext = path.with_extension("");
    let comps: Vec<String> = no_ext
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    let trimmed: Vec<&str> = comps
        .iter()
        .map(String::as_str)
        .filter(|c| *c != "src")
        .collect();
    if path.extension().and_then(|e| e.to_str()) == Some("rs") {
        trimmed.join("::")
    } else {
        trimmed.join("/")
    }
}

/// Last path-ish name used as a lightweight tag (mod.rs → parent dir name).
fn stem_tag(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    if stem == "mod" || stem == "lib" || stem == "index" {
        path.parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or(stem)
    } else {
        stem
    }
}

fn is_test_file(path: &str) -> bool {
    path.starts_with("tests/")
        || path.contains("/tests/")
        || path.ends_with("_tests.rs")
        || path.ends_with("_test.rs")
        || path.ends_with(".test.ts")
        || path.ends_with(".test.tsx")
        || path.ends_with(".spec.ts")
}

fn is_doc_file(path: &str) -> bool {
    path.ends_with(".md")
}

fn repository_unit(root: &Path, name: &str, model: &CodebaseModel) -> SourceUnit {
    let mut content = format!("Repository {name}: {}", model.project_brief());
    if let Some(intro) = read_markdown_intro(&root.join("README.md")) {
        content.push_str(&format!(" {intro}"));
    }
    SourceUnit {
        key: REPO_KEY.to_string(),
        content,
        category: MemoryCategory::Custom("architecture".to_string()),
        tags: vec!["repo".to_string(), name.to_string()],
        derived_from_items: Vec::new(),
        relations: Vec::new(),
        evidence: vec![EvidenceRef::observation(format!(
            "deterministic repository scan of {}",
            root.display()
        ))],
        wants_abstraction: false,
    }
}

fn package_unit(repo_name: &str, pkg: &str, members: &[&CodeFileModel]) -> SourceUnit {
    let total_symbols: usize = members.iter().map(|f| f.symbols.len()).sum();
    let total_lines: usize = members.iter().map(|f| f.line_count).sum();

    // Principal modules: largest by symbol count, deterministic order.
    let mut ranked: Vec<&&CodeFileModel> = members.iter().collect();
    ranked.sort_by(|a, b| {
        b.symbols
            .len()
            .cmp(&a.symbols.len())
            .then_with(|| a.path.cmp(&b.path))
    });
    let principal: Vec<String> = ranked
        .iter()
        .take(6)
        .map(|f| module_display(&f.path))
        .collect();

    let content = format!(
        "Subsystem {pkg} of {repo_name}: {} files, {} symbols, {} lines. Principal modules: {}.",
        members.len(),
        total_symbols,
        total_lines,
        principal.join(", "),
    );

    // The package derives from all member files; it is only retired when
    // every member disappears (the pipeline checks remaining backing items).
    let derived: Vec<String> = members
        .iter()
        .map(|f| f.path.to_string_lossy().to_string())
        .collect();

    SourceUnit {
        key: package_key(pkg),
        content,
        category: MemoryCategory::Custom("architecture".to_string()),
        tags: vec!["subsystem".to_string(), pkg.to_string()],
        derived_from_items: derived,
        relations: vec![UnitRelation {
            kind: EdgeKind::PartOf,
            target_key: REPO_KEY.to_string(),
            weight: 0.8,
        }],
        evidence: vec![EvidenceRef::observation(format!(
            "package aggregation over {} files",
            members.len()
        ))],
        wants_abstraction: true,
    }
}

/// Build the concept unit for a single file: module, documentation, or test.
fn file_unit(
    root: &Path,
    _repo_name: &str,
    file: &CodeFileModel,
    deps: &[String],
    manifest: &SourceManifest,
) -> SourceUnit {
    let path_str = file.path.to_string_lossy().to_string();
    let pkg = package_of(&file.path);
    let stem = stem_tag(&file.path);

    if is_doc_file(&path_str) {
        return doc_unit(root, file, &path_str, &pkg, &stem, manifest);
    }
    if is_test_file(&path_str) {
        return test_unit(file, &path_str, &pkg, &stem, manifest);
    }

    // ---- Module concept ----
    let display = module_display(&file.path);
    let intent = read_code_intent(&root.join(&file.path));

    let mut kind_counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for s in &file.symbols {
        *kind_counts.entry(symbol_kind_label(s.kind)).or_default() += 1;
    }
    let kind_summary = kind_counts
        .iter()
        .map(|(k, n)| format!("{n} {k}"))
        .collect::<Vec<_>>()
        .join(", ");

    // Key symbols: types first (they define the vocabulary), then functions.
    let mut key_symbols: Vec<&str> = file
        .symbols
        .iter()
        .filter(|s| {
            matches!(
                s.kind,
                SymbolKind::Struct | SymbolKind::Enum | SymbolKind::Trait | SymbolKind::Component
            )
        })
        .map(|s| s.name.as_str())
        .collect();
    key_symbols.extend(
        file.symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Function)
            .map(|s| s.name.as_str()),
    );
    key_symbols.dedup();
    key_symbols.truncate(8);

    let mut content = format!("Module {display} ({path_str}, {:?})", file.language);
    if let Some(intent) = &intent {
        content.push_str(&format!(": {intent}"));
    }
    content.push('.');
    if !kind_summary.is_empty() {
        content.push_str(&format!(" Contains {kind_summary}."));
    }
    if !key_symbols.is_empty() {
        content.push_str(&format!(" Key symbols: {}.", key_symbols.join(", ")));
    }
    if !deps.is_empty() {
        let dep_names: Vec<String> = deps
            .iter()
            .take(6)
            .map(|d| module_display(Path::new(d)))
            .collect();
        content.push_str(&format!(" Depends on {}.", dep_names.join(", ")));
    }

    let mut relations = vec![UnitRelation {
        kind: EdgeKind::PartOf,
        target_key: package_key(&pkg),
        weight: 0.8,
    }];
    for dep in deps.iter().take(24) {
        if manifest.items.contains_key(dep) {
            relations.push(UnitRelation {
                kind: EdgeKind::DependsOn,
                target_key: module_key(dep),
                weight: 0.7,
            });
        }
    }

    SourceUnit {
        key: module_key(&path_str),
        content,
        category: MemoryCategory::Custom("architecture".to_string()),
        tags: vec![stem, format!("pkg-{}", pkg.replace('/', "-"))],
        derived_from_items: vec![path_str.clone()],
        relations,
        evidence: vec![EvidenceRef::observation(format!(
            "structural extraction: {} symbols, {} lines in {path_str}",
            file.symbols.len(),
            file.line_count
        ))],
        wants_abstraction: file.symbols.len() >= 3,
    }
}

fn doc_unit(
    root: &Path,
    file: &CodeFileModel,
    path_str: &str,
    pkg: &str,
    stem: &str,
    manifest: &SourceManifest,
) -> SourceUnit {
    let intro = read_markdown_intro(&root.join(&file.path))
        .unwrap_or_else(|| format!("{} lines of documentation", file.line_count));
    let content = format!("Documentation {path_str}: {intro}");

    let mut relations = vec![UnitRelation {
        kind: EdgeKind::PartOf,
        target_key: package_key(pkg),
        weight: 0.7,
    }];
    // Docs that mention concrete source paths support those modules.
    for mentioned in read_mentioned_source_paths(&root.join(&file.path)) {
        if manifest.items.contains_key(&mentioned) {
            relations.push(UnitRelation {
                kind: EdgeKind::Supports,
                target_key: module_key(&mentioned),
                weight: 0.6,
            });
        }
    }

    SourceUnit {
        key: doc_key(path_str),
        content,
        category: MemoryCategory::Custom("architecture".to_string()),
        tags: vec![stem.to_string(), "docs".to_string()],
        derived_from_items: vec![path_str.to_string()],
        relations,
        evidence: vec![EvidenceRef::observation(format!(
            "documentation extraction from {path_str}"
        ))],
        wants_abstraction: false,
    }
}

fn test_unit(
    file: &CodeFileModel,
    path_str: &str,
    pkg: &str,
    stem: &str,
    manifest: &SourceManifest,
) -> SourceUnit {
    let content = format!(
        "Tests {path_str}: {} test-side symbols over {} lines.",
        file.symbols.len(),
        file.line_count
    );

    let mut relations = vec![UnitRelation {
        kind: EdgeKind::PartOf,
        target_key: package_key(pkg),
        weight: 0.6,
    }];
    // Tests support the module they exercise (deterministic name mapping).
    if let Some(target) = guess_module_under_test(path_str, manifest) {
        relations.push(UnitRelation {
            kind: EdgeKind::Supports,
            target_key: module_key(&target),
            weight: 0.8,
        });
    }

    SourceUnit {
        key: test_key(path_str),
        content,
        category: MemoryCategory::Custom("architecture".to_string()),
        tags: vec![stem.to_string(), "tests".to_string()],
        derived_from_items: vec![path_str.to_string()],
        relations,
        evidence: vec![EvidenceRef::observation(format!(
            "test extraction from {path_str}"
        ))],
        wants_abstraction: false,
    }
}

fn symbol_kind_label(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Function => "functions",
        SymbolKind::Struct => "structs",
        SymbolKind::Enum => "enums",
        SymbolKind::Trait => "traits",
        SymbolKind::Module => "modules",
        SymbolKind::Component => "components",
        SymbolKind::Type => "types",
        SymbolKind::Constant => "constants",
    }
}

/// Map a test file to the module it most plausibly exercises:
/// `src/foo_tests.rs` → `src/foo.rs`, `tests/foo_tests.rs` → `src/foo.rs`.
fn guess_module_under_test(path: &str, manifest: &SourceManifest) -> Option<String> {
    let stripped = path
        .trim_end_matches(".rs")
        .trim_end_matches("_tests")
        .trim_end_matches("_test");
    let candidates = [
        format!("{stripped}.rs"),
        format!(
            "src/{}.rs",
            stripped.trim_start_matches("tests/").trim_start_matches("src/")
        ),
    ];
    candidates
        .into_iter()
        .find(|c| c != path && manifest.items.contains_key(c))
}

/// First doc-comment / comment block of a source file, as the module's stated
/// intent. Deterministic; returns a compact single line.
fn read_code_intent(path: &Path) -> Option<String> {
    let text = read_head(path)?;
    let mut intent = String::new();
    for line in text.lines().take(30) {
        let line = line.trim();
        let doc = line
            .strip_prefix("//!")
            .or_else(|| line.strip_prefix("///"))
            .or_else(|| line.strip_prefix("/**"))
            .or_else(|| line.strip_prefix("*"))
            .or_else(|| line.strip_prefix("//"));
        match doc {
            Some(d) => {
                let d = d.trim().trim_start_matches('!').trim();
                if d.is_empty() {
                    if !intent.is_empty() {
                        break;
                    }
                } else {
                    if !intent.is_empty() {
                        intent.push(' ');
                    }
                    intent.push_str(d);
                    if intent.len() > 220 {
                        break;
                    }
                }
            }
            None => {
                if !intent.is_empty() || !line.is_empty() {
                    break;
                }
            }
        }
    }
    let intent: String = intent.chars().take(240).collect();
    if intent.is_empty() { None } else { Some(intent) }
}

/// Title + first paragraph of a markdown file, compacted to one line.
fn read_markdown_intro(path: &Path) -> Option<String> {
    let text = read_head(path)?;
    let mut title = String::new();
    let mut para = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            if !para.is_empty() {
                break;
            }
            continue;
        }
        if line.starts_with('#') && title.is_empty() {
            title = line.trim_start_matches('#').trim().to_string();
            continue;
        }
        if line.starts_with("![") || line.starts_with('<') || line.starts_with("```") {
            continue;
        }
        if !para.is_empty() {
            para.push(' ');
        }
        para.push_str(line);
        if para.len() > 240 {
            break;
        }
    }
    let mut out = String::new();
    if !title.is_empty() {
        out.push_str(&title);
        out.push_str(": ");
    }
    out.push_str(&para.chars().take(240).collect::<String>());
    let out = out.trim().trim_end_matches(':').trim().to_string();
    if out.is_empty() { None } else { Some(out) }
}

/// Source paths (`src/...`, `crates/...`) mentioned in a doc file — used to
/// create deterministic doc→module `Supports` edges.
fn read_mentioned_source_paths(path: &Path) -> Vec<String> {
    let Some(text) = read_head(path) else {
        return Vec::new();
    };
    let mut found: Vec<String> = Vec::new();
    for token in text.split(|c: char| {
        !(c.is_ascii_alphanumeric() || c == '/' || c == '.' || c == '_' || c == '-')
    }) {
        if (token.starts_with("src/") || token.starts_with("crates/"))
            && (token.ends_with(".rs") || token.ends_with(".ts") || token.ends_with(".tsx"))
            && !found.iter().any(|f| f == token)
        {
            found.push(token.to_string());
            if found.len() >= 16 {
                break;
            }
        }
    }
    found
}

fn read_head(path: &Path) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    let slice = &data[..data.len().min(MAX_HEADER_READ_BYTES)];
    Some(String::from_utf8_lossy(slice).to_string())
}

/// `<short-hash>: <subject>` of the last commit touching `path`, if any.
fn git_last_commit_for(root: &Path, path: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["log", "-1", "--format=%h %s", "--"])
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if line.is_empty() {
        None
    } else {
        Some(format!("commit {line} (touches {path})"))
    }
}

/// HEAD summary used as coarse evidence when per-file lookups are over budget.
fn git_head_summary(root: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["log", "-1", "--format=%h %s"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if line.is_empty() {
        None
    } else {
        Some(format!("repository at commit {line}"))
    }
}
