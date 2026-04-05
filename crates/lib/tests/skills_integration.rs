//! Integration tests for the skill system.
//!
//! Tests skill loading from directories, frontmatter parsing,
//! argument expansion, and bundled skill availability.

use std::io::Write;

use agent_code_lib::skills::SkillRegistry;

#[test]
fn bundled_skills_load_without_project_dir() {
    let registry = SkillRegistry::load_all(None);
    assert!(
        registry.all().len() >= 12,
        "Expected at least 12 bundled skills, got {}",
        registry.all().len()
    );
}

#[test]
fn bundled_skills_are_all_invocable() {
    let registry = SkillRegistry::load_all(None);
    let invocable = registry.user_invocable();
    assert!(
        invocable.len() >= 12,
        "Expected at least 12 invocable skills, got {}",
        invocable.len()
    );
}

#[test]
fn find_bundled_skill_by_name() {
    let registry = SkillRegistry::load_all(None);
    for name in [
        "commit",
        "review",
        "test",
        "explain",
        "debug",
        "pr",
        "refactor",
        "init",
        "security-review",
        "advisor",
        "bughunter",
        "plan",
    ] {
        assert!(
            registry.find(name).is_some(),
            "Bundled skill '{name}' not found"
        );
    }
}

#[test]
fn load_skill_from_temp_directory() {
    let dir = tempfile::tempdir().unwrap();
    let skills_dir = dir.path().join(".agent").join("skills");
    std::fs::create_dir_all(&skills_dir).unwrap();

    let skill_path = skills_dir.join("my-skill.md");
    let mut f = std::fs::File::create(&skill_path).unwrap();
    writeln!(
        f,
        "---\ndescription: A test skill\nuserInvocable: true\n---\n\nDo something with {{{{arg}}}}."
    )
    .unwrap();

    let registry = SkillRegistry::load_all(Some(dir.path()));
    let skill = registry.find("my-skill");
    assert!(skill.is_some(), "Custom skill 'my-skill' not loaded");

    let skill = skill.unwrap();
    assert_eq!(skill.metadata.description.as_deref(), Some("A test skill"));
    assert!(skill.metadata.user_invocable);

    let expanded = skill.expand(Some("main.rs"));
    assert!(expanded.contains("main.rs"));
}

#[test]
fn project_skill_overrides_bundled() {
    let dir = tempfile::tempdir().unwrap();
    let skills_dir = dir.path().join(".agent").join("skills");
    std::fs::create_dir_all(&skills_dir).unwrap();

    // Create a skill with the same name as a bundled one.
    let skill_path = skills_dir.join("commit.md");
    let mut f = std::fs::File::create(&skill_path).unwrap();
    writeln!(
        f,
        "---\ndescription: Custom commit workflow\nuserInvocable: true\n---\n\nCustom commit."
    )
    .unwrap();

    let registry = SkillRegistry::load_all(Some(dir.path()));
    let skill = registry.find("commit").unwrap();
    assert_eq!(
        skill.metadata.description.as_deref(),
        Some("Custom commit workflow"),
        "Project skill should override bundled"
    );
}

#[test]
fn directory_skill_with_skill_md() {
    let dir = tempfile::tempdir().unwrap();
    let skills_dir = dir.path().join(".agent").join("skills");
    let skill_subdir = skills_dir.join("deploy");
    std::fs::create_dir_all(&skill_subdir).unwrap();

    let skill_path = skill_subdir.join("SKILL.md");
    let mut f = std::fs::File::create(&skill_path).unwrap();
    writeln!(
        f,
        "---\ndescription: Deploy to production\nuserInvocable: true\n---\n\nDeploy steps."
    )
    .unwrap();

    let registry = SkillRegistry::load_all(Some(dir.path()));
    let skill = registry.find("deploy");
    assert!(skill.is_some(), "Directory skill 'deploy' not loaded");
    assert_eq!(
        skill.unwrap().metadata.description.as_deref(),
        Some("Deploy to production")
    );
}
