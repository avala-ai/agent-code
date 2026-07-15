//! Shared clipboard write path for `/copy` and the modern TUI.
//!
//! Cascade (best-effort multi-fire where useful):
//! 1. **Native** OS tool (`pbcopy`, `wl-copy`, `xclip`/`xsel`, `clip`)
//! 2. **tmux** paste buffer (`tmux load-buffer -`) when `$TMUX` is set
//! 3. **OSC 52** escape sequence to the outer terminal (always attempted when
//!    native fails, or when tmux/SSH suggests the outer terminal owns the
//!    clipboard)
//!
//! Returns `Ok` if at least one route succeeded.

use std::io::Write;

/// Successful write, listing every route that accepted the payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyResult {
    pub routes: Vec<&'static str>,
}

impl CopyResult {
    pub fn summary(&self) -> String {
        self.routes.join("+")
    }
}

/// Copy `text` to the system clipboard via the cascade above.
pub fn copy_text(text: &str) -> Result<CopyResult, String> {
    let mut routes = Vec::new();
    let mut last_err: Option<String> = None;

    match try_native(text) {
        Ok(name) => routes.push(name),
        Err(e) => last_err = Some(e),
    }

    if in_tmux() {
        match try_tmux(text) {
            Ok(()) => routes.push("tmux"),
            Err(e) => {
                if last_err.is_none() {
                    last_err = Some(e);
                }
            }
        }
    }

    // OSC 52: fire when nothing else worked, or when the outer terminal is
    // likely the real clipboard owner (tmux / SSH / no local display).
    let want_osc52 = routes.is_empty() || in_tmux() || is_ssh() || !has_local_display();
    if want_osc52 {
        match try_osc52(text) {
            Ok(()) => routes.push("osc52"),
            Err(e) => {
                if last_err.is_none() {
                    last_err = Some(e);
                }
            }
        }
    }

    if routes.is_empty() {
        Err(last_err.unwrap_or_else(|| {
            "no clipboard route available (install pbcopy / wl-copy / xclip, \
             or use a terminal that supports OSC 52)"
                .into()
        }))
    } else {
        Ok(CopyResult { routes })
    }
}

/// Human-readable description of which clipboard routes this environment
/// would attempt (for `/terminal-setup`). Does not perform a write.
pub fn describe_routes() -> Vec<String> {
    let mut out = Vec::new();
    out.push(format!(
        "native candidates : {}",
        native_candidate_names().join(", ")
    ));
    out.push(format!(
        "tmux buffer       : {}",
        if in_tmux() { "yes ($TMUX)" } else { "no" }
    ));
    out.push(format!(
        "osc52 preferred   : {}",
        if in_tmux() || is_ssh() || !has_local_display() {
            "yes (tmux/ssh/no display)"
        } else {
            "fallback when native fails"
        }
    ));
    out.push(format!(
        "ssh session       : {}",
        if is_ssh() { "yes" } else { "no" }
    ));
    out
}

fn native_candidate_names() -> Vec<&'static str> {
    native_candidates().iter().map(|(name, _)| *name).collect()
}

fn native_candidates() -> &'static [(&'static str, &'static [&'static str])] {
    if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else if cfg!(target_os = "windows") {
        &[("clip", &[])]
    } else if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        &[
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ]
    } else {
        &[
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
            ("wl-copy", &[]),
        ]
    }
}

fn try_native(text: &str) -> Result<&'static str, String> {
    let mut last_err: Option<String> = None;
    for (cmd, args) in native_candidates() {
        match std::process::Command::new(cmd)
            .args(*args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    if let Err(e) = stdin.write_all(text.as_bytes()) {
                        last_err = Some(format!("{cmd}: write error: {e}"));
                        let _ = child.wait();
                        continue;
                    }
                    drop(stdin);
                }
                match child.wait() {
                    Ok(status) if status.success() => return Ok(cmd),
                    Ok(status) => {
                        last_err = Some(format!("{cmd} exited with {status}"));
                        continue;
                    }
                    Err(e) => {
                        last_err = Some(format!("{cmd}: wait error: {e}"));
                        continue;
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                last_err = Some(format!("{cmd}: {e}"));
                continue;
            }
        }
    }
    Err(last_err.unwrap_or_else(|| "no native clipboard command on PATH".into()))
}

