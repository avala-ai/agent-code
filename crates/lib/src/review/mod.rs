//! Review targets and prompt resolution (issue #524, first slice).
//!
//! A review is not "summarize this diff". The reviewer is an agent with
//! repo access, and the prompt's job is to tell it **how to find** the
//! change — the merge base to diff against, the commit to inspect —
//! rather than to embed a diff in the prompt. That is what lets it open
//! the file around a hunk, read the callers, and check whether a test
//! exists, which is the difference between a real finding and a
//! plausible one.
//!
//! This module is the resolution half: target → prompt. Running the
//! review in a constrained subagent and parsing structured findings are
//! separate pieces of #524.

/// The review rubric.
///
/// Mostly negative space on purpose: what stops a reviewer being useful
/// is not missing checks, it is volume — speculative findings, style
/// nits and pre-existing issues drown the two that matter.
///
/// Overridable per project via `.agent/review-rubric.md`.
pub const RUBRIC: &str = include_str!("rubric.md");

use std::path::Path;
use std::process::Command;

/// What a review is being asked to look at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewTarget {
    /// The working tree: staged, unstaged and untracked.
    Uncommitted,
    /// Everything this branch adds on top of `base`.
    BaseBranch { base: String },
    /// One commit.
    Commit { sha: String, title: Option<String> },
    /// Free-form instructions from the user.
    Custom { instructions: String },
}

/// A target resolved against the repository, ready to prompt with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedReview {
    pub target: ReviewTarget,
    /// The instruction handed to the reviewer.
    pub prompt: String,
    /// One line naming what is under review, for a UI.
    pub hint: String,
}

/// Parse a `/review` argument.
///
/// Bare `/review` means the working tree, which is what someone asking
/// for a review of "this" almost always means.
pub fn parse_target(args: Option<&str>) -> ReviewTarget {
    let args = args.map(str::trim).unwrap_or("");
    if args.is_empty() {
        return ReviewTarget::Uncommitted;
    }
    let (head, rest) = match args.split_once(char::is_whitespace) {
        Some((h, r)) => (h, r.trim()),
        None => (args, ""),
    };
    match head {
        "uncommitted" | "working" => ReviewTarget::Uncommitted,
        "base" if !rest.is_empty() => ReviewTarget::BaseBranch {
            base: rest.to_string(),
        },
        "commit" if !rest.is_empty() => ReviewTarget::Commit {
            sha: rest.to_string(),
            title: None,
        },
        // Anything else is treated as instructions rather than rejected:
        // `/review the auth changes` should work.
        _ => ReviewTarget::Custom {
            instructions: args.to_string(),
        },
    }
}

/// Resolve a target into the prompt the reviewer receives.
///
/// Resolution can touch git (to find a merge base). A git failure
/// degrades to a prompt that tells the reviewer how to find the merge
/// base itself, rather than failing the review outright — a review that
/// runs with a slightly vaguer instruction beats no review.
pub fn resolve(target: ReviewTarget, cwd: &Path) -> ResolvedReview {
    let hint = hint_for(&target);
    let prompt = match &target {
        ReviewTarget::Uncommitted => UNCOMMITTED_PROMPT.to_string(),
        ReviewTarget::BaseBranch { base } => match merge_base(cwd, base) {
            Some(sha) => format!(
                "Review the changes this branch adds on top of '{base}'. The merge base is \
                 {sha}. Run `git diff {sha}` to see exactly what would merge into {base}, then \
                 read the surrounding code as needed. Report prioritized, actionable findings."
            ),
            None => format!(
                "Review the changes this branch adds on top of '{base}'. Find the merge base \
                 yourself (`git merge-base HEAD {base}`), diff against it, then read the \
                 surrounding code as needed. Report prioritized, actionable findings."
            ),
        },
        ReviewTarget::Commit { sha, title } => {
            let named = match title {
                Some(t) if !t.is_empty() => format!(" (\"{t}\")"),
                _ => String::new(),
            };
            format!(
                "Review the changes introduced by commit {sha}{named}. Run `git show {sha}` to \
                 inspect them, then read the surrounding code as needed. Report prioritized, \
                 actionable findings."
            )
        }
        ReviewTarget::Custom { instructions } => format!(
            "Review the code as instructed: {instructions}\n\nInspect the repository as needed \
             to ground your findings. Report prioritized, actionable findings."
        ),
    };
    ResolvedReview {
        target,
        prompt,
        hint,
    }
}

const UNCOMMITTED_PROMPT: &str = "Review the current uncommitted changes — staged, unstaged and \
     untracked. Use `git status` and `git diff` (including `--staged`) to see them, then read the \
     surrounding code as needed. Report prioritized, actionable findings.";

fn hint_for(target: &ReviewTarget) -> String {
    match target {
        ReviewTarget::Uncommitted => "uncommitted changes".to_string(),
        ReviewTarget::BaseBranch { base } => format!("changes vs {base}"),
        ReviewTarget::Commit { sha, title } => {
            let short: String = sha.chars().take(8).collect();
            match title {
                Some(t) if !t.is_empty() => format!("commit {short} — {t}"),
                _ => format!("commit {short}"),
            }
        }
        ReviewTarget::Custom { instructions } => {
            let short: String = instructions.chars().take(60).collect();
            short
        }
    }
}

