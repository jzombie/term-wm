//! Redirect and suppress OS-level file descriptors (stdout/stderr).
//!
//! macOS system frameworks (AppKit, NSPasteboard) and C libraries often write
//! debug output directly to FD 1 or 2.  When the terminal is in raw/alt-screen
//! mode this junk leaks to the display.  These helpers pipe or suppress the FD
//! through background threads and `tracing`.

use std::io::BufRead;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle;

/// RAII guard that temporarily redirects stderr to the null device.
///
/// Drops restore the original stderr.  Used to suppress transient noise
/// from `arboard` / NSPasteboard during clipboard set operations.
///
/// - **Unix**: opens `/dev/null`, `dup2`s stderr, saves/restores via `dup`.
/// - **Windows**: opens `NUL`, uses `libc::open_osfhandle` + `libc::dup2`
///   for CRT stderr and `SetStdHandle` for the Win32 handle.
#[cfg(unix)]
pub struct StderrSuppressGuard {
    saved_fd: libc::c_int,
}

#[cfg(unix)]
impl StderrSuppressGuard {
    pub fn new() -> Option<Self> {
        unsafe {
            let null_fd = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
            if null_fd < 0 {
                return None;
            }
            let saved_fd = libc::dup(libc::STDERR_FILENO);
            libc::dup2(null_fd, libc::STDERR_FILENO);
            libc::close(null_fd);
            Some(StderrSuppressGuard { saved_fd })
        }
    }
}

#[cfg(unix)]
impl Drop for StderrSuppressGuard {
    fn drop(&mut self) {
        unsafe {
            libc::dup2(self.saved_fd, libc::STDERR_FILENO);
            libc::close(self.saved_fd);
        }
    }
}

#[cfg(windows)]
pub struct StderrSuppressGuard {
    saved_handle: isize,
    saved_fd: i32,
}

#[cfg(windows)]
impl StderrSuppressGuard {
    pub fn new() -> Option<Self> {
        unsafe extern "system" {
            fn GetStdHandle(nStdHandle: u32) -> isize;
            fn SetStdHandle(nStdHandle: u32, hHandle: isize) -> i32;
        }
        const STD_ERROR_HANDLE: u32 = 0xFFFFFFF4u32;

        unsafe {
            let saved_handle = GetStdHandle(STD_ERROR_HANDLE);
            let nul = std::fs::OpenOptions::new().write(true).open("NUL").ok()?;
            let nul_handle = nul.as_raw_handle() as isize;
            let nul_fd = libc::open_osfhandle(nul_handle, 0);
            if nul_fd == -1 {
                return None;
            }
            let saved_fd = libc::dup(2);
            libc::dup2(nul_fd, 2);
            SetStdHandle(STD_ERROR_HANDLE, nul_handle);
            Some(StderrSuppressGuard {
                saved_handle,
                saved_fd,
            })
        }
    }
}

#[cfg(windows)]
impl Drop for StderrSuppressGuard {
    fn drop(&mut self) {
        unsafe extern "system" {
            fn SetStdHandle(nStdHandle: u32, hHandle: isize) -> i32;
        }
        const STD_ERROR_HANDLE: u32 = 0xFFFFFFF4u32;
        unsafe {
            libc::dup2(self.saved_fd, 2);
            libc::close(self.saved_fd);
            SetStdHandle(STD_ERROR_HANDLE, self.saved_handle);
        }
    }
}

#[cfg(not(any(unix, windows)))]
pub struct StderrSuppressGuard;

