//! Project rules.
//!
//! Short, composable steering notes loaded from `.agent/rules/*.md`
//! and injected into the system prompt. Distinct from `AGENTS.md`
//! (long-form documentation) and skills (reusable prompts) — rules
//! are imperative constraints that apply to every turn.
//!
//! Rule file format:
//!
//! ```text
//! ---
//! title: Always run tests before committing
//! priority: 10        # lower = higher priority; default 100
//! enabled: true       # default true
//! ---
//!
//! Never commit without running `cargo test`. If tests fail, stop and
//! report — do not compensate by marking the tests as ignored.
//! ```

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Frontmatter fields on a rule file. All optional.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct RuleMeta {
    title: Option<String>,
    /// Lower number = injected earlier. Defaults to 100.
    priority: Option<u32>,
    /// Whether the rule participates in the system prompt.
    /// Defaults to true; `/rules disable <name>` flips it to false.
    enabled: Option<bool>,
}

/// A loaded project rule.
#[derive(Debug, Clone, Serialize)]
pub struct Rule {
    /// Filename stem (for /rules toggle and for the user to reference).
    pub name: String,
    /// Display title, from frontmatter or derived from the name.
    pub title: String,
    /// Priority — lower number = injected earlier.
    pub priority: u32,
    /// Whether the rule is active (will be injected into the system prompt).
    pub enabled: bool,
    /// The body prose — what actually gets injected.
    pub body: String,
    /// Source path.
    pub source: PathBuf,
}

/// Load all project rules from the project-local `.agent/rules/` dir.
///
/// Rules are returned sorted by priority (ascending), with ties broken
/// by name. Disabled rules are included in the result so that `/rules
/// list` can show them; callers building the system prompt must filter
/// for `enabled`.
pub fn load_project_rules(project_root: &Path) -> Vec<Rule> {
    let dir = project_root.join(".agent").join("rules");
    if !dir.is_dir() {
        return Vec::new();
    }

    let mut rules: Vec<Rule> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
            .filter_map(|e| load_rule_file(&e.path()).ok())
            .collect(),
        Err(_) => Vec::new(),
    };

    rules.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| a.name.cmp(&b.name))
    });
    rules
}

/// Build the rules block that gets prepended to the system prompt.
/// Returns an empty string if there are no enabled rules.
pub fn rules_prompt_block(rules: &[Rule]) -> String {
    let enabled: Vec<&Rule> = rules.iter().filter(|r| r.enabled).collect();
    if enabled.is_empty() {
        return String::new();
    }
    let mut out = String::from("# Project rules\n\n");
    out.push_str(
        "These are project-specific constraints that apply to every turn. Follow them \
         even when the user's immediate ask doesn't mention them.\n\n",
    );
    for r in enabled {
        out.push_str(&format!("**{}**\n\n{}\n\n", r.title, r.body.trim()));
    }
    out
}

/// Disable a rule by setting `enabled: false` in its frontmatter. A
/// minimal frontmatter is created if the file has none. Returns
/// `Ok(true)` on toggle, `Ok(false)` if the rule was already disabled.
pub fn set_rule_enabled(project_root: &Path, name: &str, enabled: bool) -> Result<bool, String> {
    let path = project_root
        .join(".agent")
        .join("rules")
        .join(format!("{name}.md"));
    if !path.exists() {
        return Err(format!("rule '{name}' not found at {}", path.display()));
    }
    let content = std::fs::read_to_string(&path).map_err(|e| format!("read error: {e}"))?;
    let (meta, body) = parse_frontmatter(&content);
    let already = meta.enabled.unwrap_or(true);
    if already == enabled {
        return Ok(false);
    }
    let new_content = rewrite_with_enabled(&content, enabled);
    std::fs::write(&path, new_content).map_err(|e| format!("write error: {e}"))?;
    // Touch body to convince the compiler we used it (we don't reuse
    // the parsed body — we rewrite around the frontmatter only so user
    // edits to the body are preserved verbatim).
    let _ = body;
    Ok(true)
}

/// Load a single rule file.
fn load_rule_file(path: &Path) -> Result<Rule, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("read error: {e}"))?;
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "no filename stem".to_string())?
        .to_string();
    let (meta, body) = parse_frontmatter(&content);
    let title = meta.title.unwrap_or_else(|| name.replace(['-', '_'], " "));
    Ok(Rule {
        name,
        title,
        priority: meta.priority.unwrap_or(100),
        enabled: meta.enabled.unwrap_or(true),
        body,
        source: path.to_path_buf(),
    })
}

/// Parse YAML frontmatter `--- ... ---\n<body>`. Returns default meta
/// when no frontmatter is present — a rule can be a plain markdown file.
fn parse_frontmatter(content: &str) -> (RuleMeta, String) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (RuleMeta::default(), content.to_string());
    }
    let after_open = &trimmed[3..];
    let Some(close_rel) = after_open.find("\n---") else {
        return (RuleMeta::default(), content.to_string());
    };
    let yaml = after_open[..close_rel].trim();
    let body = after_open[close_rel + 4..].trim_start().to_string();

    // Very small YAML subset: `key: value`, booleans, integers.
    let mut meta = RuleMeta::default();
    for line in yaml.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let k = k.trim();
        let v = v.trim().trim_matches('"').trim_matches('\'');
        match k {
            "title" => meta.title = Some(v.to_string()),
            "priority" => meta.priority = v.parse().ok(),
            "enabled" => {
                meta.enabled = match v {
                    "true" | "yes" | "1" => Some(true),
                    "false" | "no" | "0" => Some(false),
                    _ => None,
                }
            }
            _ => {}
        }
    }
    (meta, body)
}

