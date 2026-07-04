use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// A compact, deterministic knowledge model of a repository.
///
/// This is intentionally local-first: it does not call an embedding provider.
/// Other Neura models/sidecars can load the JSON snapshot to understand the
/// codebase layout, important symbols, and coarse dependency graph before they
/// open individual files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodebaseModel {
    pub root: PathBuf,
    pub files: Vec<CodeFileModel>,
    pub dependencies: Vec<CodeDependency>,
    pub symbol_index: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodeFileModel {
    pub path: PathBuf,
    pub language: CodeLanguage,
    pub line_count: usize,
    pub byte_count: usize,
    pub symbols: Vec<CodeSymbol>,
    pub imports: Vec<String>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodeSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodeDependency {
    pub from: PathBuf,
    pub to: PathBuf,
    pub via: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum CodeLanguage {
    Rust,
    TypeScript,
    JavaScript,
    Css,
    Markdown,
    Toml,
    Json,
    Shell,
    Other,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum SymbolKind {
    Function,
    Struct,
    Enum,
    Trait,
    Module,
    Component,
    Type,
    Constant,
}

#[derive(Debug, Clone)]
pub struct CodebaseModelBuilder {
    root: PathBuf,
    max_file_bytes: usize,
}


/// Build the codebase model for `root` and save it to the default Neura
/// snapshot path: `.neura/codebase-model.json`.
pub fn refresh_default_codebase_model(root: impl Into<PathBuf>) -> Result<CodebaseModel> {
    let root = root.into();
    let model = CodebaseModelBuilder::new(&root).build()?;
    model.save_json(root.join(".neura/codebase-model.json"))?;
    Ok(model)
}

impl CodebaseModelBuilder {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            max_file_bytes: 256 * 1024,
        }
    }

    pub fn max_file_bytes(mut self, max_file_bytes: usize) -> Self {
        self.max_file_bytes = max_file_bytes;
        self
    }

    pub fn build(&self) -> Result<CodebaseModel> {
        let root = self
            .root
            .canonicalize()
            .with_context(|| format!("failed to canonicalize codebase root {}", self.root.display()))?;

        let mut files = Vec::new();
        visit_files(&root, &root, self.max_file_bytes, &mut files)?;
        files.sort_by(|left, right| left.path.cmp(&right.path));

        let dependencies = infer_dependencies(&files);
        let symbol_index = build_symbol_index(&files);

        Ok(CodebaseModel {
            root,
            files,
            dependencies,
            symbol_index,
        })
    }
}

impl CodebaseModel {
    pub fn save_json(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create codebase model directory {}", parent.display()))?;
        }

        let json = serde_json::to_string_pretty(self).context("failed to serialize codebase model")?;
        fs::write(path, json)
            .with_context(|| format!("failed to write codebase model {}", path.display()))
    }

    pub fn load_json(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let json = fs::read_to_string(path)
            .with_context(|| format!("failed to read codebase model {}", path.display()))?;
        serde_json::from_str(&json).context("failed to parse codebase model")
    }

    pub fn project_brief(&self) -> String {
        let mut language_counts: BTreeMap<CodeLanguage, usize> = BTreeMap::new();
        for file in &self.files {
            *language_counts.entry(file.language).or_default() += 1;
        }

        let languages = language_counts
            .into_iter()
            .map(|(language, count)| format!("{language:?}: {count}"))
            .collect::<Vec<_>>()
            .join(", ");

        format!(
            "{} files, {} dependency edges, {} indexed symbols. Languages: {}.",
            self.files.len(),
            self.dependencies.len(),
            self.symbol_index.len(),
            languages
        )
    }
}

fn visit_files(root: &Path, dir: &Path, max_file_bytes: usize, files: &mut Vec<CodeFileModel>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read directory {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();

        if should_ignore(&file_name) {
            continue;
        }

        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            visit_files(root, &path, max_file_bytes, files)?;
        } else if metadata.is_file() && metadata.len() as usize <= max_file_bytes {
            if let Some(model) = model_file(root, &path)? {
                files.push(model);
            }
        }
    }

    Ok(())
}

