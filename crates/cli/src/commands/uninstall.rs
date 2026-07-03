//! Interactive uninstall command.
//!
//! Detects the install method, shows what will be removed,
//! asks for confirmation, and performs the uninstall.

use std::io::{self, Write};
use std::path::PathBuf;

/// Markers delimiting the shell block written by the installer's name guard.
/// Kept in sync with `install.sh` so uninstall can remove it cleanly.
const GUARD_BEGIN: &str = "# >>> agent-code name guard >>>";
const GUARD_END: &str = "# <<< agent-code name guard <<<";

/// How agent-code was installed.
enum InstallMethod {
    Cargo,
    Homebrew,
    Npm,
    Manual(PathBuf),
}

impl InstallMethod {
    fn label(&self) -> &str {
        match self {
            InstallMethod::Cargo => "cargo",
            InstallMethod::Homebrew => "homebrew",
            InstallMethod::Npm => "npm",
            InstallMethod::Manual(_) => "binary",
        }
    }
}

/// Detect how agent-code was installed by examining the binary path.
fn detect_install_method() -> InstallMethod {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("agent"));
    let path_str = exe.to_string_lossy();

    if path_str.contains(".cargo") {
        InstallMethod::Cargo
    } else if path_str.contains("homebrew")
        || path_str.contains("Cellar")
        || path_str.contains("linuxbrew")
    {
        InstallMethod::Homebrew
    } else if path_str.contains("node_modules")
        || path_str.contains("npm")
        || path_str.contains("npx")
    {
        InstallMethod::Npm
    } else {
        InstallMethod::Manual(exe)
    }
}

/// Collect agent-code data directories that exist on disk.
fn data_directories() -> Vec<(&'static str, PathBuf)> {
    let mut dirs_found = Vec::new();

    if let Some(d) = agent_code_lib::config::agent_config_dir()
        && d.exists()
    {
        dirs_found.push(("Config", d));
    }
    if let Some(d) = dirs::cache_dir().map(|d| d.join("agent-code"))
        && d.exists()
    {
        dirs_found.push(("Cache", d));
    }
    if let Some(d) = dirs::data_local_dir().map(|d| d.join("agent-code"))
        && d.exists()
    {
        dirs_found.push(("Data", d));
    }

    dirs_found
}

/// Shell rc files that may contain the installer's name guard block.
fn shell_rc_candidates() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let mut candidates: Vec<PathBuf> = [
        ".bashrc",
        ".bash_profile",
        ".zshrc",
        ".profile",
        ".config/fish/config.fish",
    ]
    .iter()
    .map(|rc| home.join(rc))
    .collect();

    // The installer writes the zsh guard to `${ZDOTDIR:-$HOME}/.zshrc`; mirror
    // that here so a custom ZDOTDIR is not left with an orphaned guard.
    if let Some(zdotdir) = std::env::var_os("ZDOTDIR") {
        let zshrc = PathBuf::from(zdotdir).join(".zshrc");
        if !candidates.contains(&zshrc) {
            candidates.push(zshrc);
        }
    }

    candidates.retain(|p| p.exists());
    candidates
}

/// Return the contents of `text` with the guard block removed, or `None` if
/// there is nothing to remove.
///
/// Only a balanced begin/end pair is stripped. A begin marker with no matching
/// end (a hand-edited rc) is left untouched rather than dropping every line
/// after it, so user content is never lost.
fn strip_guard_block(text: &str) -> Option<String> {
    if !text.contains(GUARD_BEGIN) {
        return None;
    }
    let mut out: Vec<&str> = Vec::new();
    let mut block: Vec<&str> = Vec::new();
    let mut in_block = false;
    let mut removed_any = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if !in_block && trimmed == GUARD_BEGIN {
            in_block = true;
            block.clear();
            block.push(line);
            continue;
        }
        if in_block {
            block.push(line);
            if trimmed == GUARD_END {
                // Balanced pair — discard the buffered block.
                in_block = false;
                block.clear();
                removed_any = true;
            }
            continue;
        }
        out.push(line);
    }
    // Unbalanced begin with no end: flush the buffer back verbatim.
    if in_block {
        out.extend(block.iter().copied());
    }
    if !removed_any {
        // Nothing balanced was removed; leave the file exactly as it was.
        return None;
    }
    // Collapse any trailing blank lines left behind, keeping one final newline.
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    let mut joined = out.join("\n");
    if !joined.is_empty() {
        joined.push('\n');
    }
    Some(joined)
}

/// Remove the installer's name guard from any shell rc files that contain it.
fn remove_shell_guards() {
    for rc in shell_rc_candidates() {
        let Ok(contents) = std::fs::read_to_string(&rc) else {
            continue;
        };
        if let Some(cleaned) = strip_guard_block(&contents) {
            match std::fs::write(&rc, cleaned) {
                Ok(()) => println!("  Removed shell name guard: {}", rc.display()),
                Err(e) => eprintln!("  Failed to update {}: {e}", rc.display()),
            }
        }
    }
}

