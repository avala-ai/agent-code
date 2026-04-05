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

pub mod remote;

use serde::Deserialize;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// A loaded skill definition.
///
/// Skills are markdown files with YAML frontmatter. The body is a
/// prompt template supporting `{{arg}}` substitution. Invoke via
/// `/skill-name` in the REPL or programmatically via the Skill tool.
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

    /// Expand the skill body, stripping fenced shell blocks if disabled.
    ///
    /// When `disable_shell` is true, any fenced code block with a shell
    /// language tag (```sh, ```bash, ```shell, ```zsh) is replaced with
    /// a notice that shell execution is disabled.
    pub fn expand_safe(&self, args: Option<&str>, disable_shell: bool) -> String {
        let body = self.expand(args);
        if !disable_shell {
            return body;
        }
        strip_shell_blocks(&body)
    }
}

/// Remove fenced shell code blocks from text.
fn strip_shell_blocks(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut lines = text.lines().peekable();

    while let Some(line) = lines.next() {
        if is_shell_fence(line) {
            // Skip until closing fence.
            result.push_str("[Shell execution disabled by security policy]\n");
            for inner in lines.by_ref() {
                if inner.trim_start().starts_with("```") {
                    break;
                }
            }
        } else {
            result.push_str(line);
            result.push('\n');
        }
    }

    result
}

fn is_shell_fence(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("```sh")
        || trimmed.starts_with("```bash")
        || trimmed.starts_with("```shell")
        || trimmed.starts_with("```zsh")
}