fn model_file(root: &Path, path: &Path) -> Result<Option<CodeFileModel>> {
    let language = language_for(path);
    if language == CodeLanguage::Other {
        return Ok(None);
    }

    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return Ok(None),
    };

    let relative_path = path.strip_prefix(root).unwrap_or(path).to_path_buf();
    let symbols = extract_symbols(language, &content);
    let imports = extract_imports(language, &content);
    let line_count = content.lines().count();
    let byte_count = content.len();
    let summary = summarize_file(&relative_path, language, line_count, &symbols, &imports);

    Ok(Some(CodeFileModel {
        path: relative_path,
        language,
        line_count,
        byte_count,
        symbols,
        imports,
        summary,
    }))
}

fn should_ignore(name: &str) -> bool {
    matches!(
        name,
        ".git" | "target" | "node_modules" | "dist" | ".next" | ".turbo" | "__pycache__"
    )
}

fn language_for(path: &Path) -> CodeLanguage {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("rs") => CodeLanguage::Rust,
        Some("ts" | "tsx") => CodeLanguage::TypeScript,
        Some("js" | "jsx") => CodeLanguage::JavaScript,
        Some("css") => CodeLanguage::Css,
        Some("md") => CodeLanguage::Markdown,
        Some("toml") => CodeLanguage::Toml,
        Some("json") => CodeLanguage::Json,
        Some("sh") => CodeLanguage::Shell,
        _ => CodeLanguage::Other,
    }
}

fn extract_symbols(language: CodeLanguage, content: &str) -> Vec<CodeSymbol> {
    content
        .lines()
        .enumerate()
        .filter_map(|(index, line)| extract_symbol(language, line).map(|(kind, name)| CodeSymbol {
            name,
            kind,
            line: index + 1,
        }))
        .collect()
}

fn extract_symbol(language: CodeLanguage, line: &str) -> Option<(SymbolKind, String)> {
    let line = line.trim_start();
    match language {
        CodeLanguage::Rust => extract_rust_symbol(line),
        CodeLanguage::TypeScript | CodeLanguage::JavaScript => extract_script_symbol(line),
        _ => None,
    }
}

fn extract_rust_symbol(line: &str) -> Option<(SymbolKind, String)> {
    let line = line.strip_prefix("pub ").unwrap_or(line);
    for (prefix, kind) in [
        ("fn ", SymbolKind::Function),
        ("async fn ", SymbolKind::Function),
        ("struct ", SymbolKind::Struct),
        ("enum ", SymbolKind::Enum),
        ("trait ", SymbolKind::Trait),
        ("mod ", SymbolKind::Module),
        ("const ", SymbolKind::Constant),
        ("static ", SymbolKind::Constant),
        ("type ", SymbolKind::Type),
    ] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return Some((kind, identifier(rest)?));
        }
    }
    None
}

fn extract_script_symbol(line: &str) -> Option<(SymbolKind, String)> {
    for prefix in ["export function ", "function "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            let name = identifier(rest)?;
            let kind = if name.chars().next().is_some_and(char::is_uppercase) {
                SymbolKind::Component
            } else {
                SymbolKind::Function
            };
            return Some((kind, name));
        }
    }

    for prefix in ["export type ", "type "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return Some((SymbolKind::Type, identifier(rest)?));
        }
    }

    for prefix in ["export const ", "const "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return Some((SymbolKind::Constant, identifier(rest)?));
        }
    }

    None
}

fn identifier(text: &str) -> Option<String> {
    let ident = text
        .split(|character: char| !(character == '_' || character == '-' || character.is_ascii_alphanumeric()))
        .find(|part| !part.is_empty())?;
    Some(ident.to_string())
}

fn extract_imports(language: CodeLanguage, content: &str) -> Vec<String> {
    let mut imports = BTreeSet::new();
    for line in content.lines().map(str::trim) {
        match language {
            CodeLanguage::Rust => {
                if let Some(rest) = line.strip_prefix("use ") {
                    imports.insert(rest.trim_end_matches(';').to_string());
                } else if let Some(rest) = line.strip_prefix("mod ") {
                    imports.insert(rest.trim_end_matches(';').to_string());
                }
            }
            CodeLanguage::TypeScript | CodeLanguage::JavaScript => {
                if let Some(import_path) = line.split(" from ").nth(1).and_then(quoted_path) {
                    imports.insert(import_path);
                } else if let Some(import_path) = line.strip_prefix("import ").and_then(quoted_path) {
                    imports.insert(import_path);
                }
            }
            _ => {}
        }
    }
    imports.into_iter().collect()
}

