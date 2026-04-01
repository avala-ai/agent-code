//! Interactive permission prompts.
//!
//! When a tool requires permission in "ask" mode, display the
//! request and wait for user approval.

use crossterm::style::Stylize;

/// Ask the user whether to allow a tool operation.
///
/// Returns true if allowed, false if denied.
pub fn ask_permission(tool_name: &str, description: &str) -> bool {
    eprintln!();
    eprintln!(
        "{} {} wants to execute:",
        " PERMISSION ".on_yellow().black().bold(),
        tool_name.bold(),
    );
    eprintln!("  {}", description.dark_grey());

    let choice = super::selector::select(&[
        super::selector::SelectOption {
            label: "Allow".into(),
            description: "".into(),
            value: "y".into(),
        },
        super::selector::SelectOption {
            label: "Deny".into(),
            description: "".into(),
            value: "n".into(),
        },
    ]);

    matches!(choice.as_str(), "y")
}

/// Display a diff with colored lines.
pub fn print_colored_diff(diff: &str) {
    for line in diff.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            println!("{}", line.green());
        } else if line.starts_with('-') && !line.starts_with("---") {
            println!("{}", line.red());
        } else if line.starts_with("@@") {
            println!("{}", line.cyan());
        } else if line.starts_with("diff ") {
            println!("{}", line.bold());
        } else {
            println!("{line}");
        }
    }
}

/// Display a file edit summary with before/after context.
pub fn print_edit_summary(file_path: &str, old: &str, new: &str) {
    println!("{}", format!("  {file_path}:").bold());
    for line in old.lines().take(3) {
        println!("  {}", format!("- {line}").red());
    }
    for line in new.lines().take(3) {
        println!("  {}", format!("+ {line}").green());
    }
    if old.lines().count() > 3 || new.lines().count() > 3 {
        println!("  {}", "...".dark_grey());
    }
}