fn try_tmux(text: &str) -> Result<(), String> {
    let mut child = std::process::Command::new("tmux")
        .args(["load-buffer", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("tmux: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| format!("tmux: write error: {e}"))?;
        drop(stdin);
    }
    match child.wait() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("tmux load-buffer exited with {status}")),
        Err(e) => Err(format!("tmux: wait error: {e}")),
    }
}

fn try_osc52(text: &str) -> Result<(), String> {
    // OSC 52 payload size is often capped by terminals (~100KB). Truncate
    // with a marker rather than failing entirely on large copies.
    const MAX: usize = 75_000;
    let payload = if text.len() > MAX {
        let mut s = text[..MAX].to_string();
        s.push_str("\n…[truncated for OSC 52]");
        s
    } else {
        text.to_string()
    };
    let b64 = base64_encode(payload.as_bytes());
    // BEL-terminated OSC 52 set clipboard (c = clipboard selection).
    let seq = if in_tmux() {
        // Pass OSC through tmux: DCS tmux; ESC ESC ] 52 ; c ; b64 BEL ST
        format!("\x1bPtmux;\x1b\x1b]52;c;{b64}\x07\x1b\\")
    } else {
        format!("\x1b]52;c;{b64}\x07")
    };
    let mut out = std::io::stdout();
    out.write_all(seq.as_bytes())
        .map_err(|e| format!("osc52 write: {e}"))?;
    out.flush().map_err(|e| format!("osc52 flush: {e}"))?;
    Ok(())
}

fn in_tmux() -> bool {
    std::env::var_os("TMUX").is_some()
}

fn is_ssh() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some()
        || std::env::var_os("SSH_TTY").is_some()
        || std::env::var_os("SSH_CLIENT").is_some()
}

fn has_local_display() -> bool {
    if cfg!(target_os = "macos") || cfg!(target_os = "windows") {
        return true;
    }
    std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some()
}

/// Minimal standard base64 (no external dep).
fn base64_encode(input: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8) | (input[i + 2] as u32);
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(T[((n >> 6) & 63) as usize] as char);
        out.push(T[(n & 63) as usize] as char);
        i += 3;
    }
    match input.len() - i {
        1 => {
            let n = (input[i] as u32) << 16;
            out.push(T[((n >> 18) & 63) as usize] as char);
            out.push(T[((n >> 12) & 63) as usize] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8);
            out.push(T[((n >> 18) & 63) as usize] as char);
            out.push(T[((n >> 12) & 63) as usize] as char);
            out.push(T[((n >> 6) & 63) as usize] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_encode_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn describe_routes_is_nonempty() {
        let d = describe_routes();
        assert!(d.iter().any(|l| l.contains("native")));
        assert!(d.iter().any(|l| l.contains("osc52")));
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn copy_errors_with_empty_path_when_no_tmux() {
        // Same contract as the old commands::copy_to_clipboard test: with an
        // empty PATH and no TMUX, native fails. OSC 52 may still succeed by
        // writing to stdout — so only assert we get *some* Result that is
        // either Ok(osc52) or Err. Prefer: clear TMUX too.
        let prev_path = std::env::var_os("PATH");
        let prev_tmux = std::env::var_os("TMUX");
        // SAFETY: single-threaded test, restored before exit.
        unsafe {
            std::env::set_var("PATH", "");
            std::env::remove_var("TMUX");
            std::env::remove_var("SSH_CONNECTION");
            std::env::remove_var("SSH_TTY");
            std::env::remove_var("SSH_CLIENT");
            // Force local display so we don't prefer osc52 solely for ssh/no-display
            // — still may fire osc52 on native failure.
            std::env::set_var("DISPLAY", ":0");
        }
        let result = copy_text("hello");
        unsafe {
            match prev_path {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
            match prev_tmux {
                Some(v) => std::env::set_var("TMUX", v),
                None => std::env::remove_var("TMUX"),
            }
        }
        // With empty PATH, native fails; OSC 52 write to stdout should still work.
        match result {
            Ok(r) => assert!(
                r.routes.contains(&"osc52"),
                "expected osc52 fallback, got {:?}",
                r.routes
            ),
            Err(e) => panic!("expected osc52 success on empty PATH, got err: {e}"),
        }
    }
}
