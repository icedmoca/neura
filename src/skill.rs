use anyhow::Result;
use chrono::Utc;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SKILL_GET_MAX_CHARS: usize = 20_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillGetRequest {
    pub name: String,
    pub reason: Option<String>,
}
#[cfg(not(test))]
use std::sync::OnceLock;

/// A skill definition from SKILL.md
#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub allowed_tools: Option<Vec<String>>,
    pub content: String,
    pub path: PathBuf,
    search_text: String,
}

#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    name: String,
    description: String,
    #[serde(rename = "allowed-tools")]
    allowed_tools: Option<String>,
}

/// Registry of available skills
#[derive(Debug, Default, Clone)]
pub struct SkillRegistry {
    skills: HashMap<String, Skill>,
}

impl SkillRegistry {
    /// Load a process-wide shared immutable snapshot of skills for startup paths
    /// that only need read access.
    pub fn shared_snapshot() -> Arc<Self> {
        #[cfg(test)]
        {
            Arc::new(Self::load().unwrap_or_default())
        }

        #[cfg(not(test))]
        {
            static SHARED: OnceLock<Arc<SkillRegistry>> = OnceLock::new();
            SHARED
                .get_or_init(|| Arc::new(SkillRegistry::load().unwrap_or_default()))
                .clone()
        }
    }

    /// Import skills from Claude Code and Codex CLI on first run.
    /// Only runs if ~/.neura/skills/ doesn't exist yet.
    fn import_from_external() {
        let neura_skills = match crate::storage::neura_dir() {
            Ok(dir) => dir.join("skills"),
            Err(_) => return,
        };

        if neura_skills.exists() {
            return; // Not first run
        }

        let mut sources = Vec::new();
        let mut copied = Vec::new();

        // Import from Claude Code (~/.claude/skills/)
        if let Ok(claude_skills) = crate::storage::user_home_path(".claude/skills")
            && claude_skills.is_dir()
        {
            let count = Self::copy_skills_dir(&claude_skills, &neura_skills);
            if count > 0 {
                sources.push(format!("{} from Claude Code", count));
                copied.extend(Self::list_skill_names(&neura_skills));
            }
        }

        // Import from Codex CLI (~/.codex/skills/)
        if let Ok(codex_skills) = crate::storage::user_home_path(".codex/skills")
            && codex_skills.is_dir()
        {
            let count = Self::copy_skills_dir(&codex_skills, &neura_skills);
            if count > 0 {
                sources.push(format!("{} from Codex CLI", count));
                copied.extend(Self::list_skill_names(&neura_skills));
            }
        }

        if !sources.is_empty() {
            // Deduplicate names
            copied.sort();
            copied.dedup();
            crate::logging::info(&format!(
                "Skills: Imported {} ({}) from {}",
                copied.len(),
                copied.join(", "),
                sources.join(" + "),
            ));
        }
    }

