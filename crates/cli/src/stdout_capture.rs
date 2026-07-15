//! Capture stdout while running slash commands under the modern TUI.
//!
//! Built-in commands use `println!` for line-oriented output. In alt-screen mode
//! those writes would corrupt the UI or vanish after restore. On Unix we
//! temporarily redirect fd 1 to a pipe so the modern transcript can show the
//! captured text. On non-Unix platforms we run without capture (best-effort).

/// Run `f`, capturing anything written to stdout. Returns `(result, captured)`.
pub fn capture_stdout<F, R>(f: F) -> (R, String)
where
    F: FnOnce() -> R,
{
    #[cfg(unix)]
    {
        capture_stdout_unix(f)
    }
    #[cfg(not(unix))]
    {
        (f(), String::new())
    }
}

#[cfg(unix)]
fn capture_stdout_unix<F, R>(f: F) -> (R, String)
where
    F: FnOnce() -> R,
{
    use std::fs::File;
    use std::io::Read;
    use std::os::fd::{FromRawFd, RawFd};

    let mut pair = [0i32; 2];
    // SAFETY: standard pipe syscall.
    if unsafe { libc::pipe(pair.as_mut_ptr()) } != 0 {
        return (f(), String::new());
    }
    let read_fd: RawFd = pair[0];
    let write_fd: RawFd = pair[1];

    // SAFETY: duplicate current stdout.
    let saved = unsafe { libc::dup(1) };
    if saved < 0 {
        unsafe {
            libc::close(read_fd);
            libc::close(write_fd);
        }
        return (f(), String::new());
    }

    // SAFETY: redirect stdout to the pipe write end.
    if unsafe { libc::dup2(write_fd, 1) } < 0 {
        unsafe {
            libc::close(saved);
            libc::close(read_fd);
            libc::close(write_fd);
        }
        return (f(), String::new());
    }
    // Close our copy of the write end so the reader sees EOF after `f` and
    // the dup2'd stdout is closed/restored.
    unsafe {
        libc::close(write_fd);
    }

    let result = f();

    // Flush Rust + C stdio onto the pipe before we close the write end.
    let _ = std::io::Write::flush(&mut std::io::stdout());
    unsafe {
        libc::fflush(std::ptr::null_mut());
    }

    // SAFETY: restore original stdout (closes the only write end of the pipe).
    unsafe {
        let _ = libc::dup2(saved, 1);
        libc::close(saved);
    }

    // SAFETY: take ownership of the read end; File::drop closes it.
    let mut reader = unsafe { File::from_raw_fd(read_fd) };
    let mut buf = Vec::new();
    let _ = reader.read_to_end(&mut buf);
    drop(reader);

    let text = String::from_utf8_lossy(&buf).into_owned();
    (result, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn captures_fd1_write() {
        // Use libc::write to fd 1 — cargo's test harness may wrap Rust
        // println! buffering, but raw fd 1 is what slash commands hit
        // after libc stdout is flushed.
        let ((), out) = capture_stdout(|| {
            let msg = b"hello-capture\n";
            unsafe {
                libc::write(1, msg.as_ptr().cast(), msg.len());
            }
            let _ = std::io::Write::flush(&mut std::io::stdout());
        });
        assert!(
            out.contains("hello-capture"),
            "expected captured fd write, got {out:?}"
        );
    }
}
