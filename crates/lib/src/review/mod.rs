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

/// A target that cannot be turned into a review.
///
/// Surfaced to the user instead of being reviewed anyway: a target that
/// silently resolves to the wrong thing produces a *clean* review of
/// code nobody asked about, which is worse than an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidTarget {
    /// What the user typed.
    pub input: String,
    /// One line explaining why, for a UI.
    pub reason: String,
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
/// Resolution can touch git (to find a merge base). A git failure on the
/// *merge base* degrades to a prompt that tells the reviewer how to find
/// it, rather than failing the review outright — a review that runs with
/// a slightly vaguer instruction beats no review.
///
/// A commit target gets no such benefit of the doubt: it names the one
/// thing under review, so an unresolvable one is an error. See
/// [`validate_revision`].
pub fn resolve(target: ReviewTarget, cwd: &Path) -> Result<ResolvedReview, InvalidTarget> {
    // Validate before anything is interpolated into a command.
    let target = match target {
        ReviewTarget::Commit { sha, title } => ReviewTarget::Commit {
            sha: validate_commit(cwd, &sha)?,
            title,
        },
        ReviewTarget::BaseBranch { base } => {
            validate_revision(&base)?;
            ReviewTarget::BaseBranch { base }
        }
        other => other,
    };
    let hint = hint_for(&target);
    let prompt = match &target {
        ReviewTarget::Uncommitted => UNCOMMITTED_PROMPT.to_string(),
        ReviewTarget::BaseBranch { base } => match merge_base(cwd, base) {
            // Two-commit form on purpose. `git diff <merge-base>` diffs
            // the base against the *working tree*, so uncommitted work
            // would be reviewed as if the branch added it.
            Some(sha) => format!(
                "Review the changes this branch adds on top of '{base}'. The merge base is \
                 {sha}. Run `git diff {sha} HEAD` to see exactly what would merge into {base}, \
                 then read the surrounding code as needed. Report prioritized, actionable \
                 findings."
            ),
            None => format!(
                "Review the changes this branch adds on top of '{base}'. Find the merge base \
                 yourself (`git merge-base HEAD {base}`) and diff it against HEAD (`git diff \
                 <merge-base> HEAD`), then read the surrounding code as needed. Report \
                 prioritized, actionable findings."
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
    Ok(ResolvedReview {
        target,
        prompt,
        hint,
    })
}

/// Reject anything that is not a single revision word.
///
/// Everything here ends up interpolated into a command the reviewer is
/// told to run, and git's own usage (`git show [<options>] <object>...`)
/// parses a leading `-` as an option. `/review commit --no-patch` would
/// otherwise become `git show --no-patch`, which *succeeds* — it shows
/// HEAD with no diff — so the reviewer inspects the wrong commit and
/// comes back clean.
fn validate_revision(rev: &str) -> Result<(), InvalidTarget> {
    let invalid = |reason: &str| InvalidTarget {
        input: rev.to_string(),
        reason: reason.to_string(),
    };
    if rev.starts_with('-') {
        return Err(invalid("that is a git option, not a revision"));
    }
    if rev.split_whitespace().count() != 1 {
        return Err(invalid("pass exactly one revision"));
    }
    Ok(())
}

/// Resolve a commit target to a full SHA, or fail.
///
/// Unlike the merge base, this cannot degrade to a vaguer instruction:
/// the commit *is* the thing under review, so a target git cannot name
/// exactly one commit for is an error rather than a review of whatever
/// git picks instead.
fn validate_commit(cwd: &Path, sha: &str) -> Result<String, InvalidTarget> {
    validate_revision(sha)?;
    // `^{commit}` makes the peel explicit: a tree or blob is not a commit.
    run_git(
        cwd,
        &["rev-parse", "--verify", &format!("{sha}^{{commit}}")],
    )
    .ok_or_else(|| InvalidTarget {
        input: sha.to_string(),
        reason: "no such commit in this repository".to_string(),
    })
}

/// Untracked files get their own instruction on purpose: they are
/// outside the index, so `git diff` and `git diff --staged` both skip
/// them and `git status` lists only their paths. A brand-new file is
/// exactly the kind of code a review should not miss.
const UNCOMMITTED_PROMPT: &str = "Review the current uncommitted changes — staged, unstaged and \
     untracked. Run `git status`, then `git diff` and `git diff --staged` for edits to tracked \
     files. Untracked files appear in neither diff, so list them with `git ls-files --others \
     --exclude-standard` and read each one in full. Then read the surrounding code as needed. \
     Report prioritized, actionable findings.";

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
        let mut targets = vec![
            ReviewTarget::Uncommitted,
            ReviewTarget::BaseBranch {
                base: "main".into(),
            },
        ];
        // A commit target must name a real commit now, so use this one.
        if let Some(head) = run_git(&cwd, &["rev-parse", "HEAD"]) {
            targets.push(ReviewTarget::Commit {
                sha: head,
                title: None,
            });
        }
        for target in targets {
            let r = resolve(target.clone(), &cwd).expect("a valid target must resolve");
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
        let Some(head) = run_git(&cwd, &["rev-parse", "HEAD"]) else {
            return; // not a git checkout; nothing to resolve against
        };
        let r = resolve(
            ReviewTarget::Commit {
                // Abbreviated on input, full SHA in the prompt.
                sha: head[..8].to_string(),
                title: Some("fix the parser".into()),
            },
            &cwd,
        )
        .expect("HEAD must resolve");
        assert!(r.prompt.contains(&head), "prompt: {}", r.prompt);
        assert!(r.prompt.contains("fix the parser"));
        assert!(r.hint.contains("fix the parser"));
    }

    /// A bare `/review` promises untracked files, but no diff shows
    /// them: they are outside the index, and `git status` prints only
    /// their paths. Without an explicit instruction a whole new file
    /// gets a clean review from a reviewer that never opened it.
    #[test]
    fn the_uncommitted_prompt_says_how_to_read_untracked_files() {
        let cwd = std::env::current_dir().unwrap();
        let r = resolve(ReviewTarget::Uncommitted, &cwd).expect("the working tree always resolves");
        assert!(
            r.prompt
                .contains("git ls-files --others --exclude-standard"),
            "no way to enumerate untracked files: {}",
            r.prompt
        );
        assert!(
            r.prompt.contains("read each one"),
            "untracked files are listed but never opened: {}",
            r.prompt
        );
    }

    /// Every value here is interpolated into a command the reviewer is
    /// told to run. `git show --no-patch` succeeds — it shows HEAD with
    /// no diff — so an unvalidated target yields a confident review of a
    /// commit nobody asked about. Fail closed instead.
    #[test]
    fn a_commit_target_that_is_not_one_commit_is_rejected() {
        let cwd = std::env::current_dir().unwrap();
        for bad in ["--no-patch", "-p", "HEAD --stat", "no-such-ref-xyz"] {
            let err = resolve(
                ReviewTarget::Commit {
                    sha: bad.into(),
                    title: None,
                },
                &cwd,
            )
            .expect_err("`{bad}` was accepted as a commit");
            assert_eq!(err.input, bad);
            assert!(!err.reason.is_empty(), "no reason given for `{bad}`");
        }
        // The same goes for a base branch: it reaches `git merge-base`.
        for bad in ["--no-patch", "main extra"] {
            assert!(
                resolve(ReviewTarget::BaseBranch { base: bad.into() }, &cwd).is_err(),
                "`{bad}` was accepted as a base branch"
            );
        }
        // And a valid one still works.
        assert!(
            resolve(
                ReviewTarget::BaseBranch {
                    base: "main".into()
                },
                &cwd
            )
            .is_ok()
        );
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
        )
        .expect("an unknown base degrades, it does not fail");
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
        )
        .expect("HEAD must resolve");
        assert!(
            r.prompt.contains(&head),
            "merge base was not resolved into the prompt: {}",
            r.prompt
        );
    }

    /// `git diff <merge-base>` is the one-commit form: it compares the
    /// base to the **working tree**, so staged and unstaged edits get
    /// reviewed as if the branch introduced them. A base-branch review
    /// has to name two commits.
    #[test]
    fn a_base_review_diffs_the_merge_base_against_head() {
        let cwd = std::env::current_dir().unwrap();
        if let Some(head) = run_git(&cwd, &["rev-parse", "HEAD"]) {
            let r = resolve(
                ReviewTarget::BaseBranch {
                    base: "HEAD".into(),
                },
                &cwd,
            )
            .expect("HEAD must resolve");
            assert!(
                r.prompt.contains(&format!("git diff {head} HEAD")),
                "resolved base prompt lost the two-commit diff: {}",
                r.prompt
            );
            assert!(
                !r.prompt.contains(&format!("git diff {head}`")),
                "one-commit form pulls the working tree into the review: {}",
                r.prompt
            );
        }

        // The degraded path tells the reviewer to find the merge base
        // itself; it must ask for the same two-commit diff.
        let tmp = tempfile::tempdir().unwrap();
        let d = resolve(
            ReviewTarget::BaseBranch {
                base: "nonexistent-branch-xyz".into(),
            },
            tmp.path(),
        )
        .expect("an unknown base degrades, it does not fail");
        assert!(
            d.prompt.contains("<merge-base> HEAD"),
            "degraded base prompt lost the two-commit diff: {}",
            d.prompt
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
