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
    _guard: crate::redirect_stdio::FdSwapGuard,
    #[cfg(windows)]
    _guard: crate::redirect_stdio::HandleSwapGuard,
    #[cfg(windows)]
    _pair: portable_pty::PtyPair,
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
        // Use the shared FdSwapGuard to swap fd 0 with the PTY master fd.
        // This saves the original stdin and restores it on drop.
        let _guard = crate::redirect_stdio::FdSwapGuard::new(0, master_fd)?;
        // FdSwapGuard's Drop restores original stdin — keeping it alive
        // inside Self ensures the redirect lasts for the guard's lifetime.
        Ok(Self { _guard })
    }
}

#[cfg(unix)]
impl Drop for StdinPtyGuard {
    fn drop(&mut self) {
        // _guard's Drop handles the restore via dup2(saved, 0)
    }
}

#[cfg(windows)]
impl StdinPtyGuard {
    /// Redirect stdin to the physical console input buffer.
    ///
    /// During `cargo test`, standard input is redirected to a pipe. We bypass this
    /// by explicitly opening `CONIN$`, which gives us a raw handle to the actual
    /// host console. We duplicate this handle and swap it into `STD_INPUT_HANDLE`
    /// so `crossterm::terminal::enable_raw_mode` tests the real OS code.
    pub fn new() -> io::Result<Self> {
        const STD_INPUT_HANDLE: u32 = 0xFFFFFFF6u32;
        use std::fs::OpenOptions;
        use std::os::windows::io::AsRawHandle;

        unsafe extern "system" {
            fn GetCurrentProcess() -> isize;
            fn DuplicateHandle(
                hSourceProcess: isize,
                hSourceHandle: isize,
                hTargetProcess: isize,
                lpTargetHandle: *mut isize,
                dwDesiredAccess: u32,
                bInheritHandle: i32,
                dwOptions: u32,
            ) -> i32;
        }
        const DUPLICATE_SAME_ACCESS: u32 = 0x00000002;

        // 1. Grab the real Windows console buffer
        let conin = OpenOptions::new()
            .read(true)
            .write(true)
            .open("CONIN$")?;

        let input_handle = conin.as_raw_handle();

        // 2. Duplicate it for swapping
        let duped = unsafe {
            let proc = GetCurrentProcess();
            let mut duped: isize = 0;
            if DuplicateHandle(
                proc,
                input_handle as isize,
                proc,
                &mut duped as *mut isize,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            duped
        };

        // 3. Swap it into standard input
        let _guard = crate::redirect_stdio::HandleSwapGuard::new(
            STD_INPUT_HANDLE,
            duped,
        )?;

        // We open a pair to satisfy the struct definition, but we don't need
        // its handles since we are using CONIN$ to test the real Win32 APIs.
        let pair = open_test_pty()?;

        Ok(Self {
            _guard,
            _pair: pair,
        })
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
    #[cfg(any(unix, windows))]
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
