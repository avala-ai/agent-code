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

    let choice = super::selector::select(&[
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

    match choice.as_str() {
        "allow_once" => PermissionResponse::AllowOnce,
        "allow_session" => PermissionResponse::AllowSession,
        _ => PermissionResponse::Deny,
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