/// `git merge-base HEAD <base>`, or `None` when git cannot answer.
///
/// Tries the upstream of the base branch first: on a fork, `main` is
/// often stale while `origin/main` is what the change will actually
/// merge into, and diffing against the stale one shows unrelated commits
/// as if they were part of the change.
fn merge_base(cwd: &Path, base: &str) -> Option<String> {
    for candidate in [format!("{base}@{{upstream}}"), base.to_string()] {
        if let Some(sha) = run_git(cwd, &["merge-base", "HEAD", &candidate]) {
            return Some(sha);
        }
    }
    None
}

fn run_git(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

#[cfg(test)]
mod tests {
    /// The rubric is the difference between a reviewer and a commenter.
    /// These are the constraints that keep it from producing volume.
    #[test]
    fn the_rubric_states_its_exclusions() {
        for required in [
            "introduced by this change",
            "Pre-existing",
            "no findings",
            "AGENTS.md",
            "[P0]",
            "overall verdict",
        ] {
            assert!(
                RUBRIC.contains(required),
                "rubric lost its `{required}` guidance"
            );
        }
    }

    use super::*;

    #[test]
    fn bare_review_means_the_working_tree() {
        assert_eq!(parse_target(None), ReviewTarget::Uncommitted);
        assert_eq!(parse_target(Some("")), ReviewTarget::Uncommitted);
        assert_eq!(parse_target(Some("  ")), ReviewTarget::Uncommitted);
    }

    #[test]
    fn explicit_targets_parse() {
        assert_eq!(parse_target(Some("uncommitted")), ReviewTarget::Uncommitted);
        assert_eq!(
            parse_target(Some("base main")),
            ReviewTarget::BaseBranch {
                base: "main".into()
            }
        );
        assert_eq!(
            parse_target(Some("commit abc123")),
            ReviewTarget::Commit {
                sha: "abc123".into(),
                title: None
            }
        );
    }

    /// `/review the auth changes` should review, not error. Treating an
    /// unrecognised head as instructions is what makes that work.
    #[test]
    fn free_text_becomes_instructions() {
        assert_eq!(
            parse_target(Some("the auth changes")),
            ReviewTarget::Custom {
                instructions: "the auth changes".into()
            }
        );
        // A keyword with no argument is instructions too, not a silent
        // fallback to something the user did not ask for.
        assert_eq!(
            parse_target(Some("base")),
            ReviewTarget::Custom {
                instructions: "base".into()
            }
        );
    }

    /// The prompt must tell the reviewer how to *find* the change. A
    /// prompt that only describes it leaves the agent guessing.
    #[test]
    fn every_prompt_names_a_command_to_run() {
        let cwd = std::env::current_dir().unwrap();
        for target in [
            ReviewTarget::Uncommitted,
            ReviewTarget::BaseBranch {
                base: "main".into(),
            },
            ReviewTarget::Commit {
                sha: "deadbeef".into(),
                title: None,
            },
        ] {
            let r = resolve(target.clone(), &cwd);
            assert!(
                r.prompt.contains("git "),
                "{target:?} produced a prompt with no command: {}",
                r.prompt
            );
            assert!(
                r.prompt.contains("actionable findings"),
                "{target:?} lost the findings instruction"
            );
        }
    }

    #[test]
    fn a_commit_target_mentions_the_sha_and_title() {
        let cwd = std::env::current_dir().unwrap();
        let r = resolve(
            ReviewTarget::Commit {
                sha: "abc1234".into(),
                title: Some("fix the parser".into()),
            },
            &cwd,
        );
        assert!(r.prompt.contains("abc1234"));
        assert!(r.prompt.contains("fix the parser"));
        assert!(r.hint.contains("fix the parser"));
    }

    /// A repo with no git (or an unknown base) must still produce a
    /// usable prompt — a review that runs with a vaguer instruction
    /// beats no review.
    #[test]
    fn an_unresolvable_base_still_produces_a_prompt() {
        let tmp = tempfile::tempdir().unwrap();
        let r = resolve(
            ReviewTarget::BaseBranch {
                base: "nonexistent-branch-xyz".into(),
            },
            tmp.path(),
        );
        assert!(r.prompt.contains("merge-base"), "prompt: {}", r.prompt);
        assert!(r.prompt.contains("nonexistent-branch-xyz"));
    }

    /// In a real repo the merge base is resolved, so the reviewer gets a
    /// concrete SHA instead of being told to work it out.
    #[test]
    fn a_real_base_resolves_to_a_sha() {
        let cwd = std::env::current_dir().unwrap();
        // Every checkout has HEAD; merge-base HEAD HEAD is HEAD itself.
        let Some(head) = run_git(&cwd, &["rev-parse", "HEAD"]) else {
            return; // not a git checkout; nothing to assert
        };
        let r = resolve(
            ReviewTarget::BaseBranch {
                base: "HEAD".into(),
            },
            &cwd,
        );
        assert!(
            r.prompt.contains(&head),
            "merge base was not resolved into the prompt: {}",
            r.prompt
        );
    }

    #[test]
    fn hints_describe_what_is_under_review() {
        assert_eq!(hint_for(&ReviewTarget::Uncommitted), "uncommitted changes");
        assert_eq!(
            hint_for(&ReviewTarget::BaseBranch {
                base: "main".into()
            }),
            "changes vs main"
        );
        assert_eq!(
            hint_for(&ReviewTarget::Commit {
                sha: "abc12345678".into(),
                title: None
            }),
            "commit abc12345"
        );
    }
}
