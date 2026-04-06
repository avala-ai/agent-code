use crate::harness::EvalDef;
use crate::rig::TestRig;
use anyhow::Result;

/// Macro for declaratively defining behavioral evals.
///
/// ```rust,ignore
/// eval_def! {
///     name: "creates_new_file",
///     policy: AlwaysPasses,
///     fixture: "evals/fixtures/empty_project/",
///     prompt: "Create hello.py that prints Hello",
///     max_turns: 5,
///     assert: |rig| {
///         assert!(rig.file_exists("hello.py"));
///         Ok(())
///     }
/// }
/// ```
#[macro_export]
macro_rules! eval_def {
    (
        name: $name:expr,
        policy: $policy:ident,
        $(fixture: $fixture:expr,)?
        prompt: $prompt:expr,
        max_turns: $max_turns:expr,
        assert: $assert_fn:expr $(,)?
    ) => {
        $crate::harness::EvalDef {
            name: $name,
            policy: $crate::policy::EvalPolicy::$policy,
            fixture: eval_def!(@fixture $($fixture)?),
            prompt: $prompt,
            max_turns: $max_turns,
            assert_fn: $assert_fn,
        }
    };
    (@fixture $f:expr) => { Some($f) };
    (@fixture) => { None };
}

/// Return all registered eval definitions.
pub fn all_evals() -> Vec<EvalDef> {
    vec![
        // ── File Operations ────────────────────────────────
        eval_def! {
            name: "creates_new_file_when_asked",
            policy: AlwaysPasses,
            fixture: "evals/fixtures/empty_project/",
            prompt: "Create a file called hello.py with this exact content: print('Hello, world!')",
            max_turns: 5,
            assert: |rig: &TestRig| -> Result<()> {
                anyhow::ensure!(rig.file_exists("hello.py"), "hello.py not created");
                let content = rig.read_file("hello.py")?;
                anyhow::ensure!(content.contains("Hello, world!"), "Missing Hello, world! in content");
                Ok(())
            }
        },
        eval_def! {
            name: "edits_existing_file",
            policy: AlwaysPasses,
            fixture: "evals/fixtures/rust_project/",
            prompt: "In src/main.rs, change the greeting from 'Hello' to 'Goodbye'",
            max_turns: 5,
            assert: |rig: &TestRig| -> Result<()> {
                let content = rig.read_file("src/main.rs")?;
                anyhow::ensure!(content.contains("Goodbye"), "File not edited to contain Goodbye");
                anyhow::ensure!(!content.contains("Hello"), "Old Hello text still present");
                Ok(())
            }
        },
        eval_def! {
            name: "creates_multiple_files",
            policy: UsuallyPasses,
            fixture: "evals/fixtures/empty_project/",
            prompt: "Create three files: src/lib.rs with 'pub fn add(a: i32, b: i32) -> i32 { a + b }', src/main.rs with 'fn main() { println!(\"{}\", src::add(1, 2)); }', and Cargo.toml with a valid Rust project config named 'myapp'",
            max_turns: 8,
            assert: |rig: &TestRig| -> Result<()> {
                anyhow::ensure!(rig.file_exists("src/lib.rs"), "src/lib.rs not created");
                anyhow::ensure!(rig.file_exists("src/main.rs"), "src/main.rs not created");
                anyhow::ensure!(rig.file_exists("Cargo.toml"), "Cargo.toml not created");
                let toml = rig.read_file("Cargo.toml")?;
                anyhow::ensure!(toml.contains("myapp"), "Cargo.toml missing project name");
                Ok(())
            }
        },
        // ── Search & Navigation ────────────────────────────
        eval_def! {
            name: "uses_grep_to_find_pattern",
            policy: AlwaysPasses,
            fixture: "evals/fixtures/rust_project/",
            prompt: "Find all files containing the word 'Hello' and tell me which files and line numbers",
            max_turns: 5,
            assert: |rig: &TestRig| -> Result<()> {
                // Agent should mention main.rs in its response.
                anyhow::ensure!(
                    rig.response_text.contains("main.rs"),
                    "Response should mention main.rs"
                );
                Ok(())
            }
        },
        eval_def! {
            name: "uses_glob_to_list_files",
            policy: UsuallyPasses,
            fixture: "evals/fixtures/rust_project/",
            prompt: "List all .rs files in this project",
            max_turns: 5,
            assert: |rig: &TestRig| -> Result<()> {
                anyhow::ensure!(
                    rig.response_text.contains("main.rs"),
                    "Response should list main.rs"
                );
                Ok(())
            }
        },
        // ── Shell Execution ────────────────────────────────
        eval_def! {
            name: "executes_bash_command",
            policy: AlwaysPasses,
            prompt: "Run 'echo EVAL_TEST_MARKER' using bash and show the output",
            max_turns: 3,
            assert: |rig: &TestRig| -> Result<()> {
                anyhow::ensure!(
                    rig.response_text.contains("EVAL_TEST_MARKER"),
                    "Response should contain the echo output"
                );
                Ok(())
            }
        },
        // ── Error Recovery ─────────────────────────────────
        eval_def! {
            name: "recovers_from_nonexistent_file_read",
            policy: UsuallyPasses,
            fixture: "evals/fixtures/empty_project/",
            prompt: "Read the file called nonexistent.txt and tell me what happened",
            max_turns: 5,
            assert: |rig: &TestRig| -> Result<()> {
                // Agent should report the file doesn't exist, not crash.
                let response_lower = rig.response_text.to_lowercase();
                anyhow::ensure!(
                    response_lower.contains("not found")
                        || response_lower.contains("doesn't exist")
                        || response_lower.contains("does not exist")
                        || response_lower.contains("no such file")
                        || response_lower.contains("error"),
                    "Agent should report file not found gracefully"
                );
                Ok(())
            }
        },
        // ── Multi-turn Conversation ────────────────────────
        eval_def! {
            name: "maintains_context_across_turns",
            policy: UsuallyPasses,
            fixture: "evals/fixtures/empty_project/",
            prompt: "Create a file called secret.txt containing 'The code is ALPHA-7'. Then read it back and tell me the code.",
            max_turns: 8,
            assert: |rig: &TestRig| -> Result<()> {
                anyhow::ensure!(rig.file_exists("secret.txt"), "secret.txt not created");
                anyhow::ensure!(
                    rig.response_text.contains("ALPHA-7"),
                    "Agent should report the code ALPHA-7"
                );
                Ok(())
            }
        },
        // ── Test Writing ───────────────────────────────────
        eval_def! {
            name: "writes_test_for_function",
            policy: UsuallyPasses,
            fixture: "evals/fixtures/rust_project/",
            prompt: "Add a function 'fn add(a: i32, b: i32) -> i32 { a + b }' to src/main.rs and write a unit test for it in the same file",
            max_turns: 8,
            assert: |rig: &TestRig| -> Result<()> {
                let content = rig.read_file("src/main.rs")?;
                anyhow::ensure!(content.contains("fn add"), "add function not created");
                anyhow::ensure!(
                    content.contains("#[test]") || content.contains("#[cfg(test)]"),
                    "No test annotation found"
                );
                Ok(())
            }
        },
        // ── Plan Mode ──────────────────────────────────────
        eval_def! {
            name: "respects_read_only_prompt",
            policy: UsuallyPasses,
            fixture: "evals/fixtures/rust_project/",
            prompt: "Explain what src/main.rs does. Do NOT modify any files.",
            max_turns: 5,
            assert: |rig: &TestRig| -> Result<()> {
                // Check that main.rs was not modified.
                let content = rig.read_file("src/main.rs")?;
                anyhow::ensure!(
                    content.contains("Hello"),
                    "main.rs should still contain original Hello text"
                );
                anyhow::ensure!(
                    !rig.response_text.is_empty(),
                    "Agent should produce an explanation"
                );
                Ok(())
            }
        },
    ]
}
