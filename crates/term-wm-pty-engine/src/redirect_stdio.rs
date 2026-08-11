//! Redirect OS-level file descriptors (stdout/stderr) into `tracing`.
//!
//! macOS system frameworks (AppKit, NSPasteboard) and C libraries often write
//! debug output directly to FD 1 or 2.  When the terminal is in raw/alt-screen
//! mode this junk leaks to the display.  These helpers pipe the FD through a
//! background thread into `tracing`.  (The `StderrSuppressGuard` that used to
//! live here — a short-lived null-device redirect for `arboard` — now lives in
//! the `term-clipboard` crate.)

use std::io::BufRead;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle;

/// Redirect an OS-level file descriptor into a callback.
///
/// Spawns a background thread that reads from the FD and calls `on_line`
/// for each non-empty line.  Non-UTF-8 bytes are handled via
/// `String::from_utf8_lossy`.
///
/// - **Unix**: creates a pipe, uses `dup2` to redirect the FD.
/// - **Windows**: creates a Win32 anonymous pipe, redirects both the CRT
///   descriptor and the Win32 handle.
#[cfg(unix)]
pub fn redirect_fd<F>(target_fd: libc::c_int, on_line: F) -> std::io::Result<()>
where
    F: Fn(&str) + Send + 'static,
{
    let mut fds: [libc::c_int; 2] = [0; 2];
    unsafe {
        if libc::pipe(fds.as_mut_ptr()) == -1 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::dup2(fds[1], target_fd) == -1 {
            libc::close(fds[0]);
            libc::close(fds[1]);
            return Err(std::io::Error::last_os_error());
        }
        libc::close(fds[1]);
    }
    let read_fd = fds[0];
    std::thread::Builder::new()
        .name("fd-redirect".into())
        .spawn(move || {
            use std::os::unix::io::FromRawFd;
            let file = unsafe { std::fs::File::from_raw_fd(read_fd) };
            let mut reader = std::io::BufReader::new(file);
            let mut buf = Vec::new();
            while reader.read_until(b'\n', &mut buf).unwrap_or(0) > 0 {
                let text = String::from_utf8_lossy(&buf);
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    on_line(trimmed);
                }
                buf.clear();
            }
        })?;
    Ok(())
}

/// Windows implementation — same semantics as the Unix version.
#[cfg(windows)]
pub fn redirect_fd<F>(target_fd: i32, on_line: F) -> std::io::Result<()>
where
    F: Fn(&str) + Send + 'static,
{
    use std::os::windows::io::FromRawHandle;

    unsafe extern "system" {
        fn SetStdHandle(nStdHandle: u32, hHandle: isize) -> i32;
        fn CreatePipe(
            hReadPipe: *mut isize,
            hWritePipe: *mut isize,
            lpPipeAttributes: *const std::ffi::c_void,
            nSize: u32,
        ) -> i32;
    }

    const STD_ERROR_HANDLE: u32 = 0xFFFFFFF4u32;
    const STD_OUTPUT_HANDLE: u32 = 0xFFFFFFF5u32;

    let win_std_handle = if target_fd == 1 {
        STD_OUTPUT_HANDLE
    } else {
        STD_ERROR_HANDLE
    };

    unsafe {
        let mut read_handle: isize = 0;
        let mut write_handle: isize = 0;

        if CreatePipe(&mut read_handle, &mut write_handle, std::ptr::null(), 0) == 0 {
            return Err(std::io::Error::last_os_error());
        }

        // Redirect the Win32 handle
        SetStdHandle(win_std_handle, write_handle);

        // Redirect the CRT file descriptor
        let write_fd = libc::open_osfhandle(write_handle, 0);
        if write_fd != -1 {
            libc::dup2(write_fd, target_fd);
        }

        let file = std::fs::File::from_raw_handle(read_handle as _);

        std::thread::Builder::new()
            .name("fd-redirect".into())
            .spawn(move || {
                let mut reader = std::io::BufReader::new(file);
                let mut buf = Vec::new();
                while reader.read_until(b'\n', &mut buf).unwrap_or(0) > 0 {
                    let text = String::from_utf8_lossy(&buf);
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        on_line(trimmed);
                    }
                    buf.clear();
                }
            })?;
    }

    Ok(())
}

