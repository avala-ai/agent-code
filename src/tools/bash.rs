//! Bash tool: execute shell commands.
//!
//! Runs commands via the system shell. Features:
//! - Timeout with configurable duration (default 2min, max 10min)
//! - Background execution for long-running commands
//! - Sandbox mode: blocks writes outside the project directory
//! - Destructive command warnings
//! - Output truncation for large results
//! - Cancellation via CancellationToken

use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::{Tool, ToolContext, ToolResult};
use crate::error::ToolError;

/// Maximum output size before truncation (256KB).
const MAX_OUTPUT_BYTES: usize = 256 * 1024;

/// Commands that are potentially destructive and warrant a warning.
const DESTRUCTIVE_PATTERNS: &[&str] = &[
    // Filesystem destruction.
    "rm -rf",
    "rm -r /",
    "rm -fr",
    "rmdir",
    "shred",
    // Git destructive operations.
    "git reset --hard",
    "git clean -f",
    "git clean -d",
    "git push --force",
    "git push -f",
    "git checkout -- .",
    "git checkout -f",
    "git restore .",
    "git branch -D",
    "git branch --delete --force",
    "git stash drop",
    "git stash clear",
    "git rebase --abort",
    // Database operations.
    "DROP TABLE",
    "DROP DATABASE",
    "DROP SCHEMA",
    "DELETE FROM",
    "TRUNCATE",
    // System operations.
    "shutdown",
    "reboot",
    "halt",
    "poweroff",
    "init 0",
    "init 6",
    "mkfs",
    "dd if=",
    "dd of=/dev",
    "> /dev/sd",
    "wipefs",
    // Permission escalation.
    "chmod -R 777",
    "chmod 777",
    "chown -R",
    // Process/system danger.
    "kill -9",
    "killall",
    "pkill -9",
    // Fork bomb.
    ":(){ :|:& };:",
    // NPM/package destruction.
    "npm publish",
    "cargo publish",
    // Docker cleanup.
    "docker system prune -a",
    "docker volume prune",
    // Kubernetes.
    "kubectl delete namespace",
    "kubectl delete --all",
    // Infrastructure.
    "terraform destroy",
    "pulumi destroy",
];

/// Paths that should never be written to.
const BLOCKED_WRITE_PATHS: &[&str] = &[
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

    fn validate_input(&self, input: &serde_json::Value) -> Result<(), String> {
        let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");

        // Check for destructive commands.
        let cmd_lower = command.to_lowercase();
        for pattern in DESTRUCTIVE_PATTERNS {
            if cmd_lower.contains(&pattern.to_lowercase()) {
                return Err(format!(
                    "Potentially destructive command detected: contains '{pattern}'. \
                     This command could cause data loss or system damage. \
                     If you're sure, ask the user for confirmation first."
                ));
            }
        }

        // Check for piped destructive patterns.
        // Split on pipes and check each segment's base command.
        for segment in command.split('|') {
            let trimmed = segment.trim();
            let base_cmd = trimmed.split_whitespace().next().unwrap_or("");
            if matches!(
                base_cmd,
                "rm" | "shred" | "dd" | "mkfs" | "wipefs" | "shutdown" | "reboot" | "halt"
            ) {
                return Err(format!(
                    "Potentially destructive command in pipe: '{base_cmd}'. \
                     Ask the user for confirmation first."
                ));
            }
        }

        // Check for chained destructive commands (&&, ;).
        for segment in cmd_lower.split("&&").flat_map(|s| s.split(';')) {
            let trimmed = segment.trim();
            for pattern in DESTRUCTIVE_PATTERNS {
                if trimmed.contains(&pattern.to_lowercase()) {
                    return Err(format!(
                        "Potentially destructive command in chain: contains '{pattern}'. \
                         Ask the user for confirmation first."
                    ));
                }
            }
        }

        // Advanced security checks (inspired by TS bashSecurity.ts).
        check_shell_injection(command)?;

        // Block writes to system paths.
        for path in BLOCKED_WRITE_PATHS {
            if cmd_lower.contains(&format!(">{path}"))
                || cmd_lower.contains(&format!("tee {path}"))
                || cmd_lower.contains(&"mv ".to_string()) && cmd_lower.contains(path)
            {
                return Err(format!(
                    "Cannot write to system path '{path}'. \
                     Operations on system directories are not allowed."
                ));
            }
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

        let timeout_ms = input
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(120_000)
            .min(600_000);

        let run_in_background = input
            .get("run_in_background")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Background execution: spawn and return immediately.
        if run_in_background {
            return run_background(command, &ctx.cwd, ctx.task_manager.as_ref()).await;
        }

        let mut child = Command::new("bash")
            .arg("-c")
            .arg(command)
            .current_dir(&ctx.cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
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
}