#[cfg(not(any(unix, windows)))]
impl StderrSuppressGuard {
    pub fn new() -> Option<Self> {
        Some(StderrSuppressGuard)
    }
}

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
    #[cfg(unix)]
    use std::os::fd::FromRawFd;
    #[cfg(windows)]
    use std::os::windows::io::FromRawHandle;
    use std::sync::{Arc, Mutex};

    // ── StderrSuppressGuard ───────────────────────────────────────

    #[test]
    #[cfg(any(unix, windows))]
    fn stderr_suppress_guard_suppresses_and_restores() {
        // ---- platform-specific setup: save + redirect stderr to a pipe ----
        #[cfg(unix)]
        let (capture_fd, restore) = {
            let saved_fd = unsafe { libc::dup(libc::STDERR_FILENO) };
            assert!(saved_fd >= 0, "dup stderr");

            let mut fds: [libc::c_int; 2] = [0; 2];
            unsafe {
                assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
            }

            unsafe {
                libc::dup2(fds[1], libc::STDERR_FILENO);
            }
            unsafe {
                libc::close(fds[1]);
            }

            let restore = move || {
                unsafe {
                    libc::dup2(saved_fd, libc::STDERR_FILENO);
                }
                unsafe {
                    libc::close(saved_fd);
                }
            };

            (fds[0] as isize, restore)
        };

        #[cfg(windows)]
        let (capture_fd, restore) = {
            unsafe extern "system" {
                fn GetStdHandle(nStdHandle: u32) -> isize;
                fn SetStdHandle(nStdHandle: u32, hHandle: isize) -> i32;
                fn CreatePipe(
                    hReadPipe: *mut isize,
                    hWritePipe: *mut isize,
                    lpPipeAttributes: *const std::ffi::c_void,
                    nSize: u32,
                ) -> i32;
            }
            const STD_ERROR_HANDLE: u32 = 0xFFFFFFF4u32;

            let saved_handle = unsafe { GetStdHandle(STD_ERROR_HANDLE) };

            // Save the original CRT fd 2 before redirecting.
            let saved_fd2 = unsafe { libc::dup(2) };
            assert!(saved_fd2 >= 0);

            let mut read_handle: isize = 0;
            let mut write_handle: isize = 0;
            unsafe {
                assert_ne!(
                    CreatePipe(&mut read_handle, &mut write_handle, std::ptr::null(), 0),
                    0
                );
            }

            unsafe {
                SetStdHandle(STD_ERROR_HANDLE, write_handle);
            }
            let write_fd = unsafe { libc::open_osfhandle(write_handle, 0) };
            if write_fd != -1 {
                unsafe {
                    libc::dup2(write_fd, 2);
                }
            }

            let restore = move || {
                unsafe {
                    SetStdHandle(STD_ERROR_HANDLE, saved_handle);
                    libc::dup2(saved_fd2, 2);
                    libc::close(saved_fd2);
                }
                if write_fd != -1 {
                    unsafe {
                        libc::close(write_fd);
                    }
                }
            };

            (read_handle, restore)
        };

        // ---- shared assertions ----
        {
            let _guard = StderrSuppressGuard::new();
            assert!(_guard.is_some(), "guard creation");

            #[cfg(unix)]
            unsafe {
                libc::write(libc::STDERR_FILENO, c"suppressed\n".as_ptr().cast(), 11);
            }
            #[cfg(windows)]
            unsafe {
                libc::write(2, c"suppressed\n".as_ptr().cast(), 11);
            }
        }

        #[cfg(unix)]
        unsafe {
            libc::write(libc::STDERR_FILENO, c"restored\n".as_ptr().cast(), 9);
        }
        #[cfg(windows)]
        unsafe {
            libc::write(2, c"restored\n".as_ptr().cast(), 9);
        }

        restore();

        use std::io::Read;
        #[cfg(unix)]
        let mut file = unsafe { std::fs::File::from_raw_fd(capture_fd as _) };
        #[cfg(windows)]
        let mut file = unsafe { std::fs::File::from_raw_handle(capture_fd as _) };
        let mut output = String::new();
        file.read_to_string(&mut output).unwrap_or(0);

        assert!(
            !output.contains("suppressed"),
            "suppressed output leaked to stderr: {output:?}"
        );
        assert!(
            output.contains("restored"),
            "restored output missing from stderr: {output:?}"
        );
    }

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
