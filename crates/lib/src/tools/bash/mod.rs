//! Bash tool: execute shell commands.
//!
//! Runs commands via the system shell. Features:
//! - Timeout with configurable duration (default 2min, max 10min)
//! - Background execution for long-running commands
//! - Sandbox mode: blocks writes outside the project directory
//! - Destructive command warnings
//! - Output truncation for large results
//! - Cancellation via CancellationToken
//!
//! The safety pipeline is split into named sub-modules so each helper
//! can be tested in isolation:
//!
//! - [`bash_security`] — destructive-command classification
//! - [`command_semantics`] — coarse effect classification
//! - [`read_only_validation`] — refuse mutations under read-only profile
//! - [`sandbox_decision`] — single decision function for a command
//! - [`sed_edit_parser`] + [`sed_validation`] — gate `sed -i` through the
//!   FileEdit permission path
//!
//! `bash.rs` itself is now a thin orchestrator: it calls the helpers in
//! order from `validate_input`, then dispatches to the shell with the
//! sandbox wrapping the host already configured.

pub mod bash_security;
pub mod command_semantics;
pub mod protected_paths;
pub mod read_only_validation;
pub mod sandbox_decision;
pub mod sed_edit_parser;
pub mod sed_validation;

use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

pub use bash_security::{DestructiveFinding, DestructivenessLevel, classify_destructive};
pub use command_semantics::{Effect, classify, classify_single, is_read_only};
pub use read_only_validation::{PermissionProfile, ReadOnlyViolation, validate_read_only};
pub use sandbox_decision::{ExecutionContext, SandboxDecision, decide};
pub use sed_edit_parser::{SedEdit, parse_sed_edits};
pub use sed_validation::{FileEditPermission, SedViolation, validate_sed_edits};

/// Maximum output size before truncation (256KB).
const MAX_OUTPUT_BYTES: usize = 256 * 1024;

/// System paths that should never be written to from the bash tool.
///
/// Mirrors the protected-directories concept from
/// [`crate::permissions`] but for absolute system paths that no agent
/// command should ever modify regardless of cwd. Crate-visible so the
/// shared `protected_paths` helper can match against it without
/// duplicating the list.
///
/// The project's team-memory directory (`.agent/team-memory/`) is
/// blocked separately by [`protected_paths`], which delegates to
/// [`crate::permissions::is_team_memory_write_target`] so the same
/// predicate is used by every write surface.
pub(crate) const BLOCKED_WRITE_PATHS: &[&str] = &[
    "/etc/", "/usr/", "/bin/", "/sbin/", "/boot/", "/sys/", "/proc/",
];

pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "Bash"
    }

    fn description(&self) -> &'static str {
        "Executes a shell command and returns its output."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (max 600000)"
                },
                "description": {
                    "type": "string",
                    "description": "Description of what this command does"
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": "Run the command in the background and return immediately"
                },
                "dangerouslyDisableSandbox": {
                    "type": "boolean",
                    "description": "Disable safety checks for this command. Use only when explicitly asked."
                }
            }
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn get_path(&self, _input: &serde_json::Value) -> Option<PathBuf> {
        None
    }

    fn validate_input(&self, input: &serde_json::Value) -> Result<(), ToolError> {
        let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");

        // `dangerouslyDisableSandbox` only affects whether the command
        // runs INSIDE the sandbox or outside it — it does NOT skip
        // destructive-pattern or protected-path validation. The bypass
        // exists for tooling that legitimately needs network or full
        // host access; it must never let `rm -rf /` or a write into
        // `.git/` slip through.

        // Build a parsed view of the command so the classifier modules
        // can reason about it. If the parser fails we fall back to a
        // raw-only `ParsedCommand` so destructive-pattern detection
        // still runs.
        let parsed = super::bash_parse::parse_bash(command).unwrap_or_else(|| {
            super::bash_parse::ParsedCommand {
                raw: command.to_string(),
                ..super::bash_parse::ParsedCommand::default()
            }
        });

        // Destructive-command classifier. Mirrors the historical inline
        // checks (substring match, piped destructive base, chained
        // segments) but lives in a single named helper.
        let findings = bash_security::find_destructive(&parsed);
        if let Some(first) = findings.first() {
            return Err(ToolError::InvalidInput(format!(
                "Potentially destructive command detected: {}. \
                 This command could cause data loss or system damage. \
                 If you're sure, ask the user for confirmation first.",
                first.reason
            )));
        }

        // Advanced shell-injection checks (separate from destructive
        // patterns; flagged as "obfuscation" rather than "destruction").
        check_shell_injection(command).map_err(ToolError::InvalidInput)?;

        // Unified protected-path check: covers redirections, writer
        // commands (cp/mv/tee/dd/install/ln/rsync/truncate),
        // recursive shell payloads (`bash -c '…'`, `eval '…'`), and a
        // heuristic scan of inline interpreter source (`python -c`,
        // `node -e`, etc.) for `PROTECTED_DIRS`, the system path list
        // `BLOCKED_WRITE_PATHS`, and the team-memory directory.
        if let Err(violation) = protected_paths::check(command) {
            return Err(ToolError::InvalidInput(violation.reason));
        }

        // Tree-sitter AST analysis (catches obfuscation that regex misses).
        let violations = super::bash_parse::check_parsed_security(&parsed);
        if let Some(first) = violations.first() {
            return Err(ToolError::InvalidInput(format!(
                "AST security check: {first}"
            )));
        }

        Ok(())
    }

    async fn call(
        &self,
        input: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let command = input
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("'command' is required".into()))?;

        // Route any `sed -i` invocations through the FileEdit permission
        // path so protected directories stay protected from both surfaces.
        let parsed = super::bash_parse::parse_bash(command).unwrap_or_else(|| {
            super::bash_parse::ParsedCommand {
                raw: command.to_string(),
                ..super::bash_parse::ParsedCommand::default()
            }
        });
        if let Err(violation) =
            sed_validation::validate_sed_edits(&parsed, &ctx.cwd, ctx.permission_checker.as_ref())
        {
            return Err(ToolError::InvalidInput(format!(
                "sed -i refused for '{}': {}",
                violation.file.display(),
                violation.reason
            )));
        }

        let timeout_ms = input
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(120_000)
            .min(600_000);

        let run_in_background = input
            .get("run_in_background")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Honor a tool-call-level `dangerouslyDisableSandbox: true` by
        // skipping the sandbox wrapper. This path is blocked entirely
        // when the session has `security.disable_bypass_permissions = true`.
        let disable_sandbox_requested = input
            .get("dangerouslyDisableSandbox")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Fail closed BEFORE dispatch — including the background path, which
        // spawns bash directly and skips the sandbox wrapper. A degraded,
        // fail-closed sandbox must never run a command unsandboxed unless an
        // explicit, permitted bypass was requested.
        if let Some(ref sandbox) = ctx.sandbox
            && sandbox.must_block_when_degraded()
            && !(disable_sandbox_requested && sandbox.allow_bypass())
        {
            return Err(ToolError::PermissionDenied(
                "Sandbox is enabled but no working strategy is available on this \
                 platform, so the command was not run (fail-closed). Install the \
                 sandbox backend (e.g. `bwrap` on Linux), set `sandbox.enabled = false`, \
                 or set `sandbox.fail_closed = false` to allow running without isolation."
                    .to_string(),
            ));
        }

        // Background execution: spawn and return immediately.
        if run_in_background {
            return run_background(command, &ctx.cwd, ctx.task_manager.as_ref()).await;
        }

        // Build the base bash command.
        let mut base = Command::new("bash");
        base.arg("-c")
            .arg(command)
            .current_dir(&ctx.cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut cmd = if let Some(ref sandbox) = ctx.sandbox {
            if disable_sandbox_requested && sandbox.allow_bypass() {
                tracing::warn!(
                    "bash call set dangerouslyDisableSandbox; wrapping skipped for this call"
                );
                base
            } else {
                if disable_sandbox_requested && !sandbox.allow_bypass() {
                    tracing::warn!(
                        "dangerouslyDisableSandbox ignored: security.disable_bypass_permissions is set"
                    );
                }
                // Fail-closed refusal is handled before dispatch, above.
                sandbox.wrap(base)
            }
        } else {
            base
        };

        let mut child = cmd
            .spawn()
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to spawn: {e}")))?;

        let timeout = Duration::from_millis(timeout_ms);

        let mut stdout_handle = child.stdout.take().unwrap();
        let mut stderr_handle = child.stderr.take().unwrap();

        let mut stdout_buf = Vec::new();
        let mut stderr_buf = Vec::new();

        let result = tokio::select! {
            r = async {
                let (so, se) = tokio::join!(
                    async { stdout_handle.read_to_end(&mut stdout_buf).await },
                    async { stderr_handle.read_to_end(&mut stderr_buf).await },
                );
                so?;
                se?;
                child.wait().await
            } => {
                match r {
                    Ok(status) => {
                        let exit_code = status.code().unwrap_or(-1);
                        let content = format_output(&stdout_buf, &stderr_buf, exit_code);

                        Ok(ToolResult {
                            content,
                            is_error: exit_code != 0,
                        })
                    }
                    Err(e) => Err(ToolError::ExecutionFailed(e.to_string())),
                }
            }
            _ = tokio::time::sleep(timeout) => {
                let _ = child.kill().await;
                Err(ToolError::Timeout(timeout_ms))
            }
            _ = ctx.cancel.cancelled() => {
                let _ = child.kill().await;
                Err(ToolError::Cancelled)
            }
        };

        result
    }
}

