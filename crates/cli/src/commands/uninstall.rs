//! Interactive uninstall command.
//!
//! Detects the install method, shows what will be removed,
//! asks for confirmation, and performs the uninstall.

use std::io::{self, Write};
use std::path::PathBuf;

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

    if let Some(d) = dirs::config_dir().map(|d| d.join("agent-code"))
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
