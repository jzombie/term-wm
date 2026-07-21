//! Redirect and suppress OS-level file descriptors (stdout/stderr).
//!
//! macOS system frameworks (AppKit, NSPasteboard) and C libraries often write
//! debug output directly to FD 1 or 2.  When the terminal is in raw/alt-screen
//! mode this junk leaks to the display.  These helpers pipe or suppress the FD
//! through background threads and `tracing`.

use std::io::BufRead;

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
        extern "system" {
            fn GetStdHandle(nStdHandle: u32) -> isize;
            fn SetStdHandle(nStdHandle: u32, hHandle: isize) -> i32;
        }
        const STD_ERROR_HANDLE: u32 = 0xFFFFFFF4u32;

        unsafe {
            let saved_handle = GetStdHandle(STD_ERROR_HANDLE);
            let nul = std::fs::OpenOptions::new()
                .write(true)
                .open("NUL")
                .ok()?;
            let nul_handle = nul.as_raw_handle() as isize;
            let nul_fd = libc::open_osfhandle(nul_handle, 0);
            if nul_fd == -1 {
                return None;
            }
            let saved_fd = libc::dup(2);
            libc::dup2(nul_fd, 2);
            SetStdHandle(STD_ERROR_HANDLE, nul_handle);
            Some(StderrSuppressGuard { saved_handle, saved_fd })
        }
    }
}

#[cfg(windows)]
impl Drop for StderrSuppressGuard {
    fn drop(&mut self) {
        extern "system" {
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

/// Redirect an OS-level file descriptor into `tracing`.
///
/// Spawns a background thread that reads from the FD and forwards each line
/// to `tracing::error!` (stderr) or `tracing::info!` (stdout).
///
/// - **Unix**: creates a pipe, uses `dup2` to redirect the FD, reads from
///   the pipe in the background thread.
/// - **Windows**: creates a Win32 anonymous pipe, redirects both the CRT
///   descriptor and the Win32 handle, reads from the pipe in the background
///   thread.
///
/// Non-UTF-8 bytes are handled via `String::from_utf8_lossy`.
#[cfg(unix)]
pub fn redirect_fd_to_tracing(target_fd: libc::c_int, is_stderr: bool) -> std::io::Result<()> {
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
    let name = if is_stderr {
        "stderr-tracing"
    } else {
        "stdout-tracing"
    };
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            use std::os::unix::io::FromRawFd;
            let file = unsafe { std::fs::File::from_raw_fd(read_fd) };
            let mut reader = std::io::BufReader::new(file);
            let mut buf = Vec::new();
            while reader.read_until(b'\n', &mut buf).unwrap_or(0) > 0 {
                let text = String::from_utf8_lossy(&buf);
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    if is_stderr {
                        tracing::error!(target: "c_stderr", "{}", trimmed);
                    } else {
                        tracing::info!(target: "c_stdout", "{}", trimmed);
                    }
                }
                buf.clear();
            }
        })?;
    Ok(())
}

/// Windows implementation — same semantics as the Unix version.
#[cfg(windows)]
pub fn redirect_fd_to_tracing(target_fd: i32, is_stderr: bool) -> std::io::Result<()> {
    use std::os::windows::io::{FromRawHandle, AsRawHandle};

    extern "system" {
        fn GetStdHandle(nStdHandle: u32) -> isize;
        fn SetStdHandle(nStdHandle: u32, hHandle: isize) -> i32;
        fn CreatePipe(
            hReadPipe: *mut isize,
            hWritePipe: *mut isize,
            lpPipeAttributes: *const std::ffi::c_void,
            nSize: u32,
        ) -> i32;
    }

    const STD_ERROR_HANDLE: u32 = 0xFFFFFFF4u32; // -12
    const STD_OUTPUT_HANDLE: u32 = 0xFFFFFFF5u32; // -11
    const STD_INPUT_HANDLE: u32 = 0xFFFFFFF6u32; // -10

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

        let name = if is_stderr {
            "stderr-tracing"
        } else {
            "stdout-tracing"
        };
        let file = std::fs::File::from_raw_handle(read_handle as _);

        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let mut reader = std::io::BufReader::new(file);
                let mut buf = Vec::new();
                while reader.read_until(b'\n', &mut buf).unwrap_or(0) > 0 {
                    let text = String::from_utf8_lossy(&buf);
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        if is_stderr {
                            tracing::error!(target: "c_stderr", "{}", trimmed);
                        } else {
                            tracing::info!(target: "c_stdout", "{}", trimmed);
                        }
                    }
                    buf.clear();
                }
            })?;
    }

    Ok(())
}