/// Convenience wrapper: redirects an FD and feeds lines into `tracing`.
#[cfg(any(unix, windows))]
pub fn redirect_fd_to_tracing(target_fd: impl Into<i32>, is_stderr: bool) -> std::io::Result<()> {
    let target_fd = target_fd.into();
    if is_stderr {
        redirect_fd(target_fd, |line| {
            tracing::error!(target: "c_stderr", "{}", line);
        })
    } else {
        redirect_fd(target_fd, |line| {
            tracing::info!(target: "c_stdout", "{}", line);
        })
    }
}

/// No-op fallback for unsupported platforms (e.g. wasm).
#[cfg(not(any(unix, windows)))]
pub fn redirect_fd_to_tracing(_target_fd: i32, _is_stderr: bool) -> std::io::Result<()> {
    Ok(())
}

/// No-op fallback.
#[cfg(not(any(unix, windows)))]
pub fn redirect_fd<F>(_target_fd: i32, _on_line: F) -> std::io::Result<()>
where
    F: Fn(&str) + Send + 'static,
{
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    // ── redirect_fd_to_tracing ────────────────────────────────────

    #[test]
    #[cfg(any(unix, windows))]
    fn redirect_fd_captures_stdout_and_stderr() {
        #[cfg(unix)]
        let (stdout_fd, stderr_fd) = {
            let mut a: [libc::c_int; 2] = [0; 2];
            let mut b: [libc::c_int; 2] = [0; 2];
            unsafe {
                assert_eq!(libc::pipe(a.as_mut_ptr()), 0);
                assert_eq!(libc::pipe(b.as_mut_ptr()), 0);
            }
            (a[1], b[1])
        };
        #[cfg(windows)]
        let (stdout_fd, stderr_fd) = {
            unsafe extern "system" {
                fn CreatePipe(
                    h: *mut isize,
                    w: *mut isize,
                    a: *const std::ffi::c_void,
                    s: u32,
                ) -> i32;
            }
            let mut ra = 0isize;
            let mut wa = 0isize;
            let mut rb = 0isize;
            let mut wb = 0isize;
            unsafe {
                assert_ne!(CreatePipe(&mut ra, &mut wa, std::ptr::null(), 0), 0);
                assert_ne!(CreatePipe(&mut rb, &mut wb, std::ptr::null(), 0), 0);
            }
            let a = unsafe { libc::open_osfhandle(wa, 0) };
            let b = unsafe { libc::open_osfhandle(wb, 0) };
            assert!(a != -1 && b != -1);
            (a, b)
        };

        let stdout_lines = Arc::new(Mutex::new(Vec::new()));
        let stderr_lines = Arc::new(Mutex::new(Vec::new()));

        {
            let out = Arc::clone(&stdout_lines);
            redirect_fd(stdout_fd, move |line| {
                out.lock().unwrap().push(line.to_string())
            })
            .expect("redirect stdout");
        }
        {
            let err = Arc::clone(&stderr_lines);
            redirect_fd(stderr_fd, move |line| {
                err.lock().unwrap().push(line.to_string())
            })
            .expect("redirect stderr");
        }

        unsafe {
            libc::write(stdout_fd, c"hello from stdout\n".as_ptr().cast(), 18);
            libc::write(stderr_fd, c"hello from stderr\n".as_ptr().cast(), 18);
        }

        #[cfg(unix)]
        unsafe {
            libc::close(stdout_fd);
            libc::close(stderr_fd);
        }
        #[cfg(windows)]
        unsafe {
            libc::close(stdout_fd);
            libc::close(stderr_fd);
        }

        std::thread::sleep(std::time::Duration::from_millis(100));

        let stdout: Vec<_> = stdout_lines.lock().unwrap().clone();
        let stderr: Vec<_> = stderr_lines.lock().unwrap().clone();
        assert!(
            stdout.iter().any(|l| l.contains("hello from stdout")),
            "stdout: got {stdout:?}"
        );
        assert!(
            stderr.iter().any(|l| l.contains("hello from stderr")),
            "stderr: got {stderr:?}"
        );
    }
}
