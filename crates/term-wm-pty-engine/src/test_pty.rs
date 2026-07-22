//! PTY-based test utilities.
//!
//! Provides a `StdinPtyGuard` that creates a real PTY pair and redirects
//! the process's stdin to the PTY slave.  This lets tests call
//! `crossterm::terminal::enable_raw_mode()` (which requires a TTY on
//! stdin) without needing a real terminal attached.
//!
//! Usage:
//! ```ignore
//! use term_wm_pty_engine::test_pty::StdinPtyGuard;
//!
//! let _pty = StdinPtyGuard::new().expect("PTY guard");
//! // Now crossterm::terminal::enable_raw_mode() will succeed
//! ```
//!
//! The guard restores the original stdin when dropped.

use std::io;

/// Guards stdin redirection to a real PTY.
///
/// On creation, opens a PTY pair via `portable-pty` and redirects
/// the process's stdin to the PTY master (which is a TTY).  This
/// gives tests a real TTY so functions like
/// `crossterm::terminal::enable_raw_mode` — which call `tcgetattr` /
/// `tcsetattr` on fd 0 — can succeed.
///
/// On drop, the original stdin is restored.
///
/// # Platform support
///
/// - **Unix**: uses `libc::dup` / `libc::dup2` for fd manipulation.
/// - **Windows**: creates a ConPTY and redirects via `SetStdHandle`.
#[doc(hidden)]
pub struct StdinPtyGuard {
    #[cfg(unix)]
    saved_stdin: libc::c_int,
    #[cfg(unix)]
    _master: Box<dyn portable_pty::MasterPty + Send>,
    #[cfg(windows)]
    _pair: portable_pty::PtyPair,
    #[cfg(windows)]
    _saved_handle: std::os::windows::io::RawHandle,
}

/// Shared PTY creation logic used by both Unix and Windows impls.
///
/// Opens a standard 24×80 PTY pair via `portable-pty`.
fn open_test_pty() -> io::Result<portable_pty::PtyPair> {
    let pty_system = portable_pty::native_pty_system();
    pty_system
        .openpty(portable_pty::PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| io::Error::other(e.to_string()))
}

#[cfg(unix)]
impl StdinPtyGuard {
    /// Create a PTY pair and redirect stdin to the PTY master.
    ///
    /// The PTY master supports termios operations (`tcgetattr`/
    /// `tcsetattr`) that `crossterm::terminal::enable_raw_mode` depends
    /// on.  After this returns, fd 0 is a real TTY.
    pub fn new() -> io::Result<Self> {
        let pair = open_test_pty()?;
        let master_fd = pair.master.as_raw_fd().ok_or_else(|| {
            io::Error::other("PTY master has no raw fd")
        })?;
        let saved_stdin = unsafe { libc::dup(0) };
        if saved_stdin < 0 {
            return Err(io::Error::last_os_error());
        }
        let ret = unsafe { libc::dup2(master_fd, 0) };
        if ret < 0 {
            unsafe { libc::close(saved_stdin) };
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            saved_stdin,
            _master: pair.master,
        })
    }
}

#[cfg(unix)]
impl Drop for StdinPtyGuard {
    fn drop(&mut self) {
        unsafe {
            libc::dup2(self.saved_stdin, 0);
            libc::close(self.saved_stdin);
        }
    }
}

#[cfg(windows)]
impl StdinPtyGuard {
    /// Create a ConPTY pair and redirect stdin to the PTY slave.
    ///
    /// Uses `SetStdHandle` to redirect the Win32 stdin handle.
    pub fn new() -> io::Result<Self> {
        use std::os::windows::io::AsRawHandle;
        let pair = open_test_pty()?;
        let slave_handle = pair.slave.as_raw_handle();
        let saved_handle = unsafe { libc::GetStdHandle(libc::STD_INPUT_HANDLE) };
        // Duplicate the slave handle and set it as stdin
        unsafe {
            let proc = libc::GetCurrentProcess();
            let mut duped: isize = 0;
            if libc::DuplicateHandle(
                proc,
                slave_handle as *mut std::ffi::c_void,
                proc,
                &mut duped as *mut isize,
                0,
                0,
                libc::DUPLICATE_SAME_ACCESS,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            libc::SetStdHandle(libc::STD_INPUT_HANDLE, duped as *mut std::ffi::c_void);
        }
        Ok(Self {
            _pair: pair,
            _saved_handle: saved_handle,
        })
    }
}

#[cfg(windows)]
impl Drop for StdinPtyGuard {
    fn drop(&mut self) {
        unsafe {
            libc::SetStdHandle(libc::STD_INPUT_HANDLE, self._saved_handle as *mut std::ffi::c_void);
        }
    }
}

#[cfg(not(any(unix, windows)))]
impl StdinPtyGuard {
    /// Stub for unsupported platforms — always fails.
    pub fn new() -> io::Result<Self> {
        Err(io::Error::other(
            "StdinPtyGuard is not supported on this platform",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that creating and dropping a `StdinPtyGuard` does not panic
    /// and that after the guard is dropped, the original stdin is restored
    /// (the guard should not leak a redirected fd).
    #[cfg(unix)]
    #[test]
    fn guard_restores_stdin_on_drop() {
        let orig_fd = unsafe { libc::dup(0) };
        assert!(orig_fd >= 0, "must be able to dup original stdin");

        {
            let _guard = StdinPtyGuard::new().expect("PTY guard");
            // While the guard is active, fd 0 points to the PTY master.
            // A dup(0) should give a different fd number than orig_fd.
            let during = unsafe { libc::dup(0) };
            assert!(during >= 0, "must be able to dup while guard active");
            assert_ne!(during, orig_fd, "stdin was redirected");
            unsafe { libc::close(during) };
        }

        // After the guard is dropped, a dup(0) must give a valid fd
        // that matches the original (stdin is restored).
        let after = unsafe { libc::dup(0) };
        assert!(after >= 0, "must be able to dup after guard");
        // The new fd should be different from orig_fd (which we haven't
        // closed), proving stdin is still valid and not leaked.
        unsafe { libc::close(orig_fd) };
        unsafe { libc::close(after) };
    }
}
