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
                "pentest",
                "White-box penetration test of a target directory",
                true,
                "Conduct a white-box penetration test of the target directory (argument) \
                 or the whole repository if no argument is given. Run five phases in \
                 order; do not skip phases.\n\n\
                 1. RECON. Read the target. Map entry points (HTTP routes, CLI commands, \
                 event handlers, IPC surfaces), authentication and trust boundaries, and \
                 high-risk sinks: raw SQL (cursor.execute, .raw, .extra, f-string \
                 queries), command execution (subprocess, eval, exec, os.system), \
                 deserializers (pickle, yaml.load non-safe), user-controlled URLs \
                 (requests, urllib, httpx with untrusted input), file path joins with \
                 user input, missing authorization checks. Cite file:line for every \
                 sink.\n\n\
                 2. SLICE. Partition the code into 4-8 directory slices, each assigned \
                 1-2 CWE focus areas from CWE-89, CWE-78, CWE-79, CWE-22, CWE-502, \
                 CWE-798, CWE-862, CWE-863, CWE-918, CWE-200, CWE-352. Rank slices by \
                 suspected risk.\n\n\
                 3. VULN ANALYSIS. For each slice, trace user input from source through \
                 any sanitizer to the dangerous sink. If a sanitizer exists, explain why \
                 it is or is not sufficient for this specific sink. Produce hypothesized \
                 exploit paths.\n\n\
                 4. EXPLOIT OR DISCARD. For every finding, produce a concrete \
                 proof-of-concept: a curl command, an exact payload string, or \
                 reproduction steps. If no PoC is producible from code inspection alone, \
                 demote the finding to INFO or drop it. No theoretical findings. Any \
                 dynamic verification must target a local development instance, never \
                 production.\n\n\
                 5. REPORT. Write a markdown report grouping findings by severity \
                 (CRITICAL / HIGH / MEDIUM / LOW / INFO). Each finding includes \
                 file:line, CWE, risk description, vulnerable snippet, fix snippet, \
                 impact, and PoC. End with a summary table and a ship-readiness verdict. \
                 Save to the project's standard security reports location (for example \
                 reports/security/ or docs/security/).\n\n\
                 Target: {{arg}}. If empty, ask the user which subsystem to test first.",
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
            (
                "changelog",
                "Update CHANGELOG.md from the current diff",
                true,
                "Read CHANGELOG.md to learn the project's format (Keep a Changelog is \
                 common). Inspect the current git diff and recent commits since the last \
                 release entry. Classify changes into Added / Changed / Fixed / Removed / \
                 Security. Draft entries that describe user-visible impact, not internal \
                 refactors. Insert them under an Unreleased section, preserving existing \
                 formatting. Do not invent changes that aren't in the diff.",
            ),
            (
                "release",
                "Orchestrate a version release",
                true,
                "Follow the project's RELEASING.md if present. Determine the next version \
                 (patch / minor / major) from the nature of the changes since the last tag. \
                 Bump version numbers in all manifest files (Cargo.toml, package.json, \
                 pyproject.toml, etc.) consistently. Stamp CHANGELOG.md with the new version \
                 and today's date. Run the full test and lint gate before tagging. Create \
                 the release branch, open a PR, and on merge create the git tag. Never push \
                 tags without user confirmation.",
            ),
            (
                "benchmark",
                "Run benchmarks and compare results",
                true,
                "Locate the project's benchmark suite (cargo bench, pytest-benchmark, \
                 criterion, etc.). Run it on the current branch and capture results. If a \
                 baseline exists (from main or a stored snapshot), compare and report \
                 regressions and improvements as percentages. Flag any metric that \
                 regressed more than 5% with file:line context for the likely cause. \
                 Do not claim a speedup without a baseline to compare against.",
            ),
            (
                "coverage",
                "Produce a test coverage report and narrative",
                true,
                "Run the project's coverage tool (cargo llvm-cov, pytest --cov, c8, etc.). \
                 Summarize overall coverage and identify the lowest-covered modules. For \
                 each gap, classify: (a) untested happy path, (b) untested error path, \
                 (c) untestable boilerplate. Recommend 3-5 high-value tests to add, with \
                 specific function names. Do not propose tests for generated code or \
                 trivial getters.",
            ),
            (
                "migrate",
                "Analyze a dependency upgrade or breaking API migration",
                true,
                "Given a target dependency version or API change, read the dependency's \
                 release notes or migration guide. Grep the codebase for every call site \
                 affected by the change. Produce a migration plan listing each call site \
                 with file:line, the old pattern, the new pattern, and whether the change \
                 is mechanical or requires judgement. Flag any ambiguous call sites for \
                 human review. Do not perform the migration without an approved plan.",
            ),
            (
                "docs",
                "Sync documentation with code changes",
                true,
                "Inspect the current diff. For every public API that changed (function \
                 signatures, config keys, CLI flags, tool contracts), find the corresponding \
                 documentation (rustdoc comments, README sections, docs/ pages, Mintlify \
                 mdx files) and update it to match. Flag any documented behavior that the \
                 diff silently breaks. Do not add documentation for code that isn't part \
                 of the public surface.",
            ),
            (
                "stuck",
                "Step back and try a different angle when looping",
                true,
                "You are stuck. Stop the current approach. Read the last 10 messages of this \
                 conversation and identify: (1) what you tried, (2) why each attempt failed, \
                 (3) the assumption every attempt shares. That shared assumption is usually \
                 the thing that's wrong. List at least two different approaches that don't \
                 rely on it — e.g. a different file to read, a different tool to reach for, \
                 a different abstraction level (add logs instead of reading code, or rebuild \
                 instead of patch-fixing). Pick the most plausible one and take a single \
                 concrete step. Do not retry anything you've already tried.",
            ),
            (
                "remember",
                "Save a specific insight to user memory",
                true,
                "Extract the insight or preference the user just shared and save it as a \
                 memory following the two-step write discipline. First classify the memory \
                 type: `user` for role/preference/knowledge, `feedback` for rules about how \
                 to approach work, `project` for in-flight context, `reference` for pointers \
                 to external systems. Pick a short kebab-case filename and write the memory \
                 file with the required frontmatter (name, description, type). Then add a \
                 single index line to MEMORY.md under ~150 chars: \
                 `- [Title](file.md) — one-line hook`. Do not dump content into the index, \
                 do not duplicate an existing memory, and do not save anything already \
                 derivable from the codebase (architecture, file paths, git history, debug \
                 fixes). Finish with one line confirming what was saved.",
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
