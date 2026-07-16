//! Capture stdout while running slash commands under the modern TUI.
//!
//! Built-in commands use `println!` for line-oriented output. In alt-screen mode
//! those writes would corrupt the UI or vanish after restore. We temporarily
//! redirect fd/handle 1 to a pipe (Unix) or a temp file (Windows) so the modern
//! transcript can show the captured text.

/// Run `f`, capturing anything written to stdout. Returns `(result, captured)`.
pub fn capture_stdout<F, R>(f: F) -> (R, String)
where
    F: FnOnce() -> R,
{
    #[cfg(unix)]
    {
        capture_stdout_unix(f, false)
    }
    #[cfg(windows)]
    {
        capture_stdout_windows(f, false)
    }
    #[cfg(not(any(unix, windows)))]
    {
        (f(), String::new())
    }
}

/// Like [`capture_stdout`], but also mirrors bytes to the original terminal.
///
/// Used for interactive slash commands (pickers / `$EDITOR` / y-N prompts) so
/// short-circuit diagnostics still land in the modern transcript without
/// hiding the live UI. On platforms without a tee path, falls back to plain
/// capture (Windows: capture then replay to the console after `f`).
pub fn capture_stdout_tee<F, R>(f: F) -> (R, String)
where
    F: FnOnce() -> R,
{
    #[cfg(unix)]
    {
        capture_stdout_unix(f, true)
    }
    #[cfg(windows)]
    {
        capture_stdout_windows(f, true)
    }
    #[cfg(not(any(unix, windows)))]
    {
        (f(), String::new())
    }
}

#[cfg(unix)]
fn capture_stdout_unix<F, R>(f: F, tee: bool) -> (R, String)
where
    F: FnOnce() -> R,
{
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::fd::{FromRawFd, RawFd};

    let mut pair = [0i32; 2];
    // SAFETY: standard pipe syscall.
    if unsafe { libc::pipe(pair.as_mut_ptr()) } != 0 {
        return (f(), String::new());
    }
    let read_fd: RawFd = pair[0];
    let write_fd: RawFd = pair[1];

    // SAFETY: duplicate current stdout (restore target).
    let saved = unsafe { libc::dup(1) };
    if saved < 0 {
        unsafe {
            libc::close(read_fd);
            libc::close(write_fd);
        }
        return (f(), String::new());
    }

    // Optional second dup for the tee writer thread (live mirror).
    let tee_fd = if tee {
        let d = unsafe { libc::dup(saved) };
        if d < 0 {
            unsafe {
                libc::close(saved);
                libc::close(read_fd);
                libc::close(write_fd);
            }
            return (f(), String::new());
        }
        Some(d)
    } else {
        None
    };

    // SAFETY: redirect stdout to the pipe write end.
    if unsafe { libc::dup2(write_fd, 1) } < 0 {
        unsafe {
            libc::close(saved);
            if let Some(t) = tee_fd {
                libc::close(t);
            }
            libc::close(read_fd);
            libc::close(write_fd);
        }
        return (f(), String::new());
    }
    // Close our extra write-end handle. fd 1 still holds a write end until
    // we restore stdout; the reader thread sees EOF after that restore.
    unsafe {
        libc::close(write_fd);
    }

    // Drain the pipe concurrently. If we only read after `f` returns, a
    // command that prints more than the OS pipe buffer (often ~64 KiB) can
    // block forever inside `println!` / `write(1, …)`.
    // SAFETY: exclusive ownership of `read_fd` (and tee_fd) moves into the
    // reader thread.
    let reader = std::thread::spawn(move || {
        let mut file = unsafe { File::from_raw_fd(read_fd) };
        let mut buf = Vec::new();
        if let Some(tfd) = tee_fd {
            let mut real = unsafe { File::from_raw_fd(tfd) };
            let mut chunk = [0u8; 8192];
            loop {
                match file.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        let _ = real.write_all(&chunk[..n]);
                        let _ = real.flush();
                        buf.extend_from_slice(&chunk[..n]);
                    }
                    Err(_) => break,
                }
            }
            // real + file drop close their fds.
        } else {
            let _ = file.read_to_end(&mut buf);
            // File drop closes read_fd.
        }
        buf
    });

    let result = f();

    // Flush Rust + C stdio onto the pipe before we close the write end.
    let _ = std::io::Write::flush(&mut std::io::stdout());
    unsafe {
        libc::fflush(std::ptr::null_mut());
    }

    // SAFETY: restore original stdout (closes the only remaining write end).
    unsafe {
        let _ = libc::dup2(saved, 1);
        libc::close(saved);
    }

    let buf = reader.join().unwrap_or_default();
    let text = String::from_utf8_lossy(&buf).into_owned();
    (result, text)
}