/// Prompt the user for yes/no confirmation. Returns true if confirmed.
fn confirm(prompt: &str) -> bool {
    print!("{prompt} [y/N] ");
    io::stdout().flush().ok();

    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Remove a directory tree, printing what happened.
fn remove_dir(label: &str, path: &PathBuf) {
    match std::fs::remove_dir_all(path) {
        Ok(()) => println!("  Removed {label}: {}", path.display()),
        Err(e) => eprintln!("  Failed to remove {}: {e}", path.display()),
    }
}

/// Entry point for `/uninstall [--force]`.
pub fn run(args: Option<&str>) {
    let force = args.map(|a| a.trim()) == Some("--force");
    let method = detect_install_method();
    let data_dirs = data_directories();

    // Show what will happen.
    println!();
    println!("  Uninstall agent-code");
    println!();

    match &method {
        InstallMethod::Cargo => {
            println!("  Install method: cargo");
            println!("  Action:         cargo uninstall agent-code");
        }
        InstallMethod::Homebrew => {
            println!("  Install method: homebrew");
            println!("  Action:         brew uninstall agent-code");
        }
        InstallMethod::Npm => {
            println!("  Install method: npm");
            println!("  Action:         npm uninstall -g @avala-ai/agent-code");
        }
        InstallMethod::Manual(path) => {
            println!("  Install method: manual binary");
            println!("  Binary:         {}", path.display());
        }
    }

    if data_dirs.is_empty() {
        println!("  Data dirs:      (none found)");
    } else {
        println!();
        println!("  Data directories to remove:");
        for (label, path) in &data_dirs {
            println!("    {label}: {}", path.display());
        }
    }
    println!();

    // Confirm.
    if !force && !confirm("Proceed with uninstall?") {
        println!("  Cancelled.");
        return;
    }

    // 1. Remove data directories.
    for (label, path) in &data_dirs {
        remove_dir(label, path);
    }

    // 2. Remove the binary / package.
    let binary_ok = match &method {
        InstallMethod::Cargo => run_package_manager("cargo", &["uninstall", "agent-code"]),
        InstallMethod::Homebrew => run_package_manager("brew", &["uninstall", "agent-code"]),
        InstallMethod::Npm => {
            run_package_manager("npm", &["uninstall", "-g", "@avala-ai/agent-code"])
        }
        InstallMethod::Manual(path) => match std::fs::remove_file(path) {
            Ok(()) => {
                println!("  Removed binary: {}", path.display());
                true
            }
            Err(e) => {
                eprintln!("  Failed to remove binary: {e}");
                eprintln!("  Try manually: sudo rm {}", path.display());
                false
            }
        },
    };

    // 3. Remove the installer's shell name guard, if present.
    remove_shell_guards();

    println!();
    if binary_ok {
        println!("  agent-code has been uninstalled ({}).", method.label());
    } else {
        eprintln!("  Uninstall completed with errors. See messages above.");
    }
}

/// Run a package manager command, printing its output. Returns true on success.
fn run_package_manager(program: &str, args: &[&str]) -> bool {
    println!("  Running: {} {}", program, args.join(" "));

    match std::process::Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
    {
        Ok(status) if status.success() => true,
        Ok(status) => {
            eprintln!("  {} exited with status {status}", program);
            false
        }
        Err(e) => {
            eprintln!("  Failed to run {program}: {e}");
            if e.kind() == io::ErrorKind::NotFound {
                eprintln!("  {program} not found. Remove the binary manually.");
            }
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_guard_returns_none_when_absent() {
        assert_eq!(strip_guard_block("export PATH=/x\n"), None);
    }

    #[test]
    fn strip_guard_removes_block_and_trailing_blanks() {
        let input =
            format!("line1\nline2\n\n{GUARD_BEGIN}\nalias agent=\"/x/agent\"\n{GUARD_END}\n");
        assert_eq!(strip_guard_block(&input).as_deref(), Some("line1\nline2\n"));
    }

    #[test]
    fn strip_guard_preserves_content_after_block() {
        let input = format!("a\n{GUARD_BEGIN}\nx\n{GUARD_END}\nb\n");
        assert_eq!(strip_guard_block(&input).as_deref(), Some("a\nb\n"));
    }

    #[test]
    fn strip_guard_leaves_unbalanced_block_untouched() {
        // Begin marker but no end (hand-edited rc): nothing must be dropped.
        let input = format!("keep1\n{GUARD_BEGIN}\nalias agent=\"/x\"\nkeep2\n");
        assert_eq!(strip_guard_block(&input), None);
    }
}