/// Run a command in the background, returning a task ID immediately.
async fn run_background(
    command: &str,
    cwd: &std::path::Path,
    task_mgr: Option<&std::sync::Arc<crate::services::background::TaskManager>>,
) -> Result<ToolResult, ToolError> {
    let default_mgr = crate::services::background::TaskManager::new();
    let task_mgr = task_mgr.map(|m| m.as_ref()).unwrap_or(&default_mgr);
    let task_id = task_mgr
        .spawn_shell(command, command, cwd)
        .await
        .map_err(|e| ToolError::ExecutionFailed(format!("Background spawn failed: {e}")))?;

    Ok(ToolResult::success(format!(
        "Command running in background (task {task_id}). \
         Use TaskOutput to check results when complete."
    )))
}

/// Format stdout/stderr into a single output string with truncation.
fn format_output(stdout: &[u8], stderr: &[u8], exit_code: i32) -> String {
    let stdout_str = String::from_utf8_lossy(stdout);
    let stderr_str = String::from_utf8_lossy(stderr);

    let mut content = String::new();

    if !stdout_str.is_empty() {
        if stdout_str.len() > MAX_OUTPUT_BYTES {
            content.push_str(&stdout_str[..MAX_OUTPUT_BYTES]);
            content.push_str(&format!(
                "\n\n(stdout truncated: {} bytes total)",
                stdout_str.len()
            ));
        } else {
            content.push_str(&stdout_str);
        }
    }

    if !stderr_str.is_empty() {
        if !content.is_empty() {
            content.push('\n');
        }
        let stderr_display = if stderr_str.len() > MAX_OUTPUT_BYTES / 4 {
            format!(
                "{}...\n(stderr truncated: {} bytes total)",
                &stderr_str[..MAX_OUTPUT_BYTES / 4],
                stderr_str.len()
            )
        } else {
            stderr_str.to_string()
        };
        content.push_str(&format!("stderr:\n{stderr_display}"));
    }

    if content.is_empty() {
        content = "(no output)".to_string();
    }

    if exit_code != 0 {
        content.push_str(&format!("\n\nExit code: {exit_code}"));
    }

    content
}