    /// Copy skill directories from src to dst. Returns count of skills copied.
    fn copy_skills_dir(src: &Path, dst: &Path) -> usize {
        let entries = match std::fs::read_dir(src) {
            Ok(e) => e,
            Err(_) => return 0,
        };

        let mut count = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            // Skip Codex system skills
            if name.starts_with('.') {
                continue;
            }

            // Only copy if SKILL.md exists
            if !path.join("SKILL.md").exists() {
                continue;
            }

            let dest = dst.join(&name);
            if let Err(e) = Self::copy_dir_recursive(&path, &dest) {
                crate::logging::error(&format!("Failed to copy skill '{}': {}", name, e));
                continue;
            }
            count += 1;
        }
        count
    }

    /// Recursively copy a directory
    fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());

            if src_path.is_dir() {
                Self::copy_dir_recursive(&src_path, &dst_path)?;
            } else if src_path.is_symlink() {
                // Resolve symlink and copy the target
                let target = std::fs::read_link(&src_path)?;
                // Try to create symlink, fall back to copying the file
                if crate::platform::symlink_or_copy(&target, &dst_path).is_err()
                    && let Ok(resolved) = std::fs::canonicalize(&src_path)
                {
                    std::fs::copy(&resolved, &dst_path)?;
                }
            } else {
                std::fs::copy(&src_path, &dst_path)?;
            }
        }
        Ok(())
    }

    /// List skill directory names
    fn list_skill_names(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .ok()
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.path().is_dir())
                    .filter_map(|e| e.file_name().to_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Load skills from all standard locations
    pub fn load() -> Result<Self> {
        // First-run import from Claude Code / Codex CLI
        Self::import_from_external();

        let mut registry = Self::default();

        // Load from ~/.neura/skills/ (neura's own global skills)
        if let Ok(neura_dir) = crate::storage::neura_dir() {
            let neura_skills = neura_dir.join("skills");
            if neura_skills.exists() {
                registry.load_from_dir(&neura_skills)?;
            }
        }

        // Load from ./.neura/skills/ (project-local neura skills)
        let local_neura = Path::new(".neura").join("skills");
        if local_neura.exists() {
            registry.load_from_dir(&local_neura)?;
        }

        // Fallback: ./.claude/skills/ (project-local Claude skills for compatibility)
        let local_claude = Path::new(".claude").join("skills");
        if local_claude.exists() {
            registry.load_from_dir(&local_claude)?;
        }

        Ok(registry)
    }

    /// Load skills from a directory
    fn load_from_dir(&mut self, dir: &Path) -> Result<()> {
        if !dir.is_dir() {
            return Ok(());
        }

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                let skill_file = path.join("SKILL.md");
                if skill_file.exists()
                    && let Ok(skill) = Self::parse_skill(&skill_file)
                {
                    self.skills.insert(skill.name.clone(), skill);
                }
            }
        }

        Ok(())
    }

    /// Parse a SKILL.md file
    fn parse_skill(path: &Path) -> Result<Skill> {
        let content = std::fs::read_to_string(path)?;

        // Parse YAML frontmatter
        let (frontmatter, body) = Self::parse_frontmatter(&content)?;

        let SkillFrontmatter {
            name,
            description,
            allowed_tools,
        } = frontmatter;

        let allowed_tools =
            allowed_tools.map(|s| s.split(',').map(|t| t.trim().to_string()).collect());
        let search_text = build_skill_search_text(&name, &description, &body);

        Ok(Skill {
            name,
            description,
            allowed_tools,
            content: body,
            path: path.to_path_buf(),
            search_text,
        })
    }

    /// Parse YAML frontmatter from markdown
    fn parse_frontmatter(content: &str) -> Result<(SkillFrontmatter, String)> {
        let content = content.trim();

        if !content.starts_with("---") {
            anyhow::bail!("Missing YAML frontmatter");
        }

        let rest = &content[3..];
        let end = rest
            .find("---")
            .ok_or_else(|| anyhow::anyhow!("Unclosed frontmatter"))?;

        let yaml = &rest[..end];
        let body = rest[end + 3..].trim().to_string();

        let frontmatter: SkillFrontmatter = serde_yaml::from_str(yaml)?;

        Ok((frontmatter, body))
    }

    /// Get a skill by name
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    /// List all available skills
    pub fn list(&self) -> Vec<&Skill> {
        let mut skills: Vec<&Skill> = self.skills.values().collect();
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        skills
    }

    pub fn skill_names(&self) -> Vec<String> {
        self.list()
            .into_iter()
            .map(|skill| skill.name.clone())
            .collect()
    }

    /// Reload a specific skill by name
    pub fn reload(&mut self, name: &str) -> Result<bool> {
        // Find the skill's path first
        let path = self.skills.get(name).map(|s| s.path.clone());

        if let Some(path) = path {
            if path.exists() {
                let skill = Self::parse_skill(&path)?;
                self.skills.insert(skill.name.clone(), skill);
                Ok(true)
            } else {
                // Skill file was deleted
                self.skills.remove(name);
                Ok(false)
            }
        } else {
            Ok(false)
        }
    }

    /// Reload all skills from all locations
    pub fn reload_all(&mut self) -> Result<usize> {
        self.skills.clear();

        let mut count = 0;

        // Load from ~/.neura/skills/ (neura's own global skills)
        if let Ok(neura_dir) = crate::storage::neura_dir() {
            let neura_skills = neura_dir.join("skills");
            if neura_skills.exists() {
                count += self.load_from_dir_count(&neura_skills)?;
            }
        }

        // Load from ./.neura/skills/ (project-local neura skills)
        let local_neura = Path::new(".neura").join("skills");
        if local_neura.exists() {
            count += self.load_from_dir_count(&local_neura)?;
        }

        // Fallback: ./.claude/skills/ (project-local Claude skills for compatibility)
        let local_claude = Path::new(".claude").join("skills");
        if local_claude.exists() {
            count += self.load_from_dir_count(&local_claude)?;
        }

        Ok(count)
    }

    /// Load skills from a directory and return count
    fn load_from_dir_count(&mut self, dir: &Path) -> Result<usize> {
        if !dir.is_dir() {
            return Ok(0);
        }

        let mut count = 0;
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                let skill_file = path.join("SKILL.md");
                if skill_file.exists()
                    && let Ok(skill) = Self::parse_skill(&skill_file)
                {
                    self.skills.insert(skill.name.clone(), skill);
                    count += 1;
                }
            }
        }

        Ok(count)
    }

    /// Check if a message is a skill invocation (starts with /)
    pub fn parse_invocation(input: &str) -> Option<&str> {
        let trimmed = input.trim();
        if trimmed.starts_with('/') && !trimmed.contains(' ') {
            Some(&trimmed[1..])
        } else {
            None
        }
    }
}