/// Registry of loaded skills from bundled, project, and user directories.
///
/// Load with [`SkillRegistry::load_all`]. Skills are searched in order:
/// project (`.agent/skills/`), user (`~/.config/agent-code/skills/`),
/// then bundled. A project skill with the same name overrides a bundled one.
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

        // Load bundled skills (shipped with the binary).
        registry.load_bundled();

        debug!("Loaded {} skills", registry.skills.len());
        registry
    }

    /// Load built-in skills that ship with agent-code.
    fn load_bundled(&mut self) {
        let bundled = [
            (
                "commit",
                "Create a well-crafted git commit",
                true,
                "Review the current git diff carefully. Create a commit with a clear, \
                 concise message that explains WHY the change was made, not just WHAT changed. \
                 Follow the repository's existing commit style. Stage specific files \
                 (don't use git add -A). Never commit .env or credentials.",
            ),
            (
                "review",
                "Review code changes for bugs and issues",
                true,
                "Review the current git diff against the base branch. Look for: bugs, \
                 security issues (injection, XSS, OWASP top 10), race conditions, \
                 error handling gaps, performance problems (N+1 queries, missing indexes), \
                 and code quality issues. Report findings with file:line references.",
            ),
            (
                "test",
                "Run tests and fix failures",
                true,
                "Run the project's test suite. If any tests fail, read the failing test \
                 and the source code it tests. Identify the root cause. Fix the issue. \
                 Run the tests again to verify the fix. Repeat until all tests pass.",
            ),
            (
                "explain",
                "Explain how a piece of code works",
                true,
                "Read the file or function the user is asking about. Explain what it does, \
                 how it works, and why it's designed that way. Use clear language. \
                 Reference specific line numbers. If there are non-obvious design decisions, \
                 explain the tradeoffs.",
            ),
            (
                "debug",
                "Debug an error or unexpected behavior",
                true,
                "Investigate the error systematically. Read the error message and stack trace. \
                 Find the relevant source code. Identify the root cause (don't guess). \
                 Propose a fix with explanation. Apply the fix and verify it works.",
            ),
            (
                "pr",
                "Create a pull request",
                true,
                "Check git status and diff against the base branch. Analyze ALL commits \
                 on this branch. Draft a PR title (under 70 chars) and body with a summary \
                 section (bullet points) and a test plan. Push to remote and create the PR \
                 using gh pr create. Return the PR URL.",
            ),
            (
                "refactor",
                "Refactor code for better quality",
                true,
                "Read the code the user wants refactored. Identify specific improvements: \
                 extract functions, reduce duplication, simplify conditionals, improve naming, \
                 add missing error handling. Make changes incrementally. Run tests after \
                 each change to verify nothing broke.",
            ),
            (
                "init",
                "Initialize project configuration",
                true,
                "Create an AGENTS.md file in the project root with project context: \
                 tech stack, architecture overview, coding conventions, test commands, \
                 and important file locations. This helps the agent understand the project \
                 in future sessions.",
            ),
            (
                "security-review",
                "Review code for security vulnerabilities",
                true,
                "Perform a security review of the current changes or specified files. \
                 Check for: SQL injection (parameterized queries), XSS (output escaping), \
                 command injection (shell argument safety), hardcoded secrets (API keys, \
                 passwords, tokens), insecure deserialization, broken authentication, \
                 path traversal, and SSRF. Verify input validation at system boundaries. \
                 Report each finding with file:line, severity (critical/high/medium/low), \
                 and a concrete fix.",
            ),
            (
                "advisor",
                "Analyze project architecture and suggest improvements",
                true,
                "Read the project structure, key entry points, and dependency manifest. \
                 Evaluate: code organization (cohesion, coupling), dependency health \
                 (outdated, unused, or vulnerable packages), test coverage gaps, error \
                 handling patterns, and performance bottlenecks. Prioritize findings by \
                 impact. For each suggestion, explain the current state, the risk of \
                 inaction, and a specific next step.",
            ),
            (
                "bughunter",
                "Systematically search for bugs",
                true,
                "Hunt for bugs methodically. Run the test suite and analyze failures. \
                 Read error handling paths and look for: unchecked return values, \
                 off-by-one errors, null/nil/undefined dereferences, resource leaks \
                 (files, connections, locks), race conditions, integer overflow, and \
                 boundary conditions. For each bug found, provide: file:line, a minimal \
                 reproduction, the root cause, and a fix. Verify fixes don't break \
                 existing tests.",
            ),
            (
                "plan",
                "Create a detailed implementation plan",
                true,
                "Explore the codebase to understand the relevant architecture before \
                 planning. Identify all files that need changes. For each change, specify: \
                 the file path, what to modify, and why. Note dependencies between changes \
                 (what must happen first). Flag risks: breaking changes, migration needs, \
                 performance implications. Estimate scope (small/medium/large per file). \
                 Present the plan as an ordered checklist the user can approve before \
                 implementation begins.",
            ),
        ];

        for (name, description, user_invocable, body) in bundled {
            // Don't override user-defined skills with the same name.
            if self.skills.iter().any(|s| s.name == name) {
                continue;
            }
            self.skills.push(Skill {
                name: name.to_string(),
                metadata: SkillMetadata {
                    description: Some(description.to_string()),
                    when_to_use: None,
                    user_invocable,
                    disable_non_interactive: false,
                    paths: None,
                },
                body: body.to_string(),
                source: std::path::PathBuf::new(),
            });
        }
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

    #[test]
    fn test_expand_safe_allows_shell_by_default() {
        let skill = Skill {
            name: "deploy".into(),
            metadata: SkillMetadata::default(),
            body: "Run:\n```bash\ncargo build\n```\nDone.".into(),
            source: PathBuf::from("deploy.md"),
        };
        let result = skill.expand_safe(None, false);
        assert!(result.contains("cargo build"));
    }

    #[test]
    fn test_expand_safe_strips_shell_when_disabled() {
        let skill = Skill {
            name: "deploy".into(),
            metadata: SkillMetadata::default(),
            body: "Run:\n```bash\ncargo build\n```\nDone.".into(),
            source: PathBuf::from("deploy.md"),
        };
        let result = skill.expand_safe(None, true);
        assert!(!result.contains("cargo build"));
        assert!(result.contains("Shell execution disabled"));
        assert!(result.contains("Done."));
    }

    #[test]
    fn test_strip_shell_blocks_multiple_langs() {
        let text = "a\n```sh\nls\n```\nb\n```zsh\necho hi\n```\nc\n";
        let result = strip_shell_blocks(text);
        assert!(!result.contains("ls"));
        assert!(!result.contains("echo hi"));
        assert!(result.contains("a\n"));
        assert!(result.contains("b\n"));
        assert!(result.contains("c\n"));
    }

    #[test]
    fn test_strip_shell_blocks_preserves_non_shell() {
        let text = "```rust\nfn main() {}\n```\n";
        let result = strip_shell_blocks(text);
        assert!(result.contains("fn main()"));
    }
}
