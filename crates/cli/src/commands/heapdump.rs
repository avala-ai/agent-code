//! Process memory snapshot for debugging memory usage.
//!
//! Rust does not expose a first-class heap profiler without
//! rebuilding with a profiling allocator, so this is a best-effort
//! OS-level snapshot: RSS, VSZ, peak values, and per-segment sizes
//! where available. Output is written to a timestamped file under
//! the user's data directory so it can be attached to bug reports.

use std::path::PathBuf;

/// A single metric sampled from the OS.
struct Sample {
    key: &'static str,
    value: String,
}

/// Collect process memory samples for the current platform.
fn collect_samples() -> (Vec<Sample>, Option<&'static str>) {
    #[cfg(target_os = "linux")]
    {
        match std::fs::read_to_string("/proc/self/status") {
            Ok(content) => (parse_proc_status(&content), None),
            Err(e) => (
                Vec::new(),
                Some(match e.kind() {
                    std::io::ErrorKind::NotFound => "unable to read /proc/self/status",
                    std::io::ErrorKind::PermissionDenied => {
                        "permission denied reading /proc/self/status"
                    }
                    _ => "failed to read /proc/self/status",
                }),
            ),
        }
    }

    #[cfg(target_os = "macos")]
    {
        (macos_samples(), None)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        (Vec::new(), Some("heapdump is not supported on this platform"))
    }
}

#[cfg(target_os = "linux")]
fn parse_proc_status(content: &str) -> Vec<Sample> {
    // The keys we care about. Everything else is ignored.
    const KEYS: &[&str] = &[
        "VmPeak", "VmSize", "VmLck", "VmHWM", "VmRSS", "VmData", "VmStk", "VmExe", "VmLib",
        "VmPTE", "VmSwap", "Threads",
    ];

    let mut out = Vec::new();
    for line in content.lines() {
        let (k, v) = match line.split_once(':') {
            Some(parts) => parts,
            None => continue,
        };
        let k = k.trim();
        if let Some(key) = KEYS.iter().find(|x| **x == k) {
            out.push(Sample {
                key,
                value: v.trim().to_string(),
            });
        }
    }
    out
}

#[cfg(target_os = "macos")]
fn macos_samples() -> Vec<Sample> {
    // Shell out to `ps` to avoid adding a dependency for one command.
    // Format: `ps -o rss=,vsz= -p <pid>` -> "12345 67890" (KiB).
    let pid = std::process::id().to_string();
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=,vsz=", "-p", &pid])
        .output();

    let mut out = Vec::new();
    if let Ok(output) = output
        && output.status.success()
    {
        let text = String::from_utf8_lossy(&output.stdout);
        let mut fields = text.split_whitespace();
        if let Some(rss) = fields.next() {
            out.push(Sample {
                key: "VmRSS",
                value: format!("{rss} kB"),
            });
        }
        if let Some(vsz) = fields.next() {
            out.push(Sample {
                key: "VmSize",
                value: format!("{vsz} kB"),
            });
        }
    }
    out
}

/// Resolve the output path for heapdump files.
fn dump_dir() -> Option<PathBuf> {
    let base = dirs::data_local_dir()?.join("agent-code").join("heapdumps");
    std::fs::create_dir_all(&base).ok()?;
    Some(base)
}

/// Write the samples to a timestamped file. Returns the path.
fn write_dump(samples: &[Sample]) -> Result<PathBuf, String> {
    let dir = dump_dir().ok_or("could not resolve data directory")?;
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let path = dir.join(format!("{stamp}.txt"));

    let mut body = format!(
        "# agent-code heap snapshot\n\
         Generated: {}\n\
         PID: {}\n\
         Version: {}\n\
         Platform: {}\n\n",
        chrono::Utc::now().to_rfc3339(),
        std::process::id(),
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
    );

    if samples.is_empty() {
        body.push_str("(no samples collected — see stderr for details)\n");
    } else {
        for s in samples {
            body.push_str(&format!("{}: {}\n", s.key, s.value));
        }
    }

    std::fs::write(&path, body).map_err(|e| format!("failed to write dump: {e}"))?;
    Ok(path)
}

/// Entry point for `/heapdump`.
pub fn run() {
    let (samples, err) = collect_samples();

    if let Some(msg) = err {
        eprintln!("  {msg}");
    }

    if samples.is_empty() && err.is_some() {
        // Nothing useful to write.
        return;
    }

    // Print a short summary to the terminal for immediate feedback.
    println!();
    if !samples.is_empty() {
        let summary_keys = ["VmRSS", "VmSize", "VmPeak", "Threads"];
        for key in summary_keys {
            if let Some(s) = samples.iter().find(|s| s.key == key) {
                println!("  {}: {}", s.key, s.value);
            }
        }
    }

    match write_dump(&samples) {
        Ok(path) => println!("\n  Snapshot written to {}", path.display()),
        Err(e) => eprintln!("  {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn parses_proc_status_fields() {
        let input = "Name:\tagent\n\
                     VmPeak:\t  12345 kB\n\
                     VmSize:\t  12340 kB\n\
                     VmRSS:\t   9876 kB\n\
                     Threads:\t4\n\
                     Ignored:\tvalue\n";
        let samples = parse_proc_status(input);
        let keys: Vec<&str> = samples.iter().map(|s| s.key).collect();
        assert!(keys.contains(&"VmPeak"));
        assert!(keys.contains(&"VmSize"));
        assert!(keys.contains(&"VmRSS"));
        assert!(keys.contains(&"Threads"));
        assert!(!keys.contains(&"Ignored"));

        let rss = samples.iter().find(|s| s.key == "VmRSS").unwrap();
        assert_eq!(rss.value, "9876 kB");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parses_proc_status_handles_empty() {
        let samples = parse_proc_status("");
        assert!(samples.is_empty());
    }
}