fn quoted_path(text: &str) -> Option<String> {
    let text = text.trim().trim_end_matches(';');
    let quote = text.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let end = text[1..].find(quote)? + 1;
    Some(text[1..end].to_string())
}

fn summarize_file(
    path: &Path,
    language: CodeLanguage,
    line_count: usize,
    symbols: &[CodeSymbol],
    imports: &[String],
) -> String {
    let symbol_preview = symbols
        .iter()
        .take(5)
        .map(|symbol| symbol.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{} is a {:?} file with {} lines, {} imports, and {} symbols{}.",
        path.display(),
        language,
        line_count,
        imports.len(),
        symbols.len(),
        if symbol_preview.is_empty() {
            String::new()
        } else {
            format!(" including {symbol_preview}")
        }
    )
}

fn infer_dependencies(files: &[CodeFileModel]) -> Vec<CodeDependency> {
    let paths = files
        .iter()
        .map(|file| file.path.clone())
        .collect::<BTreeSet<_>>();
    let mut dependencies = Vec::new();

    for file in files {
        for import in &file.imports {
            if let Some(to) = resolve_import(&file.path, import, &paths) {
                dependencies.push(CodeDependency {
                    from: file.path.clone(),
                    to,
                    via: import.clone(),
                });
            }
        }
    }

    dependencies
}

fn resolve_import(from: &Path, import: &str, paths: &BTreeSet<PathBuf>) -> Option<PathBuf> {
    if import.starts_with("crate::") {
        let module = import.trim_start_matches("crate::").replace("::", "/");
        return resolve_candidates(Path::new("src"), &module, paths);
    }

    if import.starts_with("./") || import.starts_with("../") {
        let base = from.parent().unwrap_or_else(|| Path::new(""));
        let joined = normalize_relative_path(base.join(import));
        return resolve_candidates(Path::new(""), joined.to_str()?, paths);
    }

    None
}

fn resolve_candidates(prefix: &Path, module: &str, paths: &BTreeSet<PathBuf>) -> Option<PathBuf> {
    for extension in ["rs", "ts", "tsx", "js", "jsx", "css", "json"] {
        let candidate = prefix.join(format!("{module}.{extension}"));
        if paths.contains(&candidate) {
            return Some(candidate);
        }
    }

    for extension in ["rs", "ts", "tsx", "js", "jsx"] {
        let candidate = prefix.join(module).join(format!("index.{extension}"));
        if paths.contains(&candidate) {
            return Some(candidate);
        }
    }

    None
}

fn normalize_relative_path(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn build_symbol_index(files: &[CodeFileModel]) -> BTreeMap<String, Vec<String>> {
    let mut index: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for file in files {
        for symbol in &file.symbols {
            index
                .entry(symbol.name.clone())
                .or_default()
                .push(format!("{}:{}", file.path.display(), symbol.line));
        }
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_model_with_symbols_dependencies_and_ignored_dirs() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        fs::create_dir_all(root.join("src/ui"))?;
        fs::create_dir_all(root.join("target"))?;
        fs::write(
            root.join("src/lib.rs"),
            "use crate::ui::button;\npub struct AppState;\npub fn run() {}\n",
        )?;
        fs::write(root.join("src/ui/button.tsx"), "export function Button() { return null }\n")?;
        fs::write(root.join("target/generated.rs"), "pub fn ignored() {}\n")?;

        let model = CodebaseModelBuilder::new(root).build()?;

        assert_eq!(model.files.len(), 2);
        assert!(model.symbol_index.contains_key("AppState"));
        assert!(model.symbol_index.contains_key("Button"));
        assert!(!model.symbol_index.contains_key("ignored"));
        assert!(model
            .dependencies
            .iter()
            .any(|dependency| dependency.from == PathBuf::from("src/lib.rs")
                && dependency.to == PathBuf::from("src/ui/button.tsx")));
        assert!(model.project_brief().contains("indexed symbols"));

        let snapshot_path = root.join(".neura/codebase-model.json");
        model.save_json(&snapshot_path)?;
        let loaded = CodebaseModel::load_json(&snapshot_path)?;
        assert_eq!(loaded.symbol_index, model.symbol_index);

        Ok(())
    }
}