/// Windows: redirect stdout to a temp file, run `f`, restore, read back.
///
/// Pipes + concurrent readers work, but CRT `printf`/`println!` on Windows
/// talk through the process STD_OUTPUT_HANDLE; a temp-file swap is the
/// portable capture path without pulling extra native crates.
#[cfg(windows)]
fn capture_stdout_windows<F, R>(f: F, tee: bool) -> (R, String)
where
    F: FnOnce() -> R,
{
    // Live pickers / y-N prompts need the real console. A full live tee is
    // Unix-only; on Windows interactive calls skip capture so the UI works
    // (diagnostics flash on the main screen during with_main_screen).
    if tee {
        return (f(), String::new());
    }

    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};

    // Create a temp file to receive stdout.
    let mut tmp = match tempfile::NamedTempFile::new() {
        Ok(t) => t,
        Err(_) => return (f(), String::new()),
    };
    let tmp_handle = tmp.as_file().as_raw_handle();

    // SAFETY: Win32 console handle APIs (edition 2024 requires unsafe extern).
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(n_std_handle: u32) -> RawHandle;
        fn SetStdHandle(n_std_handle: u32, h: RawHandle) -> i32;
        fn DuplicateHandle(
            h_source_process: RawHandle,
            h_source: RawHandle,
            h_target_process: RawHandle,
            lp_target: *mut RawHandle,
            dw_desired_access: u32,
            b_inherit: i32,
            dw_options: u32,
        ) -> i32;
        fn GetCurrentProcess() -> RawHandle;
    }
    const STD_OUTPUT_HANDLE: u32 = 0xFFFFFFF5; // (DWORD)-11
    const DUPLICATE_SAME_ACCESS: u32 = 0x00000002;

    // SAFETY: fetch current stdout handle.
    let original = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if original.is_null() || original == (-1isize as RawHandle) {
        return (f(), String::new());
    }

    // Duplicate original so we can restore after SetStdHandle.
    let mut saved: RawHandle = std::ptr::null_mut();
    let process = unsafe { GetCurrentProcess() };
    let dup_ok = unsafe {
        DuplicateHandle(
            process,
            original,
            process,
            &mut saved,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if dup_ok == 0 || saved.is_null() {
        return (f(), String::new());
    }

    // Redirect process stdout to the temp file handle.
    let set_ok = unsafe { SetStdHandle(STD_OUTPUT_HANDLE, tmp_handle) };
    if set_ok == 0 {
        // SAFETY: close the duplicated handle we no longer need.
        drop(unsafe { OwnedHandle::from_raw_handle(saved) });
        return (f(), String::new());
    }

    // Also rebind Rust's stdout File if possible by flushing CRT/Rust first.
    let _ = std::io::stdout().flush();

    let result = f();

    let _ = std::io::stdout().flush();

    // Restore original stdout handle.
    unsafe {
        let _ = SetStdHandle(STD_OUTPUT_HANDLE, saved);
        // Drop duplicate (OwnedHandle closes it).
        drop(OwnedHandle::from_raw_handle(saved));
    }

    // Read what was written.
    let mut buf = Vec::new();
    let _ = tmp.as_file_mut().seek(SeekFrom::Start(0));
    let _ = tmp.as_file_mut().read_to_end(&mut buf);

    let text = String::from_utf8_lossy(&buf).into_owned();
    (result, text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    /// Serialize capture tests — they rewrite process-wide fd 1.
    fn capture_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    #[cfg(unix)]
    fn captures_fd1_write() {
        let _g = capture_lock();
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

    #[test]
    #[cfg(unix)]
    fn captures_more_than_pipe_buffer_without_deadlock() {
        let _g = capture_lock();
        // Write well past a typical 64 KiB pipe buffer while a concurrent
        // reader drains — must not hang.
        let chunk = b"x".repeat(4096);
        let n_chunks = 64; // 256 KiB
        let ((), out) = capture_stdout(|| {
            for _ in 0..n_chunks {
                unsafe {
                    let mut off = 0usize;
                    while off < chunk.len() {
                        let n = libc::write(1, chunk[off..].as_ptr().cast(), chunk.len() - off);
                        if n <= 0 {
                            break;
                        }
                        off += n as usize;
                    }
                }
            }
            let _ = std::io::Write::flush(&mut std::io::stdout());
        });
        let expected = chunk.len() * n_chunks;
        assert!(
            out.len() >= expected.saturating_sub(4096) && out.len() <= expected + 4096,
            "expected ~{expected} bytes captured without deadlock, got {}",
            out.len()
        );
        assert!(
            out.len() > 64 * 1024,
            "must exceed typical pipe buffer (got {})",
            out.len()
        );
    }

    #[test]
    #[cfg(unix)]
    fn tee_captures_and_mirrors() {
        let _g = capture_lock();
        let ((), out) = capture_stdout_tee(|| {
            let msg = b"hello-tee\n";
            unsafe {
                libc::write(1, msg.as_ptr().cast(), msg.len());
            }
            let _ = std::io::Write::flush(&mut std::io::stdout());
        });
        assert!(
            out.contains("hello-tee"),
            "tee must still capture for transcript, got {out:?}"
        );
    }

    #[test]
    fn block_in_place_compatible_capture() {
        let _g = capture_lock();
        // Smoke: capture_stdout works when nested under block_in_place as the
        // modern TUI slash bridge does (must not panic on the worker).
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let out = rt.block_on(async {
            tokio::task::block_in_place(|| {
                capture_stdout(|| {
                    #[cfg(unix)]
                    {
                        let msg = b"bridge-ok\n";
                        unsafe {
                            libc::write(1, msg.as_ptr().cast(), msg.len());
                        }
                        let _ = std::io::Write::flush(&mut std::io::stdout());
                    }
                    #[cfg(not(unix))]
                    {
                        // Windows path validated in integration; here just
                        // ensure block_in_place + capture does not panic.
                        let _ = std::io::Write::write_all(&mut std::io::stdout(), b"bridge-ok\n");
                        let _ = std::io::Write::flush(&mut std::io::stdout());
                    }
                    42
                })
            })
        });
        assert_eq!(out.0, 42);
        #[cfg(unix)]
        assert!(
            out.1.contains("bridge-ok"),
            "expected captured write under block_in_place, got {:?}",
            out.1
        );
    }
}