/// No-op fallback for unsupported platforms (e.g. wasm).
#[cfg(not(any(unix, windows)))]
pub fn redirect_fd_to_tracing(_target_fd: i32, _is_stderr: bool) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── StderrSuppressGuard ───────────────────────────────────────

    #[test]
    #[cfg(unix)]
    fn stderr_suppress_guard_suppresses_and_restores() {
        use std::os::fd::FromRawFd;

        let saved_fd = unsafe { libc::dup(libc::STDERR_FILENO) };
        assert!(saved_fd >= 0, "dup stderr");

        let mut fds: [libc::c_int; 2] = [0; 2];
        unsafe { assert_eq!(libc::pipe(fds.as_mut_ptr()), 0); }

        unsafe { libc::dup2(fds[1], libc::STDERR_FILENO); }
        unsafe { libc::close(fds[1]); }

        {
            let _guard = StderrSuppressGuard::new();
            assert!(_guard.is_some(), "guard creation");
            unsafe {
                libc::write(libc::STDERR_FILENO, c"suppressed\n".as_ptr().cast(), 11);
            }
        }

        unsafe {
            libc::write(libc::STDERR_FILENO, c"restored\n".as_ptr().cast(), 9);
        }

        unsafe { libc::dup2(saved_fd, libc::STDERR_FILENO); }
        unsafe { libc::close(saved_fd); }

        use std::io::Read;
        let mut file = unsafe { std::fs::File::from_raw_fd(fds[0]) };
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

    #[test]
    #[cfg(windows)]
    fn stderr_suppress_guard_suppresses_and_restores() {
        extern "system" {
            fn GetStdHandle(nStdHandle: u32) -> isize;
            fn SetStdHandle(nStdHandle: u32, hHandle: isize) -> i32;
            fn CreatePipe(
                hReadPipe: *mut isize,
                hWritePipe: *mut isize,
                lpPipeAttributes: *const std::ffi::c_void,
                nSize: u32,
            ) -> i32;
        }
        use std::os::windows::io::{FromRawHandle, AsRawHandle};
        const STD_ERROR_HANDLE: u32 = 0xFFFFFFF4u32;

        let saved_handle = unsafe { GetStdHandle(STD_ERROR_HANDLE) };

        let mut read_handle: isize = 0;
        let mut write_handle: isize = 0;
        unsafe { assert_ne!(CreatePipe(&mut read_handle, &mut write_handle, std::ptr::null(), 0), 0); }

        unsafe { SetStdHandle(STD_ERROR_HANDLE, write_handle); }
        let write_fd = unsafe { libc::open_osfhandle(write_handle, 0) };
        if write_fd != -1 {
            unsafe { libc::dup2(write_fd, 2); }
        }

        {
            let _guard = StderrSuppressGuard::new();
            assert!(_guard.is_some(), "guard creation");
            unsafe { libc::write(2, c"suppressed\n".as_ptr().cast(), 11); }
        }

        unsafe { libc::write(2, c"restored\n".as_ptr().cast(), 9); }

        unsafe { SetStdHandle(STD_ERROR_HANDLE, saved_handle); }
        if write_fd != -1 {
            unsafe { libc::close(write_fd); }
        }

        use std::io::Read;
        let mut file = unsafe { std::fs::File::from_raw_handle(read_handle as _) };
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

        unsafe { SetStdHandle(STD_ERROR_HANDLE, saved_handle); }
    }

    // ── redirect_fd_to_tracing ────────────────────────────────────

    #[test]
    #[cfg(unix)]
    fn redirect_fd_to_tracing_is_ok_on_pipe() {
        let mut fds: [libc::c_int; 2] = [0; 2];
        unsafe { assert_eq!(libc::pipe(fds.as_mut_ptr()), 0); }

        let result = redirect_fd_to_tracing(fds[1], true);
        assert!(result.is_ok(), "redirect_fd_to_tracing returned error: {result:?}");

        unsafe { libc::close(fds[0]); }
    }

    #[test]
    #[cfg(windows)]
    fn redirect_fd_to_tracing_is_ok_on_pipe() {
        extern "system" {
            fn CreatePipe(
                hReadPipe: *mut isize,
                hWritePipe: *mut isize,
                lpPipeAttributes: *const std::ffi::c_void,
                nSize: u32,
            ) -> i32;
        }
        let mut read_handle: isize = 0;
        let mut write_handle: isize = 0;
        unsafe { assert_ne!(CreatePipe(&mut read_handle, &mut write_handle, std::ptr::null(), 0), 0); }

        let write_fd = unsafe { libc::open_osfhandle(write_handle, 0) };
        assert!(write_fd != -1, "open_osfhandle");

        let result = redirect_fd_to_tracing(write_fd, true);
        assert!(result.is_ok(), "redirect_fd_to_tracing returned error: {result:?}");

        // write_fd was dup2'd inside redirect_fd_to_tracing — the pipe stays
        // alive through target_fd; read_handle is owned by the tracing thread.
    }
}