/// Rewrite a rule file to set `enabled: <value>` while preserving the
/// body verbatim. Handles three cases: existing `enabled:` line, has
/// frontmatter but no enabled field, and no frontmatter at all.
fn rewrite_with_enabled(content: &str, enabled: bool) -> String {
    let target = format!("enabled: {enabled}");
    let trimmed = content.trim_start();

    if !trimmed.starts_with("---") {
        // No frontmatter — add one.
        return format!("---\n{target}\n---\n\n{}", content);
    }

    let Some(close_rel) = trimmed[3..].find("\n---") else {
        // Malformed frontmatter; prepend a new one instead of corrupting further.
        return format!("---\n{target}\n---\n\n{}", content);
    };
    let yaml = &trimmed[3..3 + close_rel];
    let rest = &trimmed[3 + close_rel + 4..];

    // Replace existing enabled: line, or append one.
    let new_yaml = if yaml.lines().any(|l| l.trim().starts_with("enabled:")) {
        yaml.lines()
            .map(|l| {
                if l.trim().starts_with("enabled:") {
                    target.clone()
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        format!("{}\n{target}", yaml.trim_end())
    };

    format!("---\n{new_yaml}\n---{rest}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mkdir_rules(tempdir: &std::path::Path) -> PathBuf {
        let dir = tempdir.join(".agent").join("rules");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn load_rules_returns_empty_when_dir_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let rules = load_project_rules(tmp.path());
        assert!(rules.is_empty());
    }

    #[test]
    fn load_rules_parses_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = mkdir_rules(tmp.path());
        std::fs::write(
            dir.join("test-first.md"),
            "---\n\
             title: Always run tests first\n\
             priority: 5\n\
             enabled: true\n\
             ---\n\
             \n\
             Never commit broken code.\n",
        )
        .unwrap();

        let rules = load_project_rules(tmp.path());
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].name, "test-first");
        assert_eq!(rules[0].title, "Always run tests first");
        assert_eq!(rules[0].priority, 5);
        assert!(rules[0].enabled);
        assert!(rules[0].body.contains("Never commit broken"));
    }

    #[test]
    fn load_rules_sorts_by_priority_then_name() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = mkdir_rules(tmp.path());
        std::fs::write(dir.join("c.md"), "---\npriority: 20\n---\nc body\n").unwrap();
        std::fs::write(dir.join("a.md"), "---\npriority: 10\n---\na body\n").unwrap();
        std::fs::write(dir.join("b.md"), "---\npriority: 10\n---\nb body\n").unwrap();

        let rules = load_project_rules(tmp.path());
        let order: Vec<_> = rules.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(order, vec!["a", "b", "c"]);
    }

    #[test]
    fn load_rules_tolerates_missing_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = mkdir_rules(tmp.path());
        std::fs::write(
            dir.join("plain.md"),
            "Just a body with no frontmatter at all.\n",
        )
        .unwrap();

        let rules = load_project_rules(tmp.path());
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].priority, 100);
        assert!(rules[0].enabled);
        assert_eq!(rules[0].title, "plain"); // Derived from filename.
    }

    #[test]
    fn rules_prompt_block_filters_disabled() {
        let rules = vec![
            Rule {
                name: "on".into(),
                title: "Enabled rule".into(),
                priority: 1,
                enabled: true,
                body: "Do the thing.".into(),
                source: PathBuf::new(),
            },
            Rule {
                name: "off".into(),
                title: "Disabled rule".into(),
                priority: 2,
                enabled: false,
                body: "Do the other thing.".into(),
                source: PathBuf::new(),
            },
        ];
        let block = rules_prompt_block(&rules);
        assert!(block.contains("Enabled rule"));
        assert!(!block.contains("Disabled rule"));
    }

    #[test]
    fn rules_prompt_block_is_empty_when_none_enabled() {
        let rules = vec![Rule {
            name: "off".into(),
            title: "Disabled".into(),
            priority: 1,
            enabled: false,
            body: "body".into(),
            source: PathBuf::new(),
        }];
        assert_eq!(rules_prompt_block(&rules), "");
    }

    #[test]
    fn set_rule_enabled_toggles_existing_field() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = mkdir_rules(tmp.path());
        std::fs::write(
            dir.join("t.md"),
            "---\ntitle: Test\nenabled: true\n---\n\nbody\n",
        )
        .unwrap();

        let changed = set_rule_enabled(tmp.path(), "t", false).unwrap();
        assert!(changed);
        let rules = load_project_rules(tmp.path());
        assert!(!rules[0].enabled);

        // Idempotent: calling with the same state returns Ok(false).
        let changed_again = set_rule_enabled(tmp.path(), "t", false).unwrap();
        assert!(!changed_again);
    }

    #[test]
    fn set_rule_enabled_adds_field_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = mkdir_rules(tmp.path());
        std::fs::write(dir.join("t.md"), "---\ntitle: Test\n---\n\nbody\n").unwrap();

        set_rule_enabled(tmp.path(), "t", false).unwrap();
        let rules = load_project_rules(tmp.path());
        assert!(!rules[0].enabled);
        // Body preserved verbatim.
        assert!(rules[0].body.contains("body"));
    }

    #[test]
    fn set_rule_enabled_errors_on_missing_rule() {
        let tmp = tempfile::tempdir().unwrap();
        let _ = mkdir_rules(tmp.path());
        let result = set_rule_enabled(tmp.path(), "nonexistent", false);
        assert!(result.is_err());
    }
}