pub fn parse_skill_get_request(text: &str) -> Option<SkillGetRequest> {
    let line = text
        .lines()
        .find(|line| line.trim_start().starts_with(".skill_get"))?
        .trim();
    let rest = line.strip_prefix(".skill_get")?.trim();
    if rest.is_empty() {
        return None;
    }

    let mut name: Option<String> = None;
    let mut reason: Option<String> = None;
    let mut positional: Vec<&str> = Vec::new();
    let parts: Vec<&str> = rest.split_whitespace().collect();
    let mut idx = 0;
    while idx < parts.len() {
        let part = parts[idx];
        if let Some(value) = part
            .strip_prefix("name=")
            .or_else(|| part.strip_prefix("skill="))
        {
            if !value.is_empty() {
                name = Some(value.to_string());
            }
        } else if let Some(value) = part.strip_prefix("reason=") {
            let mut pieces = Vec::new();
            if !value.is_empty() {
                pieces.push(value);
            }
            pieces.extend_from_slice(&parts[idx + 1..]);
            if !pieces.is_empty() {
                reason = Some(pieces.join(" "));
            }
            break;
        } else {
            positional.push(part);
        }
        idx += 1;
    }

    if name.is_none() && !positional.is_empty() {
        name = Some(positional[0].to_string());
        if reason.is_none() && positional.len() > 1 {
            reason = Some(positional[1..].join(" "));
        }
    }

    name.map(|name| SkillGetRequest {
        name: name.trim().to_string(),
        reason: reason
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty()),
    })
    .filter(|req| !req.name.is_empty())
}

pub fn build_skill_anchor(registry: &SkillRegistry) -> Option<String> {
    let names = registry.skill_names();
    if names.is_empty() {
        return None;
    }
    Some(format!(
        "<system-reminder>\n<skill-anchor count=\"{}\" via=\".skill_get\" />\nHermes-style skills available: {}. Load exact instructions only when needed with `.skill_get name=<skill> reason=<why>`. Keep the anchor compact; do not infer hidden skill details from the names alone.\n</system-reminder>",
        names.len(),
        names.join(", ")
    ))
}

pub fn maybe_rehydrate_skill_get(registry: &SkillRegistry, model_text: &str) -> Option<String> {
    let req = parse_skill_get_request(model_text)?;
    let skill = registry.get(&req.name)?;
    let mut body = skill.get_prompt();
    if body.len() > SKILL_GET_MAX_CHARS {
        body.truncate(SKILL_GET_MAX_CHARS);
        body.push_str("\n\n[skill truncated to 20000 chars; inspect the skill files directly if more detail is needed]");
    }

    Some(format!(
        "<system-reminder>\nNeura .skill_get rehydration fulfilled (skill={}, reason={}). Treat the following SKILL.md instructions as authoritative for this turn only.\n\n```skill\n{}\n```\n</system-reminder>",
        req.name,
        req.reason.as_deref().unwrap_or("unspecified"),
        body
    ))
}

