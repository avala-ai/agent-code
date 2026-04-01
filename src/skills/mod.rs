//! Skill system.
//!
//! Skills are reusable, user-defined workflows loaded from markdown
//! files in `.agent/skills/` or `~/.config/agent-code/skills/`. Each
//! skill is a markdown file with YAML frontmatter that defines:
//!
//! - `description`: what the skill does
//! - `whenToUse`: when to invoke it
//! - `userInvocable`: whether users can invoke it via `/skill-name`
//!
//! The body of the skill file is a prompt template that gets expanded
//! when the skill is invoked. Supports `{{arg}}` substitution.

use serde::Deserialize;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// A loaded skill definition.
#[derive(Debug, Clone)]
pub struct Skill {
    /// Skill name (derived from filename without extension).
    pub name: String,
    /// Metadata from frontmatter.
    pub metadata: SkillMetadata,
    /// The prompt template body.
    pub body: String,
    /// Source file path.
    pub source: PathBuf,
}

/// Frontmatter metadata for a skill.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SkillMetadata {
    /// What this skill does.
    pub description: Option<String>,
    /// When to invoke this skill.
    #[serde(rename = "whenToUse")]
    pub when_to_use: Option<String>,
    /// Whether users can invoke this via `/name`.
    #[serde(rename = "userInvocable")]
    pub user_invocable: bool,
    /// Whether to disable in non-interactive sessions.
    #[serde(rename = "disableNonInteractive")]
    pub disable_non_interactive: bool,
    /// File patterns that trigger this skill suggestion.
    pub paths: Option<Vec<String>>,
}

impl Skill {
    /// Expand the skill body with argument substitution.
    pub fn expand(&self, args: Option<&str>) -> String {
        let mut body = self.body.clone();
        if let Some(args) = args {
            body = body.replace("{{arg}}", args);
            body = body.replace("{{ arg }}", args);
        }
        body
    }
}

/// Skill registry holding all loaded skills.
pub struct SkillRegistry {
    skills: Vec<Skill>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self { skills: Vec::new() }
    }

    /// Load skills from all configured directories.
    pub fn load_all(project_root: Option<&Path>) -> Self {
        let mut registry = Self::new();

        // Load from project-level skills directory.
        if let Some(root) = project_root {
            let project_skills = root.join(".agent").join("skills");
            if project_skills.is_dir() {
                registry.load_from_dir(&project_skills);
            }
        }

        // Load from user-level skills directory.
        if let Some(dir) = user_skills_dir()
            && dir.is_dir()
        {
            registry.load_from_dir(&dir);
        }

        debug!("Loaded {} skills", registry.skills.len());
        registry
    }

    /// Load skills from a single directory.
    fn load_from_dir(&mut self, dir: &Path) {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) => {
                warn!("Failed to read skills directory {}: {e}", dir.display());
                return;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();

            // Skills can be single .md files or directories with a SKILL.md.
            let skill_path = if path.is_file() && path.extension().is_some_and(|e| e == "md") {
                path.clone()
            } else if path.is_dir() {
                let skill_md = path.join("SKILL.md");
                if skill_md.exists() {
                    skill_md
                } else {
                    continue;
                }
            } else {
                continue;
            };

            match load_skill_file(&skill_path) {
                Ok(skill) => {
                    debug!(
                        "Loaded skill '{}' from {}",
                        skill.name,
                        skill_path.display()
                    );
                    self.skills.push(skill);
                }
                Err(e) => {
                    warn!("Failed to load skill {}: {e}", skill_path.display());
                }
            }
        }
    }

    /// Find a skill by name.
    pub fn find(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| s.name == name)
    }

    /// Get all user-invocable skills.
    pub fn user_invocable(&self) -> Vec<&Skill> {
        self.skills
            .iter()
            .filter(|s| s.metadata.user_invocable)
            .collect()
    }

    /// Get all skills.
    pub fn all(&self) -> &[Skill] {
        &self.skills
    }
}

/// Load a single skill file, parsing frontmatter and body.
fn load_skill_file(path: &Path) -> Result<Skill, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("Read error: {e}"))?;

    // Derive skill name from path.
    let name = path
        .parent()
        .and_then(|p| {
            // If this is SKILL.md in a directory, use the directory name.
            if path.file_name().is_some_and(|f| f == "SKILL.md") {
                p.file_name().and_then(|n| n.to_str())
            } else {
                None
            }
        })
        .or_else(|| path.file_stem().and_then(|s| s.to_str()))
        .unwrap_or("unknown")
        .to_string();

    // Parse YAML frontmatter (between --- delimiters).
    let (metadata, body) = parse_frontmatter(&content)?;

    Ok(Skill {
        name,
        metadata,
        body,
        source: path.to_path_buf(),
    })
}

/// Parse YAML frontmatter from markdown content.
///
/// Expects format:
/// ```text
/// ---
/// key: value
/// ---
/// body content
/// ```
fn parse_frontmatter(content: &str) -> Result<(SkillMetadata, String), String> {
    let trimmed = content.trim_start();

    if !trimmed.starts_with("---") {
        // No frontmatter — entire content is the body.
        return Ok((SkillMetadata::default(), content.to_string()));
    }

    // Find the closing ---.
    let after_first = &trimmed[3..];
    let closing = after_first
        .find("\n---")
        .ok_or("Frontmatter not closed (missing closing ---)")?;

    let yaml = &after_first[..closing].trim();
    let body = &after_first[closing + 4..].trim_start();

    let metadata: SkillMetadata = serde_yaml_parse(yaml)?;

    Ok((metadata, body.to_string()))
}

/// Parse YAML using a simple key-value approach.
/// (Avoids adding a full YAML parser dependency.)
fn serde_yaml_parse(yaml: &str) -> Result<SkillMetadata, String> {
    // Build a JSON object from simple YAML key: value pairs.
    let mut map = serde_json::Map::new();

    for line in yaml.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim().trim_matches('"').trim_matches('\'');

            // Handle booleans.
            let json_value = match value {
                "true" => serde_json::Value::Bool(true),
                "false" => serde_json::Value::Bool(false),
                _ => serde_json::Value::String(value.to_string()),
            };
            map.insert(key.to_string(), json_value);
        }
    }

    let json = serde_json::Value::Object(map);
    serde_json::from_value(json).map_err(|e| format!("Invalid frontmatter: {e}"))
}

/// Get the user-level skills directory.
fn user_skills_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("agent-code").join("skills"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_frontmatter() {
        let content = "---\ndescription: Test skill\nuserInvocable: true\n---\n\nDo the thing.";
        let (meta, body) = parse_frontmatter(content).unwrap();
        assert_eq!(meta.description, Some("Test skill".to_string()));
        assert!(meta.user_invocable);
        assert_eq!(body, "Do the thing.");
    }

    #[test]
    fn test_parse_no_frontmatter() {
        let content = "Just a prompt with no frontmatter.";
        let (meta, body) = parse_frontmatter(content).unwrap();
        assert!(meta.description.is_none());
        assert_eq!(body, content);
    }

    #[test]
    fn test_skill_expand() {
        let skill = Skill {
            name: "test".into(),
            metadata: SkillMetadata::default(),
            body: "Review {{arg}} carefully.".into(),
            source: PathBuf::from("test.md"),
        };
        assert_eq!(skill.expand(Some("main.rs")), "Review main.rs carefully.");
    }
}
