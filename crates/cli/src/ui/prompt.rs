//! Interactive permission prompts.
//!
//! When a tool requires permission in "ask" mode, display a rich
//! TUI modal with tool details and multiple response options.

use crossterm::style::Stylize;

/// Result of a permission prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionResponse {
    /// Allow this specific invocation.
    AllowOnce,
    /// Allow this tool for the rest of the session.
    AllowSession,
    /// Deny this invocation.
    Deny,
}

/// Ask the user whether to allow a tool operation.
///
/// Shows a rich prompt with tool name, description, input summary,
/// and multiple response options. Returns true if allowed.
pub fn ask_permission(tool_name: &str, description: &str) -> bool {
    let response = ask_permission_detailed(tool_name, description, None);
    matches!(
        response,
        PermissionResponse::AllowOnce | PermissionResponse::AllowSession
    )
}

/// Detailed permission prompt with full context.
pub fn ask_permission_detailed(
    tool_name: &str,
    description: &str,
    input_preview: Option<&str>,
) -> PermissionResponse {
    let t = super::theme::current();

    eprintln!();
    eprintln!(
        "{} {} wants to execute:",
        super::theme::label(" PERMISSION ", t.warning, crossterm::style::Color::Black),
        tool_name.with(t.text).bold(),
    );

    // Show the action description.
    eprintln!("  {}", description.with(t.muted));

    // Show input preview if available.
    if let Some(preview) = input_preview {
        eprintln!();
        let lines: Vec<&str> = preview.lines().take(5).collect();
        for line in &lines {
            eprintln!("  {}", line.with(t.inactive));
        }
        if preview.lines().count() > 5 {
            eprintln!("  {}", "...".with(t.muted));
        }
    }

    eprintln!();

    // Cancellable: dismissing the modal with Esc/q must Deny, not fall through
    // to the highlighted default (which is Allow) and execute the tool.
    let choice = super::selector::select_cancellable(&[
        super::selector::SelectOption {
            label: "Allow".into(),
            description: "allow this action".into(),
            value: "allow_once".into(),
            preview: None,
        },
        super::selector::SelectOption {
            label: "Allow for session".into(),
            description: format!("always allow {tool_name} this session"),
            value: "allow_session".into(),
            preview: None,
        },
        super::selector::SelectOption {
            label: "Deny".into(),
            description: "block this action".into(),
            value: "deny".into(),
            preview: None,
        },
    ]);

    match choice.as_deref() {
        Some("allow_once") => PermissionResponse::AllowOnce,
        Some("allow_session") => PermissionResponse::AllowSession,
        // None (Esc/q cancel) or "deny" → deny.
        _ => PermissionResponse::Deny,
    }
}

/// Adapter that lets the lib engine drive this interactive permission prompt.
///
/// Installed on the classic interactive path (see `run_repl`); one-shot/`-p` runs
/// leave the engine's prompter unset (auto-allow, unchanged). `ask()` is called
/// from inside the running turn (the tool executor) while the REPL's
/// escape-watcher thread holds the terminal in raw mode reading keypresses for
/// steering. To keep the two from reading stdin at once, `ask()` raises
/// `input_gate` so the watcher backs off, runs the blocking selector, restores
/// raw mode (the selector turns it off on exit) and lowers the gate.
pub struct TuiPrompter {
    pub input_gate: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl agent_code_lib::tools::PermissionPrompter for TuiPrompter {
    fn ask(
        &self,
        tool_name: &str,
        description: &str,
        input_preview: Option<&str>,
        origin: Option<&str>,
    ) -> agent_code_lib::tools::PermissionResponse {
        use agent_code_lib::tools::PermissionResponse as Lib;
        use std::sync::atomic::Ordering;

        // Signal the escape watcher to release stdin, then wait past its 100ms
        // poll window so any in-flight read returns before we own the terminal.
        self.input_gate.store(true, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(120));

        let description = match origin {
            Some(o) if !o.is_empty() => format!("{description} (from {o})"),
            _ => description.to_string(),
        };

        // The selector toggles raw mode on then off. Restore it only if it was
        // already on — i.e. a watcher owns the terminal (the normal steered
        // turn). For a command-generated turn (e.g. a slash command that runs a
        // turn without spawning the watcher) raw mode was off and must stay off,
        // or the next rustyline prompt would be stuck in raw mode.
        let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
        let response = ask_permission_detailed(tool_name, &description, input_preview);
        if was_raw {
            let _ = crossterm::terminal::enable_raw_mode();
        }
        self.input_gate.store(false, Ordering::SeqCst);

        match response {
            PermissionResponse::AllowOnce => Lib::AllowOnce,
            PermissionResponse::AllowSession => Lib::AllowSession,
            PermissionResponse::Deny => Lib::Deny,
        }
    }
}

/// Display a diff with theme-colored lines.
pub fn print_colored_diff(diff: &str) {
    let t = super::theme::current();
    for line in diff.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            println!("{}", line.with(t.diff_add));
        } else if line.starts_with('-') && !line.starts_with("---") {
            println!("{}", line.with(t.diff_remove));
        } else if line.starts_with("@@") {
            println!("{}", line.with(t.tool));
        } else if line.starts_with("diff ") {
            println!("{}", line.bold());
        } else {
            println!("{line}");
        }
    }
}

/// Display a file edit summary with before/after context.
pub fn print_edit_summary(file_path: &str, old: &str, new: &str) {
    let t = super::theme::current();
    println!("{}", format!("  {file_path}:").bold());
    for line in old.lines().take(3) {
        println!("  {}", format!("- {line}").with(t.diff_remove));
    }
    for line in new.lines().take(3) {
        println!("  {}", format!("+ {line}").with(t.diff_add));
    }
    if old.lines().count() > 3 || new.lines().count() > 3 {
        println!("  {}", "...".with(t.muted));
    }
}