impl Skill {
    /// Get the full prompt content for this skill
    pub fn get_prompt(&self) -> String {
        format!(
            "# Skill: {}\n\n{}\n\n{}",
            self.name, self.description, self.content
        )
    }

    /// Load additional files from the skill directory
    pub fn load_file(&self, filename: &str) -> Result<String> {
        let skill_dir = self
            .path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("No parent dir"))?;
        let file_path = skill_dir.join(filename);
        Ok(std::fs::read_to_string(file_path)?)
    }

    pub fn as_memory_entry(&self) -> crate::memory::MemoryEntry {
        let now = Utc::now() - chrono::Duration::days(365);
        crate::memory::MemoryEntry {
            id: format!("skill:{}", self.name),
            category: crate::memory::MemoryCategory::Custom("Skills".to_string()),
            content: format!(
                "Use skill `/{} ` when relevant.\n\n{}",
                self.name,
                self.get_prompt()
            ),
            tags: vec!["skill".to_string(), self.name.clone()],
            search_text: self.search_text.clone(),
            created_at: now,
            updated_at: now,
            access_count: 0,
            source: Some("skill_registry".to_string()),
            trust: crate::memory::TrustLevel::Medium,
            strength: 1,
            active: true,
            superseded_by: None,
            reinforcements: Vec::new(),
            embedding: None,
            confidence: 1.0,
        }
    }
}

fn build_skill_search_text(name: &str, description: &str, content: &str) -> String {
    normalize_skill_search_text(&format!("{}\n{}\n{}", name, description, content))
}

fn normalize_skill_search_text(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_skill(name: &str, description: &str, content: &str) -> Skill {
        Skill {
            name: name.to_string(),
            description: description.to_string(),
            allowed_tools: None,
            content: content.to_string(),
            path: PathBuf::from(format!("/tmp/{name}/SKILL.md")),
            search_text: build_skill_search_text(name, description, content),
        }
    }

    #[test]
    fn skill_as_memory_entry_formats_invocation_and_prompt() {
        let skill = test_skill(
            "firefox-browser",
            "Control Firefox browser sessions and logged-in pages",
            "Use this skill when you need to open websites, click buttons, or interact with browser pages.",
        );

        let entry = skill.as_memory_entry();

        assert_eq!(entry.id, "skill:firefox-browser");
        assert!(matches!(
            entry.category,
            crate::memory::MemoryCategory::Custom(ref name) if name == "Skills"
        ));
        assert!(entry.content.contains("/firefox-browser"));
        assert!(entry.content.contains("# Skill: firefox-browser"));
        assert_eq!(entry.source.as_deref(), Some("skill_registry"));
    }

    #[test]
    fn parses_skill_get_requests() {
        assert_eq!(
            parse_skill_get_request(".skill_get name=rust-tests reason=need exact steps"),
            Some(SkillGetRequest {
                name: "rust-tests".to_string(),
                reason: Some("need exact steps".to_string()),
            })
        );
        assert_eq!(
            parse_skill_get_request("please load\n.skill_get firefox-browser clicking"),
            Some(SkillGetRequest {
                name: "firefox-browser".to_string(),
                reason: Some("clicking".to_string()),
            })
        );
        assert!(parse_skill_get_request(".skill_get").is_none());
    }

    #[test]
    fn skill_anchor_is_compact_and_names_only() {
        let mut registry = SkillRegistry::default();
        registry.skills.insert(
            "firefox-browser".to_string(),
            test_skill(
                "firefox-browser",
                "Browser automation",
                "Long hidden instructions",
            ),
        );

        let anchor = build_skill_anchor(&registry).expect("anchor should render");
        assert!(anchor.contains("skill-anchor"));
        assert!(anchor.contains(".skill_get name=<skill>"));
        assert!(anchor.contains("firefox-browser"));
        assert!(!anchor.contains("Long hidden instructions"));
    }
}