/// Advanced shell injection and obfuscation detection.
///
/// Catches attack patterns that simple string matching misses:
/// variable injection, encoding tricks, process substitution, etc.
fn check_shell_injection(command: &str) -> Result<(), String> {
    // IFS injection: changing field separator to bypass argument parsing.
    if command.contains("IFS=") {
        return Err(
            "IFS manipulation detected. This can be used to bypass command parsing.".into(),
        );
    }

    // Dangerous environment variable overwrites.
    const DANGEROUS_VARS: &[&str] = &[
        "PATH=",
        "LD_PRELOAD=",
        "LD_LIBRARY_PATH=",
        "PROMPT_COMMAND=",
        "BASH_ENV=",
        "ENV=",
        "HISTFILE=",
        "HISTCONTROL=",
        "PS1=",
        "PS2=",
        "PS4=",
        "CDPATH=",
        "GLOBIGNORE=",
        "MAIL=",
        "MAILCHECK=",
        "MAILPATH=",
    ];
    for var in DANGEROUS_VARS {
        if command.contains(var) {
            return Err(format!(
                "Dangerous variable override detected: {var} \
                 This could alter shell behavior in unsafe ways."
            ));
        }
    }

    // /proc access (process environment/memory reading).
    if command.contains("/proc/") && command.contains("environ") {
        return Err("Access to /proc/*/environ detected. This reads process secrets.".into());
    }

    // Unicode/zero-width character obfuscation.
    if command.chars().any(|c| {
        matches!(
            c,
            '\u{200B}'
                | '\u{200C}'
                | '\u{200D}'
                | '\u{FEFF}'
                | '\u{00AD}'
                | '\u{2060}'
                | '\u{180E}'
        )
    }) {
        return Err("Zero-width or invisible Unicode characters detected in command.".into());
    }

    // Control characters (except common ones like \n \t).
    if command
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\t' | '\r'))
    {
        return Err("Control characters detected in command.".into());
    }

    // Backtick command substitution inside variable assignments.
    // e.g., FOO=`curl evil.com`
    if command.contains('`')
        && command
            .split('`')
            .any(|s| s.contains("curl") || s.contains("wget") || s.contains("nc "))
    {
        return Err("Command substitution with network access detected inside backticks.".into());
    }

    // Process substitution: <() or >() used to inject commands.
    if command.contains("<(") || command.contains(">(") {
        // Allow common safe uses like diff <(cmd1) <(cmd2).
        let trimmed = command.trim();
        if !trimmed.starts_with("diff ") && !trimmed.starts_with("comm ") {
            return Err(
                "Process substitution detected. This can inject arbitrary commands.".into(),
            );
        }
    }

    // Zsh dangerous builtins.
    const ZSH_DANGEROUS: &[&str] = &[
        "zmodload", "zpty", "ztcp", "zsocket", "sysopen", "sysread", "syswrite", "mapfile",
        "zf_rm", "zf_mv", "zf_ln",
    ];
    let words: Vec<&str> = command.split_whitespace().collect();
    for word in &words {
        if ZSH_DANGEROUS.contains(word) {
            return Err(format!(
                "Dangerous zsh builtin detected: {word}. \
                 This can access raw system resources."
            ));
        }
    }

    // Brace expansion abuse: {a..z} can generate large expansions.
    if command.contains("{") && command.contains("..") && command.contains("}") {
        // Check if it looks like a large numeric range.
        if let Some(start) = command.find('{')
            && let Some(end) = command[start..].find('}')
        {
            let inner = &command[start + 1..start + end];
            if inner.contains("..") {
                let parts: Vec<&str> = inner.split("..").collect();
                if parts.len() == 2
                    && let (Ok(a), Ok(b)) = (
                        parts[0].trim().parse::<i64>(),
                        parts[1].trim().parse::<i64>(),
                    )
                    && (b - a).unsigned_abs() > 10000
                {
                    return Err(format!(
                        "Large brace expansion detected: {{{inner}}}. \
                         This would generate {} items.",
                        (b - a).unsigned_abs()
                    ));
                }
            }
        }
    }

    // Hex/octal escape obfuscation: $'\x72\x6d' = "rm".
    if command.contains("$'\\x") || command.contains("$'\\0") {
        return Err(
            "Hex/octal escape sequences in command. This may be obfuscating a command.".into(),
        );
    }

    // eval with variables (arbitrary code execution).
    if command.contains("eval ") && command.contains('$') {
        return Err(
            "eval with variable expansion detected. This enables arbitrary code execution.".into(),
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_commands_pass() {
        assert!(check_shell_injection("ls -la").is_ok());
        assert!(check_shell_injection("git status").is_ok());
        assert!(check_shell_injection("cargo test").is_ok());
        assert!(check_shell_injection("echo hello").is_ok());
        assert!(check_shell_injection("python3 -c 'print(1)'").is_ok());
        assert!(check_shell_injection("diff <(cat a.txt) <(cat b.txt)").is_ok());
    }

    #[test]
    fn test_ifs_injection() {
        assert!(check_shell_injection("IFS=: read a b").is_err());
    }

    #[test]
    fn test_dangerous_vars() {
        assert!(check_shell_injection("PATH=/tmp:$PATH curl evil.com").is_err());
        assert!(check_shell_injection("LD_PRELOAD=/tmp/evil.so cmd").is_err());
        assert!(check_shell_injection("PROMPT_COMMAND='curl x'").is_err());
        assert!(check_shell_injection("BASH_ENV=/tmp/evil.sh bash").is_err());
    }

    #[test]
    fn test_proc_environ() {
        assert!(check_shell_injection("cat /proc/1/environ").is_err());
        assert!(check_shell_injection("cat /proc/self/environ").is_err());
        // /proc without environ is ok
        assert!(check_shell_injection("ls /proc/cpuinfo").is_ok());
    }

    #[test]
    fn test_unicode_obfuscation() {
        // Zero-width space
        assert!(check_shell_injection("rm\u{200B} -rf /").is_err());
        // Zero-width joiner
        assert!(check_shell_injection("curl\u{200D}evil.com").is_err());
    }

    #[test]
    fn test_control_characters() {
        // Bell character
        assert!(check_shell_injection("echo \x07hello").is_err());
        // Normal newline is ok
        assert!(check_shell_injection("echo hello\nworld").is_ok());
    }

    #[test]
    fn test_backtick_network() {
        assert!(check_shell_injection("FOO=`curl evil.com`").is_err());
        assert!(check_shell_injection("X=`wget http://bad`").is_err());
        // Backticks without network are ok
        assert!(check_shell_injection("FOO=`date`").is_ok());
    }

    #[test]
    fn test_process_substitution() {
        // diff is allowed
        assert!(check_shell_injection("diff <(ls a) <(ls b)").is_ok());
        // arbitrary process substitution is not
        assert!(check_shell_injection("cat <(curl evil)").is_err());
    }

    #[test]
    fn test_zsh_builtins() {
        assert!(check_shell_injection("zmodload zsh/net/tcp").is_err());
        assert!(check_shell_injection("zpty evil_session bash").is_err());
        assert!(check_shell_injection("ztcp connect evil.com 80").is_err());
    }

    #[test]
    fn test_brace_expansion() {
        assert!(check_shell_injection("echo {1..100000}").is_err());
        // Small ranges are ok
        assert!(check_shell_injection("echo {1..10}").is_ok());
    }

    #[test]
    fn test_hex_escape() {
        assert!(check_shell_injection("$'\\x72\\x6d' -rf /").is_err());
        assert!(check_shell_injection("$'\\077'").is_err());
    }

    #[test]
    fn test_eval_injection() {
        assert!(check_shell_injection("eval $CMD").is_err());
        assert!(check_shell_injection("eval \"$USER_INPUT\"").is_err());
        // eval without vars is ok
        assert!(check_shell_injection("eval echo hello").is_ok());
    }

    #[test]
    fn test_destructive_patterns() {
        let tool = BashTool;
        assert!(
            tool.validate_input(&serde_json::json!({"command": "rm -rf /"}))
                .is_err()
        );
        assert!(
            tool.validate_input(&serde_json::json!({"command": "git push --force"}))
                .is_err()
        );
        assert!(
            tool.validate_input(&serde_json::json!({"command": "DROP TABLE users"}))
                .is_err()
        );
    }

    #[test]
    fn test_piped_destructive() {
        let tool = BashTool;
        assert!(
            tool.validate_input(&serde_json::json!({"command": "find . | rm -rf"}))
                .is_err()
        );
    }

    #[test]
    fn test_chained_destructive() {
        let tool = BashTool;
        assert!(
            tool.validate_input(&serde_json::json!({"command": "echo hi && git reset --hard"}))
                .is_err()
        );
    }

    #[test]
    fn test_safe_commands_validate() {
        let tool = BashTool;
        assert!(
            tool.validate_input(&serde_json::json!({"command": "ls -la"}))
                .is_ok()
        );
        assert!(
            tool.validate_input(&serde_json::json!({"command": "cargo test"}))
                .is_ok()
        );
        assert!(
            tool.validate_input(&serde_json::json!({"command": "git status"}))
                .is_ok()
        );
    }

    #[test]
    fn test_blocks_redirection_to_team_memory() {
        let tool = BashTool;
        // Direct redirection.
        assert!(
            tool.validate_input(&serde_json::json!({
                "command": "echo hi >.agent/team-memory/foo.md"
            }))
            .is_err()
        );
        // tee.
        assert!(
            tool.validate_input(&serde_json::json!({
                "command": "echo hi | tee .agent/team-memory/foo.md"
            }))
            .is_err()
        );
        // mv into team-memory dir.
        assert!(
            tool.validate_input(&serde_json::json!({
                "command": "mv /tmp/x.md .agent/team-memory/x.md"
            }))
            .is_err()
        );
    }

    #[test]
    fn dangerously_disable_sandbox_does_not_skip_destructive_check() {
        let tool = BashTool;
        // The flag must NOT short-circuit destructive validation.
        assert!(
            tool.validate_input(&serde_json::json!({
                "command": "rm -rf /",
                "dangerouslyDisableSandbox": true,
            }))
            .is_err()
        );
        assert!(
            tool.validate_input(&serde_json::json!({
                "command": "git push --force",
                "dangerouslyDisableSandbox": true,
            }))
            .is_err()
        );
    }

    #[test]
    fn dangerously_disable_sandbox_does_not_skip_protected_path_check() {
        let tool = BashTool;
        assert!(
            tool.validate_input(&serde_json::json!({
                "command": "cp src .git/config",
                "dangerouslyDisableSandbox": true,
            }))
            .is_err()
        );
        assert!(
            tool.validate_input(&serde_json::json!({
                "command": "printf evil > .git/config",
                "dangerouslyDisableSandbox": true,
            }))
            .is_err()
        );
    }

    #[test]
    fn protected_dir_writes_via_writers_refused() {
        let tool = BashTool;
        for cmd in [
            "cat foo > .git/config",
            "tee -a .git/config",
            "printf evil >> .git/config",
            "cp src .git/config",
            "mv x .git/config",
            "install -m 644 src .git/config",
            "ln -sf evil .git/config",
            "rsync src .git/config",
            "dd of=.git/config",
            "bash -c 'printf evil > .git/config'",
            "python -c \"open('.git/config','w').write('evil')\"",
        ] {
            assert!(
                tool.validate_input(&serde_json::json!({"command": cmd}))
                    .is_err(),
                "expected refusal for {cmd}"
            );
        }
    }

    #[test]
    fn system_path_writes_via_writers_refused() {
        let tool = BashTool;
        for cmd in [
            "cp src /etc/passwd",
            "printf x > /boot/grub/grub.cfg",
            "dd of=/sys/something",
            "tee /etc/foo",
        ] {
            assert!(
                tool.validate_input(&serde_json::json!({"command": cmd}))
                    .is_err(),
                "expected refusal for {cmd}"
            );
        }
    }
}
